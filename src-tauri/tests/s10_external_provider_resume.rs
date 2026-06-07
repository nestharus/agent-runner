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

mod age153_support;

use age153_support::assert_result_envelope_shape;
use oulipoly_state::InvocationStatus;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODEL: &str = "provider-ref-model";
const PROVIDER: &str = "provider-ref-account";
const SESSION_ID: &str = "a9a8c8d0-8f5f-402e-857c-c5c549446beb";
const INCIDENT_TERMINAL_REASON: &str =
    "provider error: opencode UnknownError: Failed to execute statement";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    workspace: PathBuf,
    hostile_cwd: PathBuf,
    record_path: PathBuf,
}

struct FixturePaths {
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    workspace: PathBuf,
    hostile_cwd: PathBuf,
    record_path: PathBuf,
}

#[derive(Debug)]
struct InvocationSessionRow {
    session_id: Option<String>,
    session_capture_method: Option<String>,
    provider_session_id: Option<String>,
    resume_input_id: Option<String>,
    provider_session_capture_method: Option<String>,
}

#[derive(Debug)]
struct InvocationOutcomeRow {
    status: String,
    success: i64,
    exit_code: i64,
    terminal_reason: Option<String>,
}

struct ProviderOptions {
    launch_session_key: &'static str,
    session_capability: bool,
}

impl ProviderOptions {
    fn provider_session_id() -> Self {
        Self {
            launch_session_key: "provider_session_id",
            session_capability: true,
        }
    }

    fn session_id_without_session_capability() -> Self {
        Self {
            launch_session_key: "session_id",
            session_capability: false,
        }
    }
}

impl Fixture {
    fn new() -> Self {
        Self::new_with_provider_options(ProviderOptions::provider_session_id())
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
        let mut cmd = self.command();
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.current_dir(&self.workspace)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("first prompt");
        cmd.output().unwrap()
    }

    fn run_resume(&self) -> Output {
        self.run_resume_with_env(&[])
    }

    fn run_resume_with_env(&self, envs: &[(&str, &str)]) -> Output {
        let mut cmd = self.command();
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.current_dir(&self.hostile_cwd)
            .arg("resume")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("--session-id")
            .arg(SESSION_ID)
            .arg("--prompt")
            .arg("resume prompt");
        cmd.output().unwrap()
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
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
}

fn materialize_fixture(root: &Path, paths: &FixturePaths, options: ProviderOptions) {
    create_fixture_directories(paths);
    let provider_path = write_external_provider(root, &paths.record_path, options);
    write_model_config(&paths.models_dir, &provider_path);
    write_providers_config(&paths.app_config_dir);
}

fn fixture_paths(root: &Path) -> FixturePaths {
    let config_home = root.join("config");
    let app_config_dir = config_home.join("oulipoly-agent-runner");
    FixturePaths {
        data_home: root.join("data"),
        models_dir: app_config_dir.join("models"),
        workspace: root.join("workspace"),
        hostile_cwd: root.join("hostile-cwd"),
        record_path: root.join("provider-records.jsonl"),
        config_home,
        app_config_dir,
    }
}

fn create_fixture_directories(paths: &FixturePaths) {
    fs::create_dir_all(&paths.models_dir).unwrap();
    fs::create_dir_all(&paths.workspace).unwrap();
    fs::create_dir_all(&paths.hostile_cwd).unwrap();
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

fn write_providers_config(app_config_dir: &Path) {
    fs::write(
        app_config_dir.join("providers.toml"),
        providers_config_toml(),
    )
    .unwrap();
}

fn providers_config_toml() -> String {
    format!(
        r#"[{PROVIDER}]
command = "native-provider"
args = ["--base"]
prompt_mode = "arg"
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
        record_path: paths.record_path,
    }
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
        "SELECT status, success, exit_code, terminal_reason
           FROM invocations
          WHERE provider_name = ?1
          ORDER BY id DESC
          LIMIT 1",
        [PROVIDER],
        invocation_outcome_row,
    )
    .unwrap()
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

#[test]
fn external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd() {
    let fixture = Fixture::new();

    let launch = fixture.run_launch();
    assert_success(&launch);

    let resume = fixture.run_resume();
    assert_success(&resume);
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
    assert_external_launch_session_capture_row(&rows[0]);
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

    assert_failed_terminal_error_process(&output);
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
    assert_external_launch_session_capture_row(&rows[0]);

    let resume = &rows[1];
    assert_eq!(resume.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(resume.session_capture_method.as_deref(), Some("resumed"));
    assert_eq!(resume.provider_session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(resume.resume_input_id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        resume.provider_session_capture_method.as_deref(),
        Some("resumed")
    );
}

fn assert_external_launch_session_capture_row(launch: &InvocationSessionRow) {
    assert_eq!(launch.session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        launch.session_capture_method.as_deref(),
        Some("external_provider_launch")
    );
    assert_eq!(launch.provider_session_id.as_deref(), Some(SESSION_ID));
    assert_eq!(launch.resume_input_id.as_deref(), None);
    assert_eq!(
        launch.provider_session_capture_method.as_deref(),
        Some("external_provider_launch")
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

fn assert_failed_terminal_error_output(output: &Output) {
    assert_failed_terminal_error_process(output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let result = assert_result_envelope_shape(&stdout);
    assert_eq!(result["status"], "failed");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], -1);
    assert_eq!(result["terminal_reason"], INCIDENT_TERMINAL_REASON);
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
    return base64.b64encode(("answer:" + payload + "\n").encode("utf-8")).decode("ascii")

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

def provider_error_exit_event():
    return launch_exit_event(2, launch_terminal_signal("unknown", {incident_terminal_reason}, 2))

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
    emit(launch_stdout_event(1, launch_payload()))
    if launch_error_exit_requested():
        emit(provider_error_exit_event())
        return
    exit_seq = 2
    if launch_long_stream_requested():
        emit_long_launch_heartbeats()
        exit_seq = 702
    emit(clean_exit_event(exit_seq))

def read_turns():
    params = request.get("params", {{}})
    extra = params.get("extra", {{}})
    session_id = params.get("session_id") or extra.get("start_bound_provider_session_id") or extra.get("pinned_target") or SESSION_ID
    turn_id = "turn-" + extra.get("invocation_uuid", "fixture")[:8]
    return envelope({{
        "turns": [{{
            "session_id": session_id,
            "turn_id": turn_id,
            "role": "assistant",
            "timestamp": "2026-06-01T00:00:00Z",
            "body": [{{"type": "text", "text": "fixture turn"}}],
        }}],
        "turn_count": 1,
        "complete": True,
    }})

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
elif subcommand == "session.read_turns":
    print(json.dumps(read_turns()))
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
