#![cfg(unix)]

//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use oulipoly_provider::generated::CONTRACT_VERSION;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODEL: &str = "session-import-model";
const UNSUPPORTED_MODEL: &str = "session-import-unsupported-model";
const PROVIDER_A: &str = "provider-a";
const PROVIDER_B: &str = "provider-b";
const PROVIDER_UNSUPPORTED: &str = "provider-unsupported";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
    scripts_dir: PathBuf,
    workspace: PathBuf,
    record_path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let config_home = root.join("config");
        let data_home = root.join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        let scripts_dir = root.join("scripts");
        let workspace = root.join("workspace");
        let record_path = root.join("provider-records.jsonl");
        for path in [&models_dir, &scripts_dir, &workspace] {
            fs::create_dir_all(path).unwrap();
        }
        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
            scripts_dir,
            workspace,
            record_path,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn run_session_import(&self, args: &[&str]) -> Output {
        let mut cmd = self.command();
        cmd.arg("session")
            .arg("import")
            .args(args)
            .output()
            .unwrap()
    }

    fn run_session_list_json(&self) -> Output {
        self.command()
            .arg("session")
            .arg("list")
            .arg("--json")
            .output()
            .unwrap()
    }

    fn write_provider_script(&self, name: &str, enumerate_capability: bool) -> PathBuf {
        let path = self.scripts_dir.join(name);
        fs::write(
            &path,
            fake_provider_script(&self.record_path, &self.workspace, enumerate_capability),
        )
        .unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn write_model(&self, name: &str, provider_script: &Path, providers: &[&str]) {
        fs::write(
            self.models_dir.join(format!("{name}.toml")),
            model_config_toml(provider_script, providers),
        )
        .unwrap();
    }

    fn write_providers(&self, providers: &[&str]) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            providers_config_toml(providers),
        )
        .unwrap();
    }

    fn read_records(&self) -> Vec<Value> {
        let Ok(text) = fs::read_to_string(&self.record_path) else {
            return Vec::new();
        };
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn session_import_cli_imports_provider_native_sessions_backfills_turns_and_lists_them() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("fake-provider.py", true);
    fixture.write_providers(&[PROVIDER_A]);
    fixture.write_model(MODEL, &provider_script, &[PROVIDER_A]);

    let output = fixture.run_session_import(&["--provider", PROVIDER_A, "--backfill-turns"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let stdout = stdout(&output);
    assert!(stdout.contains("Session import report"), "{stdout}");
    assert!(stdout.contains("provider=provider-a"), "{stdout}");
    assert!(stdout.contains("status=succeeded"), "{stdout}");
    assert!(stdout.contains("imported=1"), "{stdout}");
    assert!(stdout.contains("turns_backfilled=1"), "{stdout}");

    let list_output = fixture.run_session_list_json();
    assert_eq!(list_output.status.code(), Some(0), "{list_output:?}");
    assert!(stderr(&list_output).is_empty(), "{list_output:?}");
    let rows: Vec<Value> = serde_json::from_slice(&list_output.stdout).unwrap();
    let row = rows
        .iter()
        .find(|row| row["active_provider_session_id"] == "provider-a-native")
        .expect("provider-a imported row should be listed");
    assert_eq!(row["active_provider"], PROVIDER_A);
    assert_eq!(row["title"], "Provider A native");
    assert_eq!(row["cwd"], fixture.workspace.display().to_string());
    assert_eq!(row["turn_count"], 1);
    assert_eq!(row["is_imported"], true);
}

#[test]
fn session_import_cli_json_filters_provider_and_forwards_enumeration_options() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("fake-provider.py", true);
    fixture.write_providers(&[PROVIDER_A, PROVIDER_B]);
    fixture.write_model(MODEL, &provider_script, &[PROVIDER_A, PROVIDER_B]);

    let output = fixture.run_session_import(&[
        "--provider",
        PROVIDER_B,
        "--limit",
        "1",
        "--since-unix-ms",
        "1782000000000",
        "--json",
    ]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(report["providers"][0]["provider_name"], PROVIDER_B);
    assert_eq!(report["providers"][0]["status"]["kind"], "succeeded");
    assert_eq!(report["providers"][0]["discovered"], 1);
    assert_eq!(report["providers"][0]["imported"], 1);
    assert_eq!(report["totals"]["providers_total"], 1);
    assert_eq!(report["totals"]["warnings"], 1);

    let enumerate_records: Vec<Value> = fixture
        .read_records()
        .into_iter()
        .filter(|record| record["subcommand"] == "session.enumerate")
        .collect();
    assert_eq!(enumerate_records.len(), 1, "{enumerate_records:?}");
    let params = &enumerate_records[0]["params"];
    assert_eq!(params["settings_id"], PROVIDER_B);
    assert_eq!(params["limit"], 1);
    assert_eq!(params["since_unix_ms"], 1782000000000_u64);
}

#[test]
fn session_import_cli_reports_skipped_provider_when_enumerate_capability_is_missing() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("unsupported-provider.py", false);
    fixture.write_providers(&[PROVIDER_UNSUPPORTED]);
    fixture.write_model(UNSUPPORTED_MODEL, &provider_script, &[PROVIDER_UNSUPPORTED]);

    let output = fixture.run_session_import(&["--provider", PROVIDER_UNSUPPORTED, "--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(
        report["providers"][0]["provider_name"],
        PROVIDER_UNSUPPORTED
    );
    assert_eq!(report["providers"][0]["status"]["kind"], "skipped");
    assert!(
        report["providers"][0]["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("session_enumerate_capability_missing"),
        "{report}"
    );
    assert_eq!(report["totals"]["providers_skipped"], 1);
    assert_eq!(report["totals"]["providers_failed"], 0);
}

#[test]
fn session_import_cli_empty_config_reports_no_targets() {
    let fixture = Fixture::new();

    let output = fixture.run_session_import(&[]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        stdout(&output),
        "No session import provider targets found\n"
    );
    assert!(stderr(&output).is_empty(), "{output:?}");
}

fn model_config_toml(provider_script: &Path, providers: &[&str]) -> String {
    let mut body = format!(
        "provider = {{ path = {} }}\nprompt_mode = \"arg\"\n",
        toml_string(&provider_script.display().to_string())
    );
    for provider in providers {
        body.push_str(&format!(
            "\n[[providers]]\nname = {}\nargs = []\n",
            toml_string(provider)
        ));
    }
    body
}

fn providers_config_toml(providers: &[&str]) -> String {
    providers
        .iter()
        .map(|provider| {
            format!(
                "[{}]\ncommand = \"native-provider\"\nargs = []\nprompt_mode = \"arg\"\n",
                provider
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn fake_provider_script(
    record_path: &Path,
    workspace: &Path,
    enumerate_capability: bool,
) -> String {
    let mut script = r#"#!/usr/bin/env python3
import json
import pathlib
import sys

CONTRACT = __CONTRACT_JSON__
RECORD_PATH = pathlib.Path(__RECORD_PATH_JSON__)
WORKSPACE = __WORKSPACE_JSON__
ENUMERATE_CAPABILITY = __ENUMERATE_CAPABILITY__

request = json.loads(sys.stdin.read() or "{}")
subcommand = sys.argv[1] if len(sys.argv) > 1 else ""
params = request.get("params", {})
settings_id = params.get("settings_id", "")

def record_invocation():
    RECORD_PATH.parent.mkdir(parents=True, exist_ok=True)
    with RECORD_PATH.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps({
            "subcommand": subcommand,
            "settings_id": settings_id,
            "params": params,
        }) + "\n")

def envelope(result):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-import"),
        "ok": True,
        "result": result,
    }

def error(code):
    return {
        "contract": request.get("contract", CONTRACT),
        "request_id": request.get("request_id", "request-session-import"),
        "ok": False,
        "error": {
            "category": "failed",
            "code": code,
            "message": code,
            "retryable": False,
        },
    }

def describe():
    return envelope({
        "provider_id": "session-import-fixture",
        "display_name": "Session Import Fixture",
        "contract_versions": [CONTRACT],
        "preferred_contract": CONTRACT,
        "capabilities": {
            "launch": False,
            "policy": False,
            "quota": False,
            "session": True,
            "session_enumerate": ENUMERATE_CAPABILITY,
            "terminal": False,
            "rotation": False,
            "discovery": False,
            "settings": False,
            "setup_brain": False,
            "setup": False,
            "migration": False,
        },
    })

def source():
    return {"kind": "provider_native_list", "detail": "fixture"}

def session(session_id, title, updated_unix_ms, turn_count):
    return {
        "provider_session_id": session_id,
        "title": title,
        "cwd": WORKSPACE,
        "created_unix_ms": updated_unix_ms - 1000,
        "updated_unix_ms": updated_unix_ms,
        "turn_count": turn_count,
        "source": source(),
    }

def sessions_for_settings():
    if settings_id == "provider-a":
        return [session("provider-a-native", "Provider A native", 1782000001000, 3)]
    if settings_id == "provider-b":
        return [
            session("provider-b-native", "Provider B native", 1782000002000, 5),
            session("provider-b-old", "Provider B old", 1781999999000, 7),
        ]
    return [session(settings_id + "-native", settings_id + " native", 1782000003000, 1)]

def enumerate_sessions():
    sessions = sessions_for_settings()
    since_unix_ms = params.get("since_unix_ms")
    if since_unix_ms is not None:
        sessions = [item for item in sessions if item["updated_unix_ms"] >= since_unix_ms]
    limit = params.get("limit")
    if limit is not None:
        sessions = sessions[:int(limit)]
    warnings = ["provider-b warning"] if settings_id == "provider-b" else []
    return envelope({
        "sessions": sessions,
        "complete": True,
        "next_cursor": None,
        "warnings": warnings,
    })

def read_turns():
    session_id = params.get("session_id") or "missing-session"
    return envelope({
        "turns": [{
            "session_id": session_id,
            "turn_id": "turn-" + session_id,
            "role": "assistant",
            "timestamp": "2026-06-01T00:00:00Z",
            "body": [{"type": "text", "text": "turn for " + session_id}],
        }],
        "turn_count": 1,
        "complete": True,
    })

record_invocation()
if subcommand == "describe":
    response = describe()
elif subcommand == "session.enumerate":
    response = enumerate_sessions()
elif subcommand == "session.read_turns":
    response = read_turns()
else:
    response = error("unsupported_subcommand")

print(json.dumps(response))
"#
    .to_string();
    script = script.replace("__CONTRACT_JSON__", &toml_string(CONTRACT_VERSION));
    script = script.replace(
        "__RECORD_PATH_JSON__",
        &toml_string(&record_path.display().to_string()),
    );
    script = script.replace(
        "__WORKSPACE_JSON__",
        &toml_string(&workspace.display().to_string()),
    );
    script = script.replace(
        "__ENUMERATE_CAPABILITY__",
        if enumerate_capability {
            "True"
        } else {
            "False"
        },
    );
    script
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn toml_string(input: &str) -> String {
    serde_json::to_string(input).unwrap()
}
