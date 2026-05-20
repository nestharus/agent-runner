//! AGE-162 Symptoms 1+4 — AGE-148's `DecisionFailed` guard silently absorbs
//! the auto-migrate-on-quota-threshold path when the source JSONL is missing.
//!
//! Headline from `supplementary-evidence-resume-pinning.md`: root confirmed
//! "routing was disabled" and traced the regression to
//! `crates/oulipoly-runtime/src/services/migration.rs:60-69`, the match arm
//! added by AGE-148 (PR #109, commit `f4b7364`):
//!
//! ```rust,ignore
//! Err(
//!     err @ (MigrationError::SourceMissingStorage { .. }
//!     | MigrationError::SourceMissing { .. }),
//! ) if request.manual_target.is_none()
//!     && reason == TransitionReason::QuotaThreshold =>
//! {
//!     Ok(MigrationServiceOutput::DecisionFailed { warning: format!("{err:?}") })
//! }
//! ```
//!
//! AGE-100 (`c77fdc7 fix(runtime): migrate resume dispatch on quota_exhausted
//! heuristic`) was the predecessor that established the auto-migrate-on-
//! quota-threshold contract: when the active provider is heavily loaded and
//! a healthier sibling exists, resume dispatch SHOULD rotate. AGE-148's new
//! guard intercepts the `SourceMissingStorage` / `SourceMissing` error class
//! and turns it into `MigrationServiceOutput::DecisionFailed`. The caller in
//! `src-tauri/src/main.rs` lines 2271-2272 + 2767-2768 treats `DecisionFailed`
//! identically to `Stay` via the no-op `{}` arm:
//!
//! ```rust,ignore
//! Ok(MigrationServiceOutput::Stay)
//! | Ok(MigrationServiceOutput::DecisionFailed { .. }) => {}
//! ```
//!
//! Net effect: when `migrate_chain_segment` cannot find the source JSONL on
//! the auto-migrate-on-quota-threshold path, the runtime silently keeps the
//! original (loaded) provider and dispatches there. The downstream symptom
//! is the live AGE-159 trace `OULIPOLY_INVOCATION={"source":"claude5",...}`
//! followed by `[diagnostics] rate_limit` on the over-loaded provider.
//!
//! Expected contract derived from AGE-100 intent: when `decide_migration`
//! returns `Migrate { _, QuotaThreshold }` and the inner migrate fails with
//! a SourceMissing-class error, the service output MUST NOT be a
//! `DecisionFailed` variant the caller short-circuits as a no-op. The caller
//! has no way to distinguish "migration succeeded; stay" from "migration
//! failed; pretend it didn't happen" once the guard collapses them.

use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::balancer::{MigrationDecision, TransitionReason, decide_migration};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService,
};
use oulipoly_state::{InvocationStart, QuotaWindowInput, ResolvedResume, StateDb};
use std::path::{Path, PathBuf};

const SESSION_OWNER: &str = "claude5_repro";
const SIBLING: &str = "claude4_repro";
const SESSION_ID: &str = "866f8b0f-4a89-4917-b27a-cb1ee8fc9506";

struct Fixture {
    _dir: tempfile::TempDir,
    state: StateDb,
    owner_projects: PathBuf,
    sibling_projects: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let owner_projects = dir.path().join("claude5-projects");
        let sibling_projects = dir.path().join("claude4-projects");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&owner_projects).unwrap();
        std::fs::create_dir_all(&sibling_projects).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        Self {
            _dir: dir,
            state,
            owner_projects,
            sibling_projects,
            workspace,
        }
    }

    fn model(&self) -> ModelConfig {
        ModelConfig {
            name: "age162-repro".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                provider_config(SESSION_OWNER, self.owner_projects.clone()),
                provider_config(SIBLING, self.sibling_projects.clone()),
            ],
            inputs: Vec::new(),
        }
    }

    fn seed_resolved(&self, model: &ModelConfig) -> ResolvedResume {
        let invocation_row_id = self
            .state
            .start_invocation(&InvocationStart {
                invocation_uuid: uuid::Uuid::new_v4().to_string(),
                model_name: model.name.clone(),
                provider_name: SESSION_OWNER.to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        self.state
            .update_session_capture(invocation_row_id, Some(SESSION_ID), "fixture")
            .unwrap();
        self.state
            .mint_chain_for_invocation_session(invocation_row_id)
            .unwrap();
        let chain_id = self
            .state
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
}

fn provider_config(name: &str, projects_dir: PathBuf) -> ProviderConfig {
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

/// Seed quotas so `decide_migration` returns `Migrate { idx=1, QuotaThreshold }`.
///
/// Setup mirrors the live AGE-159 snapshot intent: the session owner
/// (`claude5_repro`) is heavily loaded but NOT yet marked `exhausted_at`
/// (the auto-migrate-on-quota-threshold path is precisely what runs when
/// projections favor rotation but no formal exhausted flag is set). The
/// sibling (`claude4_repro`) is fresh.
///
/// Per-window deltas are seeded so the projection layer's `bootstrap_burn_rate`
/// returns a finite value and projections produce real `projected_used`
/// numbers; otherwise both providers come out unlearned/tied and
/// `quota_threshold_migration_decision` returns `Stay`.
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

/// (a) Unit-level pin against `ProductionMigrationService::migrate`.
///
/// Preconditions:
/// - 2 ClaudeCode providers; session owner (index 0) is heavily loaded;
///   sibling (index 1) is fresh.
/// - Quota deltas seeded so projections yield real numbers.
/// - Source JSONL is intentionally NOT created on disk → `migrate_chain_segment`
///   fails with `MigrationError::SourceMissingStorage`.
/// - `manual_target = None` (auto-migrate path).
///
/// Expected (per AGE-100 intent, root attestation): the response must NOT be
/// `MigrationServiceOutput::DecisionFailed`. The caller's no-op arm
/// (`Ok(Stay) | Ok(DecisionFailed { .. }) => {}`) means a `DecisionFailed`
/// return silently keeps the runtime dispatching to the over-loaded session
/// owner — exactly what root reported as "routing was disabled".
///
/// Current code (AGE-148 guard at `services/migration.rs:60-69`) returns
/// `Ok(DecisionFailed { warning })`. This test fails RED.
#[test]
fn age162_decision_failed_does_not_block_quota_threshold_rotation() {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    seed_quotas_favoring_sibling(&fixture.state);

    let decision = decide_migration(&fixture.state, &model, &resolved, None).unwrap();
    assert_eq!(
        decision,
        MigrationDecision::Migrate {
            target_provider_index: 1,
            reason: TransitionReason::QuotaThreshold,
        },
        "fixture precondition: decide_migration must surface a QuotaThreshold \
         migration so the AGE-148 guard arm is the one under test (not the \
         Exhausted or Manual arms)"
    );

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: None,
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .expect("migration service must not return ServiceError on the \
                 SourceMissingStorage+QuotaThreshold path under AGE-148");

    match &output {
        MigrationServiceOutput::DecisionFailed { warning } => panic!(
            "AGE-162 Symptoms 1+4: services::migration::migrate returned \
             DecisionFailed on the auto-migrate-on-quota-threshold path with \
             a SourceMissingStorage inner error. The src-tauri caller \
             (`main.rs` lines 2271-2272 + 2767-2768) treats DecisionFailed \
             as a Stay no-op, so this return value silently disables \
             routing — root-attested as the AGE-148 regression of AGE-100's \
             auto-rotate intent. Warning was: {warning:?}"
        ),
        MigrationServiceOutput::Stay => panic!(
            "AGE-162 Symptoms 1+4: migration service returned Stay despite \
             decide_migration surfacing Migrate{{1, QuotaThreshold}}. AGE-100's \
             intent is to rotate when a healthier sibling exists; the caller \
             needs a signal it can act on, not Stay."
        ),
        MigrationServiceOutput::Migrated { segment } => {
            // Acceptable: real migration succeeded by some other path.
            // The contract this test pins is "NOT DecisionFailed"; an actual
            // Migrated output would mean the bug is gone.
            assert_eq!(segment.target_provider, SIBLING);
        }
    }
}

/// Branch-coverage companion: when the same fixture supplies a `manual_target`
/// (so the migration is `Manual` not `QuotaThreshold`), or when the source
/// JSONL DOES exist, the AGE-148 guard arm MUST NOT fire. The guard is
/// supposed to be a narrow short-circuit; this test pins the narrowness.
///
/// Setup: identical to the failing test above except a source JSONL is staged.
/// `migrate_chain_segment` succeeds → output is `Migrated{...}`. The point of
/// this test is to ensure the failing test above is not a fixture artifact:
/// the only difference is the missing source JSONL, which is exactly the
/// trigger condition the AGE-148 guard intercepts.
#[test]
fn age162_decision_failed_arm_does_not_fire_when_source_jsonl_is_present() {
    let fixture = Fixture::new();
    let model = fixture.model();
    let resolved = fixture.seed_resolved(&model);
    seed_quotas_favoring_sibling(&fixture.state);
    stage_source_jsonl(&fixture.owner_projects, &fixture.workspace, SESSION_ID);

    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();
    let output = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: None,
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .expect("happy-path migration must not return ServiceError");

    match &output {
        MigrationServiceOutput::Migrated { segment } => {
            assert_eq!(segment.target_provider, SIBLING);
            assert_eq!(segment.reason, TransitionReason::QuotaThreshold);
        }
        other => panic!(
            "with source JSONL present, the migration service must produce \
             Migrated; got {other:?}. If this passes Stay, the fixture is not \
             surfacing the QuotaThreshold decision path and the sibling test \
             above is not exercising the AGE-148 guard."
        ),
    }
}

fn stage_source_jsonl(owner_projects: &Path, workspace: &Path, session_id: &str) {
    let dir = owner_projects.join(claude_project_dir_name(workspace));
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
