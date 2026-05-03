#![cfg(unix)]

#[path = "../../../src-tauri/tests/fixtures/b2_process_runner.rs"]
mod b2_process_runner;

use agent_runner_runtime::repl::{ReplOptions, run_repl_with_services};
use agent_runner_runtime::{RuntimePaths, RuntimeServices};
use b2_process_runner::FakeProcessRunner;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

struct ReplFixture {
    _dir: tempfile::TempDir,
    config_root: PathBuf,
    data_root: PathBuf,
    models_dir: PathBuf,
}

impl ReplFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config").join("oulipoly-agent-runner");
        let data_root = dir.path().join("data").join("oulipoly-agent-runner");
        let models_dir = config_root.join("models");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&data_root).unwrap();

        Self {
            _dir: dir,
            config_root,
            data_root,
            models_dir,
        }
    }

    fn write_interactive_model(&self, model_name: &str, provider_name: &str, command: &str) {
        fs::write(
            self.models_dir.join(format!("{model_name}.toml")),
            format!(
                r#"[[providers]]
name = "{provider_name}"
"#
            ),
        )
        .unwrap();
        fs::write(
            self.config_root.join("providers.toml"),
            format!(
                r#"[{provider_name}]
command = "{command}"
args = ["one-shot-only"]
interactive_args = ["launch"]
prompt_mode = "arg"
"#
            ),
        )
        .unwrap();
    }

    fn services(&self, runner: Arc<FakeProcessRunner>) -> RuntimeServices {
        RuntimeServices::new(Box::new(self.paths()), runner)
    }

    fn paths(&self) -> FixtureRuntimePaths {
        FixtureRuntimePaths {
            config_root: self.config_root.clone(),
            data_root: self.data_root.clone(),
            models_dir: self.models_dir.clone(),
        }
    }
}

#[derive(Clone)]
struct FixtureRuntimePaths {
    config_root: PathBuf,
    data_root: PathBuf,
    models_dir: PathBuf,
}

impl RuntimePaths for FixtureRuntimePaths {
    fn data_root(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.clone())
    }

    fn config_root(&self) -> PathBuf {
        self.config_root.clone()
    }

    fn models_dir(&self) -> PathBuf {
        self.models_dir.clone()
    }

    fn agents_dir(&self) -> PathBuf {
        self.config_root.join("agents")
    }

    fn state_db_path(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.join("state.db"))
    }

    fn providers_path(&self) -> PathBuf {
        self.config_root.join("providers.toml")
    }

    fn sessions_path(&self) -> PathBuf {
        self.config_root.join("sessions.toml")
    }

    fn lock_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.join("locks"))
    }

    fn replace_journal_dir(&self) -> Result<PathBuf, String> {
        Ok(self.data_root.join("replace_journal"))
    }
}

/// Risk: D-T5 (REPL extraction preserves existing CLI REPL dispatch)
/// Source: D-agent-binary contract §7
/// Level: component
/// Fixture source: crates/runtime/tests/d_repl_extraction.rs
#[test]
fn run_repl_with_services_preserves_interactive_command_shape() {
    let fixture = ReplFixture::new();
    fixture.write_interactive_model("fixture-model", "fixture-provider", "fixture-cli");
    let runner = Arc::new(FakeProcessRunner::new());
    runner.push_interactive_response(Ok(0));
    let cwd = fixture.data_root.join("project");
    fs::create_dir_all(&cwd).unwrap();

    let exit = run_repl_with_services(
        ReplOptions {
            model: Some("fixture-model".to_string()),
            resume: None,
            migrate: None,
            working_dir: Some(cwd.clone()),
            models_dir_override: Some(fixture.models_dir.clone()),
        },
        fixture.services(runner.clone()),
    )
    .unwrap();

    assert_eq!(exit, 0);
    assert!(runner.calls().is_empty());
    let call = runner.single_interactive_call();
    assert_eq!(call.program, "fixture-cli");
    assert_eq!(call.args, ["launch"]);
    assert_eq!(call.cwd.as_deref(), Some(cwd.as_path()));
    assert_eq!(call.env.len(), 1, "REPL should only inject invocation env");
    assert!(call.env.contains_key("OULIPOLY_PARENT_INVOCATION"));
}
