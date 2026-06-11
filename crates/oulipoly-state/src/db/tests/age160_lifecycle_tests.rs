//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
#[test]
fn age160_lifecycle_log_facade_start_finalize_session_capture_preserves_records() {
    let (db, sink) = age160_lifecycle_fixture();
    let invocation_uuid = "16000000-0000-4000-8000-000000000001";

    let row_id = db
        .start_invocation(&age160_invocation_start(invocation_uuid))
        .unwrap();
    db.update_session_capture(row_id, Some("session-age160"), "resumed")
        .unwrap();
    db.finalize_invocation(row_id, true, 0, None, Some("done"))
        .unwrap();

    let records = age160_lifecycle_records(&sink);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["event_name"], "invocation.started");
    assert_eq!(records[1]["event_name"], "invocation.session_captured");
    assert_eq!(records[2]["event_name"], "invocation.finalized");

    assert_eq!(
        age160_record_keys(&records[0]),
        vec![
            "chain_id",
            "error_chain",
            "event_name",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "model",
            "operation_result",
            "parent_invocation_uuid",
            "provider",
            "provider_source",
            "session_id",
        ]
    );
    assert_eq!(
        age160_record_keys(&records[1]),
        vec![
            "capture_method",
            "chain_id",
            "error_chain",
            "event_name",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "marker_emitted",
            "operation_result",
            "provider_source",
            "resume_input_id",
            "session_id",
        ]
    );
    assert_eq!(
        age160_record_keys(&records[2]),
        vec![
            "chain_id",
            "error_category",
            "error_chain",
            "event_name",
            "exit_code",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "operation_result",
            "provider_source",
            "raw_artifact_paths",
            "session_id",
            "terminal_reason",
            "terminal_status",
        ]
    );

    assert_eq!(records[0]["invocation_uuid"], invocation_uuid);
    assert_eq!(records[0]["operation_result"], "ok");
    assert_eq!(records[0]["invocation_row_id"], serde_json::json!(row_id));
    assert_eq!(records[1]["capture_method"], "resumed");
    assert_eq!(records[1]["marker_emitted"], true);
    assert_eq!(records[1]["resume_input_id"], "session-age160");
    assert_eq!(records[2]["terminal_status"], "success");
    assert_eq!(records[2]["exit_code"], 0);
    assert_eq!(records[2]["terminal_reason"], "done");
}

#[test]
fn age160_post_cleanup_a6_medium_rows_resolved_or_declared() {
    let db_rs = include_str!("../../db.rs");
    let serde_direct_symbols = age160_direct_symbol_count(
        db_rs,
        &[
            "serde_json::to_string",
            "serde_json::from_str",
            "serde_json::json!",
            "serde_json::to_vec",
            "serde_json::Value",
        ],
    );
    assert!(
        serde_direct_symbols < 12 || db_rs.contains("AGE-160 serde_json residual disposition"),
        "db.rs direct serde_json symbol count must fall below the A6 MEDIUM threshold or carry a local residual disposition; count={serde_direct_symbols}"
    );
    assert!(
        db_rs.contains("crate::schema")
            && db_rs.contains("AGE-160 intrinsic schema-version carrier"),
        "db.rs must declare crate::schema as the intrinsic StateDb schema-version carrier"
    );
    assert!(
        db_rs.contains("use chrono") && db_rs.contains("AGE-160 intrinsic timestamp carrier"),
        "db.rs must declare chrono as the intrinsic StateDb timestamp carrier"
    );
}
