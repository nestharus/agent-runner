use super::agent::{AgentConfig, load_agent_file, load_agents};
use super::model::{ModelConfig, load_models};
use super::providers::ProvidersConfig;
use super::sessions::SessionsConfig;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub trait ModelConfigRepository {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String>;
    fn save_model(&self, model: &ModelConfig) -> Result<(), String>;
    fn delete_model(&self, name: &str) -> Result<(), String>;
}

pub trait ProviderConfigSource {
    fn load_providers(&self) -> Result<ProvidersConfig, String>;
}

pub trait SessionsConfigSource {
    fn load_sessions(&self) -> Result<SessionsConfig, String>;
}

pub trait AgentConfigRepository {
    fn load_agent_file(&self, path: &Path) -> Result<AgentConfig, String>;
    fn load_agents(&self) -> Result<HashMap<String, AgentConfig>, String>;
}

pub struct FilesystemModelConfigRepository {
    models_dir: PathBuf,
}

impl FilesystemModelConfigRepository {
    pub fn new(models_dir: PathBuf) -> Self {
        Self { models_dir }
    }
}

impl ModelConfigRepository for FilesystemModelConfigRepository {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
        load_models(&self.models_dir)
    }

    fn save_model(&self, model: &ModelConfig) -> Result<(), String> {
        let path = self.models_dir.join(format!("{}.toml", model.name));
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;
        std::fs::write(&path, model.to_toml())
            .map_err(|e| format!("Failed to write model file: {e}"))?;
        Ok(())
    }

    fn delete_model(&self, name: &str) -> Result<(), String> {
        let path = self.models_dir.join(format!("{name}.toml"));
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("Failed to delete model file: {e}"))?;
        }
        Ok(())
    }
}

pub struct FilesystemProviderConfigSource {
    path: PathBuf,
}

impl FilesystemProviderConfigSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl ProviderConfigSource for FilesystemProviderConfigSource {
    fn load_providers(&self) -> Result<ProvidersConfig, String> {
        ProvidersConfig::load(&self.path)
    }
}

pub struct FilesystemSessionsConfigSource {
    path: PathBuf,
}

impl FilesystemSessionsConfigSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl SessionsConfigSource for FilesystemSessionsConfigSource {
    fn load_sessions(&self) -> Result<SessionsConfig, String> {
        SessionsConfig::load(&self.path)
    }
}

pub struct FilesystemAgentConfigRepository {
    agents_dir: PathBuf,
}

impl FilesystemAgentConfigRepository {
    pub fn new(agents_dir: PathBuf) -> Self {
        Self { agents_dir }
    }
}

impl AgentConfigRepository for FilesystemAgentConfigRepository {
    fn load_agent_file(&self, path: &Path) -> Result<AgentConfig, String> {
        load_agent_file(path)
    }

    fn load_agents(&self) -> Result<HashMap<String, AgentConfig>, String> {
        load_agents(&self.agents_dir)
    }
}
