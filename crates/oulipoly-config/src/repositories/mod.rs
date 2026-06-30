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
        validate_model_for_save(model)?;
        ensure_model_directory(dir)?;
        let path = model_file_path(dir, model);
        let toml = format_model_toml(model);
        write_model_file(&path, &toml)
    }

    fn list_model_files(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        if is_missing_model_directory(dir) {
            return Ok(Vec::new());
        }
        let entries = read_model_directory(dir)?;
        collect_model_toml_files(entries)
    }

    fn delete_model_file(&self, dir: &Path, name: &str) -> Result<(), String> {
        let path = model_file_path_for_name(dir, name);
        delete_file_if_present(&path)
    }
}

fn validate_model_for_save(model: &ModelConfig) -> Result<(), String> {
    validate_model_name_for_save(model)?;
    validate_model_providers_for_save(model)?;
    Ok(())
}

fn validate_model_name_for_save(model: &ModelConfig) -> Result<(), String> {
    if is_empty_model_name(model) {
        return Err(format_empty_model_name_error());
    }
    Ok(())
}

fn is_empty_model_name(model: &ModelConfig) -> bool {
    model.name.is_empty()
}

fn format_empty_model_name_error() -> String {
    "Model name cannot be empty".to_string()
}

fn validate_model_providers_for_save(model: &ModelConfig) -> Result<(), String> {
    if has_no_model_providers(model) {
        return Err(format_empty_model_providers_error());
    }
    Ok(())
}

fn has_no_model_providers(model: &ModelConfig) -> bool {
    model.providers.is_empty()
}

fn format_empty_model_providers_error() -> String {
    "Model must have at least one provider".to_string()
}

fn ensure_model_directory(dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(format_model_directory_create_error)
}

fn format_model_directory_create_error(err: std::io::Error) -> String {
    format!("Failed to create models directory: {err}")
}

fn model_file_path(dir: &Path, model: &ModelConfig) -> PathBuf {
    model_file_path_for_name(dir, &model.name)
}

fn model_file_path_for_name(dir: &Path, name: &str) -> PathBuf {
    dir.join(format_model_file_name(name))
}

fn format_model_file_name(name: &str) -> String {
    format!("{name}.toml")
}

fn format_model_toml(model: &ModelConfig) -> String {
    model.to_toml()
}

fn write_model_file(path: &Path, toml: &str) -> Result<(), String> {
    fs::write(path, toml).map_err(format_model_write_error)
}

fn format_model_write_error(err: std::io::Error) -> String {
    format!("Failed to write model file: {err}")
}

fn is_missing_model_directory(dir: &Path) -> bool {
    !dir.is_dir()
}

fn read_model_directory(dir: &Path) -> Result<fs::ReadDir, String> {
    fs::read_dir(dir).map_err(format_model_directory_read_error)
}

fn format_model_directory_read_error(err: std::io::Error) -> String {
    format!("Failed to read models directory: {err}")
}

fn collect_model_toml_files(entries: fs::ReadDir) -> Result<Vec<PathBuf>, String> {
    let paths = entries
        .map(read_directory_entry_path)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(filter_model_toml_paths(paths))
}

fn read_directory_entry_path(
    entry: Result<fs::DirEntry, std::io::Error>,
) -> Result<PathBuf, String> {
    entry
        .map(|entry| entry.path())
        .map_err(format_directory_entry_read_error)
}

fn format_directory_entry_read_error(err: std::io::Error) -> String {
    format!("Failed to read directory entry: {err}")
}

fn filter_model_toml_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(filter_model_toml_path)
        .collect()
}

fn filter_model_toml_path(path: PathBuf) -> Option<PathBuf> {
    is_toml_path(&path).then_some(path)
}

fn is_toml_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("toml")
}

fn delete_file_if_present(path: &Path) -> Result<(), String> {
    if is_missing_model_file(path) {
        return Ok(());
    }
    remove_model_file(path)
}

fn is_missing_model_file(path: &Path) -> bool {
    !path.exists()
}

fn remove_model_file(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(format_model_delete_error)
}

fn format_model_delete_error(err: std::io::Error) -> String {
    format!("Failed to delete model file: {err}")
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
