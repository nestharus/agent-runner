use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AppConfig {
    pub diagnostics_model: Option<String>,
    pub default_model: Option<String>,
}

pub fn load_app_config() -> AppConfig {
    let config_dir = dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."));
    load_app_config_from_path(&config_dir.join("config.toml")).unwrap_or_default()
}

pub fn load_app_config_from_path(path: &Path) -> Result<AppConfig, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    toml::from_str(&content).map_err(|e| format!("Failed to parse {}: {e}", path.display()))
}
