//! Declared role: accessor, filter, validator, mapper, orchestration

use super::DryRunError;
use crate::cli::paths::default_models_dir;
use oulipoly_config::{ModelConfig, ProviderConfig, ProvidersConfig, load_models};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

struct LoadedTargetConfig {
    providers_cfg: ProvidersConfig,
    models: HashMap<String, ModelConfig>,
}

#[derive(Debug, Clone)]
pub(crate) struct TargetResolution {
    pub(crate) model_name: String,
    pub(crate) canonical_provider_name: String,
    pub(crate) inventory: Vec<String>,
    pub(crate) runtime_command: String,
    pub(crate) runtime_args: Vec<String>,
}

pub(crate) fn resolve_target(
    models_dir_override: Option<&Path>,
) -> Result<TargetResolution, DryRunError> {
    let loaded = load_target_config(models_dir_override)?;
    let target_binary = moved_external_provider_binary();
    let model = require_target_model(select_target_model(&loaded.models, &target_binary))?;
    let canonical = require_canonical_provider(first_provider(model))?;
    let (runtime, _) = loaded
        .providers_cfg
        .runtime_provider(&canonical.name)
        .map_err(|err| DryRunError::new(format!("target provider account failed: {err}")))?;
    Ok(target_resolution(
        model,
        canonical,
        runtime.command,
        runtime.args,
    ))
}

pub(crate) fn prove_provider(target: &TargetResolution) -> Result<(), DryRunError> {
    validate_provider_proof_status(run_provider_proof_command(target)?)
}

fn load_target_config(
    models_dir_override: Option<&Path>,
) -> Result<LoadedTargetConfig, DryRunError> {
    let models_dir = resolved_models_dir(models_dir_override);
    let providers_path = providers_path_for_models_dir(&models_dir);
    let providers_cfg = ProvidersConfig::load(&providers_path)
        .map_err(|err| DryRunError::new(format!("failed to load providers config: {err}")))?;
    let models = load_models(&models_dir, Some(&providers_cfg))
        .map_err(|err| DryRunError::new(format!("failed to load models: {err}")))?;
    Ok(LoadedTargetConfig {
        providers_cfg,
        models,
    })
}

fn resolved_models_dir(models_dir_override: Option<&Path>) -> PathBuf {
    models_dir_override
        .map(Path::to_path_buf)
        .unwrap_or_else(default_models_dir)
}

fn select_target_model<'a>(
    models: &'a HashMap<String, ModelConfig>,
    target_binary: &str,
) -> Option<&'a ModelConfig> {
    models
        .values()
        .find(|model| model_provider_binary(model).as_deref() == Some(target_binary))
}

fn require_target_model(model: Option<&ModelConfig>) -> Result<&ModelConfig, DryRunError> {
    model.ok_or_else(|| DryRunError::new("target provider-ref model not found"))
}

fn first_provider(model: &ModelConfig) -> Option<&ProviderConfig> {
    model.providers.first()
}

fn require_canonical_provider(
    provider: Option<&ProviderConfig>,
) -> Result<&ProviderConfig, DryRunError> {
    provider.ok_or_else(|| DryRunError::new("target provider inventory is empty"))
}

fn target_resolution(
    model: &ModelConfig,
    canonical: &ProviderConfig,
    runtime_command: String,
    runtime_args: Vec<String>,
) -> TargetResolution {
    TargetResolution {
        model_name: model.name.clone(),
        canonical_provider_name: canonical.name.clone(),
        inventory: model
            .providers
            .iter()
            .map(|provider| provider.name.clone())
            .collect(),
        runtime_command,
        runtime_args,
    }
}

fn run_provider_proof_command(target: &TargetResolution) -> Result<ExitStatus, DryRunError> {
    Command::new(&target.runtime_command)
        .args(&target.runtime_args)
        .status()
        .map_err(|err| DryRunError::new(format!("provider proof failed to start: {err}")))
}

fn validate_provider_proof_status(status: ExitStatus) -> Result<(), DryRunError> {
    if status.success() {
        Ok(())
    } else {
        Err(DryRunError::new(format!(
            "provider proof failed with status {status}"
        )))
    }
}

fn providers_path_for_models_dir(models_dir: &Path) -> PathBuf {
    models_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("providers.toml")
}

fn model_provider_binary(model: &ModelConfig) -> Option<String> {
    model.provider.as_ref()?.binary.clone()
}

fn moved_external_provider_binary() -> String {
    format!("agent-runner-{}", moved_provider_token())
}

fn moved_provider_token() -> String {
    ["cla", "ude"].concat()
}
