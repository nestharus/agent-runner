//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//! - parser
//! - validator
//!
//! Role set: { accessor, formatter, orchestration, parser, validator }

use super::common::*;
use super::*;
#[test]
fn age132_invocation_artifact_contract_and_warning_only_failure_paths() {
    let (memory, memory_id) = finalized_memory_invocation();
    assert_invocation_status(&memory, memory_id, "succeeded");

    let dir = artifact_tempdir();
    let db = artifact_db(dir.path());
    let invocation_uuid = "99999999-9999-4999-8999-999999999999";
    let id = start_artifact_invocation(&db, invocation_uuid);
    assert_invocation_artifact(dir.path(), invocation_uuid);

    db.finalize_invocation(id, false, 42, Some("rate_limit"), Some("limited"))
        .unwrap();
    assert_result_artifact(dir.path(), invocation_uuid);

    let (failing, id) = failing_artifact_invocation();
    failing
        .finalize_invocation(id, true, 0, None, None)
        .unwrap();
    assert_invocation_status(&failing, id, "succeeded");
}

fn finalized_memory_invocation() -> (StateDb, i64) {
    let memory = StateDb::open(Path::new(":memory:")).unwrap();
    let memory_id = start_artifact_invocation(&memory, &Uuid::new_v4().to_string());
    memory
        .finalize_invocation(memory_id, true, 0, None, None)
        .unwrap();
    (memory, memory_id)
}

fn artifact_tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn artifact_db(dir: &Path) -> StateDb {
    StateDb::open(&dir.join("state.db")).unwrap()
}

fn start_artifact_invocation(db: &StateDb, invocation_uuid: &str) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: invocation_uuid.to_string(),
        model_name: "provider-a-opus".to_string(),
        provider_name: "provider-a".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap()
}

fn assert_invocation_status(db: &StateDb, id: i64, expected: &str) {
    let status = invocation_status(db, id);
    assert_eq!(status, expected);
}

fn invocation_status(db: &StateDb, id: i64) -> String {
    db.connection()
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            sqlite::params![id],
            invocation_status_row,
        )
        .unwrap()
}

fn invocation_status_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(0)
}

fn assert_invocation_artifact(dir: &Path, invocation_uuid: &str) {
    let invocation_path = artifact_path(dir, invocation_uuid, "invocation");
    assert_complete_artifact_path(&invocation_path, "invocation");
    let payload = json_file_payload(&invocation_path);
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "running");
    assert_eq!(payload["model_name"], "provider-a-opus");
    assert_eq!(payload["provider_name"], "provider-a");
    assert!(payload["pid"].as_u64().is_some());
    assert_rfc3339_json_field(&payload, "started_at");
}

fn assert_result_artifact(dir: &Path, invocation_uuid: &str) {
    let result_path = artifact_path(dir, invocation_uuid, "result");
    assert_complete_artifact_path(&result_path, "result");
    let payload = json_file_payload(&result_path);
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["exit_code"], 42);
    assert_eq!(payload["error_category"], "rate_limit");
    assert_eq!(payload["terminal_reason"], "limited");
    assert_rfc3339_json_field(&payload, "finished_at");
}

fn artifact_path(dir: &Path, invocation_uuid: &str, extension: &str) -> std::path::PathBuf {
    dir.join("invocations")
        .join(format!("{invocation_uuid}.{extension}"))
}

fn assert_complete_artifact_path(path: &Path, extension: &str) {
    assert!(path.exists());
    assert!(!path.with_extension(format!("{extension}.tmp")).exists());
}

fn json_file_payload(path: &Path) -> serde_json::Value {
    parse_json_payload(&file_bytes(path))
}

fn file_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap()
}

fn parse_json_payload(bytes: &[u8]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap()
}

fn assert_rfc3339_json_field(payload: &serde_json::Value, field: &str) {
    assert!(parse_rfc3339_json_field(payload, field).is_ok());
}

fn parse_rfc3339_json_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<DateTime<chrono::FixedOffset>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(json_string_field(payload, field))
}

fn json_string_field<'a>(payload: &'a serde_json::Value, field: &str) -> &'a str {
    payload[field].as_str().unwrap()
}

fn failing_artifact_invocation() -> (StateDb, i64) {
    let failing_dir = artifact_tempdir();
    let failing = artifact_db(failing_dir.path());
    write_invocations_path_blocker(failing_dir.path());
    let id = start_artifact_invocation(&failing, &Uuid::new_v4().to_string());
    (failing, id)
}

fn write_invocations_path_blocker(dir: &Path) {
    std::fs::write(dir.join("invocations"), b"not a directory").unwrap();
}

#[test]
fn age132_returned_artifacts_validate_identity_bounds_and_rollback_failed_retry() {
    let db = test_db();
    let invocation_uuid = Uuid::new_v4();
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "provider-a-opus".to_string(),
            provider_name: "provider-a".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let good = returned_artifact_ref(invocation_uuid, "alpha.txt", 1);
    db.record_returned_artifacts(id, std::slice::from_ref(&good))
        .unwrap();

    let mut bad_workflow = returned_artifact_ref(invocation_uuid, "bad-workflow.txt", 1);
    bad_workflow.store_address.workflow_run_id = "not-return-namespace".to_string();
    assert!(
        db.record_returned_artifacts(id, &[bad_workflow])
            .unwrap_err()
            .contains("workflow_run_id")
    );

    let mut bad_version = returned_artifact_ref(invocation_uuid, "bad-version.txt", 1);
    bad_version.version_id = "store://wrong-version".to_string();
    assert!(
        db.record_returned_artifacts(id, &[bad_version])
            .unwrap_err()
            .contains("version_id mismatch")
    );

    let mut overflow = returned_artifact_ref(invocation_uuid, "overflow.txt", 1);
    overflow.content_len = u64::MAX;
    assert!(
        db.record_returned_artifacts(id, &[overflow])
            .unwrap_err()
            .contains("content_len exceeds SQLite INTEGER range")
    );
    assert_eq!(db.list_returned_artifacts(id).unwrap(), vec![good]);
}
