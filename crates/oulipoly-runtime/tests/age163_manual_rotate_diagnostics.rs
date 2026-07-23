//! AGE-163 WU-A.5 — typed manual-rotate rejection translation.
//!
//! Drives `ProductionMigrationService::migrate` with a manual target and
//! asserts that each of the four `ManualMigrationRejection` cases is
//! surfaced as the matching `RotationFailedReason` variant. The
//! src-tauri operator-facing diagnostic wording is owned by `main.rs`'s
//! `format_rotation_failed_reason` helper.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService, RotationFailedReason,
};
use oulipoly_state::{InvocationStart, ResolvedResume, StateDb};
use std::path::PathBuf;

const SESSION_OWNER: &str = "claude_owner_age163";
const SESSION_ID: &str = "8a3f1e7e-2a1b-4a8c-9876-1234567890ab";

fn provider_claude(name: &str, projects_dir: PathBuf) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: Some(SessionStorage::ClaudeCode { projects_dir }),
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn provider_no_storage(name: &str) -> ProviderConfig {
    ProviderConfig {
        environment: Default::default(),
        unset_environment: Default::default(),
        name: name.to_string(),
        command: name.to_string(),
        args: Vec::new(),
        interactive_args: Some(vec!["launch".to_string()]),
        resume: Some(ResumeStrategy {
            kind: ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }),
        session_capture: None,
        resume_acceptance: None,
        session_storage: None,
        system_prompt_override: None,
        tool_restrictions: None,
        invocation_mode: Default::default(),
    }
}

fn seed_session(state: &StateDb, model: &ModelConfig) -> ResolvedResume {
    let invocation_row_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: model.name.clone(),
            provider_name: SESSION_OWNER.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .update_session_capture(invocation_row_id, Some(SESSION_ID), "fixture")
        .unwrap();
    state
        .mint_chain_for_invocation_session(invocation_row_id)
        .unwrap();
    let chain_id = state
        .chain_id_for_segment(SESSION_OWNER, SESSION_ID)
        .unwrap()
        .unwrap();
    ResolvedResume {
        chain_id,
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: SESSION_OWNER.to_string(),
        active_session_id: SESSION_ID.to_string(),
    }
}

#[test]
fn age163_seam_emits_manual_target_not_in_pool_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let sibling_projects = dir.path().join("sibling-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&sibling_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-manual".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider_claude(SESSION_OWNER, owner_projects.clone()),
            provider_claude("claude_sibling_age163", sibling_projects.clone()),
        ],
        inputs: Vec::new(),
        provider: None,
    };
    let resolved = seed_session(&state, &model);

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("nonexistent_target"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .unwrap();
    match output {
        MigrationServiceOutput::RotationFailed { reason } => match reason {
            RotationFailedReason::ManualTargetNotInPool { target, pool } => {
                assert_eq!(target, "nonexistent_target");
                assert_eq!(pool.len(), 2);
            }
            other => panic!("expected ManualTargetNotInPool, got {other:?}"),
        },
        other => panic!("expected RotationFailed, got {other:?}"),
    }
}

#[test]
fn age163_seam_emits_manual_target_not_migratable_pair_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-pair".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider_claude(SESSION_OWNER, owner_projects.clone()),
            provider_no_storage("script_target"),
        ],
        inputs: Vec::new(),
        provider: None,
    };
    let resolved = seed_session(&state, &model);

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("script_target"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .unwrap();
    match output {
        MigrationServiceOutput::RotationFailed { reason } => match reason {
            RotationFailedReason::ManualTargetNotMigratable { source, target } => {
                assert_eq!(source, SESSION_OWNER);
                assert_eq!(target, "script_target");
            }
            other => panic!("expected ManualTargetNotMigratable, got {other:?}"),
        },
        other => panic!("expected RotationFailed, got {other:?}"),
    }
}

#[test]
fn age163_seam_emits_single_provider_pool_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-single".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider_claude(SESSION_OWNER, owner_projects.clone())],
        inputs: Vec::new(),
        provider: None,
    };
    let resolved = seed_session(&state, &model);

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("anything"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .unwrap();
    match output {
        MigrationServiceOutput::RotationFailed { reason } => match reason {
            RotationFailedReason::ManualTargetIsSingleProviderPool { provider } => {
                assert_eq!(provider, SESSION_OWNER);
            }
            other => panic!("expected ManualTargetIsSingleProviderPool, got {other:?}"),
        },
        other => panic!("expected RotationFailed, got {other:?}"),
    }
}

#[test]
fn age163_seam_emits_manual_target_active_not_in_pool_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let sibling_projects = dir.path().join("sibling-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&sibling_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-active-not".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider_claude(SESSION_OWNER, owner_projects.clone()),
            provider_claude("claude_sibling_age163", sibling_projects.clone()),
        ],
        inputs: Vec::new(),
        provider: None,
    };
    let mut resolved = seed_session(&state, &model);
    resolved.active_provider = "archived_provider".to_string();

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("claude_sibling_age163"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .unwrap();
    match output {
        MigrationServiceOutput::RotationFailed { reason } => match reason {
            RotationFailedReason::ManualTargetActiveNotInPool { active } => {
                assert_eq!(active, "archived_provider");
            }
            other => panic!("expected ManualTargetActiveNotInPool, got {other:?}"),
        },
        other => panic!("expected RotationFailed, got {other:?}"),
    }
}
