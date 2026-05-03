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

#[derive(Debug, Clone)]
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
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;
        let path = self.models_dir.join(format!("{}.toml", model.name));
        std::fs::write(&path, model.to_toml())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    fn delete_model(&self, name: &str) -> Result<(), String> {
        let path = self.models_dir.join(format!("{name}.toml"));
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FilesystemProviderConfigSource {
    providers_path: PathBuf,
}

impl FilesystemProviderConfigSource {
    pub fn new(providers_path: PathBuf) -> Self {
        Self { providers_path }
    }
}

impl ProviderConfigSource for FilesystemProviderConfigSource {
    fn load_providers(&self) -> Result<ProvidersConfig, String> {
        ProvidersConfig::load(&self.providers_path)
    }
}

#[derive(Debug, Clone)]
pub struct FilesystemSessionsConfigSource {
    sessions_path: PathBuf,
}

impl FilesystemSessionsConfigSource {
    pub fn new(sessions_path: PathBuf) -> Self {
        Self { sessions_path }
    }
}

impl SessionsConfigSource for FilesystemSessionsConfigSource {
    fn load_sessions(&self) -> Result<SessionsConfig, String> {
        SessionsConfig::load(&self.sessions_path)
    }
}

#[derive(Debug, Clone)]
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
