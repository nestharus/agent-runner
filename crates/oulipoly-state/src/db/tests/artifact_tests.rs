//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn age132_invocation_artifact_contract_and_warning_only_failure_paths() {
    let memory = StateDb::open(Path::new(":memory:")).unwrap();
    let memory_id = memory
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    memory
        .finalize_invocation(memory_id, true, 0, None, None)
        .unwrap();
    let memory_status: String = memory
        .connection()
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            sqlite::params![memory_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memory_status, "succeeded");

    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let invocation_uuid = "99999999-9999-4999-8999-999999999999";
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let invocation_path = dir
        .path()
        .join("invocations")
        .join(format!("{invocation_uuid}.invocation"));
    assert!(invocation_path.exists());
    assert!(!invocation_path.with_extension("invocation.tmp").exists());
    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&invocation_path).unwrap()).unwrap();
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "running");
    assert_eq!(payload["model_name"], "claude-opus");
    assert_eq!(payload["provider_name"], "claude");
    assert!(payload["pid"].as_u64().is_some());
    assert!(DateTime::parse_from_rfc3339(payload["started_at"].as_str().unwrap()).is_ok());

    db.finalize_invocation(id, false, 42, Some("rate_limit"), Some("limited"))
        .unwrap();
    let result_path = dir
        .path()
        .join("invocations")
        .join(format!("{invocation_uuid}.result"));
    assert!(result_path.exists());
    assert!(!result_path.with_extension("result.tmp").exists());
    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result_path).unwrap()).unwrap();
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["exit_code"], 42);
    assert_eq!(payload["error_category"], "rate_limit");
    assert_eq!(payload["terminal_reason"], "limited");
    assert!(DateTime::parse_from_rfc3339(payload["finished_at"].as_str().unwrap()).is_ok());

    let failing_dir = tempfile::tempdir().unwrap();
    let failing = StateDb::open(&failing_dir.path().join("state.db")).unwrap();
    std::fs::write(failing_dir.path().join("invocations"), b"not a directory").unwrap();
    let id = failing
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    failing
        .finalize_invocation(id, true, 0, None, None)
        .unwrap();
    let status: String = failing
        .conn
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "succeeded");
}

#[test]
fn age132_returned_artifacts_validate_identity_bounds_and_rollback_failed_retry() {
    let db = test_db();
    let invocation_uuid = Uuid::new_v4();
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
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
