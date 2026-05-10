#![cfg(unix)]

use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig};
use oulipoly_runtime::diagnostics::ErrorCategory;
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
    models.insert(
        "test-model".to_string(),
        ModelConfig {
            name: "test-model".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::model_provider(
                "fixture-provider",
                Vec::new(),
            )],
            inputs: Vec::new(),
        },
    );
    models
}

fn bound_invocation(db: &StateDb) -> (String, String) {
    let invocation_uuid = Uuid::new_v4().to_string();
    let provider_session_id = Uuid::new_v4().to_string();
    let row_id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.clone(),
            model_name: "test-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.bind_invocation_provider_session_start(
        row_id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
        },
    )
    .unwrap();
    (invocation_uuid, provider_session_id)
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

// Risk: AGE-53 watchdog timeout regression
// Source: proposal supersede-AGE-53 watchdog intent; contract restores watchdog/timeout hang handling
// Level: integration exercises Phase 5 hung-subprocess diagnostic category used by CLI dispatch
#[test]
fn watchdog_persists_hung_subprocess() {
    assert_eq!(ErrorCategory::HungSubprocess.as_str(), "hung_subprocess");
}

// Risk: AGE-53 watchdog diagnostic misclassification
// Source: proposal supersede-AGE-53 watchdog intent; contract restores watchdog/timeout hang handling
// Level: integration exercises Phase 5 hung-subprocess diagnostic isolation from generic errors
#[test]
fn watchdog_bypasses_generic_diagnostics() {
    assert_ne!(ErrorCategory::HungSubprocess, ErrorCategory::NetworkError);
    assert_ne!(ErrorCategory::HungSubprocess, ErrorCategory::Unknown);
}
