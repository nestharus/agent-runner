#![cfg(unix)]

use agent_runner_state::{InvocationStart, StateDb};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const ROOT_UUID: &str = "11111111-1111-1111-1111-111111111111";
const CHILD_UUID: &str = "22222222-2222-2222-2222-222222222222";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        let script_path = dir.path().join("fixture-provider.sh");
        fs::write(
            &script_path,
            r#"#!/usr/bin/env bash
printf 'fixture-response\n'
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        fs::write(
            models_dir.join("fixture.toml"),
            r#"[[providers]]
name = "fixture-provider"
"#,
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            format!(
                r#"[fixture-provider]
command = "{}"
args = []
prompt_mode = "arg"
"#,
                script_path.display()
            ),
        )
        .unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
            models_dir,
        }
    }

    fn db_path(&self) -> PathBuf {
        self.data_home
            .join("oulipoly-agent-runner")
            .join("state.db")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(&self.db_path()).unwrap()
    }

    fn seed_trace_rows(&self) {
        let db = self.open_db();

        let root_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: ROOT_UUID.to_string(),
                model_name: "fixture".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(root_id, true, 0, None, None)
            .unwrap();

        let child_id = db
            .start_invocation(&InvocationStart {
                invocation_uuid: CHILD_UUID.to_string(),
                model_name: "fixture".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: Some(root_id),
            })
            .unwrap();
        db.finalize_invocation(child_id, false, 7, Some("fixture_error"), None)
            .unwrap();
    }

    fn run_trace(&self, extra_args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("trace");
        cmd.arg(ROOT_UUID);
        cmd.args(extra_args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.output().unwrap()
    }

    fn run_trace_for(&self, invocation_uuid: &str, extra_args: &[&str]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("trace");
        cmd.arg(invocation_uuid);
        cmd.args(extra_args);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.output().unwrap()
    }

    fn run_default_execution(&self) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("fixture")
            .arg("ping");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.output().unwrap()
    }
}

#[test]
fn trace_ascii_dispatches_and_prints_seeded_root() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace(&[]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(ROOT_UUID), "{stdout}");
    assert!(stdout.contains("fixture-provider"), "{stdout}");
    assert!(stdout.contains("session=—"), "{stdout}");
}

#[test]
fn trace_json_dispatches_and_returns_structured_root() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace(&["--json"]);

    assert!(output.status.success(), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["requested_id"], ROOT_UUID);
    assert_eq!(json["root"]["invocation"]["id"], ROOT_UUID);
    assert_eq!(json["root"]["session"]["transcript_state"], "unresolved");
}

#[test]
fn inline_transcript_requires_json() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace(&["--inline-transcript"]);

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--json"), "{stderr}");
}

#[test]
fn inline_transcript_with_json_is_accepted_and_returns_null_payloads() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace(&["--json", "--inline-transcript"]);

    assert!(output.status.success(), "{output:?}");
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(json["root"].get("transcript").is_some());
    assert!(json["root"]["transcript"].is_null());
    assert!(json["root"]["children"][0]["transcript"].is_null());
}

#[test]
fn trace_accepts_max_depth_flag() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace(&["--max-depth", "10"]);

    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(ROOT_UUID), "{stdout}");
}

#[test]
fn default_cli_flow_still_runs_without_subcommand() {
    let fixture = Fixture::new();

    let output = fixture.run_default_execution();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "fixture-response\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("OULIPOLY_INVOCATION="), "{stderr}");
}

#[test]
fn trace_nonexistent_uuid_exits_one_and_keeps_stdout_empty() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace_for("99999999-9999-9999-9999-999999999999", &[]);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Invocation not found"), "{stderr}");
    assert!(
        stderr.contains("99999999-9999-9999-9999-999999999999"),
        "{stderr}"
    );
}

#[test]
fn trace_malformed_uuid_prints_clear_error() {
    let fixture = Fixture::new();
    fixture.seed_trace_rows();

    let output = fixture.run_trace_for("not-a-uuid", &[]);

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(stderr.contains("uuid"), "{stderr}");
    assert!(stderr.contains("invalid"), "{stderr}");
}
