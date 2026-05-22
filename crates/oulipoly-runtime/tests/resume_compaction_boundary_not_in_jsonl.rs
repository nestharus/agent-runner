use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig,
};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService,
};
use oulipoly_state::{InvocationStart, ResolvedResume, SessionTurnIngest, StateDb};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "fc7d9c2c-f197-41a6-a60f-bf2ca7a033e6";
const COMPACTION_BOUNDARY_TURN_ID: &str = "fed01dbd-96b2-41be-9831-25afcd160a2d";

struct Fixture {
    _dir: tempfile::TempDir,
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
            _dir: dir,
            state,
            source_projects,
            target_projects,
            workspace,
        }
    }

    fn model_with_storage(&self) -> ModelConfig {
        model_with_storage(&self.source_projects, &self.target_projects)
    }

    fn seed_resolved(&self, model: &ModelConfig) -> ResolvedResume {
        seed_resolved(&self.state, model)
    }

    fn seed_source_jsonl_without_boundary_turn(&self) {
        seed_source_jsonl_without_boundary_turn(&self.source_projects, &self.workspace);
    }

    fn seed_recorded_compaction_boundary(&self) {
        self.state
            .ingest_session_turns_batch(
                "claude",
                &[SessionTurnIngest {
                    session_id: SESSION_ID.to_string(),
                    turn_id: COMPACTION_BOUNDARY_TURN_ID.to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                }],
            )
            .unwrap();
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

fn seed_resolved(state: &StateDb, model: &ModelConfig) -> ResolvedResume {
    let invocation_id = state
        .start_invocation(&InvocationStart {
            invocation_uuid: "9a926742-3de6-4f9e-9e65-46a1ca4c845f".to_string(),
            model_name: model.name.clone(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    state
        .update_session_capture(invocation_id, Some(SESSION_ID), "fixture")
        .unwrap();
    state
        .mint_chain_for_invocation_session(invocation_id)
        .unwrap();
    let chain_id = state
        .chain_id_for_segment("claude", SESSION_ID)
        .unwrap()
        .unwrap();
    ResolvedResume {
        chain_id,
        model_name: Some(model.name.clone()),
        model: Some(model.clone()),
        active_provider: "claude".to_string(),
        active_session_id: SESSION_ID.to_string(),
    }
}

fn seed_source_jsonl_without_boundary_turn(source_projects: &Path, workspace: &Path) -> PathBuf {
    let source_dir = source_projects.join(claude_project_dir_name(workspace));
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_path = source_dir.join(format!("{SESSION_ID}.jsonl"));
    let first_turn = serde_json::json!({
        "uuid": "11111111-1111-4111-8111-111111111111",
        "parentUuid": null,
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:01:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "continue after compaction"}],
        },
    });
    let second_turn = serde_json::json!({
        "uuid": "22222222-2222-4222-8222-222222222222",
        "parentUuid": "11111111-1111-4111-8111-111111111111",
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:01:01Z",
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "continued"}],
        },
    });
    std::fs::write(&source_path, format!("{first_turn}\n{second_turn}\n")).unwrap();
    source_path
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

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn resume_migration_completes_for_session_with_recorded_compaction_boundary_absent_from_jsonl() {
    let fixture = Fixture::new();
    fixture.seed_source_jsonl_without_boundary_turn();
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let mut stderr = Vec::new();

    let output = ProductionMigrationService::new()
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("claude2"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .unwrap_or_else(|err| {
            panic!(
                "resume migration should complete without surfacing CompactionBoundaryNotInJsonl, got {err:?}"
            )
        });

    match output {
        MigrationServiceOutput::Migrated { segment } => {
            assert_eq!(segment.source_provider, "claude");
            assert_eq!(segment.target_provider, "claude2");
            assert_eq!(segment.source_session_id, SESSION_ID);
            assert_eq!(segment.target_session_id, SESSION_ID);
            assert!(segment.target_jsonl_path.is_file());
        }
        other => panic!("expected migrated output, got {other:?}"),
    }
}

fn seed_alternate_jsonl_with_boundary_turn(
    source_projects: &Path,
    alternate_workspace: &Path,
) -> (PathBuf, String) {
    let alt_dir = source_projects.join(claude_project_dir_name(alternate_workspace));
    std::fs::create_dir_all(&alt_dir).unwrap();
    let alt_path = alt_dir.join(format!("{SESSION_ID}.jsonl"));
    let boundary_turn = serde_json::json!({
        "uuid": COMPACTION_BOUNDARY_TURN_ID,
        "parentUuid": null,
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:00:00Z",
        "type": "assistant",
        "isCompactSummary": true,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "compact summary"}],
        },
    });
    let post_turn_a = serde_json::json!({
        "uuid": "33333333-3333-4333-8333-333333333333",
        "parentUuid": COMPACTION_BOUNDARY_TURN_ID,
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:00:30Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": "post-boundary user"}],
        },
    });
    let post_turn_b = serde_json::json!({
        "uuid": "44444444-4444-4444-8444-444444444444",
        "parentUuid": "33333333-3333-4333-8333-333333333333",
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:00:45Z",
        "type": "assistant",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "post-boundary assistant"}],
        },
    });
    let boundary_line = boundary_turn.to_string();
    std::fs::write(
        &alt_path,
        format!("{boundary_line}\n{post_turn_a}\n{post_turn_b}\n"),
    )
    .unwrap();
    (alt_path, boundary_line)
}

#[test]
fn resume_migration_slices_from_alternate_jsonl_containing_recorded_boundary() {
    let fixture = Fixture::new();
    fixture.seed_source_jsonl_without_boundary_turn();
    let alternate_workspace = fixture._dir.path().join("worktrees").join("alternate");
    let (_alt_path, boundary_line) =
        seed_alternate_jsonl_with_boundary_turn(&fixture.source_projects, &alternate_workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let mut stderr = Vec::new();

    let output = ProductionMigrationService::new()
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("claude2"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .unwrap_or_else(|err| panic!("alternate-jsonl migration should succeed, got {err:?}"));

    match output {
        MigrationServiceOutput::Migrated { segment } => {
            assert!(segment.target_jsonl_path.is_file());
            let target_contents = std::fs::read_to_string(&segment.target_jsonl_path).unwrap();
            assert!(
                target_contents.starts_with(&boundary_line),
                "expected sliced target to start with boundary line from alternate JSONL,\nactual first 200 bytes:\n{}",
                &target_contents.chars().take(200).collect::<String>()
            );
        }
        other => panic!("expected migrated output, got {other:?}"),
    }

    let stderr_text = String::from_utf8_lossy(&stderr);
    assert!(
        !stderr_text.contains("Warning:"),
        "expected no degraded-fallback Warning when alternate JSONL contains the boundary, got:\n{stderr_text}"
    );
}

#[test]
fn resume_migration_warns_when_no_candidate_jsonl_contains_recorded_boundary() {
    let fixture = Fixture::new();
    let source_path =
        seed_source_jsonl_without_boundary_turn(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let mut stderr = Vec::new();

    let output = ProductionMigrationService::new()
        .migrate(MigrationServiceRequest {
            state: &fixture.state,
            sessions_cfg: &SessionsConfig::default(),
            resolved: &resolved,
            manual_target: Some("claude2"),
            active_exhausted: false,
            migration_model: &model,
            effective_cwd: &fixture.workspace,
            stderr: &mut stderr,
        })
        .unwrap_or_else(|err| {
            panic!("no-candidate fallback migration should succeed, got {err:?}")
        });

    match output {
        MigrationServiceOutput::Migrated { segment } => {
            assert!(segment.target_jsonl_path.is_file());
        }
        other => panic!("expected migrated output, got {other:?}"),
    }

    let stderr_text = String::from_utf8_lossy(&stderr).into_owned();
    println!("[runtime-capture] stderr:\n{stderr_text}");
    assert!(
        stderr_text.contains("Warning:"),
        "expected Warning: line when no candidate JSONL contains the boundary turn id, got:\n{stderr_text}"
    );
    assert!(
        stderr_text.contains(COMPACTION_BOUNDARY_TURN_ID),
        "expected Warning: line to name the boundary turn id, got:\n{stderr_text}"
    );
    assert!(
        stderr_text.contains(SESSION_ID),
        "expected Warning: line to name the session id, got:\n{stderr_text}"
    );
    let located_filename = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    assert!(
        stderr_text.contains(located_filename),
        "expected Warning: line to reference the located JSONL filename {located_filename}, got:\n{stderr_text}"
    );
}
