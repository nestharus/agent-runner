use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::balancer::{MigrationDecision, decide_migration};
use oulipoly_runtime::migration::{MigratedSegment, migrate_chain_segment};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService, ServiceError,
};
use oulipoly_state::{InvocationStart, ResolvedResume, StateDb};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const SESSION_A: &str = "dd116a3c-6819-42b1-b3d2-f512331eb5ec";

struct Fixture {
    dir: tempfile::TempDir,
    state: StateDb,
    source_projects: PathBuf,
    target_projects: PathBuf,
    workspace: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state = StateDb::open(&dir.path().join("state.db")).unwrap();
        let source_projects = dir.path().join("source-projects");
        let target_projects = dir.path().join("target-projects");
        let workspace = dir.path().join("worktrees").join("same-workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        Self {
            dir,
            state,
            source_projects,
            target_projects,
            workspace,
        }
    }

    fn conn(&self) -> Connection {
        Connection::open(self.dir.path().join("state.db")).unwrap()
    }

    fn model_with_storage(&self) -> ModelConfig {
        model_with_storage(&self.source_projects, &self.target_projects)
    }

    fn seed_resolved(&self, model: &ModelConfig, session_id: &str) -> ResolvedResume {
        seed_resolved(&self.state, model, session_id)
    }

    fn seed_source_jsonl(&self, session_id: &str) -> PathBuf {
        seed_source_jsonl(&self.source_projects, &self.workspace, session_id)
    }
}

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

fn model_with_storage(source_projects: &Path, target_projects: &Path) -> ModelConfig {
    ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider("claude", source_projects.to_path_buf()),
            provider("claude2", target_projects.to_path_buf()),
        ],
        inputs: Vec::new(),
    }
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

fn seed_source_jsonl(source_projects: &Path, source_workspace: &Path, session_id: &str) -> PathBuf {
    let source_dir = source_projects.join(claude_project_dir_name(source_workspace));
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_path = source_dir.join(format!("{session_id}.jsonl"));
    std::fs::write(
        &source_path,
        format!(
            r#"{{"uuid":"turn-1","sessionId":"{session_id}","timestamp":"2026-04-17T08:00:00Z","type":"assistant"}}"#
        ),
    )
    .unwrap();
    source_path
}

fn seed_resolved(state: &StateDb, model: &ModelConfig, session_id: &str) -> ResolvedResume {
    let invocation_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            model_name: model.name.clone(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .update_session_capture(invocation_id, Some(session_id), "fixture")
        .unwrap();
    state
        .mint_chain_for_invocation_session(invocation_id)
        .unwrap();
    let chain_id = state
        .chain_id_for_segment("claude", session_id)
        .unwrap()
        .unwrap();
    ResolvedResume {
        chain_id,
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "claude".to_string(),
        active_session_id: session_id.to_string(),
    }
}

fn assert_segment_same(actual: &MigratedSegment, expected: &MigratedSegment) {
    assert_eq!(actual.source_provider, expected.source_provider);
    assert_eq!(actual.source_session_id, expected.source_session_id);
    assert_eq!(actual.target_provider, expected.target_provider);
    assert_eq!(actual.target_provider_index, expected.target_provider_index);
    assert_eq!(actual.target_session_id, expected.target_session_id);
    assert_eq!(actual.reason, expected.reason);
    // chain_id and target_jsonl_path are fixture-specific: chain ids are minted
    // with random UUIDs and each parity run uses its own temp project root.
    assert!(!actual.chain_id.is_empty(), "actual chain_id should be set");
    assert!(
        uuid::Uuid::parse_str(&actual.chain_id).is_ok(),
        "actual chain_id should be a UUID"
    );
    assert!(
        !expected.chain_id.is_empty(),
        "expected chain_id should be set"
    );
    assert!(
        uuid::Uuid::parse_str(&expected.chain_id).is_ok(),
        "expected chain_id should be a UUID"
    );
    assert!(
        actual.target_jsonl_path.is_file(),
        "actual target JSONL should exist"
    );
    assert!(
        expected.target_jsonl_path.is_file(),
        "expected target JSONL should exist"
    );
}

#[test]
fn migration_service_stay_matches_decide_migration() {
    let fixture = Fixture::new();
    let model = ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider("claude", fixture.source_projects.clone())],
        inputs: Vec::new(),
    };
    let resolved = fixture.seed_resolved(&model, SESSION_A);
    let decision = decide_migration(&fixture.state, &model, &resolved, None).unwrap();
    assert!(matches!(decision, MigrationDecision::Stay));
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
        .expect("stay decision is not an infrastructure failure");

    assert!(matches!(output, MigrationServiceOutput::Stay));
    assert!(stderr.is_empty());
}

#[test]
fn migration_service_parity_unchanged() {
    let fixture = Fixture::new();
    let model = ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider("claude", fixture.source_projects.clone())],
        inputs: Vec::new(),
    };
    let resolved = fixture.seed_resolved(&model, SESSION_A);
    let mut stderr = Vec::new();

    let output = ProductionMigrationService::new()
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
        .expect("stay migration remains a non-error service output");

    assert!(matches!(output, MigrationServiceOutput::Stay));
    assert!(stderr.is_empty());
}

#[test]
fn migration_service_decision_failure_is_nonfatal_output() {
    let fixture = Fixture::new();
    let model = fixture.model_with_storage();
    let resolved = fixture.seed_resolved(&model, SESSION_A);
    // Force the same dependency failure path that `decide_migration` would surface
    // from a broken state store, while keeping the adapter under test concrete.
    fixture
        .conn()
        .execute_batch("DROP TABLE provider_quotas;")
        .unwrap();
    let mut stderr = Vec::new();
    let service = ProductionMigrationService::new();

    let output = service.migrate(MigrationServiceRequest {
        state: &fixture.state,
        sessions_cfg: &SessionsConfig::default(),
        resolved: &resolved,
        manual_target: None,
        active_exhausted: false,
        migration_model: &model,
        effective_cwd: &fixture.workspace,
        stderr: &mut stderr,
    });

    assert!(
        matches!(
            &output,
            Ok(MigrationServiceOutput::DecisionFailed { warning }) if !warning.is_empty()
        ),
        "expected nonfatal decision failure with a warning, got {output:?}"
    );
    if let Ok(MigrationServiceOutput::DecisionFailed { warning }) = &output {
        assert!(
            warning.contains("provider_quotas"),
            "warning should mention the missing table: {warning}"
        );
    }
    assert!(stderr.is_empty());
}

#[test]
fn migration_service_migrate_matches_decide_and_migrate_chain_segment() {
    let direct_fixture = Fixture::new();
    direct_fixture.seed_source_jsonl(SESSION_A);
    let direct_model = direct_fixture.model_with_storage();
    let direct_resolved = direct_fixture.seed_resolved(&direct_model, SESSION_A);
    let direct_decision = decide_migration(
        &direct_fixture.state,
        &direct_model,
        &direct_resolved,
        Some("claude2"),
    )
    .unwrap();
    let MigrationDecision::Migrate {
        target_provider_index,
        reason,
    } = direct_decision
    else {
        panic!("manual target should migrate");
    };
    let mut direct_stderr = Vec::new();
    let expected = migrate_chain_segment(
        &direct_fixture.state,
        &SessionsConfig::default(),
        &direct_model,
        &direct_resolved,
        &direct_fixture.workspace,
        target_provider_index,
        reason,
        &mut direct_stderr,
    )
    .unwrap();

    let service_fixture = Fixture::new();
    service_fixture.seed_source_jsonl(SESSION_A);
    let service_model = service_fixture.model_with_storage();
    let service_resolved = service_fixture.seed_resolved(&service_model, SESSION_A);
    let mut service_stderr = Vec::new();
    let service = ProductionMigrationService::new();

    let output = service
        .migrate(MigrationServiceRequest {
            state: &service_fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &service_resolved,
            manual_target: Some("claude2"),
            active_exhausted: false,
            migration_model: &service_model,
            effective_cwd: &service_fixture.workspace,
            stderr: &mut service_stderr,
        })
        .expect("service migration succeeds");

    match output {
        MigrationServiceOutput::Migrated { segment } => {
            assert_segment_same(&segment, &expected);
        }
        other => panic!("expected migrated output, got {other:?}"),
    }
    assert_eq!(service_stderr, direct_stderr);
}

#[test]
fn migration_service_dispatch_failure_maps_to_dependency_and_cli_migration_failed() {
    let fixture = Fixture::new();
    let model = fixture.model_with_storage();
    let resolved = fixture.seed_resolved(&model, SESSION_A);
    let decision = decide_migration(&fixture.state, &model, &resolved, Some("claude2")).unwrap();
    let MigrationDecision::Migrate {
        target_provider_index,
        reason,
    } = decision
    else {
        panic!("manual target should migrate");
    };
    let mut direct_stderr = Vec::new();
    let expected_message = format!(
        "{:?}",
        migrate_chain_segment(
            &fixture.state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            &fixture.workspace,
            target_provider_index,
            reason,
            &mut direct_stderr,
        )
        .unwrap_err()
    );
    let mut service_stderr = Vec::new();
    let service = ProductionMigrationService::new();

    let err = service
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("claude2"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut service_stderr,
        })
        .unwrap_err();

    match err {
        ServiceError::Dependency { message } => {
            assert_eq!(message, expected_message);
        }
        other => panic!("expected dependency error, got {other:?}"),
    }
    assert_eq!(service_stderr, direct_stderr);
}
