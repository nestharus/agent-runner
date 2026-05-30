//! ## Declared roles
//!
//! `accessor`, `mapper`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/app_paths.rs
//!     role: adapter
//!     Translates:
//!       - Oulipoly config-directory contract
//!       - app config.toml load/default contract
//!       - models-dir to providers.toml parent-path contract
//!       - providers.toml repository load/default contract
//! ```

use oulipoly_config as config;
use oulipoly_config::repositories::{
    FilesystemProvidersConfigRepository, ProvidersConfigRepository,
};
use std::path::{Path, PathBuf};

pub type AppConfig = oulipoly_config::app::AppConfig;

pub fn load_app_config() -> AppConfig {
    let config_path = app_config_path();
    oulipoly_config::app::AppConfig::load(&config_path).unwrap_or_default()
}

pub fn load_providers_for_models_dir(models_dir: &Path) -> config::ProvidersConfig {
    let repo = FilesystemProvidersConfigRepository;
    load_providers_for_models_dir_with(models_dir, &repo)
}

pub fn load_providers_for_models_dir_with(
    models_dir: &Path,
    repo: &dyn ProvidersConfigRepository,
) -> config::ProvidersConfig {
    let providers_path = providers_config_path_for_models_dir(models_dir);
    repo.load_providers(&providers_path).unwrap_or_default()
}

fn app_config_path() -> PathBuf {
    app_config_dir().join("config.toml")
}

fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("oulipoly-agent-runner"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn providers_config_path_for_models_dir(models_dir: &Path) -> PathBuf {
    models_config_root(models_dir).join("providers.toml")
}

pub(crate) fn models_config_root(models_dir: &Path) -> PathBuf {
    models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
