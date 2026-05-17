use crate::PromptMode;
use crate::agent::{AgentConfig, load_agent_file, load_agents};
use crate::app::AppConfig;
use crate::model::{ModelConfig, ProviderConfig, load_models};
use crate::providers::{ProviderEntry, ProvidersConfig};
use crate::sessions::{SessionSourceEntry, SessionsConfig};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Repository over app-level `config.toml`.
pub trait AppConfigRepository {
    fn load_app_config(&self, path: &Path) -> Result<AppConfig, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemAppConfigRepository;

impl AppConfigRepository for FilesystemAppConfigRepository {
    fn load_app_config(&self, path: &Path) -> Result<AppConfig, String> {
        AppConfig::load(path)
    }
}

/// Repository over named-agent markdown/frontmatter files.
pub trait AgentConfigRepository {
    fn load_agent_file(&self, path: &Path) -> Result<AgentConfig, String>;
    fn load_agents(&self, dir: &Path) -> Result<HashMap<String, AgentConfig>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemAgentConfigRepository;

impl AgentConfigRepository for FilesystemAgentConfigRepository {
    fn load_agent_file(&self, path: &Path) -> Result<AgentConfig, String> {
        load_agent_file(path)
    }

    fn load_agents(&self, dir: &Path) -> Result<HashMap<String, AgentConfig>, String> {
        load_agents(dir)
    }
}

/// Repository over model TOML files.
pub trait ModelConfigRepository {
    fn load_models(&self, dir: &Path) -> Result<HashMap<String, ModelConfig>, String>;
    fn save_model(&self, dir: &Path, model: &ModelConfig) -> Result<(), String>;
    fn list_model_files(&self, dir: &Path) -> Result<Vec<PathBuf>, String>;
    fn delete_model_file(&self, dir: &Path, name: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemModelConfigRepository;

impl ModelConfigRepository for FilesystemModelConfigRepository {
    fn load_models(&self, dir: &Path) -> Result<HashMap<String, ModelConfig>, String> {
        Ok(load_models(dir, None)?)
    }

    fn save_model(&self, dir: &Path, model: &ModelConfig) -> Result<(), String> {
        if model.name.is_empty() {
            return Err("Model name cannot be empty".to_string());
        }
        if model.providers.is_empty() {
            return Err("Model must have at least one provider".to_string());
        }
        fs::create_dir_all(dir).map_err(|e| format!("Failed to create models directory: {e}"))?;
        fs::write(dir.join(format!("{}.toml", model.name)), model.to_toml())
            .map_err(|e| format!("Failed to write model file: {e}"))
    }

    fn list_model_files(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let entries =
            fs::read_dir(dir).map_err(|e| format!("Failed to read models directory: {e}"))?;
        let mut files = Vec::new();
        for entry in entries {
            let path = entry
                .map_err(|e| format!("Failed to read directory entry: {e}"))?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("toml") {
                files.push(path);
            }
        }
        Ok(files)
    }

    fn delete_model_file(&self, dir: &Path, name: &str) -> Result<(), String> {
        let path = dir.join(format!("{name}.toml"));
        if path.exists() {
            fs::remove_file(&path).map_err(|e| format!("Failed to delete model file: {e}"))?;
        }
        Ok(())
    }
}

/// Repository over `providers.toml` and provider resolution.
pub trait ProvidersConfigRepository {
    fn load_providers(&self, path: &Path) -> Result<ProvidersConfig, String>;
    fn get<'a>(&self, config: &'a ProvidersConfig, name: &str) -> Option<&'a ProviderEntry>;
    fn effective_provider(
        &self,
        config: &ProvidersConfig,
        model_provider: &ProviderConfig,
    ) -> Result<(ProviderConfig, PromptMode), String>;
    fn runtime_provider(
        &self,
        config: &ProvidersConfig,
        name: &str,
    ) -> Result<(ProviderConfig, PromptMode), String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemProvidersConfigRepository;

impl ProvidersConfigRepository for FilesystemProvidersConfigRepository {
    fn load_providers(&self, path: &Path) -> Result<ProvidersConfig, String> {
        Ok(ProvidersConfig::load(path)?)
    }

    fn get<'a>(&self, config: &'a ProvidersConfig, name: &str) -> Option<&'a ProviderEntry> {
        config.get(name)
    }

    fn effective_provider(
        &self,
        config: &ProvidersConfig,
        model_provider: &ProviderConfig,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        config.effective_provider(model_provider)
    }

    fn runtime_provider(
        &self,
        config: &ProvidersConfig,
        name: &str,
    ) -> Result<(ProviderConfig, PromptMode), String> {
        config.runtime_provider(name)
    }
}

/// Repository over `sessions.toml`.
pub trait SessionsConfigRepository {
    fn load_sessions(&self, path: &Path) -> Result<SessionsConfig, String>;
    fn get_source<'a>(
        &self,
        config: &'a SessionsConfig,
        name: &str,
    ) -> Option<&'a SessionSourceEntry>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FilesystemSessionsConfigRepository;

impl SessionsConfigRepository for FilesystemSessionsConfigRepository {
    fn load_sessions(&self, path: &Path) -> Result<SessionsConfig, String> {
        SessionsConfig::load(path)
    }

    fn get_source<'a>(
        &self,
        config: &'a SessionsConfig,
        name: &str,
    ) -> Option<&'a SessionSourceEntry> {
        config.get(name)
    }
}
