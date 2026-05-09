use oulipoly_config::SessionsConfig;
use oulipoly_runtime::services::{
    ProductionTraceService, TraceServiceFailure, TraceServicePort, TraceServiceRequest,
};
use oulipoly_runtime::trace::{
    TraceOptions, TraceReport, render_ascii_trace, trace_invocation_with_sessions,
};
use oulipoly_state::{InvocationStart, StateDb};
use serde_json::Value;
use std::path::Path;

const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const MISSING_UUID: &str = "22222222-2222-4222-8222-222222222222";

fn seeded_trace_fixture() -> (StateDb, SessionsConfig) {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    db.start_invocation(&InvocationStart {
        invocation_uuid: ROOT_UUID.to_string(),
        model_name: "claude~high".to_string(),
        provider_name: "claude".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap();
    (db, SessionsConfig::default())
}

fn trace_options(max_depth: Option<usize>) -> TraceOptions {
    TraceOptions {
        max_depth: max_depth.unwrap_or(64),
        json: true,
        inline_transcript: false,
        transcript: false,
    }
}

fn report_without_generated_at(report: &TraceReport) -> Value {
    let mut value = serde_json::to_value(report).unwrap();
    value.as_object_mut().unwrap().remove("generated_at");
    value
}

#[test]
fn trace_service_matches_trace_invocation_with_sessions_for_report() {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let max_depth = Some(64);
    let service = ProductionTraceService::default();

    let service_report = service
        .trace(TraceServiceRequest {
            state: &db,
            sessions_cfg: &sessions_cfg,
            invocation_uuid: ROOT_UUID,
            options: trace_options(max_depth),
        })
        .unwrap()
        .result
        .unwrap();
    let direct_report = trace_invocation_with_sessions(
        &db,
        ROOT_UUID,
        trace_options(max_depth),
        Some(&sessions_cfg),
    )
    .unwrap();

    assert_eq!(
        report_without_generated_at(&service_report),
        report_without_generated_at(&direct_report)
    );
}

#[test]
fn trace_service_forwards_transcript_options() {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let service = ProductionTraceService::default();
    let inline_options = TraceOptions {
        max_depth: 64,
        json: true,
        inline_transcript: true,
        transcript: false,
    };

    let service_report = service
        .trace(TraceServiceRequest {
            state: &db,
            sessions_cfg: &sessions_cfg,
            invocation_uuid: ROOT_UUID,
            options: inline_options,
        })
        .unwrap()
        .result
        .unwrap();
    let direct_report =
        trace_invocation_with_sessions(&db, ROOT_UUID, inline_options, Some(&sessions_cfg))
            .unwrap();

    assert_eq!(
        report_without_generated_at(&service_report),
        report_without_generated_at(&direct_report)
    );
    assert_eq!(
        serde_json::to_value(&service_report).unwrap()["root"]["transcript"],
        Value::Array(Vec::new())
    );

    let footer_options = TraceOptions {
        max_depth: 64,
        json: false,
        inline_transcript: false,
        transcript: true,
    };
    let service_report = service
        .trace(TraceServiceRequest {
            state: &db,
            sessions_cfg: &sessions_cfg,
            invocation_uuid: ROOT_UUID,
            options: footer_options,
        })
        .unwrap()
        .result
        .unwrap();
    let direct_report =
        trace_invocation_with_sessions(&db, ROOT_UUID, footer_options, Some(&sessions_cfg))
            .unwrap();
    let service_ascii = render_ascii_trace(&service_report);

    assert_eq!(service_ascii, render_ascii_trace(&direct_report));
    assert!(service_ascii.contains("=== Transcript: 11111111-1111-4111-8111-111111111111 ==="));
}

#[test]
fn trace_service_classifies_missing_invocation_as_invocation_not_found() {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let service = ProductionTraceService::default();

    let failure = service
        .trace(TraceServiceRequest {
            state: &db,
            sessions_cfg: &sessions_cfg,
            invocation_uuid: MISSING_UUID,
            options: trace_options(Some(64)),
        })
        .unwrap()
        .result
        .unwrap_err();

    match failure {
        TraceServiceFailure::InvocationNotFound { input, message } => {
            assert_eq!(input, MISSING_UUID);
            assert!(message.contains("Invocation not found"), "{message}");
        }
        other => panic!("expected InvocationNotFound, got {other:?}"),
    }
}

#[test]
fn trace_service_classifies_malformed_uuid_as_invalid_invocation_id() {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let service = ProductionTraceService::default();
    let input = "not-a-uuid";

    let failure = service
        .trace(TraceServiceRequest {
            state: &db,
            sessions_cfg: &sessions_cfg,
            invocation_uuid: input,
            options: trace_options(Some(64)),
        })
        .unwrap()
        .result
        .unwrap_err();

    match failure {
        TraceServiceFailure::InvalidInvocationId {
            input: got,
            message,
        } => {
            assert_eq!(got, input);
            assert!(
                message
                    .to_ascii_lowercase()
                    .contains("invalid invocation uuid"),
                "{message}"
            );
        }
        other => panic!("expected InvalidInvocationId, got {other:?}"),
    }
}
