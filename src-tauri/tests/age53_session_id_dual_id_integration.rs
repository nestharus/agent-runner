#![cfg(unix)]
//! ## Declared roles
//! orchestration, accessor, mapper, filter, predicate, validator
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/age53_session_id_dual_id_integration.rs
//!     role: intrinsic-surface
//!     Domain: state-db-dual-id-resume-integration-test-domain
//!     Owns:
//!       - oulipoly_state::InvocationStart dual-id invocation-start contract
//!       - oulipoly_state::ModelStore model lookup contract
//!       - oulipoly_state::ProviderSessionBinding provider-session binding contract
//!       - oulipoly_state::ResumeError wrong-id error envelope
//!       - oulipoly_state::StateDb dual-id persistence and resume methods
//!       - oulipoly_state::WrongIdKindInput wrong-id classification contract
//!       - StateDb::start_invocation
//!       - StateDb::bind_invocation_provider_session_start
//!       - StateDb::get_invocation_by_uuid
//!       - StateDb::update_session_capture
//!       - StateDb::resolve_resume
//!       - oulipoly_config ModelConfig, PromptMode, and ProviderConfig fixture surface

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_state::{
    InvocationStart, ModelStore, ProviderSessionBinding, ResumeError, StateDb, WrongIdKindInput,
};
use uuid::Uuid;

fn state_db() -> (StateDb, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let db = StateDb::open(&path).unwrap();
    (db, dir)
}

fn model_store() -> ModelStore {
    let mut models = ModelStore::new();
    models.insert("test-model".to_string(), test_model_config());
    models
}

fn test_model_config() -> ModelConfig {
    ModelConfig {
        name: "test-model".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::model_provider(
            "fixture-provider",
            Vec::new(),
        )],
        inputs: Vec::new(),
    }
}

fn bound_invocation(db: &StateDb) -> (String, String) {
    let invocation_uuid = Uuid::new_v4().to_string();
    let provider_session_id = Uuid::new_v4().to_string();
    let row_id = db
        .start_invocation(&invocation_start(&invocation_uuid))
        .unwrap();
    db.bind_invocation_provider_session_start(
        row_id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();
    (invocation_uuid, provider_session_id)
}

fn invocation_start(invocation_uuid: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: invocation_uuid.to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

// Risk: anti-scope regression / start-bound provider ID clobber
// Source: proposal TI-bind_invocation_provider_session_start happy path; contract Restore the AGE-53 dual-id surface
// Level: integration exercises Phase 5 ProviderSessionBinding plus dual-id-aware update_session_capture
#[test]
fn post_run_ingest_preserves_start_bound_provider_id() {
    let (db, _dir) = state_db();
    let (invocation_uuid, provider_session_id) = bound_invocation(&db);
    let row = db
        .get_invocation_by_uuid(&invocation_uuid)
        .unwrap()
        .unwrap();
    let weaker_id = Uuid::new_v4().to_string();

    db.update_session_capture(row.id, Some(&weaker_id), "turn_script")
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some(provider_session_id.as_str())
    );
}

// Risk: wrong-id resume confusion
// Source: proposal supersede-AGE-53 wrong-id loud-failure intent; contract Restore the AGE-53 dual-id surface
// Level: integration exercises Phase 5 provider-session-aware resume resolution and error envelope
#[test]
fn resume_wrong_invocation_id_loud_failure() {
    let (db, _dir) = state_db();
    let (invocation_uuid, provider_session_id) = bound_invocation(&db);

    let err = db
        .resolve_resume(&model_store(), &invocation_uuid, None)
        .unwrap_err();

    match err {
        ResumeError::WrongIdKind {
            input,
            input_kind,
            provider_session_id: actual_provider_session_id,
            agent_runner_invocation_id,
            provider_name,
            ..
        } => {
            assert_eq!(input, invocation_uuid);
            assert_eq!(input_kind, WrongIdKindInput::AgentRunnerInvocationId);
            assert_eq!(
                actual_provider_session_id.as_deref(),
                Some(provider_session_id.as_str())
            );
            assert_eq!(agent_runner_invocation_id, input);
            assert_eq!(provider_name.as_deref(), Some("fixture-provider"));
        }
        other => panic!("expected WrongIdKind, got {other:?}"),
    }
}

// Risk: wrong-id resume JSON regression
// Source: proposal supersede-AGE-53 wrong-id JSON intent; contract Restore the AGE-53 dual-id surface
// Level: integration exercises Phase 5 dual-id WrongIdKind classification for command error projection
#[test]
fn resume_wrong_id_json_error() {
    let (db, _dir) = state_db();
    let (invocation_uuid, _) = bound_invocation(&db);

    let err = db
        .resolve_resume(&model_store(), &invocation_uuid, None)
        .unwrap_err();

    assert!(matches!(
        err,
        ResumeError::WrongIdKind {
            input_kind: WrongIdKindInput::AgentRunnerInvocationId,
            ..
        }
    ));
}
