#![cfg(unix)]

use agent_runner_lib::state::{CompositeInvocationId, InvocationStatus, StateDb};
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
                r#"prompt_mode = "arg"

[[providers]]
name = "{provider_name}"
command = "{}"
args = ["one-shot-only"]
interactive_args = ["launch"]
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
