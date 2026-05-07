#![cfg(unix)]

use oulipoly_state::{CompositeInvocationId, InvocationStart, InvocationStatus, StateDb};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

struct Fixture {
    dir: tempfile::TempDir,
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

        Self {
            dir,
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

    fn write_script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.dir.path().join(name);
        fs::write(
            &path,
            format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}\n"),
        )
        .unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    fn write_model(&self, model_name: &str, provider_name: &str, script_path: &Path) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
args = ["one-shot-only"]
interactive_args = ["model-interactive"]
"#,
            ),
        )
        .unwrap();
        fs::write(
            self.config_home
                .join("oulipoly-agent-runner")
                .join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = "{}"
args = []
interactive_args = ["launch"]
prompt_mode = "arg"
"#,
                script_path.display()
            ),
        )
        .unwrap();
    }

    fn write_model_without_interactive_args(
        &self,
        model_name: &str,
        provider_name: &str,
        script_path: &Path,
    ) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
args = ["one-shot-only"]
            "#,
            ),
        )
        .unwrap();
        fs::write(
            self.config_home
                .join("oulipoly-agent-runner")
                .join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = "{}"
args = []
prompt_mode = "arg"
"#,
                script_path.display()
            ),
        )
        .unwrap();
    }

    fn base_repl_command(&self, model_name: &str, parent_env: Option<&str>) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("repl")
            .arg("--models-dir")
            .arg(&self.models_dir)
            .arg(model_name);
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_PARENT_INVOCATION");
        if let Some(parent_env) = parent_env {
            cmd.env("OULIPOLY_PARENT_INVOCATION", parent_env);
        }
        cmd
    }

    fn run_repl(&self, model_name: &str, parent_env: Option<&str>) -> Output {
        self.base_repl_command(model_name, parent_env)
            .output()
            .unwrap()
    }

    fn spawn_repl(&self, model_name: &str, parent_env: Option<&str>) -> Child {
        self.base_repl_command(model_name, parent_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn spawn_repl_in_own_process_group(&self, model_name: &str, parent_env: Option<&str>) -> Child {
        let mut cmd = self.base_repl_command(model_name, parent_env);
        cmd.process_group(0);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
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
        "stderr should contain exactly one invocation line: {stderr}"
    );
    let raw = lines[0].strip_prefix("OULIPOLY_INVOCATION=").unwrap();
    CompositeInvocationId::parse_env_value(raw).unwrap()
}

fn wait_for_path(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}

#[test]
fn repl_happy_path_emits_single_invocation_line_and_finalizes_succeeded_row() {
    let fixture = Fixture::new();
    let env_dump_path = fixture.dir.path().join("env_dump.txt");
    let script = fixture.write_script(
        "fixture-provider.sh",
        &format!(
            r#"printf '%s' "${{OULIPOLY_PARENT_INVOCATION-}}" > "{dump}"
exit 0"#,
            dump = env_dump_path.display()
        ),
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let invocation = parse_invocation(&stderr);
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(row.parent_invocation_id, None);
    assert_eq!(
        fs::read_to_string(&env_dump_path).unwrap(),
        serde_json::to_string(&invocation).unwrap()
    );
}

#[test]
fn repl_resolves_parent_env_and_overwrites_child_parent_env_payload() {
    let fixture = Fixture::new();
    let env_dump_path = fixture.dir.path().join("env_dump.txt");
    let script = fixture.write_script(
        "fixture-provider.sh",
        &format!(
            r#"printf '%s' "${{OULIPOLY_PARENT_INVOCATION-}}" > "{dump}"
exit 0"#,
            dump = env_dump_path.display()
        ),
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let parent_output = fixture.run_repl("fixture", None);
    assert_eq!(parent_output.status.code(), Some(0), "{parent_output:?}");
    let parent = parse_invocation(&String::from_utf8_lossy(&parent_output.stderr));
    let parent_row = fixture
        .open_db()
        .get_invocation_by_uuid(&parent.id)
        .unwrap()
        .unwrap();

    let parent_env = serde_json::to_string(&parent).unwrap();
    let child_output = fixture.run_repl("fixture", Some(&parent_env));
    assert_eq!(child_output.status.code(), Some(0), "{child_output:?}");
    let child = parse_invocation(&String::from_utf8_lossy(&child_output.stderr));
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(&child.id)
        .unwrap()
        .unwrap();

    assert_eq!(child_row.parent_invocation_id, Some(parent_row.id));
    assert_eq!(
        fs::read_to_string(&env_dump_path).unwrap(),
        serde_json::to_string(&child).unwrap()
    );
}

#[test]
fn repl_sigterm_to_parent_is_forwarded_to_child_and_finalized() {
    let fixture = Fixture::new();
    let ready_path = fixture.dir.path().join("ready.txt");
    let term_marker_path = fixture.dir.path().join("term.txt");
    let script = fixture.write_script(
        "fixture-sigterm.sh",
        &format!(
            r#"trap 'printf term > "{term_marker}"; exit 0' TERM
: > "{ready}"
while :; do
  sleep 1
done"#,
            ready = ready_path.display(),
            term_marker = term_marker_path.display()
        ),
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let child = fixture.spawn_repl("fixture", None);
    wait_for_path(&ready_path);

    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success(), "failed to send SIGTERM to runner");

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(fs::read_to_string(&term_marker_path).unwrap(), "term");

    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert!(row.finished_at.is_some());
}

#[test]
fn repl_sigint_to_process_group_allows_parent_to_reap_and_finalize() {
    let fixture = Fixture::new();
    let ready_path = fixture.dir.path().join("ready.txt");
    let int_marker_path = fixture.dir.path().join("int.txt");
    let script = fixture.write_script(
        "fixture-sigint.sh",
        &format!(
            r#"trap 'printf int > "{int_marker}"; exit 130' INT
: > "{ready}"
while :; do
  sleep 1
done"#,
            ready = ready_path.display(),
            int_marker = int_marker_path.display()
        ),
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let child = fixture.spawn_repl_in_own_process_group("fixture", None);
    wait_for_path(&ready_path);

    let status = Command::new("kill")
        .arg("-INT")
        .arg("--")
        .arg(format!("-{}", child.id()))
        .status()
        .unwrap();
    assert!(status.success(), "failed to send SIGINT to process group");

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(130), "{output:?}");
    assert_eq!(fs::read_to_string(&int_marker_path).unwrap(), "int");

    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(130));
    assert!(row.finished_at.is_some());
}

#[test]
fn repl_normal_nonzero_child_exit_finalizes_failed_row_with_exit_nonzero_reason() {
    let fixture = Fixture::new();
    let script = fixture.write_script("fixture-exit-7.sh", "exit 7");
    fixture.write_model("fixture", "fixture-provider", &script);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(row.finished_at.is_some());
}

// risk: exhaustive surfaces 4-6, 10, 13-14, 17, 23-26; level: particular-integration; source: contract § 5.5, A1, A5, A10
#[test]
fn repl_raw_sigterm_child_death_finalizes_failed_row_with_128_plus_signal_exit_code() {
    // CHARACTERIZATION: T-FINAL-REPL-SIGNAL preserves D-022 exit_code=143 and adds terminal_reason=signal:SIGTERM.
    let fixture = Fixture::new();
    let script = fixture.write_script(
        "fixture-raw-sigterm.sh",
        r#"kill -TERM "$$"
sleep 1"#,
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(143), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(143));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("signal:SIGTERM"));
    assert!(row.finished_at.is_some());
}

#[test]
fn repl_spawn_error_finalizes_failed_row_with_spawn_error_reason() {
    let fixture = Fixture::new();
    let missing_command = fixture.dir.path().join("definitely-missing-repl-provider");
    fixture.write_model("fixture", "fixture-provider", &missing_command);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let invocation = parse_invocation(&String::from_utf8_lossy(&output.stderr));
    let row = fixture
        .open_db()
        .get_invocation_by_uuid(&invocation.id)
        .unwrap()
        .unwrap();

    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(1));
    assert_eq!(row.error_category.as_deref(), Some("spawn_error"));
    assert_eq!(row.terminal_reason.as_deref(), Some("spawn_error"));
    assert!(row.finished_at.is_some());
}

// RISK: process-supervision fence could accidentally parse inherited interactive stderr and finalize unrelated child rows (proposal §test-intent "supervised-child fence test", assumptions A1/A2/A3)
// LEVEL: particular-integration
// SOURCE: contracts/nes-250-contract.md § Test catalog § Process-supervision fence (T-FENCE-SUPERVISOR-INTERACTIVE-SCOPE)
#[test]
fn t_fence_supervisor_interactive_scope_leaves_marker_row_running() {
    let fixture = Fixture::new();
    let child_uuid = "55555555-5555-5555-5555-555555555555";
    fixture
        .open_db()
        .start_invocation(&InvocationStart {
            invocation_uuid: child_uuid.to_string(),
            model_name: "fixture-child-model".to_string(),
            provider_name: "fixture-child".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let child_marker = CompositeInvocationId {
        source: "fixture-child".to_string(),
        id: child_uuid.to_string(),
    }
    .stderr_line();
    let script = fixture.write_script(
        "fixture-marker.sh",
        &format!(
            r#"printf '%s\n' "{child_marker}" >&2
exit 7"#
        ),
    );
    fixture.write_model("fixture", "fixture-provider", &script);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(7), "{output:?}");
    let child_row = fixture
        .open_db()
        .get_invocation_by_uuid(child_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(child_row.status, InvocationStatus::Running);
    assert_eq!(child_row.success, None);
    assert_eq!(child_row.exit_code, None);
    assert_eq!(child_row.terminal_reason, None);
    assert_eq!(child_row.finished_at, None);
}

#[test]
fn repl_exits_one_when_model_is_unknown() {
    let fixture = Fixture::new();
    let output = fixture.run_repl("nonexistent-model", None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.to_lowercase().contains("nonexistent-model"),
        "stderr should name the unknown model: {stderr}"
    );
}

#[test]
fn repl_exits_one_when_provider_has_no_interactive_args() {
    let fixture = Fixture::new();
    let script = fixture.write_script("fixture-noop.sh", "exit 0");
    fixture.write_model_without_interactive_args("fixture", "fixture-provider", &script);

    let output = fixture.run_repl("fixture", None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("interactive_args"),
        "stderr should mention interactive_args: {stderr}"
    );
}
