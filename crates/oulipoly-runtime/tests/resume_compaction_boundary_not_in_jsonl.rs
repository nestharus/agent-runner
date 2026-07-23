use chrono::{DateTime, Utc};
use oulipoly_config::{
    ModelConfig, PromptMode, ProviderConfig, ResumeKind, ResumeStrategy, SessionStorage,
    SessionsConfig, provider_implementation_ref::ProviderImplementationRef,
};
use oulipoly_runtime::balancer::TransitionReason;
use oulipoly_runtime::migration::{
    MigrationError, ProviderRefBoundOutcome, bound_provider_ref_resume_segment,
};
use oulipoly_runtime::services::{
    MigrationServiceOutput, MigrationServicePort, MigrationServiceRequest,
    ProductionMigrationService, ServiceError,
};
use oulipoly_state::{InvocationStart, ResolvedResume, SessionTurnIngest, StateDb};
use std::path::{Path, PathBuf};

const SESSION_ID: &str = "fc7d9c2c-f197-41a6-a60f-bf2ca7a033e6";
const COMPACTION_BOUNDARY_TURN_ID: &str = "fed01dbd-96b2-41be-9831-25afcd160a2d";
const FRESH_SESSION_ID: &str = "7d76dcfd-cc16-44db-89d5-3c2bca11caba";
const COLLIDING_FRESH_SESSION_ID: &str = "6d5d7b9d-8182-4f69-9890-95d586d9e9aa";
const SECOND_FRESH_SESSION_ID: &str = "7808f6a4-fd1f-485c-8a4d-394e5d2805a2";

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

    fn provider_ref_model_with_storage(&self) -> ModelConfig {
        provider_ref_model_with_storage(&self.source_projects)
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

fn model_with_storage(source_projects: &Path, target_projects: &Path) -> ModelConfig {
    ModelConfig {
        name: "claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            provider("claude", source_projects.to_path_buf()),
            provider("claude2", target_projects.to_path_buf()),
        ],
        inputs: Vec::new(),
        provider: None,
    }
}

fn provider_ref_model_with_storage(source_projects: &Path) -> ModelConfig {
    ModelConfig {
        name: "provider-ref-claude-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![provider("claude", source_projects.to_path_buf())],
        inputs: Vec::new(),
        provider: Some(ProviderImplementationRef {
            path: Some("/synthetic/provider-ref-fixture".to_string()),
            crate_name: None,
            version: None,
            binary: None,
            script: None,
        }),
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

#[derive(Debug)]
struct JsonlTailFixture {
    source_path: PathBuf,
    pre_boundary_line: String,
    boundary_line: String,
    post_boundary_user_line: String,
    post_boundary_assistant_line: String,
}

fn seed_source_jsonl_with_boundary_turn(
    source_projects: &Path,
    workspace: &Path,
) -> JsonlTailFixture {
    seed_source_jsonl_with_boundary_turn_and_pre_text(
        source_projects,
        workspace,
        "pre-boundary body",
    )
}

fn seed_source_jsonl_with_boundary_turn_and_pre_text(
    source_projects: &Path,
    workspace: &Path,
    pre_boundary_text: &str,
) -> JsonlTailFixture {
    let source_dir = source_projects.join(claude_project_dir_name(workspace));
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_path = source_dir.join(format!("{SESSION_ID}.jsonl"));
    let pre_boundary = serde_json::json!({
        "uuid": "00000000-0000-4000-8000-000000000001",
        "parentUuid": null,
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T07:59:00Z",
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "text", "text": pre_boundary_text}],
        },
    });
    let boundary = serde_json::json!({
        "uuid": COMPACTION_BOUNDARY_TURN_ID,
        "parentUuid": "00000000-0000-4000-8000-000000000001",
        "sessionId": SESSION_ID,
        "timestamp": "2026-04-17T08:00:00Z",
        "type": "assistant",
        "isCompactSummary": true,
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "compact summary"}],
        },
    });
    let post_user = serde_json::json!({
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
    let post_assistant = serde_json::json!({
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
    let pre_boundary_line = pre_boundary.to_string();
    let boundary_line = boundary.to_string();
    let post_boundary_user_line = post_user.to_string();
    let post_boundary_assistant_line = post_assistant.to_string();
    std::fs::write(
        &source_path,
        format!(
            "{pre_boundary_line}\n{boundary_line}\n{post_boundary_user_line}\n{post_boundary_assistant_line}\n"
        ),
    )
    .unwrap();
    JsonlTailFixture {
        source_path,
        pre_boundary_line,
        boundary_line,
        post_boundary_user_line,
        post_boundary_assistant_line,
    }
}

fn seed_source_jsonl_boundary_at_head(
    source_projects: &Path,
    workspace: &Path,
) -> JsonlTailFixture {
    let source_dir = source_projects.join(claude_project_dir_name(workspace));
    std::fs::create_dir_all(&source_dir).unwrap();
    let source_path = source_dir.join(format!("{SESSION_ID}.jsonl"));
    let boundary = serde_json::json!({
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
    let post_user = serde_json::json!({
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
    let boundary_line = boundary.to_string();
    let post_boundary_user_line = post_user.to_string();
    std::fs::write(
        &source_path,
        format!("{boundary_line}\n{post_boundary_user_line}\n"),
    )
    .unwrap();
    JsonlTailFixture {
        source_path,
        pre_boundary_line: String::new(),
        boundary_line,
        post_boundary_user_line,
        post_boundary_assistant_line: String::new(),
    }
}

fn seed_turn_history_with_compaction_boundary(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) {
    state
        .ingest_session_turns_batch(
            provider_name,
            &[
                SessionTurnIngest {
                    session_id: session_id.to_string(),
                    turn_id: "00000000-0000-4000-8000-000000000001".to_string(),
                    timestamp: ts("2026-04-17T07:59:00Z"),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some("pre-boundary body".to_string()),
                },
                SessionTurnIngest {
                    session_id: session_id.to_string(),
                    turn_id: COMPACTION_BOUNDARY_TURN_ID.to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: Some("00000000-0000-4000-8000-000000000001".to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: Some("compact summary".to_string()),
                },
                SessionTurnIngest {
                    session_id: session_id.to_string(),
                    turn_id: "33333333-3333-4333-8333-333333333333".to_string(),
                    timestamp: ts("2026-04-17T08:00:30Z"),
                    role: "user".to_string(),
                    parent_turn_id: Some(COMPACTION_BOUNDARY_TURN_ID.to_string()),
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: Some("post-boundary user".to_string()),
                },
            ],
        )
        .unwrap();
}

fn target_jsonl_path(projects_dir: &Path, workspace: &Path, session_id: &str) -> PathBuf {
    projects_dir
        .join(claude_project_dir_name(workspace))
        .join(format!("{session_id}.jsonl"))
}

fn segment_snapshot(state: &StateDb) -> Vec<(String, String, String, Option<String>)> {
    let mut stmt = state
        .connection()
        .prepare(
            "SELECT chain_id, provider_name, session_id, ended_at
             FROM session_chain_segments
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn project_file_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    if !root.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let file_type = entry.file_type().unwrap();
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                files.push((
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                ));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn active_segment(state: &StateDb, chain_id: &str) -> (String, String) {
    state
        .connection()
        .query_row(
            "SELECT provider_name, session_id
             FROM session_chain_segments
             WHERE chain_id = ?1 AND ended_at IS NULL",
            rusqlite::params![chain_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
}

fn seed_conflicting_active_segment(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> String {
    let chain_id = "99999999-9999-4999-8999-999999999999".to_string();
    state
        .connection()
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)",
            rusqlite::params![chain_id, "2026-04-17T09:00:00Z", "provider-ref-claude-opus"],
        )
        .unwrap();
    state
        .open_chain_segment(
            &chain_id,
            provider_name,
            session_id,
            &ts("2026-04-17T09:00:00Z"),
            TransitionReason::Initial,
        )
        .unwrap();
    chain_id
}

fn provider_ref_bound_request(
    fixture: &Fixture,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    fresh_ids: &[&str],
) -> (ProviderRefBoundOutcome, String) {
    let mut stderr = Vec::new();
    let mut ids = fresh_ids.iter().copied();
    let mut fresh_session_id = || ids.next().expect("fresh session id fixture").to_string();
    let outcome = bound_provider_ref_resume_segment(
        &fixture.state,
        &SessionsConfig::default(),
        model,
        resolved,
        &fixture.workspace,
        &mut fresh_session_id,
        &mut stderr,
    )
    .unwrap();
    (outcome, String::from_utf8(stderr).unwrap())
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
fn provider_ref_bound_resume_rotates_to_fresh_tail_and_preserves_original_chain() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_with_boundary_turn(&fixture.source_projects, &fixture.workspace);
    seed_turn_history_with_compaction_boundary(&fixture.state, "claude", SESSION_ID);
    let original_bytes = std::fs::read(&jsonl.source_path).unwrap();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);

    let (outcome, stderr) =
        provider_ref_bound_request(&fixture, &model, &resolved, &[FRESH_SESSION_ID]);

    let ProviderRefBoundOutcome::Rotated(segment) = outcome else {
        panic!("expected provider-ref boundary helper to self-rotate, got {outcome:?}");
    };
    assert_eq!(segment.chain_id, resolved.chain_id);
    assert_eq!(segment.source_provider, "claude");
    assert_eq!(segment.target_provider, "claude");
    assert_eq!(segment.source_session_id, SESSION_ID);
    assert_eq!(segment.target_session_id, FRESH_SESSION_ID);
    assert_eq!(segment.target_provider_index, 0);
    assert_eq!(
        segment.target_jsonl_path,
        target_jsonl_path(
            &fixture.source_projects,
            &fixture.workspace,
            FRESH_SESSION_ID
        )
    );
    let target_contents = std::fs::read_to_string(&segment.target_jsonl_path).unwrap();
    assert!(
        target_contents.starts_with(&jsonl.boundary_line),
        "{target_contents}"
    );
    assert!(target_contents.contains(&jsonl.post_boundary_user_line));
    assert!(target_contents.contains(&jsonl.post_boundary_assistant_line));
    assert!(!target_contents.contains(&jsonl.pre_boundary_line));
    assert_eq!(std::fs::read(&jsonl.source_path).unwrap(), original_bytes);
    assert_eq!(
        fixture
            .state
            .chain_id_for_segment("claude", SESSION_ID)
            .unwrap(),
        Some(resolved.chain_id.clone())
    );
    assert_eq!(
        fixture
            .state
            .chain_id_for_segment("claude", FRESH_SESSION_ID)
            .unwrap(),
        Some(resolved.chain_id.clone())
    );
    assert_eq!(
        active_segment(&fixture.state, &resolved.chain_id),
        ("claude".to_string(), FRESH_SESSION_ID.to_string())
    );
    let model_store = std::collections::HashMap::from([(model.name.clone(), model.clone())]);
    let by_original_id = fixture
        .state
        .resolve_resume(&model_store, SESSION_ID, None)
        .unwrap();
    assert_eq!(by_original_id.chain_id, resolved.chain_id);
    assert_eq!(by_original_id.active_session_id, FRESH_SESSION_ID);
    let by_chain_id = fixture
        .state
        .resolve_resume(&model_store, &resolved.chain_id, None)
        .unwrap();
    assert_eq!(by_chain_id.active_session_id, FRESH_SESSION_ID);
    assert_eq!(
        fixture
            .state
            .latest_compaction_boundary("claude", FRESH_SESSION_ID)
            .unwrap()
            .map(|(turn_id, _)| turn_id),
        Some(COMPACTION_BOUNDARY_TURN_ID.to_string())
    );
    assert_eq!(
        fixture
            .state
            .count_session_turns("claude", SESSION_ID)
            .unwrap()
            .total,
        3
    );
    assert_eq!(
        fixture
            .state
            .count_session_turns("claude", FRESH_SESSION_ID)
            .unwrap()
            .total,
        1,
        "fresh boundary marker preservation must not copy pre-boundary history"
    );
    assert!(
        stderr.contains("[migrate]") || stderr.is_empty(),
        "{stderr}"
    );
}

#[test]
fn provider_ref_bound_resume_no_boundary_warns_and_mutates_nothing() {
    let fixture = Fixture::new();
    let source_path =
        seed_source_jsonl_without_boundary_turn(&fixture.source_projects, &fixture.workspace);
    let original_bytes = std::fs::read(&source_path).unwrap();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let before_segments = segment_snapshot(&fixture.state);
    let before_project_files = project_file_snapshot(&fixture.source_projects);
    let target_path = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        FRESH_SESSION_ID,
    );
    let mut stderr = Vec::new();
    let mut fresh_session_id =
        || -> String { panic!("no-boundary no-op must not mint a fresh provider session id") };

    let outcome = bound_provider_ref_resume_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        &mut fresh_session_id,
        &mut stderr,
    )
    .unwrap();
    let stderr = String::from_utf8(stderr).unwrap();

    assert!(matches!(outcome, ProviderRefBoundOutcome::NoBoundary));
    assert_eq!(std::fs::read(&source_path).unwrap(), original_bytes);
    assert!(
        !target_path.exists(),
        "no-boundary path must not create a fresh JSONL"
    );
    assert_eq!(
        project_file_snapshot(&fixture.source_projects),
        before_project_files,
        "no-boundary path must not create any provider JSONL or temp file in storage"
    );
    assert_eq!(segment_snapshot(&fixture.state), before_segments);
    assert_eq!(
        active_segment(&fixture.state, &resolved.chain_id).1,
        SESSION_ID
    );
    assert!(stderr.contains("Warning"), "{stderr}");
    assert!(stderr.contains("compaction boundary"), "{stderr}");
    assert!(stderr.contains(SESSION_ID), "{stderr}");
}

#[test]
fn provider_ref_bound_resume_boundary_not_found_warns_and_mutates_nothing() {
    let fixture = Fixture::new();
    let source_path =
        seed_source_jsonl_without_boundary_turn(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let original_bytes = std::fs::read(&source_path).unwrap();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let before_segments = segment_snapshot(&fixture.state);
    let before_project_files = project_file_snapshot(&fixture.source_projects);
    let target_path = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        FRESH_SESSION_ID,
    );
    let mut stderr = Vec::new();
    let mut fresh_session_id = || -> String {
        panic!("boundary-not-found no-op must not mint a fresh provider session id")
    };

    let outcome = bound_provider_ref_resume_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        &mut fresh_session_id,
        &mut stderr,
    )
    .unwrap();
    let stderr = String::from_utf8(stderr).unwrap();

    assert!(matches!(outcome, ProviderRefBoundOutcome::BoundaryNotFound));
    assert_eq!(std::fs::read(&source_path).unwrap(), original_bytes);
    assert!(
        !target_path.exists(),
        "boundary-not-found path must not create a fresh JSONL"
    );
    assert_eq!(
        project_file_snapshot(&fixture.source_projects),
        before_project_files,
        "boundary-not-found path must not create any provider JSONL or temp file in storage"
    );
    assert_eq!(segment_snapshot(&fixture.state), before_segments);
    assert_eq!(
        active_segment(&fixture.state, &resolved.chain_id).1,
        SESSION_ID
    );
    assert!(stderr.contains("Warning"), "{stderr}");
    assert!(stderr.contains(COMPACTION_BOUNDARY_TURN_ID), "{stderr}");
    assert!(stderr.contains(SESSION_ID), "{stderr}");
}

#[test]
fn provider_ref_bound_resume_uses_alternate_tail_when_located_source_lacks_boundary() {
    let fixture = Fixture::new();
    let located_source =
        seed_source_jsonl_without_boundary_turn(&fixture.source_projects, &fixture.workspace);
    let alternate_workspace = fixture._dir.path().join("worktrees").join("alternate");
    let (_alternate, boundary_line) =
        seed_alternate_jsonl_with_boundary_turn(&fixture.source_projects, &alternate_workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);

    let (outcome, stderr) =
        provider_ref_bound_request(&fixture, &model, &resolved, &[FRESH_SESSION_ID]);

    let ProviderRefBoundOutcome::Rotated(segment) = outcome else {
        panic!("expected alternate JSONL boundary to rotate, got {outcome:?}");
    };
    let target_contents = std::fs::read_to_string(&segment.target_jsonl_path).unwrap();
    assert!(
        target_contents.starts_with(&boundary_line),
        "{target_contents}"
    );
    assert_ne!(
        target_contents,
        std::fs::read_to_string(located_source).unwrap()
    );
    assert!(!stderr.contains("Warning"), "{stderr}");
}

#[test]
#[allow(clippy::drop_non_drop)]
fn provider_ref_bound_resume_already_bounded_is_idempotent_noop() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_boundary_at_head(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let original_bytes = std::fs::read(&jsonl.source_path).unwrap();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let before_segments = segment_snapshot(&fixture.state);
    let before_project_files = project_file_snapshot(&fixture.source_projects);
    let mut stderr = Vec::new();
    let mut generator_called = false;
    let mut fresh_session_id = || {
        generator_called = true;
        FRESH_SESSION_ID.to_string()
    };

    let outcome = bound_provider_ref_resume_segment(
        &fixture.state,
        &SessionsConfig::default(),
        &model,
        &resolved,
        &fixture.workspace,
        &mut fresh_session_id,
        &mut stderr,
    )
    .unwrap();

    drop(fresh_session_id);
    assert!(matches!(outcome, ProviderRefBoundOutcome::AlreadyBounded));
    assert!(
        !generator_called,
        "idempotent no-op must not mint a fresh id"
    );
    assert_eq!(std::fs::read(&jsonl.source_path).unwrap(), original_bytes);
    assert_eq!(
        project_file_snapshot(&fixture.source_projects),
        before_project_files,
        "already-bounded path must not create any provider JSONL or temp file in storage"
    );
    assert_eq!(segment_snapshot(&fixture.state), before_segments);
    assert_eq!(
        active_segment(&fixture.state, &resolved.chain_id).1,
        SESSION_ID
    );
    assert!(String::from_utf8(stderr).unwrap().is_empty());
}

#[test]
fn provider_ref_bound_resume_matches_boundary_turn_id_field_not_body_substring() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_with_boundary_turn_and_pre_text(
        &fixture.source_projects,
        &fixture.workspace,
        &format!("body mentions {COMPACTION_BOUNDARY_TURN_ID} before the actual turn field"),
    );
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);

    let (outcome, _stderr) =
        provider_ref_bound_request(&fixture, &model, &resolved, &[FRESH_SESSION_ID]);

    let ProviderRefBoundOutcome::Rotated(segment) = outcome else {
        panic!("expected exact boundary field match to rotate, got {outcome:?}");
    };
    let target_contents = std::fs::read_to_string(&segment.target_jsonl_path).unwrap();
    assert!(
        target_contents.starts_with(&jsonl.boundary_line),
        "{target_contents}"
    );
    assert!(target_contents.contains(&jsonl.post_boundary_user_line));
    assert!(!target_contents.contains(&jsonl.pre_boundary_line));
}

#[test]
fn provider_ref_bound_resume_regenerates_colliding_target_path_without_clobber() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_with_boundary_turn(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let colliding_path = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        COLLIDING_FRESH_SESSION_ID,
    );
    std::fs::create_dir_all(colliding_path.parent().unwrap()).unwrap();
    std::fs::write(&colliding_path, "do-not-overwrite\n").unwrap();

    let (outcome, _stderr) = provider_ref_bound_request(
        &fixture,
        &model,
        &resolved,
        &[COLLIDING_FRESH_SESSION_ID, SECOND_FRESH_SESSION_ID],
    );

    let ProviderRefBoundOutcome::Rotated(segment) = outcome else {
        panic!("expected collision to regenerate fresh id, got {outcome:?}");
    };
    assert_eq!(segment.target_session_id, SECOND_FRESH_SESSION_ID);
    assert_eq!(
        std::fs::read_to_string(&colliding_path).unwrap(),
        "do-not-overwrite\n"
    );
    let target_contents = std::fs::read_to_string(&segment.target_jsonl_path).unwrap();
    assert!(
        target_contents.starts_with(&jsonl.boundary_line),
        "{target_contents}"
    );
}

#[test]
fn provider_ref_bound_resume_regenerates_fresh_id_active_on_other_chain_before_mutation() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_with_boundary_turn(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let conflicting_chain =
        seed_conflicting_active_segment(&fixture.state, "claude", COLLIDING_FRESH_SESSION_ID);
    let before_source = std::fs::read(&jsonl.source_path).unwrap();
    let colliding_target = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        COLLIDING_FRESH_SESSION_ID,
    );

    let (outcome, _stderr) = provider_ref_bound_request(
        &fixture,
        &model,
        &resolved,
        &[COLLIDING_FRESH_SESSION_ID, SECOND_FRESH_SESSION_ID],
    );

    let ProviderRefBoundOutcome::Rotated(segment) = outcome else {
        panic!("expected active-id conflict to regenerate fresh id, got {outcome:?}");
    };
    assert_eq!(segment.target_session_id, SECOND_FRESH_SESSION_ID);
    assert!(
        !colliding_target.exists(),
        "conflicting fresh id must not be written"
    );
    assert_eq!(std::fs::read(&jsonl.source_path).unwrap(), before_source);
    assert_eq!(
        fixture
            .state
            .chain_id_for_segment("claude", COLLIDING_FRESH_SESSION_ID)
            .unwrap(),
        Some(conflicting_chain)
    );
}

#[test]
fn provider_ref_bound_resume_collision_exhaustion_fails_before_any_mutation() {
    let fixture = Fixture::new();
    let jsonl = seed_source_jsonl_with_boundary_turn(&fixture.source_projects, &fixture.workspace);
    fixture.seed_recorded_compaction_boundary();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let first_colliding_path = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        COLLIDING_FRESH_SESSION_ID,
    );
    let second_colliding_path = target_jsonl_path(
        &fixture.source_projects,
        &fixture.workspace,
        SECOND_FRESH_SESSION_ID,
    );
    std::fs::create_dir_all(first_colliding_path.parent().unwrap()).unwrap();
    std::fs::write(&first_colliding_path, "first-collision-must-survive\n").unwrap();
    std::fs::write(&second_colliding_path, "second-collision-must-survive\n").unwrap();
    let before_source = std::fs::read(&jsonl.source_path).unwrap();
    let before_first_target = std::fs::read(&first_colliding_path).unwrap();
    let before_second_target = std::fs::read(&second_colliding_path).unwrap();
    let before_segments = segment_snapshot(&fixture.state);
    let mut stderr = Vec::new();
    let mut generated_count = 0;
    let result = {
        let mut fresh_session_id = || {
            generated_count += 1;
            if generated_count % 2 == 1 {
                COLLIDING_FRESH_SESSION_ID
            } else {
                SECOND_FRESH_SESSION_ID
            }
            .to_string()
        };
        bound_provider_ref_resume_segment(
            &fixture.state,
            &SessionsConfig::default(),
            &model,
            &resolved,
            &fixture.workspace,
            &mut fresh_session_id,
            &mut stderr,
        )
    };

    let err = result
        .expect_err("expected no-clobber collision exhaustion to return an error before mutation");
    match err {
        MigrationError::TargetAlreadyExists { provider, path } => {
            assert_eq!(provider, resolved.active_provider);
            assert!(
                path == first_colliding_path.display().to_string()
                    || path == second_colliding_path.display().to_string(),
                "collision exhaustion must report one of the colliding paths, got {path}"
            );
        }
        other => panic!("expected target-already-exists exhaustion error, got {other:?}"),
    }
    assert!(
        generated_count >= 2,
        "collision-exhaustion fixture must exercise regenerated colliding ids before failure"
    );
    assert_eq!(std::fs::read(&jsonl.source_path).unwrap(), before_source);
    assert_eq!(
        std::fs::read(&first_colliding_path).unwrap(),
        before_first_target
    );
    assert_eq!(
        std::fs::read(&second_colliding_path).unwrap(),
        before_second_target
    );
    assert_eq!(segment_snapshot(&fixture.state), before_segments);
    assert_eq!(
        active_segment(&fixture.state, &resolved.chain_id).1,
        SESSION_ID
    );
}

#[test]
fn external_provider_migration_branch_still_requires_explicit_manual_target() {
    let fixture = Fixture::new();
    let model = fixture.provider_ref_model_with_storage();
    let resolved = fixture.seed_resolved(&model);
    let mut stderr = Vec::new();

    let err = ProductionMigrationService::new()
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
        .expect_err("external provider branch without a manual target must still fail");

    let ServiceError::Dependency { message } = err else {
        panic!("expected dependency-mapped external target validation error, got {err:?}");
    };
    assert!(
        message.contains("external rotation target requires an explicit manual target"),
        "{message}"
    );
}

#[test]
fn explicit_migration_no_boundary_still_copies_full_source() {
    let fixture = Fixture::new();
    let source_path =
        seed_source_jsonl_without_boundary_turn(&fixture.source_projects, &fixture.workspace);
    let source_contents = std::fs::read_to_string(&source_path).unwrap();
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
        .unwrap();

    let MigrationServiceOutput::Migrated { segment } = output else {
        panic!("expected explicit manual migration, got {output:?}");
    };
    assert_eq!(segment.target_provider, "claude2");
    assert_eq!(
        std::fs::read_to_string(&segment.target_jsonl_path).unwrap(),
        source_contents,
        "explicit migration must retain its no-boundary full-source fallback"
    );
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
