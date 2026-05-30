//! ## Declared roles
//!
//! `formatter`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/formatter.rs
//!     role: adapter
//!     Translates:
//!       - model file path formatting contract
//!       - provider-aware model TOML rendering contract
//!       - model command validation error-string contract
//!       - model command IO error-string contract
//! ```

use super::accessor::ModelPersistenceError;
use super::validator::ModelValidationError;
use crate::AppState;
use crate::app_paths::load_providers_for_models_dir_with;
use oulipoly_config::{self as config, ModelConfig};
use std::path::{Path, PathBuf};

const EMPTY_MODEL_NAME_ERROR: &str = "Model name cannot be empty";
const NO_PROVIDERS_ERROR: &str = "Model must have at least one provider";

pub(crate) fn render_model_for_write(
    state: &AppState,
    model: &ModelConfig,
) -> Result<String, String> {
    let providers = load_providers_for_models_dir_with(&state.models_dir, &*state.providers_config);
    Ok(config::render_validated_model_toml(
        model,
        Some(&providers),
    )?)
}

pub(crate) fn model_file_path(models_dir: &Path, name: &str) -> PathBuf {
    models_dir.join(format!("{name}.toml"))
}

pub(crate) fn model_not_found_error(name: &str) -> String {
    format!("Model '{}' not found", name)
}

pub(crate) fn format_model_validation_error(error: ModelValidationError) -> String {
    match error {
        ModelValidationError::EmptyName => EMPTY_MODEL_NAME_ERROR.to_string(),
        ModelValidationError::NoProviders => NO_PROVIDERS_ERROR.to_string(),
        ModelValidationError::EmptyProviderName(position) => empty_provider_name_error(position),
    }
}

pub(crate) fn empty_provider_name_error(position: usize) -> String {
    format!("Provider {position} has empty name")
}

pub(crate) fn format_model_persistence_error(error: ModelPersistenceError) -> String {
    match error {
        ModelPersistenceError::CreateDir(error) => create_models_dir_error(error),
        ModelPersistenceError::WriteFile(error) => write_model_file_error(error),
    }
}

pub(crate) fn create_models_dir_error(error: std::io::Error) -> String {
    format!("Failed to create models directory: {error}")
}

pub(crate) fn write_model_file_error(error: std::io::Error) -> String {
    format!("Failed to write model file: {error}")
}

pub(crate) fn delete_model_file_error(error: std::io::Error) -> String {
    format!("Failed to delete model file: {error}")
}
