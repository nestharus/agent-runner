//! ## Declared roles
//!
//! `accessor`

use oulipoly_config::repositories::ProvidersConfigRepository;
use oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProvidersConfig};
use oulipoly_state::StateDb;
use oulipoly_state::repositories::StateDbOpener;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::formatter;

pub(crate) struct TestModelContext {
    pub(crate) db: StateDb,
    pub(crate) providers_cfg: ProvidersConfig,
}

fn model_config_for_name(
    models: &Mutex<HashMap<String, ModelConfig>>,
    name: &str,
) -> Result<ModelConfig, String> {
    let models = models.lock().map_err(|e| e.to_string())?;
    models
        .get(name)
        .cloned()
        .ok_or_else(|| formatter::format_model_not_found_error(name))
}

pub(crate) fn test_model_command_inputs(
    models: &Mutex<HashMap<String, ModelConfig>>,
    models_dir: &Path,
    name: &str,
) -> Result<(ModelConfig, PathBuf), String> {
    Ok((
        model_config_for_name(models, name)?,
        state_db_path_for_models_dir(models_dir),
    ))
}

#[cfg(test)]
pub(crate) fn test_model_for_test_inputs(
    models: &HashMap<String, ModelConfig>,
    models_dir: &Path,
    name: &str,
) -> Result<(ModelConfig, PathBuf), String> {
    Ok((
        models
            .get(name)
            .cloned()
            .ok_or_else(|| formatter::format_model_not_found_error(name))?,
        state_db_path_for_models_dir(models_dir),
    ))
}

fn open_test_model_state_db(
    state_db_opener: &(dyn StateDbOpener + Send + Sync),
    db_path: &Path,
) -> Result<StateDb, String> {
    state_db_opener.open_at(db_path).map_err(|e| e.to_string())
}

fn state_db_path_for_models_dir(models_dir: &Path) -> PathBuf {
    models_dir.parent().unwrap_or(models_dir).join("state.db")
}

fn load_providers_config_or_default(
    providers_repository: &(dyn ProvidersConfigRepository + Send + Sync),
    models_dir: &Path,
) -> ProvidersConfig {
    let providers_path = models_dir
        .parent()
        .unwrap_or(models_dir)
        .join("providers.toml");
    providers_repository
        .load_providers(&providers_path)
        .unwrap_or_default()
}

pub(crate) fn test_model_context(
    state_db_opener: &(dyn StateDbOpener + Send + Sync),
    providers_repository: &(dyn ProvidersConfigRepository + Send + Sync),
    models_dir: &Path,
    db_path: &Path,
) -> Result<TestModelContext, String> {
    Ok(TestModelContext {
        db: open_test_model_state_db(state_db_opener, db_path)?,
        providers_cfg: load_providers_config_or_default(providers_repository, models_dir),
    })
}

pub(crate) fn pool_member_provider_at_index(
    model: &ModelConfig,
    provider_index: usize,
) -> ProviderConfig {
    model
        .providers
        .get(provider_index)
        .expect("validated provider index should resolve")
        .clone()
}

pub(crate) fn model_command_provider_name(provider: &ProviderConfig) -> &str {
    &provider.command
}

pub(crate) fn configured_effective_provider_for_provider(
    providers_cfg: &ProvidersConfig,
    provider: &ProviderConfig,
) -> Result<(ProviderConfig, PromptMode), String> {
    providers_cfg.effective_provider(provider)
}
