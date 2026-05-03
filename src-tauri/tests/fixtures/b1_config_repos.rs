#![allow(dead_code)]

use agent_runner_lib::config::{ModelConfig, PromptMode, ProviderConfig};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub struct ConfigRepoFixture {
    dir: tempfile::TempDir,
    models_dir: PathBuf,
    agents_dir: PathBuf,
    providers_path: PathBuf,
    sessions_path: PathBuf,
}

impl ConfigRepoFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let models_dir = dir.path().join("models");
        let agents_dir = dir.path().join("agents");
        let providers_path = dir.path().join("providers.toml");
        let sessions_path = dir.path().join("sessions.toml");
        Self {
            dir,
            models_dir,
            agents_dir,
            providers_path,
            sessions_path,
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub fn agents_dir(&self) -> &Path {
        &self.agents_dir
    }

    pub fn providers_path(&self) -> &Path {
        &self.providers_path
    }

    pub fn sessions_path(&self) -> &Path {
        &self.sessions_path
    }

    pub fn write_model_toml(&self, file_stem: &str, body: &str) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(self.models_dir.join(format!("{file_stem}.toml")), body).unwrap();
    }

    pub fn write_non_utf8_model_filename(&self) {
        fs::create_dir_all(&self.models_dir).unwrap();
        let mut bytes = b"bad-".to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(b".toml");
        fs::write(
            self.models_dir.join(OsString::from_vec(bytes)),
            "[[providers]]\nname = \"x\"\n",
        )
        .unwrap();
    }

    pub fn block_models_dir_creation(&self) -> PathBuf {
        let blocking_file = self.root().join("not-a-directory");
        fs::write(&blocking_file, "blocks directory creation").unwrap();
        blocking_file.join("models")
    }

    pub fn write_providers_toml(&self, body: &str) {
        fs::write(&self.providers_path, body).unwrap();
    }

    pub fn write_sessions_toml(&self, body: &str) {
        fs::write(&self.sessions_path, body).unwrap();
    }

    pub fn write_agent_file(&self, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(&self.agents_dir).unwrap();
        let path = self.agents_dir.join(format!("{name}.md"));
        fs::write(&path, body).unwrap();
        path
    }

    pub fn write_non_md_agent_file(&self, name: &str) {
        fs::create_dir_all(&self.agents_dir).unwrap();
        fs::write(
            self.agents_dir.join(format!("{name}.txt")),
            "---\nmodel: x\n---\nignored",
        )
        .unwrap();
    }
}

pub fn model(name: &str, providers: &[&str]) -> ModelConfig {
    ModelConfig {
        name: name.to_string(),
        prompt_mode: PromptMode::Stdin,
        providers: providers
            .iter()
            .map(|provider| ProviderConfig::model_provider(*provider, Vec::new()))
            .collect(),
        inputs: Vec::new(),
    }
}

pub fn single_provider_model_toml(provider: &str) -> String {
    format!("[[providers]]\nname = \"{provider}\"\n")
}

pub fn provider_config_toml(storage_root: &Path) -> String {
    format!(
        r#"[claude]
command = "claude"
args = ["--fast"]
interactive_args = ["repl"]
prompt_mode = "arg"

[claude.session_storage]
kind = "claude_code"
projects_dir = "{}"
"#,
        storage_root.display()
    )
}

pub fn sessions_config_toml() -> &'static str {
    r#"[claude]
turn_script = "cat transcript.jsonl"
transcript_locator = "printf transcript.jsonl"
state_dir = "~/oulipoly-state"
"#
}

pub fn agent_markdown(model_name: &str) -> String {
    format!(
        r#"---
model: {model_name}
---
# Agent

Use the configured model.
"#
    )
}

pub fn isolated_home<F: FnOnce()>(home: &Path, body: F) {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let previous = std::env::var_os("HOME");
    unsafe {
        std::env::set_var("HOME", home);
    }
    body();
    unsafe {
        match previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
    }
}
