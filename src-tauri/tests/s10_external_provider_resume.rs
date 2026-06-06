#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODEL: &str = "provider-ref-model";
const PROVIDER: &str = "provider-ref-account";
const SESSION_ID: &str = "a9a8c8d0-8f5f-402e-857c-c5c549446beb";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
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
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let workspace = dir.path().join("workspace");
        let hostile_cwd = dir.path().join("hostile-cwd");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&hostile_cwd).unwrap();

        let record_path = dir.path().join("provider-records.jsonl");
        let provider_path = write_external_provider(dir.path(), &record_path, options);
        fs::write(
            models_dir.join(format!("{MODEL}.toml")),
            format!(
                r#"provider = {{ path = {:?} }}
prompt_mode = "arg"

[[providers]]
name = {:?}
args = ["--model", "haiku"]
"#,
                provider_path.display().to_string(),
                PROVIDER,
            ),
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            format!(
                r#"[{PROVIDER}]
command = "native-provider"
args = ["--base"]
prompt_mode = "arg"
"#,
            ),
        )
        .unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            models_dir,
            workspace,
            hostile_cwd,
            record_path,
        }
    }

    fn run_launch(&self) -> Output {
        let mut cmd = self.command();
        cmd.current_dir(&self.workspace)
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg(MODEL)
            .arg("first prompt");
        cmd.output().unwrap()
    }

    fn run_resume(&self) -> Output {
        let mut cmd = self.command();
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
        let conn = rusqlite::Connection::open(self.db_path()).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, session_capture_method, provider_session_id,
                        resume_input_id, provider_session_capture_method
                   FROM invocations
                  ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |row| {
            Ok(InvocationSessionRow {
                session_id: row.get(0)?,
                session_capture_method: row.get(1)?,
                provider_session_id: row.get(2)?,
                resume_input_id: row.get(3)?,
                provider_session_capture_method: row.get(4)?,
            })
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
    }

    fn records(&self) -> Vec<Value> {
        fs::read_to_string(&self.record_path)
            .unwrap()
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
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

fn records_for_subcommand<'a>(records: &'a [Value], subcommand: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["subcommand"] == subcommand)
        .collect()
}

fn assert_no_rotation_or_migration_provider_calls(records: &[Value]) {
    let subcommands = records
        .iter()
        .map(|record| record["subcommand"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(
        subcommands
            .iter()
            .all(|subcommand| !subcommand.starts_with("rotation.")
                && !subcommand.starts_with("migration.")),
        "unexpected rotation/migration calls: {subcommands:?}"
    );
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

def launch():
    payload = request.get("params", {{}}).get("model", {{}}).get("inputs", {{}}).get("prompt", "")
    emit({{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": 1,
        "time_unix_ms": 1001,
        "kind": "stdout",
        "data_base64": base64.b64encode(("answer:" + payload + "\n").encode("utf-8")).decode("ascii"),
    }})
    emit({{
        "contract": CONTRACT,
        "request_id": request_id(),
        "seq": 2,
        "time_unix_ms": 1002,
        "kind": "exit",
        "status": {{"kind": "exited", "code": 0}},
        "terminal_signal": {{
            "kind": "clean_exit",
            "evidence": "fixture clean exit",
            "observed_at_unix_ms": 1002,
        }},
        "session": {{
            {launch_session_key}: SESSION_ID,
            "state": {{"cursor": "after-launch"}},
        }},
    }})

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
        launch_session_key = serde_json::to_string(options.launch_session_key).unwrap(),
        session_capability = if options.session_capability {
            "True"
        } else {
            "False"
        },
        record_path = serde_json::to_string(&record_path.display().to_string()).unwrap(),
    )
}
