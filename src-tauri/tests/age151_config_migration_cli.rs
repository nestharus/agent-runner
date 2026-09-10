#![cfg(unix)]

mod provider_authority_fixture;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

struct CliFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    app_config_dir: PathBuf,
    models_dir: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        Self {
            _dir: dir,
            config_home,
            data_home,
            app_config_dir,
            models_dir,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        cmd.env("HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        cmd
    }

    fn write_model(&self, name: &str, body: &str) -> PathBuf {
        let path = self.models_dir.join(format!("{name}.toml"));
        fs::write(&path, body).unwrap();
        path
    }

    fn providers_path(&self) -> PathBuf {
        self.app_config_dir.join("providers.toml")
    }

    fn sessions_path(&self) -> PathBuf {
        self.app_config_dir.join("sessions.toml")
    }

    fn write_valid_providers(&self, body: &str) {
        fs::write(
            self.providers_path(),
            provider_authority_fixture::with_explicit_provider_authority(body),
        )
        .unwrap();
    }

    fn migrate_config_output(&self) -> Output {
        self.command()
            .arg("migrate-config")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .output()
            .unwrap()
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn age151_migrate_config_missing_sessions_toml_is_noop_for_session_storage_backfill() {
    let fixture = CliFixture::new();
    fixture.write_valid_providers(
        r#"[claude]
command = "claude"
args = []
prompt_mode = "arg"
"#,
    );
    assert!(
        !fixture.sessions_path().exists(),
        "fixture must exercise the missing sessions.toml branch"
    );

    let output = fixture.migrate_config_output();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        stdout(&output).contains("migrate-config: providers_touched=1 model_files_rewritten=0"),
        "{output:?}"
    );
    let providers = fs::read_to_string(fixture.providers_path()).unwrap();
    let parsed = providers.parse::<toml::Table>().unwrap();
    assert!(
        parsed["claude"]
            .as_table()
            .unwrap()
            .get("session_storage")
            .is_none(),
        "{providers}"
    );
}

#[test]
fn age151_migrate_config_preserves_existing_session_storage_and_ignores_unbackfillable_sessions() {
    let fixture = CliFixture::new();
    fixture.write_valid_providers(
        r#"[claude]
command = "claude"
args = []
prompt_mode = "arg"

[claude.session_storage]
kind = "claude_code"
projects_dir = "/already/configured"

[codex]
command = "codex"
args = []
prompt_mode = "arg"

[missing-provider]
command = "missing-provider"
args = []
prompt_mode = "arg"
"#,
    );
    fs::write(
        fixture.sessions_path(),
        r#"[claude]
turn_script = "claude-code-turns /ignored/because-existing-storage"
state_dir = "/state"

[codex]
state_dir = "/state"

[not-in-providers]
turn_script = "claude-code-turns /ignored/because-provider-absent"
state_dir = "/state"
"#,
    )
    .unwrap();

    let output = fixture.migrate_config_output();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        !stdout(&output).contains("turn_script -> providers.toml"),
        "{output:?}"
    );
    let providers = fs::read_to_string(fixture.providers_path()).unwrap();
    let parsed = providers.parse::<toml::Table>().unwrap();
    let claude_storage = parsed["claude"]
        .as_table()
        .unwrap()
        .get("session_storage")
        .unwrap()
        .as_table()
        .unwrap();
    assert_eq!(claude_storage["kind"].as_str(), Some("script"));
    assert_eq!(claude_storage["storage_type"].as_str(), Some("claude_code"));
    assert_eq!(
        claude_storage["transcript_script"].as_str(),
        Some("claude-code-locate-transcript /already/configured")
    );
    assert!(!providers.contains("/ignored/because-existing-storage"));
    assert!(
        parsed["codex"]
            .as_table()
            .unwrap()
            .get("session_storage")
            .is_none(),
        "{providers}"
    );
    assert!(
        parsed["missing-provider"]
            .as_table()
            .unwrap()
            .get("session_storage")
            .is_none(),
        "{providers}"
    );
}

#[test]
fn age151_migrate_config_reports_malformed_providers_toml_without_rewriting_models() {
    let fixture = CliFixture::new();
    let model_path = fixture.write_model(
        "legacy",
        r#"[[providers]]
name = "legacy"
command = "legacy"
"#,
    );
    let before_model = fs::read(&model_path).unwrap();
    fs::write(fixture.providers_path(), "[broken\n").unwrap();

    let output = fixture.migrate_config_output();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        stderr(&output).contains("TOML parse error in"),
        "{output:?}"
    );
    assert_eq!(fs::read(&model_path).unwrap(), before_model);
}

#[test]
fn age151_migrate_config_rejects_non_table_provider_root_entry_without_rewriting_models() {
    let fixture = CliFixture::new();
    fs::write(fixture.providers_path(), "legacy = \"not-a-table\"\n").unwrap();
    let model_path = fixture.write_model(
        "legacy",
        r#"[[providers]]
name = "legacy"
command = "legacy"
"#,
    );
    let before_model = fs::read(&model_path).unwrap();

    let output = fixture.migrate_config_output();

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        stderr(&output).contains("providers.toml entry [legacy] is not a table"),
        "{output:?}"
    );
    assert_eq!(fs::read(&model_path).unwrap(), before_model);
}

#[test]
fn age151_migrate_config_keeps_missing_model_flag_values_model_specific_and_lone_c_runtime() {
    let fixture = CliFixture::new();
    let model_path = fixture.write_model(
        "legacy",
        r#"[[providers]]
name = "legacy"
command = "env -u CLAUDECODE claude"
args = ["--model", "-m", "-c", "-c", "model_temperature=1", "-c", "sandbox=workspace-write"]
"#,
    );

    let output = fixture.migrate_config_output();

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let model = fs::read_to_string(model_path).unwrap();
    let providers = fs::read_to_string(fixture.providers_path()).unwrap();
    let model_toml = model.parse::<toml::Table>().unwrap();
    let model_args = model_toml["providers"]
        .as_array()
        .unwrap()
        .first()
        .unwrap()
        .as_table()
        .unwrap()["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(model_args, ["--model", "-m"], "{model}");

    let providers_toml = providers.parse::<toml::Table>().unwrap();
    let runtime_args = providers_toml["legacy"].as_table().unwrap()["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        runtime_args,
        [
            "-u",
            "CLAUDECODE",
            "claude",
            "-c",
            "-c",
            "model_temperature=1",
            "-c",
            "sandbox=workspace-write"
        ],
        "{providers}"
    );
}
