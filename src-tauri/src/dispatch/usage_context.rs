//! Declared roles: accessor, mapper, filter

use crate::cli::paths::{default_config_root, resolve_models_dir};
use crate::usage::cli::Cli;
use oulipoly_config::{ModelConfig, ProvidersConfig, load_models};
use std::collections::HashMap;

pub(super) struct UsageContext {
    pub(super) providers_cfg: ProvidersConfig,
    pub(super) models: Vec<ModelConfig>,
}

pub(super) fn load_usage_context(cli: &Cli) -> Result<UsageContext, String> {
    let parts = load_usage_context_parts(cli)?;
    Ok(usage_context_from_parts(
        parts.providers_cfg,
        parts.models_map,
    ))
}

struct UsageContextParts {
    providers_cfg: ProvidersConfig,
    models_map: HashMap<String, ModelConfig>,
}

fn load_usage_context_parts(cli: &Cli) -> Result<UsageContextParts, String> {
    let providers_cfg = load_usage_providers_config()?;
    let models_map = load_usage_models(cli, &providers_cfg)?;
    Ok(UsageContextParts {
        providers_cfg,
        models_map,
    })
}

fn load_usage_providers_config() -> Result<ProvidersConfig, String> {
    Ok(ProvidersConfig::load(
        &default_config_root().join("providers.toml"),
    )?)
}

fn load_usage_models(
    cli: &Cli,
    providers_cfg: &ProvidersConfig,
) -> Result<HashMap<String, ModelConfig>, String> {
    Ok(load_models(&resolve_models_dir(cli), Some(providers_cfg))?)
}

fn usage_context_from_parts(
    providers_cfg: ProvidersConfig,
    models_map: HashMap<String, ModelConfig>,
) -> UsageContext {
    UsageContext {
        providers_cfg,
        models: sorted_models(models_map),
    }
}

fn sorted_models(models_map: HashMap<String, ModelConfig>) -> Vec<ModelConfig> {
    let mut models: Vec<ModelConfig> = models_map.into_values().collect();
    models.sort_by(|a, b| a.name.cmp(&b.name));
    models
}
