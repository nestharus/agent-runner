//! ## Declared roles
//!
//! `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/pools/validator.rs
//!     role: adapter
//!     Translates:
//!       - pool command-set validation contract
//!       - matching-model existence contract
//!       - zero-provider prevention contract
//! ```

use oulipoly_config::ModelConfig;

pub(crate) enum PoolValidationError {
    EmptyCommands,
    NoMatchingModels,
    ZeroProviders(String),
}

pub(crate) fn validate_new_pool_commands(
    new_commands: &[String],
) -> Result<(), PoolValidationError> {
    if new_commands.is_empty() {
        return Err(PoolValidationError::EmptyCommands);
    }
    Ok(())
}

pub(crate) fn validate_matching_models_exist(
    matching_names: &[String],
) -> Result<(), PoolValidationError> {
    if matching_names.is_empty() {
        return Err(PoolValidationError::NoMatchingModels);
    }
    Ok(())
}

pub(crate) fn validate_model_keeps_provider(
    model: &ModelConfig,
) -> Result<(), PoolValidationError> {
    if model.providers.is_empty() {
        return Err(PoolValidationError::ZeroProviders(model.name.clone()));
    }
    Ok(())
}

pub(crate) fn validate_rewritten_pool_models(
    models: &[(String, ModelConfig)],
) -> Result<(), PoolValidationError> {
    for (_, model) in models {
        validate_model_keeps_provider(model)?;
    }
    Ok(())
}
