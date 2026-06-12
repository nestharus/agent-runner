//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn start_invocation_accepts_parent_rowid() {
    let db = test_db();
    let parent = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let parent_id = db.start_invocation(&parent).unwrap();

    let child = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: Some(parent_id),
    };
    db.start_invocation(&child).unwrap();

    let row = db
        .get_invocation_by_uuid(&child.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.parent_invocation_id, Some(parent_id));
}

#[test]
fn finalize_invocation_sets_terminal_fields() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.finalize_invocation(id, false, 7, None, Some("exit_nonzero"))
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(row.finished_at.is_some());
}

#[test]
fn finalize_invocation_updates_provider_aggregate_stats() {
    let db = test_db();
    let failed = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let succeeded = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    let failed_id = db.start_invocation(&failed).unwrap();
    db.finalize_invocation(
        failed_id,
        false,
        1,
        Some("rate_limit"),
        Some("429 Too Many Requests"),
    )
    .unwrap();
    let succeeded_id = db.start_invocation(&succeeded).unwrap();
    db.finalize_invocation(succeeded_id, true, 0, None, None)
        .unwrap();

    let provider = db
        .get_provider("test-model", "fixture-provider")
        .unwrap()
        .unwrap();
    assert_eq!(provider.invocation_count, 2);
    assert_eq!(provider.error_count, 1);
    assert_eq!(
        provider.last_error.as_deref(),
        Some("429 Too Many Requests")
    );
    assert!(provider.last_invoked_at.is_some());
}

#[test]
fn finalize_invocation_skips_provider_aggregate_for_null_provider_name() {
    let db = test_db();

    let mut ids = Vec::new();
    for provider_index in [0, 1] {
        db.conn
            .execute(
                "INSERT INTO invocations (
                        invocation_uuid, model_name, provider_name, provider_index,
                        status, created_at
                     ) VALUES (?1, 'legacy-model', NULL, ?2, 'running', ?3)",
                sqlite::params![
                    Uuid::new_v4().to_string(),
                    provider_index,
                    Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        ids.push(db.conn.last_insert_rowid());
    }

    db.finalize_invocation(ids[0], true, 0, None, None).unwrap();
    db.finalize_invocation(ids[1], false, 1, Some("rate_limit"), Some("429"))
        .unwrap();

    let provider_rows = provider_rows_for_model(&db, "legacy-model");
    assert_eq!(provider_rows, 0);
}

#[test]
fn finalize_invocation_errors_for_missing_row() {
    let db = test_db();
    let err = db
        .finalize_invocation(99, false, 1, Some("rate_limit"), None)
        .unwrap_err();
    assert!(err.contains("99"));
}

#[test]
fn finalize_invocation_errors_when_called_twice() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    db.finalize_invocation(id, true, 0, None, Some("exit_zero"))
        .unwrap();

    let err = db
        .finalize_invocation(
            id,
            false,
            -1,
            None,
            Some("supervisor_observed_unknown_exit"),
        )
        .unwrap_err();
    assert!(err.contains("already"));

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_zero"));
}
