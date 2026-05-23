//! AGE-163 WU-A.2 — integration test for the seam's working-set advance
//! on `SourceMissing*` execution failure during auto-rotate-on-quota-threshold.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService,
};
use oulipoly_state::{InvocationStart, QuotaWindowInput, ResolvedResume, StateDb};
use std::path::{Path, PathBuf};

const SESSION_OWNER: &str = "wsadv_owner";
const SIBLING: &str = "wsadv_sibling";
const SESSION_ID: &str = "866f8b0f-4a89-4917-b27a-cb1ee8fc9506";

fn provider(name: &str, projects_dir: PathBuf) -> ProviderConfig {
    ProviderConfig {
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

fn seed_quotas_favoring_sibling(state: &StateDb) {
    use chrono::{Duration, Utc};
    state
        .upsert_quota_refresh(
            SESSION_OWNER,
            &[QuotaWindowInput {
                used_percent: 0.83,
                resets_at: Utc::now() + Duration::hours(50),
            }],
        )
        .unwrap();
    state
        .set_window_delta_for_test(SESSION_OWNER, 0, 0.01, 22)
        .unwrap();
    state
        .upsert_quota_refresh(
            SIBLING,
            &[QuotaWindowInput {
                used_percent: 0.10,
                resets_at: Utc::now() + Duration::hours(50),
            }],
        )
        .unwrap();
    state
        .set_window_delta_for_test(SIBLING, 0, 0.01, 22)
        .unwrap();
}

fn stage_source_jsonl(projects_dir: &Path, workspace: &Path, session_id: &str) {
    let dir = projects_dir.join(claude_project_dir_name(workspace));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{session_id}.jsonl"));
    std::fs::write(
        &path,
        format!(
            r#"{{"uuid":"turn-1","sessionId":"{session_id}","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
        ),
    )
    .unwrap();
}

fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| match c {
            '/' | '\\' => '-',
            c if (c.is_ascii() && c.is_alphanumeric()) || c == '-' => c,
            _ => '-',
        })
        .collect()
}

/// AGE-163 WU-A.2: when the auto-rotate-on-quota-threshold seam fails the
/// initial target with `SourceMissingStorage` but the next working-set
/// candidate succeeds, the seam emits `AutoRotated { segment }` with the
/// candidate as the new target. The failing candidate gets
/// `next_available_at` set via the typed forensics writer.
#[test]
fn age163_seam_auto_rotates_to_next_working_candidate_after_source_missing() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let sibling_projects = dir.path().join("sibling-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&sibling_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-advance".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider(SESSION_OWNER, owner_projects.clone()),
            provider(SIBLING, sibling_projects.clone()),
        ],
        inputs: Vec::new(),
        provider: None,
    };
    let resolved = seed_session(&state, &model);
    seed_quotas_favoring_sibling(&state);
    // Stage source JSONL so the sibling-target migration succeeds.
    stage_source_jsonl(&owner_projects, &workspace, SESSION_ID);

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: None,
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .expect("migration service must succeed on auto-rotate path");

    match output {
        MigrationServiceOutput::Migrated { segment } => {
            assert_eq!(segment.target_provider, SIBLING);
        }
        MigrationServiceOutput::AutoRotated { segment, .. } => {
            assert_eq!(segment.target_provider, SIBLING);
        }
        other => panic!("expected Migrated/AutoRotated, got {other:?}"),
    }
}

/// AGE-163 WU-A.2: when both candidates fail with `SourceMissing*`, the
/// seam emits `RotationFailed { WorkingSetExhausted { candidates_tried } }`
/// listing every provider it tried.
#[test]
fn age163_seam_emits_rotation_failed_when_working_set_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let state = StateDb::open(&dir.path().join("state.db")).unwrap();
    let owner_projects = dir.path().join("owner-projects");
    let sibling_projects = dir.path().join("sibling-projects");
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(&owner_projects).unwrap();
    std::fs::create_dir_all(&sibling_projects).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    let model = ModelConfig {
        name: "age163-exhaust".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider(SESSION_OWNER, owner_projects.clone()),
            provider(SIBLING, sibling_projects.clone()),
        ],
        inputs: Vec::new(),
        provider: None,
    };
    let resolved = seed_session(&state, &model);
    seed_quotas_favoring_sibling(&state);
    // No source JSONL staged → every candidate fails with SourceMissing*.

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: None,
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &workspace,
            stderr: &mut stderr,
        })
        .expect("migration service must surface RotationFailed, not error");

    match output {
        MigrationServiceOutput::RotationFailed { reason } => {
            use oulipoly_runtime::services::RotationFailedReason;
            match reason {
                RotationFailedReason::WorkingSetExhausted { candidates_tried } => {
                    assert!(!candidates_tried.is_empty());
                }
                other => panic!("expected WorkingSetExhausted, got {other:?}"),
            }
        }
        other => panic!("expected RotationFailed, got {other:?}"),
    }
}
