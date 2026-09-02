//! ## Declared roles
//! validator

use super::super::{ExternalRotationError, error_formatter};
use oulipoly_config::ModelConfig;
use oulipoly_state::ResolvedResume;

pub(super) fn validate_external_model_identity(
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider: &str,
) -> Result<(), ExternalRotationError> {
    if target_provider == resolved.active_provider {
        return Err(error_formatter::malformed_external_identity(
            "external rotation target matches active provider",
        ));
    }
    if !model
        .providers
        .iter()
        .any(|provider| provider.name == target_provider)
    {
        return Err(error_formatter::malformed_external_identity(
            "external rotation target is not in model pool",
        ));
    }
    Ok(())
}
