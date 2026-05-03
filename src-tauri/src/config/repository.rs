use super::agent::{AgentConfig, load_agent_file, load_agents};
use super::model::{ModelConfig, load_models};
use super::providers::ProvidersConfig;
use super::sessions::SessionsConfig;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub trait ModelConfigRepository: Send + Sync {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String>;
    fn save_model(&self, model: &ModelConfig) -> Result<(), String>;
    fn delete_model(&self, name: &str) -> Result<(), String>;
}

pub trait ProviderConfigSource: Send + Sync {
    fn load_providers(&self) -> Result<ProvidersConfig, String>;
}

pub trait SessionsConfigSource: Send + Sync {
    fn load_sessions(&self) -> Result<SessionsConfig, String>;
}

pub trait AgentConfigRepository: Send + Sync {
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

    fn validate_model_file_name(name: &str) -> Result<(), String> {
        if name.trim().is_empty()
            || name.contains("..")
            || name.contains('/')
            || name.contains('\\')
            || name.contains(':')
            || name.ends_with([' ', '.'])
        {
            return Err(format!("Invalid model name for file path: {name}"));
        }

        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(format!("Invalid model name for file path: {name}"));
        }

        let reserved_name = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
        let is_reserved = matches!(reserved_name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || reserved_name
                .strip_prefix("COM")
                .and_then(|suffix| suffix.parse::<u8>().ok())
                .is_some_and(|number| (1..=9).contains(&number))
            || reserved_name
                .strip_prefix("LPT")
                .and_then(|suffix| suffix.parse::<u8>().ok())
                .is_some_and(|number| (1..=9).contains(&number));
        if is_reserved {
            return Err(format!("Invalid model name for file path: {name}"));
        }

        Ok(())
    }
}

impl ModelConfigRepository for FilesystemModelConfigRepository {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
        load_models(&self.models_dir)
    }

    fn save_model(&self, model: &ModelConfig) -> Result<(), String> {
        Self::validate_model_file_name(&model.name)?;
        std::fs::create_dir_all(&self.models_dir)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;
        let path = self.models_dir.join(format!("{}.toml", model.name));
        std::fs::write(&path, model.to_toml())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    fn delete_model(&self, name: &str) -> Result<(), String> {
        Self::validate_model_file_name(name)?;
        let path = self.models_dir.join(format!("{name}.toml"));
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete {}: {e}", path.display()))?;
        }
        Ok(())
    }
}

impl ModelConfigRepository for HashMap<String, ModelConfig> {
    fn load_models(&self) -> Result<HashMap<String, ModelConfig>, String> {
        Ok(self.clone())
    }

    fn save_model(&self, _model: &ModelConfig) -> Result<(), String> {
        Err("in-memory model repository is read-only".to_string())
    }

    fn delete_model(&self, _name: &str) -> Result<(), String> {
        Err("in-memory model repository is read-only".to_string())
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

impl ProviderConfigSource for ProvidersConfig {
    fn load_providers(&self) -> Result<ProvidersConfig, String> {
        Ok(self.clone())
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

impl SessionsConfigSource for SessionsConfig {
    fn load_sessions(&self) -> Result<SessionsConfig, String> {
        Ok(self.clone())
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
