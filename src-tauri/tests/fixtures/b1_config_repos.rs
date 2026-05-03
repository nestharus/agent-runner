#![allow(dead_code)]

use agent_runner_config::{
    FilesystemAgentConfigRepository, FilesystemModelConfigRepository,
    FilesystemProviderConfigSource, FilesystemSessionsConfigSource,
};
use std::fs;
use std::path::{Path, PathBuf};

pub struct ConfigRepoFixture {
    _dir: tempfile::TempDir,
    models_dir: PathBuf,
    providers_path: PathBuf,
    sessions_path: PathBuf,
    agents_dir: PathBuf,
}

impl ConfigRepoFixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        Self {
            models_dir: root.join("models"),
            providers_path: root.join("providers.toml"),
            sessions_path: root.join("sessions.toml"),
            agents_dir: root.join("agents"),
            _dir: dir,
        }
    }

    pub fn model_repo(&self) -> FilesystemModelConfigRepository {
        FilesystemModelConfigRepository::new(self.models_dir.clone())
    }

    pub fn provider_source(&self) -> FilesystemProviderConfigSource {
        FilesystemProviderConfigSource::new(self.providers_path.clone())
    }

    pub fn sessions_source(&self) -> FilesystemSessionsConfigSource {
        FilesystemSessionsConfigSource::new(self.sessions_path.clone())
    }

    pub fn agent_repo(&self) -> FilesystemAgentConfigRepository {
        FilesystemAgentConfigRepository::new(self.agents_dir.clone())
    }

    pub fn write_model_file(&self, filename: &str, body: &str) {
        fs::create_dir_all(&self.models_dir).unwrap();
        fs::write(self.models_dir.join(filename), body).unwrap();
    }

    pub fn write_provider_file(&self, body: &str) {
        write_parented(&self.providers_path, body);
    }

    pub fn write_sessions_file(&self, body: &str) {
        write_parented(&self.sessions_path, body);
    }

    pub fn write_agent_file(&self, filename: &str, body: &str) {
        fs::create_dir_all(&self.agents_dir).unwrap();
        fs::write(self.agents_dir.join(filename), body).unwrap();
    }
}

fn write_parented(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}
