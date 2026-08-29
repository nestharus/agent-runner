#![cfg(unix)]

use oulipoly_state::{CompositeInvocationId, InvocationStatus, StateDb};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PRIMARY_PROVIDER: &str = "age289-primary-provider";
const DIAGNOSTIC_PROVIDER: &str = "age289-diagnostic-provider";

struct Fixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
    models_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let default_config_root = config_home.join("oulipoly-agent-runner");
        let override_config_root = dir.path().join("override-config");
        let models_dir = override_config_root.join("models");
        fs::create_dir_all(&default_config_root).expect("default config root");
        fs::create_dir_all(&models_dir).expect("override models dir");

        let primary = dir.path().join("primary-provider.sh");
        write_executable(
            &primary,
            "#!/usr/bin/env bash\nprintf 'primary provider failed before producing a report\\n' >&2\nexit 7\n",
        );
        write_models(&models_dir);
        let providers = providers_toml(&primary);
        fs::write(default_config_root.join("providers.toml"), &providers)
            .expect("default providers");
        fs::write(override_config_root.join("providers.toml"), providers)
            .expect("override providers");
        fs::write(
            default_config_root.join("config.toml"),
            "diagnostics_model = \"age289-diagnostic\"\n",
        )
        .expect("app config");

        Self {
            _dir: dir,
            config_home,
            data_home,
            models_dir,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        command.env("XDG_CONFIG_HOME", &self.config_home);
        command.env("XDG_DATA_HOME", &self.data_home);
        command.env(
            "OULIPOLY_DATA_DIR",
            self.data_home.join("oulipoly-agent-runner"),
        );
        command.env_remove("OULIPOLY_PARENT_INVOCATION");
        command
    }

    fn run_failure(&self) -> Output {
        let mut command = self.command();
        command
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg("--model")
            .arg("age289-primary")
            .arg("run security audit");
        command.output().expect("run AGE-289 fixture")
    }

    fn trace(&self, invocation_id: &str) -> Value {
        let mut command = self.command();
        let output = command
            .arg("trace")
            .arg(invocation_id)
            .arg("--json")
            .output()
            .expect("trace AGE-289 fixture");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        serde_json::from_slice(&output.stdout).expect("trace JSON")
    }

    fn open_db(&self) -> StateDb {
        StateDb::open(
            &self
                .data_home
                .join("oulipoly-agent-runner")
                .join("state.db"),
        )
        .expect("state db")
    }
}

#[test]
fn provider_exit_plus_diagnostic_failure_preserves_primary_and_secondary_evidence() {
    let fixture = Fixture::new();
    let output = fixture.run_failure();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    assert!(
        stderr.contains("primary provider failed before producing a report"),
        "{stderr}"
    );
    assert!(
        stderr.contains(
            "[diagnostics] Failed to diagnose: Failed to spawn 'age289-diagnostic-provider'"
        ),
        "{stderr}"
    );

    let diagnostic_failure = marker_payload(&stderr, "OULIPOLY_DIAGNOSTIC_FAILURE");
    assert_eq!(diagnostic_failure["stage"], "diagnostics");
    assert_eq!(diagnostic_failure["operation"], "diagnose_error");
    assert_eq!(diagnostic_failure["error_category"], "diagnostics_failure");
    assert_eq!(diagnostic_failure["provider_exit_code"], 7);

    let result = marker_payload(&stdout, "OULIPOLY_RESULT");
    assert_eq!(result["success"], false);
    assert_eq!(result["exit_code"], 7);
    assert_eq!(result["terminal_reason"], "exit_nonzero");
    assert!(result["error_category"].is_null(), "{result}");
    assert_eq!(result["provider_name"], PRIMARY_PROVIDER);
    assert!(result["provider_session_id"].is_string(), "{result}");
    assert_eq!(
        stdout
            .lines()
            .filter(|line| !line.starts_with("OULIPOLY_RESULT=") && !line.trim().is_empty())
            .count(),
        0,
        "provider failure must not synthesize an agent report: {stdout}"
    );

    let invocation = parse_invocation(&stderr);
    assert_eq!(result["agent_runner_invocation_id"], invocation.id);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .expect("invocation query")
        .expect("invocation row");
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));

    let trace = fixture.trace(&invocation.id);
    let trace_invocation = &trace["root"]["invocation"];
    let trace_session = &trace["root"]["session"];
    assert_eq!(trace_invocation["id"], invocation.id);
    assert!(trace_invocation["error_category"].is_null(), "{trace}");
    assert_eq!(trace_invocation["terminal_reason"], "exit_nonzero");
    assert_eq!(
        trace_session["provider_session_id"],
        result["provider_session_id"]
    );
    assert_eq!(trace_session["transcript_state"], "no_locator");
    assert_eq!(trace_session["turn_count"], 0);
    assert_eq!(trace_session["assistant_turn_count"], 0);
}

fn write_models(models_dir: &Path) {
    fs::write(
        models_dir.join("age289-primary.toml"),
        format!("[[providers]]\nname = {PRIMARY_PROVIDER:?}\n"),
    )
    .expect("primary model");
    fs::write(
        models_dir.join("age289-diagnostic.toml"),
        format!(
            "provider = {{ path = \"/synthetic/age289-diagnostic-provider\" }}\n\n[[providers]]\nname = {DIAGNOSTIC_PROVIDER:?}\n"
        ),
    )
    .expect("diagnostic model");
}

fn providers_toml(primary: &Path) -> String {
    format!(
        r#"[{PRIMARY_PROVIDER}]
command = {primary:?}
args = []
prompt_mode = "arg"

[{PRIMARY_PROVIDER}.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[{DIAGNOSTIC_PROVIDER}]
command = "unused-external-diagnostic-command"
args = []
prompt_mode = "stdin"
"#,
        primary = primary.display().to_string(),
    )
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write executable");
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("chmod executable");
}

fn marker_payload(output: &str, marker: &str) -> Value {
    let prefix = format!("{marker}=");
    let lines = output
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 1, "expected one {marker} marker: {output}");
    serde_json::from_str(lines[0]).expect("marker JSON")
}

fn parse_invocation(stderr: &str) -> CompositeInvocationId {
    let payload = marker_payload(stderr, "OULIPOLY_INVOCATION");
    CompositeInvocationId::parse_env_value(&payload.to_string()).expect("invocation marker")
}
