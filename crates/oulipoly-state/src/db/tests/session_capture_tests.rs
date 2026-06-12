//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn update_session_capture_persists_verified_session_id_and_method() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_session_capture(
        id,
        Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
        "forced_flag_verified",
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.session_id.as_deref(),
        Some("5169694d-de0f-40d1-890c-6e28e55bab27")
    );
    assert_eq!(
        row.session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );
}

#[test]
fn update_session_capture_none_none_persists_none_marker() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    // Before any update: column is NULL (start_invocation doesn't set it).
    let before = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(before.session_capture_method, None);

    db.update_session_capture(id, None, "none").unwrap();

    let after = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(after.session_id, None);
    assert_eq!(
        after.session_capture_method.as_deref(),
        Some("none"),
        "completed-no-capture rows must record 'none' explicitly per V10"
    );
}

#[test]
fn update_session_capture_safe_to_call_multiple_times() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_session_capture(id, Some("first"), "forced_flag_verified")
        .unwrap();
    db.update_session_capture(id, Some("second"), "stdout_json_event")
        .unwrap();
    db.update_session_capture(id, Some("third"), "failed")
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some("third"));
    assert_eq!(row.session_capture_method.as_deref(), Some("failed"));
}

#[test]
fn update_session_capture_leaves_other_columns_alone() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "specific-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 7,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    let before = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();

    db.update_session_capture(id, Some("sid"), "forced_flag_verified")
        .unwrap();

    let after = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(after.invocation_uuid, before.invocation_uuid);
    assert_eq!(after.model_name, before.model_name);
    assert_eq!(after.provider_index, before.provider_index);
    assert_eq!(after.status, before.status);
    assert_eq!(after.created_at, before.created_at);
}

#[test]
fn update_session_capture_dual_id_semantics_for_non_resumed_and_resumed_rows() {
    let db = test_db();
    let non_resumed = seed_running_invocation(&db);
    let resumed = seed_running_invocation(&db);
    db.conn
        .execute(
            "UPDATE invocations
                 SET provider_session_id = 'active-provider-session'
                 WHERE id = ?1",
            sqlite::params![resumed],
        )
        .unwrap();

    db.update_session_capture(non_resumed, Some("new-provider-session"), "stdout")
        .unwrap();
    db.update_session_capture(resumed, Some("attempted-resume-id"), "resumed")
        .unwrap();

    let non_resumed_row = invocation_capture_projection(&db, non_resumed);
    assert_eq!(non_resumed_row.0.as_deref(), Some("new-provider-session"));
    assert_eq!(non_resumed_row.1, None);
    assert_eq!(non_resumed_row.2.as_deref(), Some("stdout"));

    let resumed_row = invocation_capture_projection(&db, resumed);
    assert_eq!(resumed_row.0.as_deref(), Some("active-provider-session"));
    assert_eq!(resumed_row.1.as_deref(), Some("attempted-resume-id"));
    assert_eq!(resumed_row.2, None);
    assert_eq!(invocation_count(&db), 2);
}

#[test]
fn record_legacy_resume_input_session_id_updates_only_resumed_row() {
    let db = test_db();
    let resumed = seed_running_invocation(&db);
    let non_resumed = seed_running_invocation(&db);
    db.update_session_capture(resumed, Some("active-session"), "resumed")
        .unwrap();
    db.update_session_capture(non_resumed, Some("provider-session"), "stdout")
        .unwrap();

    db.record_legacy_resume_input_session_id(resumed, "attempted-resume")
        .unwrap();
    db.record_legacy_resume_input_session_id(non_resumed, "must-not-apply")
        .unwrap();

    let resumed_session = invocation_session_id(&db, resumed);
    let non_resumed_session = invocation_session_id(&db, non_resumed);

    assert_eq!(resumed_session.as_deref(), Some("attempted-resume"));
    assert_eq!(non_resumed_session.as_deref(), Some("provider-session"));
    assert_eq!(invocation_count(&db), 2);
}
