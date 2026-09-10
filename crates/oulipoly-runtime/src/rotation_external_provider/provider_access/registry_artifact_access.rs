//! ## Declared roles
//! accessor

use super::registry_error_mapper;
use crate::provider_registry::ProviderRegistry;
use oulipoly_config::ModelConfig;

use super::super::ExternalRotationError;
use crate::provider_registry::PinnedProviderEndpoint;
use std::sync::Arc;

pub(super) fn preflight_external_model_provider(
    registry: &ProviderRegistry,
    _model: &ModelConfig,
    target_provider: &str,
) -> Result<Arc<PinnedProviderEndpoint>, ExternalRotationError> {
    registry
        .preflight_account(target_provider)
        .map_err(registry_error_mapper::map_registry_identity_error)
}
