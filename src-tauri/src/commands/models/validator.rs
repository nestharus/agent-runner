//! ## Declared roles
//!
//! `validator`
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: src-tauri/src/commands/models/validator.rs
//!     role: adapter
//!     Translates:
//!       - Tauri-side model prevalidation contract
//!       - provider-name emptiness contract
//!       - config-layer provider-aware validation handoff contract
//! ```

use oulipoly_config::ModelConfig;

pub(crate) enum ModelValidationError {
    EmptyName,
    NoProviders,
    EmptyProviderName(usize),
}

pub(crate) fn validate_model_for_save(model: &ModelConfig) -> Result<(), ModelValidationError> {
    if model.name.is_empty() {
        return Err(ModelValidationError::EmptyName);
    }
    if model.providers.is_empty() {
        return Err(ModelValidationError::NoProviders);
    }
    validate_provider_names(model)
}

pub(crate) fn validate_provider_names(model: &ModelConfig) -> Result<(), ModelValidationError> {
    for (i, p) in model.providers.iter().enumerate() {
        validate_provider_name(i + 1, &p.name)?;
    }
    Ok(())
}

pub(crate) fn validate_provider_name(
    position: usize,
    name: &str,
) -> Result<(), ModelValidationError> {
    if name.is_empty() {
        return Err(ModelValidationError::EmptyProviderName(position));
    }
    Ok(())
}
