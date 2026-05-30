//! ## Declared roles
//!
//! `accessor`, `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/writer.rs
//!     role: adapter
//!     Translates:
//!       - provider-aware pool TOML rendering contract
//!       - pool model file write contract
//!       - per-model write error-string contract
//! ```

use super::validator::PoolValidationError;
use oulipoly_config::{self as config, ModelConfig, ProvidersConfig};
use std::path::Path;

const EMPTY_COMMANDS_ERROR: &str = "Pool must have at least one command";
const NO_MATCHING_MODELS_ERROR: &str = "No models found with the specified command set";

pub(crate) fn render_pool_model_update(
    model: &ModelConfig,
    providers: &ProvidersConfig,
) -> Result<String, String> {
    Ok(config::render_validated_model_toml(model, Some(providers))?)
}

pub(crate) fn render_pool_model_updates(
    rewritten: &[(String, ModelConfig)],
    providers: &ProvidersConfig,
) -> Result<Vec<(String, ModelConfig, String)>, String> {
    let mut updates = Vec::new();
    for (name, model) in rewritten {
        let toml_content = render_pool_model_update(model, providers)?;
        updates.push((name.clone(), model.clone(), toml_content));
    }
    Ok(updates)
}

pub(crate) fn write_pool_model_update(
    models_dir: &Path,
    name: &str,
    toml_content: &str,
) -> Result<(), String> {
    let path = models_dir.join(format!("{name}.toml"));
    std::fs::write(&path, toml_content).map_err(|e| pool_write_error(name, e))
}

pub(crate) fn format_pool_validation_error(error: PoolValidationError) -> String {
    match error {
        PoolValidationError::EmptyCommands => EMPTY_COMMANDS_ERROR.to_string(),
        PoolValidationError::NoMatchingModels => NO_MATCHING_MODELS_ERROR.to_string(),
        PoolValidationError::ZeroProviders(name) => zero_providers_error(&name),
    }
}

pub(crate) fn zero_providers_error(name: &str) -> String {
    format!("Model '{}' would end up with zero providers", name)
}

pub(crate) fn pool_write_error(name: &str, error: std::io::Error) -> String {
    format!("Failed to write model file for '{}': {error}", name)
}
