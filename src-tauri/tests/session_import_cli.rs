#![cfg(unix)]

//! ## Declared roles
//! orchestration, accessor, mapper, parser, filter, predicate, validator, formatter

use oulipoly_provider::generated::CONTRACT_VERSION;
use serde_json::Value;
use std::env;
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
        cmd.env("OULIPOLY_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env("HOME", &self.data_home);
        cmd.env("PATH", self.path_with_scripts_first());
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn path_with_scripts_first(&self) -> String {
        let current = env::var_os("PATH").unwrap_or_default();
        format!(
            "{}:{}",
            self.scripts_dir.display(),
            current.to_string_lossy()
        )
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

    fn write_empty_stdout_command(&self, name: &str) -> PathBuf {
        let path = self.scripts_dir.join(name);
        fs::write(&path, "#!/usr/bin/env bash\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
        path
    }

    fn write_model_without_provider_ref(&self, name: &str, providers: &[&str]) {
        fs::write(
            self.models_dir.join(format!("{name}.toml")),
            model_config_toml_without_provider_ref(providers),
        )
        .unwrap();
    }

    fn write_providers_with_commands(&self, providers: &[(&str, &Path)]) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            providers_config_toml_with_commands(providers),
        )
        .unwrap();
    }

    fn write_providers_with_command_names(&self, providers: &[(&str, &str)]) {
        fs::write(
            self.app_config_dir.join("providers.toml"),
            providers_config_toml_with_command_names(providers),
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
fn session_import_cli_backfills_provider_native_turns_synchronously_and_lists_metadata() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("fake-provider.py", true);
    fixture.write_providers_with_commands(&[(PROVIDER_A, &provider_script)]);
    fixture.write_model_without_provider_ref(MODEL, &[PROVIDER_A]);

    let output = fixture.run_session_import(&["--provider", PROVIDER_A, "--backfill-turns"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let stdout = stdout(&output);
    assert!(
        stdout.contains("Session import report"),
        "{stdout}; records={:?}",
        fixture.read_records()
    );
    assert!(stdout.contains("provider=provider-a"), "{stdout}");
    assert!(stdout.contains("status=succeeded"), "{stdout}");
    assert!(stdout.contains("imported=1"), "{stdout}");
    assert!(stdout.contains("turns_backfilled=3"), "{stdout}");

    let records = fixture.read_records();
    assert!(
        records
            .iter()
            .any(|record| record["subcommand"] == "session.read_turns"),
        "explicit backfill must read turns synchronously: {records:?}"
    );
    let connection = rusqlite::Connection::open(
        fixture
            .data_home
            .join("oulipoly-agent-runner")
            .join("state.db"),
    )
    .unwrap();
    let stream: (String, String) = connection
        .query_row(
            "SELECT projection, status
             FROM session_turn_ingest_streams
             WHERE provider_name = ?1 AND session_id = ?2",
            rusqlite::params![PROVIDER_A, "provider-a-native"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        stream,
        ("canonical_ingest".to_string(), "caught_up".to_string())
    );

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
    assert_eq!(row["turn_count"], 3);
    assert_eq!(row["is_imported"], true);
}

#[test]
fn session_import_cli_json_filters_provider_and_forwards_enumeration_options() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("fake-provider.py", true);
    fixture.write_providers_with_commands(&[
        (PROVIDER_A, &provider_script),
        (PROVIDER_B, &provider_script),
    ]);
    fixture.write_model_without_provider_ref(MODEL, &[PROVIDER_A, PROVIDER_B]);

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
    fixture.write_providers_with_commands(&[(PROVIDER_UNSUPPORTED, &provider_script)]);
    fixture.write_model_without_provider_ref(UNSUPPORTED_MODEL, &[PROVIDER_UNSUPPORTED]);

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
fn session_import_cli_targets_provider_instances_without_top_level_provider_ref() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("provider-instance-shim.py", true);
    fixture.write_providers_with_commands(&[
        (PROVIDER_A, &provider_script),
        (PROVIDER_B, &provider_script),
    ]);
    fixture.write_model_without_provider_ref(MODEL, &[PROVIDER_A, PROVIDER_B]);

    let output = fixture.run_session_import(&["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 2);
    assert_eq!(report["providers"][0]["provider_name"], PROVIDER_A);
    assert_eq!(report["providers"][1]["provider_name"], PROVIDER_B);
    assert_eq!(report["totals"]["providers_total"], 2);
    assert_eq!(report["totals"]["imported"], 3);

    let enumerate_settings = enumerate_settings(&fixture);
    assert_eq!(enumerate_settings, vec![PROVIDER_A, PROVIDER_B]);
}

#[test]
fn session_import_cli_provider_filter_matches_provider_instance_without_top_level_ref() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("opencode-shim.py", true);
    fixture.write_providers_with_commands(&[("opencode", &provider_script)]);
    fixture.write_model_without_provider_ref("opencode-test", &["opencode"]);

    let output = fixture.run_session_import(&["--provider", "opencode", "--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(report["providers"][0]["provider_name"], "opencode");
    assert_eq!(report["providers"][0]["status"]["kind"], "succeeded");
    assert_eq!(report["totals"]["providers_total"], 1);
    assert_eq!(report["totals"]["imported"], 1);
}

#[test]
fn session_import_cli_instance_slot_command_uses_provider_shim_binary() {
    let fixture = Fixture::new();
    fixture.write_provider_script("agent-runner-opencode", true);
    fixture.write_empty_stdout_command("opencode1");
    fixture.write_providers_with_command_names(&[("opencode", "opencode1")]);
    fixture.write_model_without_provider_ref("opencode-test", &["opencode"]);

    let output = fixture.run_session_import(&["--provider", "opencode", "--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(report["providers"][0]["provider_name"], "opencode");
    assert_eq!(report["providers"][0]["status"]["kind"], "succeeded");
    assert_eq!(report["totals"]["imported"], 1);
    assert_eq!(enumerate_settings(&fixture), vec!["opencode"]);
}

#[test]
fn session_import_cli_skips_non_session_provider_when_describe_transport_is_unavailable() {
    let fixture = Fixture::new();
    fixture.write_empty_stdout_command("media-cli");
    fixture.write_providers_with_command_names(&[("media", "media-cli")]);
    fixture.write_model_without_provider_ref("media-model", &["media"]);

    let output = fixture.run_session_import(&["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 1);
    assert_eq!(report["providers"][0]["provider_name"], "media");
    assert_eq!(report["providers"][0]["status"]["kind"], "skipped");
    assert!(
        report["providers"][0]["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("session_provider_describe_unavailable"),
        "{report}"
    );
    assert_eq!(report["totals"]["providers_skipped"], 1);
    assert_eq!(report["totals"]["providers_failed"], 0);
    assert!(
        fixture.read_records().is_empty(),
        "non-session provider should not be enumerated"
    );
}

#[test]
fn session_import_cli_deduplicates_aliases_with_same_enumerated_source() {
    let fixture = Fixture::new();
    let provider_script = fixture.write_provider_script("shared-store-shim.py", true);
    fixture.write_providers_with_commands(&[
        ("shared-alias-a", &provider_script),
        ("shared-alias-b", &provider_script),
    ]);
    fixture.write_model_without_provider_ref(MODEL, &["shared-alias-a", "shared-alias-b"]);

    let output = fixture.run_session_import(&["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 2);
    assert_eq!(report["providers"][0]["provider_name"], "shared-alias-a");
    assert_eq!(report["providers"][0]["status"]["kind"], "succeeded");
    assert_eq!(report["providers"][0]["imported"], 1);
    assert_eq!(report["providers"][1]["provider_name"], "shared-alias-b");
    assert_eq!(report["providers"][1]["status"]["kind"], "skipped");
    assert!(
        report["providers"][1]["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("duplicate_enumerate_source"),
        "{report}"
    );
    assert_eq!(report["totals"]["imported"], 1);

    let list_output = fixture.run_session_list_json();
    assert_eq!(list_output.status.code(), Some(0), "{list_output:?}");
    let rows: Vec<Value> = serde_json::from_slice(&list_output.stdout).unwrap();
    let shared_rows = rows
        .iter()
        .filter(|row| row["active_provider_session_id"] == "shared-native")
        .collect::<Vec<_>>();
    assert_eq!(shared_rows.len(), 1, "{rows:?}");
    assert_eq!(shared_rows[0]["active_provider"], "shared-alias-a");
}

#[test]
fn session_import_cli_deduplicates_opencode_instance_aliases_through_shared_shim() {
    let fixture = Fixture::new();
    fixture.write_provider_script("agent-runner-opencode", true);
    fixture.write_empty_stdout_command("opencode1");
    fixture.write_empty_stdout_command("opencode2");
    fixture.write_providers_with_command_names(&[
        ("opencode", "opencode1"),
        ("opencode2", "opencode2"),
    ]);
    fixture.write_model_without_provider_ref("opencode-test", &["opencode", "opencode2"]);

    let output = fixture.run_session_import(&["--json"]);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(stderr(&output).is_empty(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["providers"].as_array().unwrap().len(), 2);
    assert_eq!(report["providers"][0]["provider_name"], "opencode");
    assert_eq!(report["providers"][0]["status"]["kind"], "succeeded");
    assert_eq!(report["providers"][1]["provider_name"], "opencode2");
    assert_eq!(report["providers"][1]["status"]["kind"], "skipped");
    assert!(
        report["providers"][1]["status"]["reason"]
            .as_str()
            .unwrap()
            .contains("duplicate_enumerate_source"),
        "{report}"
    );
    assert_eq!(report["totals"]["imported"], 1);

    let list_output = fixture.run_session_list_json();
    assert_eq!(list_output.status.code(), Some(0), "{list_output:?}");
    let rows: Vec<Value> = serde_json::from_slice(&list_output.stdout).unwrap();
    let opencode_rows = rows
        .iter()
        .filter(|row| row["active_provider_session_id"] == "opencode-shared-native")
        .collect::<Vec<_>>();
    assert_eq!(opencode_rows.len(), 1, "{rows:?}");
    assert_eq!(opencode_rows[0]["active_provider"], "opencode");
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

fn model_config_toml_without_provider_ref(providers: &[&str]) -> String {
    let mut body = "prompt_mode = \"arg\"\n".to_string();
    append_model_providers(&mut body, providers);
    body
}

fn append_model_providers(body: &mut String, providers: &[&str]) {
    for provider in providers {
        body.push_str(&format!(
            "\n[[providers]]\nname = {}\nargs = []\n",
            toml_string(provider)
        ));
    }
}

fn providers_config_toml_with_commands(providers: &[(&str, &Path)]) -> String {
    providers
        .iter()
        .map(|(provider, command)| {
            format!(
                "[{}]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
                provider,
                toml_string(&command.display().to_string())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn providers_config_toml_with_command_names(providers: &[(&str, &str)]) -> String {
    providers
        .iter()
        .map(|(provider, command)| {
            format!(
                "[{}]\ncommand = {}\nargs = []\nprompt_mode = \"arg\"\n",
                provider,
                toml_string(command)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn enumerate_settings(fixture: &Fixture) -> Vec<String> {
    fixture
        .read_records()
        .into_iter()
        .filter(|record| record["subcommand"] == "session.enumerate")
        .map(|record| record["settings_id"].as_str().unwrap().to_string())
        .collect()
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
            "session_turn_pages_v1": ENUMERATE_CAPABILITY,
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
    if settings_id in ("opencode", "opencode2", "opencode3", "opencode4", "opencode5"):
        return [session("opencode-shared-native", "OpenCode shared native", 1782000005000, 4)]
    if settings_id in ("shared-alias-a", "shared-alias-b"):
        return [session("shared-native", "Shared native", 1782000004000, 2)]
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
    session_id = params.get("session_id")
    advertised = next(
        (item["turn_count"] for item in sessions_for_settings()
         if item["provider_session_id"] == session_id),
        0,
    )
    turns = []
    for sequence in range(advertised):
        turns.append({
            "session_id": session_id,
            "turn_id": session_id + "-turn-" + str(sequence),
            "snapshot_sequence": sequence,
            "timestamp": "2026-06-21T00:00:00Z",
            "role": "assistant" if sequence % 2 else "user",
            "parent_turn_id": None,
            "is_sidechain": False,
            "is_compaction_boundary": False,
            "body_state": "absent",
            "body": None,
            "body_bytes": None,
            "body_sha256": None,
            "canonical_text_sha256": None,
        })
    return envelope({
        "read_protocol": "oulipoly.session_turn_pages/v1",
        "provider_instance_id": request.get("provider_instance_id"),
        "settings_id": settings_id,
        "session_id": session_id,
        "turn_projection": params.get("turn_projection"),
        "snapshot_id": "session-import-snapshot:" + session_id,
        "page_index": 0,
        "page_start_sequence": 0,
        "turns": turns,
        "page_turn_count": len(turns),
        "source_bytes_examined": 1,
        "scan_progress": False,
        "snapshot_complete": True,
        "next_page_token": None,
        "resume_token": "session-import-resume:" + session_id,
        "source_final": True,
        "warnings": [],
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
