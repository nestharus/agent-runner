//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use oulipoly_config::SessionsConfig;
use oulipoly_runtime::services::{
    ProductionTraceService, TraceServiceFailure, TraceServicePort, TraceServiceRequest,
};
use oulipoly_runtime::trace::{TraceOptions, TraceReport, trace_invocation_with_sessions};
use oulipoly_state::{InvocationStart, StateDb};
use serde_json::Value;
use std::path::Path;

const ROOT_UUID: &str = "11111111-1111-4111-8111-111111111111";
const MISSING_UUID: &str = "22222222-2222-4222-8222-222222222222";

fn seeded_trace_fixture() -> (StateDb, SessionsConfig) {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    db.start_invocation(&root_invocation_start()).unwrap();
    (db, SessionsConfig::default())
}

fn root_invocation_start() -> InvocationStart {
    InvocationStart {
        invocation_uuid: ROOT_UUID.to_string(),
        model_name: "claude~high".to_string(),
        provider_name: "claude".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn trace_options() -> TraceOptions {
    TraceOptions {
        max_depth: 64,
        json: true,
        inline_transcript: false,
        transcript: false,
    }
}

fn report_without_generated_at(report: &TraceReport) -> Value {
    remove_generated_at(report_json_value(report))
}

fn report_json_value(report: &TraceReport) -> Value {
    serde_json::to_value(report).unwrap()
}

fn remove_generated_at(mut value: Value) -> Value {
    value.as_object_mut().unwrap().remove("generated_at");
    value
}

fn trace_failure_for(invocation_uuid: &str) -> TraceServiceFailure {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let service = ProductionTraceService::default();

    service
        .trace(trace_service_request(&db, &sessions_cfg, invocation_uuid))
        .unwrap()
        .result
        .unwrap_err()
}

fn trace_service_request<'a>(
    state: &'a StateDb,
    sessions_cfg: &'a SessionsConfig,
    invocation_uuid: &'a str,
) -> TraceServiceRequest<'a> {
    TraceServiceRequest {
        state,
        sessions_cfg,
        invocation_uuid,
        options: trace_options(),
    }
}

#[test]
fn idx_svc_01_missing_invocation_returns_invocation_not_found_variant() {
    match trace_failure_for(MISSING_UUID) {
        TraceServiceFailure::InvocationNotFound { input, .. } => {
            assert_eq!(input, MISSING_UUID);
        }
        other => panic!("expected InvocationNotFound, got {other:?}"),
    }
}

#[test]
fn idx_svc_01_malformed_invocation_returns_invalid_invocation_id_variant() {
    let input = "not-a-uuid";

    match trace_failure_for(input) {
        TraceServiceFailure::InvalidInvocationId { input: got, .. } => {
            assert_eq!(got, input);
        }
        other => panic!("expected InvalidInvocationId, got {other:?}"),
    }
}

#[test]
fn idx_svc_01_trace_report_matches_direct_trace_without_generated_at() {
    let (db, sessions_cfg) = seeded_trace_fixture();
    let service = ProductionTraceService::default();

    let service_report = service
        .trace(trace_service_request(&db, &sessions_cfg, ROOT_UUID))
        .unwrap()
        .result
        .unwrap();
    let direct_report =
        trace_invocation_with_sessions(&db, ROOT_UUID, trace_options(), Some(&sessions_cfg))
            .unwrap();

    assert_eq!(
        report_without_generated_at(&service_report),
        report_without_generated_at(&direct_report)
    );
}
