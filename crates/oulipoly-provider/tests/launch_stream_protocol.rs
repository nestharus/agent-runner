//! ## Declared roles
//!
//! Roles: validator, orchestration, formatter, mapper, accessor.
//!
//! - validator: launch-stream protocol tests assert accepted event mixtures,
//!   bounded retention, malformed/kind/schema/base64/correlation failures,
//!   sequence errors, and final-exit requirements.
//! - orchestration: tests build JSONL fixtures, configure `LaunchJsonlReader`,
//!   run the parser, and compare returned results or transport errors.
//! - formatter: `format!`, JSONL helper calls, and string joins materialize
//!   valid and invalid launch-stream fixture payloads.
//! - mapper: table-driven case arrays map fixture labels and JSONL payloads to
//!   expected provider-client transport kinds.
//! - accessor: tests read event counts, retained marker values, stdout/stderr
//!   bytes, omitted-event counts, and exit sequence values.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-provider/tests/launch_stream_protocol.rs
//!     role: adapter
//!     Translates:
//!       - launch-jsonl-stream-contract
//!       - oulipoly-provider-generated-dto-contract
//!       - byte-limit-capture-contract
//!       - provider-client-error-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-provider/tests/launch_stream_protocol.rs
//!     role: intrinsic-surface
//!     Domain: launch JSONL protocol parser test suite
//!     Owns:
//!       - valid stdout, stderr, marker, heartbeat, and final-exit coverage
//!       - bounded retention and output-byte budget coverage
//!       - malformed line, unknown kind, and schema-invalid event coverage
//!       - contract, request-id, and base64 rejection coverage
//!       - sequence and finality error matrix coverage
//! ```

pub mod support {
    pub mod provider_client;
}

use oulipoly_provider::stream::{LaunchJsonlReader, LaunchStreamLimits};
use serde_json::json;
use support::provider_client::{
    REQUEST_ID, json_line, launch_exit_event, launch_heartbeat_event, launch_marker_event,
    launch_stderr_event, launch_stdout_event,
};

#[test]
fn launch_reader_accepts_stdout_stderr_marker_heartbeat_and_final_exit() {
    let jsonl = [
        json_line(&launch_stdout_event(1, "AAH/")),
        json_line(&launch_stderr_event(2, "ZXJy")),
        json_line(&launch_marker_event(3)),
        json_line(&launch_heartbeat_event(4)),
        json_line(&launch_exit_event(5, 0)),
    ]
    .join("");

    let result = LaunchJsonlReader::new(REQUEST_ID)
        .read(jsonl.as_bytes())
        .expect("ordered valid JSONL should parse");

    assert_eq!(result.events.len(), 5);
    assert_eq!(result.stdout_bytes(), vec![0x00, 0x01, 0xff]);
    assert_eq!(result.stderr_bytes(), b"err".to_vec());
}

#[test]
fn launch_reader_retains_bounded_state_independent_of_stream_volume() {
    let mut jsonl = String::new();
    for seq in 1..=5_000 {
        jsonl.push_str(&json_line(&launch_stdout_event(seq, "YQ==")));
    }
    jsonl.push_str(&json_line(&launch_marker_event(5_001)));
    jsonl.push_str(&json_line(&launch_exit_event(5_002, 0)));

    let limits = LaunchStreamLimits {
        retained_events: 8,
        retained_event_bytes: 128,
        retained_output_bytes: 64,
        max_line_bytes: 4096,
    };
    let result = LaunchJsonlReader::new(REQUEST_ID)
        .with_limits(limits)
        .read(jsonl.as_bytes())
        .expect("large valid launch JSONL should parse with bounded retention");

    assert_eq!(result.exit.seq, 5_002);
    assert!(result.events.len() <= limits.retained_events);
    assert!(result.retained_events_omitted() > 0);
    assert_eq!(result.stdout_bytes().len(), limits.retained_output_bytes);
    assert_eq!(
        result.retained_marker_value("example-marker"),
        Some(&json!({ "phase": "example" }))
    );
}

#[test]
fn semantic_markers_survive_generic_marker_retention_exhaustion() {
    let semantic_value = json!({ "produced": true });
    let semantic_marker = json!({
        "contract": "oulipoly.provider/v1",
        "request_id": REQUEST_ID,
        "seq": 2,
        "time_unix_ms": 1,
        "kind": "marker",
        "name": "oulipoly.produced_assistant_response",
        "value": semantic_value,
    });
    let jsonl = [
        json_line(&launch_marker_event(1)),
        json_line(&semantic_marker),
        json_line(&launch_exit_event(3, 0)),
    ]
    .join("");
    let result = LaunchJsonlReader::new(REQUEST_ID)
        .with_limits(LaunchStreamLimits {
            retained_events: 1,
            retained_event_bytes: 32,
            retained_output_bytes: 1,
            max_line_bytes: 4096,
        })
        .read(jsonl.as_bytes())
        .expect("semantic markers should not depend on generic retention capacity");

    assert_eq!(
        result.retained_marker_value("oulipoly.produced_assistant_response"),
        Some(&semantic_value)
    );
}

#[test]
fn launch_reader_rejects_malformed_line_unknown_kind_and_schema_invalid_event() {
    let cases = [
        ("malformed", "{not-json}\n".to_owned(), "malformed_line"),
        (
            "blank-line",
            format!(
                "{}   \n{}",
                json_line(&launch_stdout_event(1, "YQ==")),
                json_line(&launch_exit_event(2, 0))
            ),
            "malformed_line",
        ),
        (
            "unknown-kind",
            "{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"seq\":1,\"time_unix_ms\":1,\"kind\":\"unknown\"}\n".to_owned(),
            "unknown_event_kind",
        ),
        (
            "schema-invalid",
            "{\"contract\":\"oulipoly.provider/v1\",\"request_id\":\"request-example-001\",\"seq\":1,\"time_unix_ms\":1,\"kind\":\"stdout\"}\n".to_owned(),
            "schema_invalid_event",
        ),
    ];

    for (label, jsonl, expected) in cases {
        let error = LaunchJsonlReader::new(REQUEST_ID)
            .read(jsonl.as_bytes())
            .expect_err("invalid launch JSONL should fail");
        assert_eq!(error.transport_kind(), expected, "{label}");
    }
}

#[test]
fn launch_reader_rejects_invalid_base64_wrong_contract_and_wrong_request_id() {
    let mut wrong_contract = launch_stdout_event(1, "YQ==");
    wrong_contract["contract"] = json!("example.contract/v0");
    let mut wrong_request_id = launch_stdout_event(1, "YQ==");
    wrong_request_id["request_id"] = json!("request-example-other");

    let cases = [
        (
            "invalid-base64",
            json_line(&launch_stdout_event(1, "@@@")),
            "invalid_base64",
        ),
        (
            "wrong-contract",
            json_line(&wrong_contract),
            "mismatched_contract",
        ),
        (
            "wrong-request-id",
            json_line(&wrong_request_id),
            "mismatched_request_id",
        ),
    ];

    for (label, jsonl, expected) in cases {
        let error = LaunchJsonlReader::new(REQUEST_ID)
            .read(jsonl.as_bytes())
            .expect_err("correlation or base64 error should fail");
        assert_eq!(error.transport_kind(), expected, "{label}");
    }
}

#[test]
fn launch_reader_rejects_sequence_and_finality_errors() {
    let duplicate_seq = [
        json_line(&launch_stdout_event(1, "YQ==")),
        json_line(&launch_stderr_event(1, "Yg==")),
    ]
    .join("");
    let skipped_seq = [
        json_line(&launch_stdout_event(1, "YQ==")),
        json_line(&launch_exit_event(3, 0)),
    ]
    .join("");
    let decreasing_seq = [
        json_line(&launch_stdout_event(2, "YQ==")),
        json_line(&launch_exit_event(1, 0)),
    ]
    .join("");
    let missing_final = json_line(&launch_stdout_event(1, "YQ=="));
    let duplicate_exit = [
        json_line(&launch_exit_event(1, 0)),
        json_line(&launch_exit_event(2, 0)),
    ]
    .join("");
    let event_after_exit = [
        json_line(&launch_exit_event(1, 0)),
        json_line(&launch_stdout_event(2, "YQ==")),
    ]
    .join("");

    let cases = [
        ("duplicate-seq", duplicate_seq, "duplicate_seq"),
        ("skipped-seq", skipped_seq, "skipped_seq"),
        ("decreasing-seq", decreasing_seq, "decreasing_seq"),
        ("missing-final", missing_final, "missing_final_exit"),
        ("duplicate-exit", duplicate_exit, "duplicate_exit"),
        ("event-after-exit", event_after_exit, "event_after_exit"),
    ];

    for (label, jsonl, expected) in cases {
        let error = LaunchJsonlReader::new(REQUEST_ID)
            .read(jsonl.as_bytes())
            .expect_err("ordering or finality error should fail");
        assert_eq!(error.transport_kind(), expected, "{label}");
    }
}

#[test]
fn missing_final_exit_preserves_retained_marker_evidence() {
    let marker = launch_marker_event(1);
    let error = LaunchJsonlReader::new(REQUEST_ID)
        .read(json_line(&marker).as_bytes())
        .expect_err("missing final exit should remain a protocol failure");

    assert_eq!(error.transport_kind(), "missing_final_exit");
    assert_eq!(
        error.retained_launch_marker_value("example-marker"),
        Some(&json!({ "phase": "example" }))
    );
}
