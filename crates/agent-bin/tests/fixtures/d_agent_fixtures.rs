#![allow(dead_code)]

use agent_runner_runtime::{RuntimePaths, RuntimeServices};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::b2_process_runner::FakeProcessRunner;

pub struct AgentFixture {
    _dir: tempfile::TempDir,
    config_root: PathBuf,
    data_root: PathBuf,
    models_dir: PathBuf,
}

impl AgentFixture {
    pub fn new() -> Self {
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

    pub fn config_path(&self) -> PathBuf {
        self.config_root.join("config.toml")
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub fn write_config(&self, body: &str) {
        fs::write(self.config_path(), body).unwrap();
    }

    pub fn write_interactive_model(&self, model_name: &str, provider_name: &str, command: &str) {
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

    pub fn services(&self, runner: Arc<FakeProcessRunner>) -> RuntimeServices {
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
