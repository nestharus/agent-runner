#![cfg(unix)]

//! ## Declared roles
//!
//! Roles: orchestration, formatter, mapper, accessor, parser, validator, predicate, filter.
//!
//! TEST: external-provider launch/resume end-to-end fixtures — fake provider
//! CLI script formatters, fixture model mappers, record accessors, JSON/record
//! parsers, allowed-subcommand predicates, record-line/subcommand filters,
//! envelope/row validators, and test orchestration.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/tests/s10_external_provider_resume.rs
//!     role: adapter
//!     Translates:
//!       - external-provider-runtime-cli-contract
//!       - provider-launch-jsonl-contract
//!       - invocation-state-db-contract
//!       - session-resume-contract
//!       - test-fixture-process-contract
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/s10_external_provider_resume.rs
//!     role: intrinsic-surface
//!     Domain: external-provider launch/resume CLI regression suite
//!     Owns:
//!       - isolated config/data fixture materialization
//!       - external provider Python script generation and executable setup
//!       - launch/resume command invocation and environment isolation
//!       - provider record parsing and subcommand filtering assertions
//!       - invocation session/outcome database assertions
//! ```

mod provider_authority_fixture;

mod age153_support;

use age153_support::assert_result_envelope_shape;
use oulipoly_state::{InvocationStatus, SessionTurnIngest, StateDb};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODEL: &str = "provider-ref-model";
const MISMATCH_MODEL: &str = "provider-ref-wrong-provider-model";
const PROVIDER: &str = "provider-ref-account";
const MISMATCH_PROVIDER: &str = "provider-ref-other-account";
const SESSION_ID: &str = "a9a8c8d0-8f5f-402e-857c-c5c549446beb";
const COMPACTION_BOUNDARY_TURN_ID: &str = "68a65f70-d8ef-466f-a0b2-2396d64f353b";
const INCIDENT_TERMINAL_REASON: &str =
    "provider error: opencode UnknownError: Failed to execute statement";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    workspace: PathBuf,
    hostile_cwd: PathBuf,
    projects_dir: PathBuf,
    record_path: PathBuf,
}

struct FixturePaths {
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    workspace: PathBuf,
    hostile_cwd: PathBuf,
    projects_dir: PathBuf,
    record_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
struct InvocationSessionRow {
    session_id: Option<String>,
    session_capture_method: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
    provider_session_capture_method: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct InvocationOutcomeRow {
    status: String,
    success: i64,
    exit_code: i64,
    terminal_reason: Option<String>,
}

type InvocationSnapshotRow = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

#[derive(Debug, PartialEq, Eq)]
struct ProviderRefStateSnapshot {
    chains: Vec<(String, String)>,
    segments: Vec<(i64, String, String, String, Option<String>)>,
    turns: Vec<(i64, String, String, String, i64)>,
    invocations: Vec<InvocationSnapshotRow>,
}

#[derive(Clone, Copy)]
struct ProviderOptions {
    launch_session_key: &'static str,
    session_capability: bool,
    session_storage: bool,
}

#[derive(Clone, Copy, Debug)]
enum ProviderRefNonRotatedCase {
    NoBoundary,
    BoundaryNotFound,
    AlreadyBounded,
}

impl ProviderOptions {
    fn provider_session_id() -> Self {
        Self {
            launch_session_key: "provider_session_id",
            session_capability: true,
            session_storage: false,
        }
    }

    fn provider_session_id_with_storage() -> Self {
        Self {
            session_storage: true,
            ..Self::provider_session_id()
        }
    }

    fn session_id_without_session_capability() -> Self {
        Self {
            launch_session_key: "session_id",
            session_capability: false,
            session_storage: false,
        }
    }
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_provider_options(ProviderOptions::provider_session_id())
    }

    fn new_with_provider_storage() -> Self {
        Self::new_with_provider_options(ProviderOptions::provider_session_id_with_storage())
    }

    fn new_with_provider_options(options: ProviderOptions) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = fixture_paths(dir.path());
        materialize_fixture(dir.path(), &paths, options);
        fixture_from_paths(dir, paths)
    }

    fn run_launch(&self) -> Output {
        self.run_launch_with_env(&[])
    }

    fn run_launch_with_env(&self, envs: &[(&str, &str)]) -> Output {
        run_fixture_command(command_with_envs(self.launch_command(), envs))
    }

    fn run_resume(&self) -> Output {
        self.run_resume_with_env(&[])
    }

    fn run_resume_with_env(&self, envs: &[(&str, &str)]) -> Output {
        run_fixture_command(command_with_envs(self.resume_command(), envs))
    }

    fn launch_command(&self) -> Command {
        let mut cmd = self.command();
        apply_launch_command_shape(&mut cmd, &self.workspace, &self.models_dir);
        cmd
    }

    fn resume_command(&self) -> Command {
        self.resume_command_with_model(MODEL)
    }

    fn resume_command_with_model(&self, model: &str) -> Command {
        let mut cmd = self.command();
        apply_resume_command_shape(&mut cmd, &self.hostile_cwd, &self.models_dir, model);
        cmd
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn invocation_session_rows(&self) -> Vec<InvocationSessionRow> {
        invocation_session_rows_from_db(&self.db_path())
    }

    fn latest_invocation_outcome(&self) -> InvocationOutcomeRow {
        latest_invocation_outcome_from_db(&self.db_path())
    }

    fn records(&self) -> Vec<Value> {
        provider_records_from_path(&self.record_path)
    }

    fn write_mismatched_provider_ref_model(&self) {
        fs::write(
            self.models_dir.join(format!("{MISMATCH_MODEL}.toml")),
            mismatched_model_config_toml(),
        )
        .unwrap();
        let providers_path = self
            .config_home
            .join("oulipoly-agent-runner")
            .join("providers.toml");
        let mut providers = fs::read_to_string(&providers_path).unwrap();
        providers.push_str(&mismatched_provider_config_toml());
        fs::write(
            providers_path,
            provider_authority_fixture::with_explicit_provider_authority(&providers),
        )
        .unwrap();
    }

    fn provider_ref_transcript_dir(&self) -> PathBuf {
        self.projects_dir
            .join(claude_project_dir_name(&self.workspace))
    }

    fn seed_provider_ref_boundary_jsonl(&self) -> (PathBuf, String, String, String) {
        let transcript_dir = self.provider_ref_transcript_dir();
        fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join(format!("{SESSION_ID}.jsonl"));
        let pre_boundary = serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000001",
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:00:00Z",
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "pre-boundary headless prompt"}],
            },
        });
        let boundary = serde_json::json!({
            "uuid": COMPACTION_BOUNDARY_TURN_ID,
            "parentUuid": "00000000-0000-4000-8000-000000000001",
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:01:00Z",
            "type": "assistant",
            "isCompactSummary": true,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "compact summary"}],
            },
        });
        let post_boundary = serde_json::json!({
            "uuid": "11111111-1111-4111-8111-111111111111",
            "parentUuid": COMPACTION_BOUNDARY_TURN_ID,
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:02:00Z",
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "post-boundary headless prompt"}],
            },
        });
        let pre_boundary_line = pre_boundary.to_string();
        let boundary_line = boundary.to_string();
        let post_boundary_line = post_boundary.to_string();
        fs::write(
            &transcript_path,
            format!("{pre_boundary_line}\n{boundary_line}\n{post_boundary_line}\n"),
        )
        .unwrap();
        (
            transcript_path,
            boundary_line,
            pre_boundary_line,
            post_boundary_line,
        )
    }

    fn seed_provider_ref_jsonl_without_boundary(&self) -> PathBuf {
        let transcript_dir = self.provider_ref_transcript_dir();
        fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join(format!("{SESSION_ID}.jsonl"));
        let first = serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000011",
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:00:00Z",
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "headless no-boundary prompt"}],
            },
        });
        let second = serde_json::json!({
            "uuid": "00000000-0000-4000-8000-000000000012",
            "parentUuid": "00000000-0000-4000-8000-000000000011",
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:01:00Z",
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "headless no-boundary answer"}],
            },
        });
        fs::write(&transcript_path, format!("{first}\n{second}\n")).unwrap();
        transcript_path
    }

    fn seed_provider_ref_boundary_at_head_jsonl(&self) -> PathBuf {
        let transcript_dir = self.provider_ref_transcript_dir();
        fs::create_dir_all(&transcript_dir).unwrap();
        let transcript_path = transcript_dir.join(format!("{SESSION_ID}.jsonl"));
        let boundary = serde_json::json!({
            "uuid": COMPACTION_BOUNDARY_TURN_ID,
            "parentUuid": null,
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:01:00Z",
            "type": "assistant",
            "isCompactSummary": true,
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "compact summary"}],
            },
        });
        let post = serde_json::json!({
            "uuid": "11111111-1111-4111-8111-111111111111",
            "parentUuid": COMPACTION_BOUNDARY_TURN_ID,
            "sessionId": SESSION_ID,
            "timestamp": "2026-06-01T00:02:00Z",
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": "headless already-bounded prompt"}],
            },
        });
        fs::write(&transcript_path, format!("{boundary}\n{post}\n")).unwrap();
        transcript_path
    }

    fn seed_recorded_compaction_boundary(&self) {
        let state = StateDb::open(&self.db_path()).unwrap();
        state
            .ingest_session_turns_batch(
                PROVIDER,
                &[SessionTurnIngest {
                    session_id: SESSION_ID.to_string(),
                    turn_id: COMPACTION_BOUNDARY_TURN_ID.to_string(),
                    timestamp: timestamp("2026-06-01T00:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: Some("compact summary".to_string()),
                }],
            )
            .unwrap();
    }
}

fn command_with_envs(mut cmd: Command, envs: &[(&str, &str)]) -> Command {
    apply_command_envs(&mut cmd, envs);
    cmd
}

fn apply_command_envs(cmd: &mut Command, envs: &[(&str, &str)]) {
    for (key, value) in envs {
        cmd.env(key, value);
    }
}

fn apply_launch_command_shape(cmd: &mut Command, workspace: &Path, models_dir: &Path) {
    cmd.current_dir(workspace)
        .arg("--models-dir")
        .arg(models_dir)
        .arg("--model")
        .arg(MODEL)
        .arg("first prompt");
}

fn apply_resume_command_shape(
    cmd: &mut Command,
    hostile_cwd: &Path,
    models_dir: &Path,
    model: &str,
) {
    cmd.current_dir(hostile_cwd)
        .arg("resume")
        .arg("--models-dir")
        .arg(models_dir)
        .arg("--model")
        .arg(model)
        .arg("--session-id")
        .arg(SESSION_ID)
        .arg("--prompt")
        .arg("resume prompt");
}

fn run_fixture_command(mut cmd: Command) -> Output {
    cmd.output().unwrap()
}

fn materialize_fixture(root: &Path, paths: &FixturePaths, options: ProviderOptions) {
    create_fixture_directories(paths);
    let provider_path = write_external_provider(root, &paths.record_path, options);
    write_model_config(&paths.models_dir, &provider_path);
    write_providers_config(
        &paths.app_config_dir,
        &provider_path,
        options
            .session_storage
            .then_some(paths.projects_dir.as_path()),
    );
}

fn fixture_paths(root: &Path) -> FixturePaths {
    let config_home = root.join("config");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    FixturePaths {
        data_home: root.join("data"),
        models_dir: app_config_dir.join("models"),
        workspace: root.join("workspace"),
        hostile_cwd: root.join("hostile-cwd"),
        projects_dir: root.join("provider-projects"),
        record_path: root.join("provider-records.jsonl"),
        config_home,
        app_config_dir,
    }
}

fn create_fixture_directories(paths: &FixturePaths) {
    fs::create_dir_all(&paths.models_dir).unwrap();
    fs::create_dir_all(&paths.workspace).unwrap();
    fs::create_dir_all(&paths.hostile_cwd).unwrap();
    fs::create_dir_all(&paths.projects_dir).unwrap();
}

fn write_model_config(models_dir: &Path, provider_path: &Path) {
    fs::write(
        models_dir.join(format!("{MODEL}.toml")),
        model_config_toml(provider_path),
    )
    .unwrap();
}

fn model_config_toml(provider_path: &Path) -> String {
    format!(
        r#"provider = {{ path = {:?} }}
prompt_mode = "arg"

[[providers]]
name = {:?}
args = ["--model", "haiku"]
"#,
        provider_path.display().to_string(),
        PROVIDER,
    )
}

fn mismatched_model_config_toml() -> String {
    format!(
        r#"provider = {{ path = "/synthetic/provider-ref-mismatch" }}
prompt_mode = "arg"

[[providers]]
name = {MISMATCH_PROVIDER:?}
args = []
"#,
    )
}

fn mismatched_provider_config_toml() -> String {
    format!(
        r#"
[{MISMATCH_PROVIDER}]
command = "native-provider"
args = []
prompt_mode = "arg"
"#,
    )
}

fn write_providers_config(
    app_config_dir: &Path,
    provider_path: &Path,
    storage_projects_dir: Option<&Path>,
) {
    fs::write(
        app_config_dir.join("providers.toml"),
        provider_authority_fixture::with_explicit_provider_authority_at(
            &providers_config_toml(storage_projects_dir),
            "s10-external-provider",
            provider_path,
        ),
    )
    .unwrap();
}

fn providers_config_toml(storage_projects_dir: Option<&Path>) -> String {
    let storage = storage_projects_dir
        .map(|projects_dir| {
            format!(
                r#"
[{PROVIDER}.session_storage]
kind = "claude_code"
projects_dir = {:?}
"#,
                projects_dir.display().to_string()
            )
        })
        .unwrap_or_default();
    format!(
        r#"[{PROVIDER}]
command = "native-provider"
args = ["--base"]
prompt_mode = "arg"
{storage}
"#,
    )
}

fn fixture_from_paths(dir: tempfile::TempDir, paths: FixturePaths) -> Fixture {
    Fixture {
        _dir: dir,
        config_home: paths.config_home,
        data_home: paths.data_home,
        models_dir: paths.models_dir,
        workspace: paths.workspace,
        hostile_cwd: paths.hostile_cwd,
        projects_dir: paths.projects_dir,
        record_path: paths.record_path,
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

fn timestamp(value: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn invocation_session_rows_from_db(path: &Path) -> Vec<InvocationSessionRow> {
    let conn = open_invocation_db(path);
    query_invocation_session_rows(&conn)
}

fn open_invocation_db(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

fn query_invocation_session_rows(conn: &rusqlite::Connection) -> Vec<InvocationSessionRow> {
    let mut stmt = invocation_session_rows_statement(conn);
    collect_invocation_session_rows(&mut stmt)
}

fn invocation_session_rows_statement(conn: &rusqlite::Connection) -> rusqlite::Statement<'_> {
    conn.prepare(
        "SELECT session_id, session_capture_method, provider_session_id,
                resume_input_id, provider_session_capture_method
           FROM invocations
           ORDER BY id",
    )
    .unwrap()
}

fn collect_invocation_session_rows(
    stmt: &mut rusqlite::Statement<'_>,
) -> Vec<InvocationSessionRow> {
    stmt.query_map([], invocation_session_row)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn invocation_session_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationSessionRow> {
    Ok(InvocationSessionRow {
        session_id: row.get(0)?,
        session_capture_method: row.get(1)?,
        provider_session_id: row.get(2)?,
        resume_input_id: row.get(3)?,
        provider_session_capture_method: row.get(4)?,
    })
}

fn latest_invocation_outcome_from_db(path: &Path) -> InvocationOutcomeRow {
    let conn = open_invocation_db(path);
    conn.query_row(
        latest_invocation_outcome_sql(),
        [PROVIDER],
        invocation_outcome_row,
    )
    .unwrap()
}

fn latest_invocation_outcome_sql() -> &'static str {
    "SELECT status, success, exit_code, terminal_reason
       FROM invocations
      WHERE provider_name = ?1
      ORDER BY id DESC
      LIMIT 1"
}

fn invocation_outcome_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationOutcomeRow> {
    Ok(InvocationOutcomeRow {
        status: row.get(0)?,
        success: row.get(1)?,
        exit_code: row.get(2)?,
        terminal_reason: row.get(3)?,
    })
}

fn provider_record_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn provider_records_from_path(path: &Path) -> Vec<Value> {
    parse_provider_records(&provider_record_text(path))
}

fn provider_ref_state_snapshot(path: &Path) -> ProviderRefStateSnapshot {
    let conn = open_invocation_db(path);
    ProviderRefStateSnapshot {
        chains: query_chain_snapshot(&conn),
        segments: query_segment_snapshot(&conn),
        turns: query_turn_snapshot(&conn),
        invocations: query_invocation_snapshot(&conn),
    }
}

fn provider_ref_segment_snapshot(
    path: &Path,
) -> Vec<(i64, String, String, String, Option<String>)> {
    let conn = open_invocation_db(path);
    query_segment_snapshot(&conn)
}

fn query_chain_snapshot(conn: &rusqlite::Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT chain_id, model_name FROM session_chains ORDER BY chain_id")
        .unwrap();
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn query_segment_snapshot(
    conn: &rusqlite::Connection,
) -> Vec<(i64, String, String, String, Option<String>)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, chain_id, provider_name, session_id, ended_at
             FROM session_chain_segments
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn query_turn_snapshot(conn: &rusqlite::Connection) -> Vec<(i64, String, String, String, i64)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, provider_name, session_id, turn_id, is_compaction_boundary
             FROM session_turns
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn query_invocation_snapshot(conn: &rusqlite::Connection) -> Vec<InvocationSnapshotRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, model_name, provider_name, session_id, provider_session_id, status
             FROM invocations
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn transcript_file_snapshot(dir: &Path) -> Vec<(String, String)> {
    let mut files = fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read_to_string(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn parse_provider_records(text: &str) -> Vec<Value> {
    parse_provider_record_lines(provider_record_lines_with_content(text))
}

fn provider_record_lines_with_content(text: &str) -> Vec<&str> {
    filter_provider_record_lines(provider_record_lines(text))
}

fn provider_record_lines(text: &str) -> std::str::Lines<'_> {
    text.lines()
}

fn filter_provider_record_lines<'a>(lines: std::str::Lines<'a>) -> Vec<&'a str> {
    lines
        .filter(|line| provider_record_line_has_content(line))
        .collect()
}

fn parse_provider_record_lines(lines: Vec<&str>) -> Vec<Value> {
    lines.into_iter().map(parse_provider_record).collect()
}

fn provider_record_line_has_content(line: &str) -> bool {
    !line.trim().is_empty()
}

fn parse_provider_record(line: &str) -> Value {
    serde_json::from_str(line).unwrap()
}

fn assert_unconfirmed_resume(output: &Output) {
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let result = assert_result_envelope_shape(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 0);
    assert_eq!(result["error_category"], "resume_completion_unconfirmed");
    assert_eq!(result["terminal_reason"], "resume_completion_unconfirmed");
    assert_eq!(result["provider_name"], PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION_ID);
    assert_eq!(result["agent_runner_invocation_id"], result["id"]);
}

#[test]
fn external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd() {
    let fixture = Fixture::new();

    let launch = fixture.run_launch();
    assert_success(&launch);

    let resume = fixture.run_resume();
    assert_unconfirmed_resume(&resume);
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        !resume_stderr.contains("migration failed"),
        "{resume_stderr}"
    );
    assert!(
        !resume_stderr.contains("could not resolve original cwd"),
        "{resume_stderr}"
    );

    let records = fixture.records();
    assert_no_rotation_or_migration_provider_calls(&records);
    let launches = records_for_subcommand(&records, "launch");
    assert_eq!(launches.len(), 2, "records: {records:?}");

    let resume_launch = &launches[1]["request"];
    assert_eq!(
        resume_launch["params"]["session"]["known_provider_session_id"].as_str(),
        Some(SESSION_ID),
        "resume must pass the provider session captured by the first external launch"
    );
    assert_eq!(
        resume_launch["params"]["session"]["start_mode"].as_str(),
        Some("resume"),
        "resume must tell external providers to resume the known session"
    );
    assert_eq!(
        resume_launch["params"]["model"]["inputs"]["prompt"].as_str(),
        Some("resume prompt")
    );
    assert_eq!(
        resume_launch["params"]["working_directory"].as_str(),
        Some(fixture.workspace.to_string_lossy().as_ref()),
        "resume must use the original launch cwd, not the caller's current cwd"
    );
    assert_eq!(
        resume_launch["params"]["provider_name"].as_str(),
        None,
        "provider_name lives in session requests, not launch params"
    );

    assert_external_launch_session_capture_rows(&fixture.invocation_session_rows());
}

#[test]
fn external_provider_ref_resume_launches_existing_headless_session_unbounded() {
    let fixture = Fixture::new_with_provider_storage();

    let launch = fixture.run_launch();
    assert_success(&launch);
    let (source_path, _boundary_line, _pre_boundary_line, _post_boundary_line) =
        fixture.seed_provider_ref_boundary_jsonl();
    fixture.seed_recorded_compaction_boundary();
    let transcript_dir = fixture.provider_ref_transcript_dir();
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    let before_segments = provider_ref_segment_snapshot(&fixture.db_path());

    let resume = fixture.run_resume();

    assert_unconfirmed_resume(&resume);
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        !resume_stderr.contains("external rotation target requires an explicit manual target"),
        "default provider-ref resume must not route through the external manual-target branch: {resume_stderr}"
    );
    let records = fixture.records();
    assert_no_rotation_or_migration_provider_calls(&records);
    let launches = records_for_subcommand(&records, "launch");
    assert_eq!(launches.len(), 2, "records: {records:?}");
    let resume_launch = &launches[1]["request"];
    assert_eq!(
        resume_launch["params"]["session"]["known_provider_session_id"].as_str(),
        Some(SESSION_ID),
        "provider-ref default resume must launch the existing provider session id unbounded"
    );
    assert_eq!(
        resume_launch["params"]["session"]["start_mode"].as_str(),
        Some("resume")
    );
    assert_eq!(
        resume_launch["params"]["model"]["provider_args"],
        serde_json::json!(["--model", "haiku"]),
        "provider-ref unbounded resume must not change model/provider argument choice"
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(
        transcript_file_snapshot(&transcript_dir),
        before_files,
        "provider-ref default resume must not create a fresh JSONL or temp file"
    );
    assert_eq!(
        provider_ref_segment_snapshot(&fixture.db_path()),
        before_segments,
        "provider-ref default resume must not close/open chain segments"
    );
}

#[test]
fn external_provider_ref_resume_no_boundary_launches_original_headless_session() {
    assert_headless_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::NoBoundary);
}

#[test]
fn external_provider_ref_resume_boundary_not_found_launches_original_headless_session() {
    assert_headless_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::BoundaryNotFound);
}

#[test]
fn external_provider_ref_resume_already_bounded_launches_original_headless_session() {
    assert_headless_non_rotated_provider_ref_resume(ProviderRefNonRotatedCase::AlreadyBounded);
}

fn assert_headless_non_rotated_provider_ref_resume(case: ProviderRefNonRotatedCase) {
    let fixture = Fixture::new_with_provider_storage();

    let launch = fixture.run_launch();
    assert_success(&launch);
    let source_path = match case {
        ProviderRefNonRotatedCase::NoBoundary | ProviderRefNonRotatedCase::BoundaryNotFound => {
            fixture.seed_provider_ref_jsonl_without_boundary()
        }
        ProviderRefNonRotatedCase::AlreadyBounded => {
            fixture.seed_provider_ref_boundary_at_head_jsonl()
        }
    };
    if !matches!(case, ProviderRefNonRotatedCase::NoBoundary) {
        fixture.seed_recorded_compaction_boundary();
    }
    let transcript_dir = fixture.provider_ref_transcript_dir();
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    let before_segments = provider_ref_segment_snapshot(&fixture.db_path());

    let resume = fixture.run_resume();

    assert_unconfirmed_resume(&resume);
    let resume_stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(
        !resume_stderr.contains("external rotation target requires an explicit manual target"),
        "default provider-ref resume must not route through the external manual-target branch: {resume_stderr}"
    );
    let records = fixture.records();
    assert_no_rotation_or_migration_provider_calls(&records);
    let launches = records_for_subcommand(&records, "launch");
    assert_eq!(launches.len(), 2, "records: {records:?}");
    let resume_launch = &launches[1]["request"];
    assert_eq!(
        resume_launch["params"]["session"]["known_provider_session_id"].as_str(),
        Some(SESSION_ID),
        "non-rotated provider-ref headless resume must keep the original provider session id for {case:?}"
    );
    assert_eq!(
        resume_launch["params"]["session"]["start_mode"].as_str(),
        Some("resume")
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(
        transcript_file_snapshot(&transcript_dir),
        before_files,
        "non-rotated provider-ref headless resume must not create a fresh JSONL or temp file for {case:?}"
    );
    assert_eq!(
        provider_ref_segment_snapshot(&fixture.db_path()),
        before_segments,
        "non-rotated provider-ref headless resume must not close/open chain segments for {case:?}"
    );
}

#[test]
fn external_provider_ref_resume_model_provider_mismatch_rejects_before_default_migration() {
    let fixture = Fixture::new_with_provider_storage();
    fixture.write_mismatched_provider_ref_model();

    let launch = fixture.run_launch();
    assert_success(&launch);
    let (source_path, _boundary_line, _pre_boundary_line, _post_boundary_line) =
        fixture.seed_provider_ref_boundary_jsonl();
    fixture.seed_recorded_compaction_boundary();
    let transcript_dir = fixture
        .projects_dir
        .join(claude_project_dir_name(&fixture.workspace));
    let original_source = fs::read(&source_path).unwrap();
    let before_files = transcript_file_snapshot(&transcript_dir);
    let before_state = provider_ref_state_snapshot(&fixture.db_path());

    let output = run_fixture_command(fixture.resume_command_with_model(MISMATCH_MODEL));

    assert_failed_resume_resolution(&output);
    let combined = combined_output(&output);
    assert!(
        combined.contains(&format!(
            "session {SESSION_ID} belongs to provider {PROVIDER}, which is not in model {MISMATCH_MODEL}'s provider pool"
        )),
        "{combined}"
    );
    assert!(
        !combined.contains("external rotation target requires an explicit manual target"),
        "mismatch must fail before the external migration branch: {combined}"
    );
    assert_eq!(fs::read(&source_path).unwrap(), original_source);
    assert_eq!(transcript_file_snapshot(&transcript_dir), before_files);
    assert_eq!(
        provider_ref_state_snapshot(&fixture.db_path()),
        before_state
    );
}

#[test]
fn external_launch_session_id_alias_persists_external_capture_method_without_session_capability() {
    let fixture = Fixture::new_with_provider_options(
        ProviderOptions::session_id_without_session_capability(),
    );

    let launch = fixture.run_launch();
    assert_success(&launch);
    let stderr = String::from_utf8_lossy(&launch.stderr);
    assert!(stderr.contains("Session ingest failed"), "{stderr}");

    let rows = fixture.invocation_session_rows();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_external_launch_session_capture_row(&rows[0], "external_provider_launch");
}

#[test]
fn external_provider_unavailable_persists_failure_without_replay_or_quota_mutation() {
    for raw_code in [0, 1] {
        for resume in [false, true] {
            let fixture = Fixture::new();
            if resume {
                assert_success(&fixture.run_launch());
            }
            let launches_before = records_for_subcommand(&fixture.records(), "launch").len();
            let raw_code_text = raw_code.to_string();
            let env = [
                ("S10_PROVIDER_UNAVAILABLE", "1"),
                ("S10_PROVIDER_UNAVAILABLE_EXIT_CODE", raw_code_text.as_str()),
            ];
            let output = if resume {
                fixture.run_resume_with_env(&env)
            } else {
                fixture.run_launch_with_env(&env)
            };
            assert!(!output.status.success(), "{output:?}");
            let result = assert_result_envelope_shape(&String::from_utf8_lossy(&output.stdout));
            assert_eq!(result["status"], "failed");
            assert_eq!(result["error_category"], "provider_unavailable");
            assert_eq!(result["terminal_reason"], "provider_unavailable");
            let row = fixture.latest_invocation_outcome();
            assert_eq!(row.status, "failed");
            assert_eq!(row.success, 0);
            assert_eq!(row.exit_code, if raw_code == 0 { -1 } else { raw_code });
            assert_eq!(row.terminal_reason.as_deref(), Some("provider_unavailable"));
            let conn = open_invocation_db(&fixture.db_path());
            let category: String = conn.query_row(
            "SELECT error_category FROM invocations WHERE provider_name = ?1 ORDER BY id DESC LIMIT 1",
            [PROVIDER], |row| row.get(0),
        ).unwrap();
            assert_eq!(category, "provider_unavailable");
            let quota_mutations: i64 = conn.query_row(
            "SELECT COUNT(*) FROM provider_quotas WHERE exhausted_at IS NOT NULL OR next_available_at IS NOT NULL OR failure_class IS NOT NULL",
            [], |row| row.get(0),
        ).unwrap();
            assert_eq!(quota_mutations, 0);
            let records = fixture.records();
            let launches = records_for_subcommand(&records, "launch");
            assert_eq!(
                launches.len(),
                launches_before + 1,
                "request must not be replayed"
            );
            assert_eq!(
                launches.last().unwrap()["request"]["host"]["env"]["OULIPOLY_HOST_TERMINAL_UNAVAILABLE_V1"],
                "1"
            );
            assert_no_rotation_or_migration_provider_calls(&records);
        }
    }
}

#[test]
fn external_provider_launch_terminal_error_exit_zero_finalizes_as_failed() {
    let fixture = Fixture::new();

    let output = fixture.run_launch_with_env(&[("S10_PROVIDER_ERROR_EXIT_ZERO", "1")]);

    assert_failed_terminal_error_output(&output);
    assert_latest_invocation_failed_with_terminal_error(&fixture);
}

#[test]
fn external_provider_resume_terminal_error_exit_zero_finalizes_as_failed() {
    let fixture = Fixture::new();
    assert_success(&fixture.run_launch());

    let output = fixture.run_resume_with_env(&[("S10_PROVIDER_ERROR_EXIT_ZERO", "1")]);

    assert_failed_terminal_error_output(&output);
    assert_latest_invocation_failed_with_terminal_error(&fixture);
}

#[test]
fn external_provider_launch_stream_over_capture_limit_finalizes_succeeded() {
    let fixture = Fixture::new();

    let output = fixture.run_launch_with_env(&[("S10_LAUNCH_LONG_STREAM", "1")]);

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("answer:first prompt"), "{stdout}");
    assert_latest_invocation_succeeded(&fixture);
}

fn assert_external_launch_session_capture_rows(rows: &[InvocationSessionRow]) {
    assert_eq!(rows.len(), 2, "rows: {rows:?}");
    assert_external_launch_session_capture_row(&rows[0], "provider_session_capture");

    let resume = &rows[1];
    assert_eq!(resume.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        resume.session_capture_method.as_deref(),
        Some("external_provider_launch")
    );
    assert_eq!(resume.provider_session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(resume.resume_input_id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        resume.provider_session_capture_method.as_deref(),
        Some("external_provider_launch")
    );
}

fn assert_external_launch_session_capture_row(
    launch: &InvocationSessionRow,
    expected_capture_method: &str,
) {
    assert_eq!(launch.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        launch.session_capture_method.as_deref(),
        Some(expected_capture_method)
    );
    assert_eq!(launch.provider_session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(launch.resume_input_id.as_deref(), None);
    assert_eq!(
        launch.provider_session_capture_method.as_deref(),
        Some(expected_capture_method)
    );
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failed_resume_resolution(output: &Output) {
    assert_ne!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn combined_output(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_failed_terminal_error_output(output: &Output) {
    assert_failed_terminal_error_process(output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation_id = single_invocation_id(&stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["id"], invocation_id);
    assert_eq!(result["agent_runner_invocation_id"], invocation_id);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], -1);
    assert_eq!(result["error_category"], INCIDENT_TERMINAL_REASON);
    assert_eq!(result["terminal_reason"], INCIDENT_TERMINAL_REASON);
    assert_eq!(result["provider_name"], PROVIDER);
    assert_eq!(result["provider_session_id"], SESSION_ID);
    assert!(
        result["agent_runner_chain_id"].as_str().is_some(),
        "{result}"
    );
    assert!(result["finished_at"].as_str().is_some(), "{result}");
}

fn single_invocation_id(stderr: &str) -> String {
    let lines: Vec<_> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("OULIPOLY_INVOCATION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "terminal-error execution must emit one invocation identity:\n{stderr}"
    );
    let value: Value = serde_json::from_str(lines[0]).expect("parse invocation identity");
    value["id"]
        .as_str()
        .expect("invocation identity id")
        .to_string()
}

fn assert_failed_terminal_error_process(output: &Output) {
    assert_ne!(
        output.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_latest_invocation_failed_with_terminal_error(fixture: &Fixture) {
    let row = fixture.latest_invocation_outcome();
    assert_eq!(row.status, InvocationStatus::Failed.as_str(), "{row:?}");
    assert_eq!(row.success, 0, "{row:?}");
    assert_eq!(row.exit_code, -1, "{row:?}");
    assert_eq!(
        row.terminal_reason.as_deref(),
        Some(INCIDENT_TERMINAL_REASON)
    );
}

fn assert_latest_invocation_succeeded(fixture: &Fixture) {
    let row = fixture.latest_invocation_outcome();
    assert_eq!(row.status, InvocationStatus::Succeeded.as_str(), "{row:?}");
    assert_eq!(row.success, 1, "{row:?}");
    assert_eq!(row.exit_code, 0, "{row:?}");
}

fn records_for_subcommand<'a>(records: &'a [Value], subcommand: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["subcommand"] == subcommand)
        .collect()
}

fn assert_no_rotation_or_migration_provider_calls(records: &[Value]) {
    let subcommands = provider_record_subcommands(records);
    assert_no_forbidden_provider_subcommands(&subcommands);
}

fn provider_record_subcommands(records: &[Value]) -> Vec<&str> {
    records.iter().map(provider_record_subcommand).collect()
}

fn provider_record_subcommand(record: &Value) -> &str {
    record["subcommand"].as_str().unwrap_or_default()
}

fn assert_no_forbidden_provider_subcommands(subcommands: &[&str]) {
    assert!(
        provider_subcommands_are_allowed(subcommands),
        "unexpected rotation/migration calls: {subcommands:?}"
    );
}

fn provider_subcommands_are_allowed(subcommands: &[&str]) -> bool {
    subcommands
        .iter()
        .all(|subcommand| provider_subcommand_is_allowed(subcommand))
}

fn provider_subcommand_is_allowed(subcommand: &str) -> bool {
    !subcommand.starts_with("rotation.") && !subcommand.starts_with("migration.")
}

fn write_external_provider(dir: &Path, record_path: &Path, options: ProviderOptions) -> PathBuf {
    fs::write(record_path, "").unwrap();
    let path = dir.join("external-provider.py");
    fs::write(&path, external_provider_script(record_path, options)).unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).unwrap();
    path
}

fn external_provider_script(record_path: &Path, options: ProviderOptions) -> String {
    format!(
        r#"#!/usr/bin/env python3
import base64
import hashlib
import json
import os
import pathlib
import sys

CONTRACT = "oulipoly.provider/v1"
PROVIDER = {provider}
SESSION_ID = {session_id}
RECORD_PATH = pathlib.Path({record_path})

subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
request = json.loads(sys.stdin.read() or "{{}}")
with RECORD_PATH.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({{"subcommand": subcommand, "request": request}}, sort_keys=True) + "\n")

def request_id():
    return request.get("request_id", "s10-request")

def envelope(result):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": True,
        "result": result,
    }}

def error(code):
    return {{
        "contract": request.get("contract", CONTRACT),
        "request_id": request_id(),
        "ok": False,
        "error": {{"category": "failed", "code": code, "message": code, "retryable": False}},
    }}

def describe():
    return envelope({{
        "provider_id": "agent-runner-provider-ref-fixture",
        "display_name": "External Provider Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {{
            "launch": True,
            "launch_output_v1": True,
            "policy": True,
            "quota": False,
            "session": {session_capability},
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        }},
    }})

def policy_evaluate():
    return envelope({{
        "accepted": True,
        "env": {{}},
        "stdin": None,
        "prompt": None,
        "diagnostics": [],
        "markers": [],
    }})

def emit(event):
    print(json.dumps(event, separators=(",", ":")))

def launch_payload():
    return request.get("params", {{}}).get("model", {{}}).get("inputs", {{}}).get("prompt", "")

def launch_stdout_data(payload):
    return base64.b64encode(launch_stdout_bytes(payload)).decode("ascii")

def launch_stdout_bytes(payload):
    return ("answer:" + payload + "\n").encode("utf-8")

def launch_stdout_event(seq, payload):
    return {{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "stdout",
        "data_base64": launch_stdout_data(payload),
    }}

def launch_error_exit_requested():
    return os.environ.get("S10_PROVIDER_ERROR_EXIT_ZERO") == "1"

def launch_long_stream_requested():
    return os.environ.get("S10_LAUNCH_LONG_STREAM") == "1"

def launch_terminal_signal(kind, evidence, seq):
    return {{
        "kind": kind,
        "evidence": evidence,
        "observed_at_unix_ms": 1000 + seq,
    }}

def launch_session_state():
    return {{
        {launch_session_key}: SESSION_ID,
        "state": {{"cursor": "after-launch"}},
    }}

def launch_exit_event(seq, terminal_signal):
    return {{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "exit",
        "status": {{"kind": "exited", "code": 0}},
        "terminal_signal": terminal_signal,
        "session": launch_session_state(),
    }}

def launch_output_complete_event(seq, payload):
    stdout = launch_stdout_bytes(payload)
    return {{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "marker",
        "name": "oulipoly.launch_output_complete/v1",
        "value": {{
            "protocol": "oulipoly.launch_output/v1",
            "stdout": {{"bytes": len(stdout), "sha256": hashlib.sha256(stdout).hexdigest()}},
            "stderr": {{"bytes": 0, "sha256": hashlib.sha256(b"").hexdigest()}},
            "data_event_count": 1,
        }},
    }}

def provider_error_exit_event(seq):
    return launch_exit_event(seq, launch_terminal_signal("unknown", {incident_terminal_reason}, seq))

def clean_exit_event(seq):
    return launch_exit_event(seq, launch_terminal_signal("clean_exit", "fixture clean exit", seq))

def launch_heartbeat_detail():
    return "h" * 4096

def launch_heartbeat_event(seq, detail):
    return {{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": seq,
        "time_unix_ms": 1000 + seq,
        "kind": "heartbeat",
        "detail": detail,
    }}

def emit_long_launch_heartbeats():
    detail = launch_heartbeat_detail()
    for seq in range(2, 702):
        emit(launch_heartbeat_event(seq, detail))

def launch():
    payload = launch_payload()
    emit(launch_stdout_event(1, payload))
    if os.environ.get("S10_PROVIDER_UNAVAILABLE") == "1":
        selected = request.get("host", {{}}).get("env", {{}}).get("OULIPOLY_HOST_TERMINAL_UNAVAILABLE_V1") == "1"
        kind = "provider_unavailable" if selected else "unknown"
        emit(launch_output_complete_event(2, payload))
        event = launch_exit_event(3, launch_terminal_signal(kind, "upstream temporarily unavailable", 3))
        event["status"]["code"] = int(os.environ.get("S10_PROVIDER_UNAVAILABLE_EXIT_CODE", "0"))
        emit(event)
        return
    if launch_error_exit_requested():
        emit(launch_output_complete_event(2, payload))
        emit(provider_error_exit_event(3))
        return
    if launch_long_stream_requested():
        emit_long_launch_heartbeats()
        emit(launch_output_complete_event(702, payload))
        emit(clean_exit_event(703))
        return
    emit(launch_output_complete_event(2, payload))
    emit(clean_exit_event(3))

def capture():
    params = request.get("params", {{}})
    extra = params.get("extra", {{}})
    return envelope({{
        "provider_session_id": extra.get("pinned_target") or extra.get("start_bound_provider_session_id") or SESSION_ID,
        "state": {{"captured": True}},
        "artifacts": [],
    }})

if subcommand == "describe":
    print(json.dumps(describe()))
elif subcommand == "policy.evaluate":
    print(json.dumps(policy_evaluate()))
elif subcommand == "launch":
    launch()
elif subcommand == "session.capture":
    print(json.dumps(capture()))
else:
    print(json.dumps(error("unsupported_subcommand")))
"#,
        provider = serde_json::to_string(PROVIDER).unwrap(),
        session_id = serde_json::to_string(SESSION_ID).unwrap(),
        incident_terminal_reason = serde_json::to_string(INCIDENT_TERMINAL_REASON).unwrap(),
        launch_session_key = serde_json::to_string(options.launch_session_key).unwrap(),
        session_capability = if options.session_capability {
            "True"
        } else {
            "False"
        },
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
    )
}
