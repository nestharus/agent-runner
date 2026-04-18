#![cfg(unix)]

use agent_runner_lib::state::{CompositeInvocationId, InvocationStatus, StateDb};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
    env_dump_path: PathBuf,
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
        let env_dump_path = dir.path().join("env_dump.txt");
        fs::write(
            &script_path,
            r#"#!/usr/bin/env bash
SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
printf '%s' "${OULIPOLY_PARENT_INVOCATION-}" > "$SCRIPT_DIR/env_dump.txt"
printf '{}'
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        fs::write(
            models_dir.join("fixture.toml"),
            format!(
                r#"prompt_mode = "arg"

[[providers]]
name = "fixture-provider"
command = "{}"
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
            env_dump_path,
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

    fn run(&self, parent_env: Option<&str>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("fixture")
            .arg("ping");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        if let Some(value) = parent_env {
            cmd.env("OULIPOLY_PARENT_INVOCATION", value);
        }
        cmd.output().unwrap()
    }
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("OULIPOLY_INVOCATION="))
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "stderr should contain exactly one ID line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_INVOCATION=").unwrap();
    CompositeInvocationId::parse_env_value(raw).unwrap()
}

#[test]
fn emits_single_invocation_line_and_finalizes_succeeded_row() {
    let fixture = Fixture::new();
    let output = fixture.run(None);
    assert!(output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    let db = fixture.open_db();
    let row = db.get_invocation_by_uuid(&invocation.id).unwrap().unwrap();

    assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert!(row.finished_at.is_some());
    assert_eq!(
        fs::read_to_string(&fixture.env_dump_path).unwrap(),
        serde_json::to_string(&invocation).unwrap()
    );
}

#[test]
fn resolves_parent_env_and_overwrites_child_subprocess_env() {
    let fixture = Fixture::new();

    let parent_output = fixture.run(None);
    assert!(parent_output.status.success());
    let parent = parse_invocation(&String::from_utf8_lossy(&parent_output.stderr));
    let parent_row = fixture
        .open_db()
        .get_invocation_by_uuid(&parent.id)
        .unwrap()
        .unwrap();

    let parent_env = serde_json::to_string(&parent).unwrap();
    let child_output = fixture.run(Some(&parent_env));
    assert!(child_output.status.success());

    let child = parse_invocation(&String::from_utf8_lossy(&child_output.stderr));
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(&child.id)
        .unwrap()
        .unwrap();

    assert_eq!(child_row.parent_invocation_id, Some(parent_row.id));
    assert_eq!(
        fs::read_to_string(&fixture.env_dump_path).unwrap(),
        serde_json::to_string(&child).unwrap()
    );
}

#[test]
fn ignores_malformed_and_unresolved_parent_env_values() {
    let fixture = Fixture::new();

    for raw in [
        // Malformed JSON entirely.
        "not-json".to_string(),
        // Well-formed JSON with a valid UUID that doesn't exist in the DB.
        r#"{"source":"fixture-provider","id":"00000000-0000-0000-0000-000000000000"}"#.to_string(),
        // Well-formed JSON shape but the id field is not a valid UUID.
        // The contract says invalid UUIDs are silently treated as root —
        // the binary must NOT panic and must record a root invocation.
        r#"{"source":"fixture-provider","id":"not-a-uuid"}"#.to_string(),
    ] {
        let output = fixture.run(Some(&raw));
        assert!(output.status.success());

        let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
        let row = fixture
            .open_db()
            .get_invocation_by_uuid(&invocation.id)
            .unwrap()
            .unwrap();
        assert_eq!(row.parent_invocation_id, None, "{raw}");
    }
}
