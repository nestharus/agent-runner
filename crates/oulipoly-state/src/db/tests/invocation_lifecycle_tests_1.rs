//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn start_invocation_inserts_running_row_with_null_terminal_fields() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    let id = db.start_invocation(&start).unwrap();
    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();

    assert_eq!(row.id, id);
    assert_eq!(row.status, InvocationStatus::Running);
    assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
    assert_eq!(row.parent_invocation_id, None);
    assert_eq!(row.success, None);
    assert_eq!(row.exit_code, None);
    assert_eq!(row.terminal_reason, None);
    assert_eq!(row.finished_at, None);
}

#[test]
fn running_invocation_provider_session_id() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Running);
    assert_eq!(row.finished_at, None);
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some(provider_session_id.as_str())
    );
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );
}

#[test]
fn running_invocation_chain_minted() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    let chain_id = db
        .chain_id_for_segment("fixture-provider", &provider_session_id)
        .unwrap()
        .expect("chain segment must be minted");
    assert!(Uuid::parse_str(&chain_id).is_ok());
}

#[test]
fn bind_invocation_provider_session_start_same_id_is_idempotent() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: provider_session_id.clone(),
        capture_method: "forced_flag_verified",
        resume_input_id: None,
        provider_session_resolved_account: None,
    };

    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();
    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();

    assert_eq!(segment_count(&db), 1);
    assert!(
        db.chain_id_for_segment("fixture-provider", &provider_session_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn bind_invocation_provider_session_start_conflicting_rebind_rejects_without_mutation() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();
    let before_segments = segment_count(&db);

    let err = db
        .bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: Uuid::new_v4().to_string(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap_err();

    assert!(
        err.contains("already bound") || err.contains("refusing"),
        "{err}"
    );
    assert_eq!(segment_count(&db), before_segments);
    let stored = invocation_provider_session_id(&db, id);
    assert_eq!(stored.as_deref(), Some(provider_session_id.as_str()));
}

#[test]
fn bind_invocation_provider_session_start_matching_resume_input_does_not_mint_duplicate_chain() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "resumed",
            resume_input_id: Some(provider_session_id.clone()),
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    assert_eq!(segment_count(&db), 0);
    let row = invocation_provider_and_resume_input_ids(&db, id);
    assert_eq!(row.0.as_deref(), Some(provider_session_id.as_str()));
    assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
}

#[test]
fn bind_then_record_legacy_then_rebind_preserves_legacy_resume_session_id() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    let legacy_resume_input = Uuid::new_v4().to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: provider_session_id.clone(),
        capture_method: "resumed",
        resume_input_id: Some(legacy_resume_input.clone()),
        provider_session_resolved_account: None,
    };

    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();
    db.record_legacy_resume_input_session_id(id, &legacy_resume_input)
        .unwrap();
    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();

    let row = invocation_session_provider_resume_ids(&db, id);
    assert_eq!(row.0.as_deref(), Some(legacy_resume_input.as_str()));
    assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
    assert_eq!(row.2.as_deref(), Some(legacy_resume_input.as_str()));
}

#[test]
fn start_invocation_rejects_duplicate_uuid() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    db.start_invocation(&start).unwrap();
    let err = db.start_invocation(&start).unwrap_err();
    assert!(err.contains("invocation"));
}
