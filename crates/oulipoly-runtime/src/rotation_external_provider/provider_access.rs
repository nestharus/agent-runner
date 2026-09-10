//! ## Declared roles
//! orchestration, validator, accessor, predicate, mapper

mod capability_predicates;
mod identity_mapper;
mod identity_validation;
mod registry_artifact_access;
mod registry_error_mapper;

use super::{ExternalRotationError, ExternalRotationIdentity};
use crate::provider_registry::{PinnedProviderEndpoint, ProviderRegistry, ProviderRegistryHandle};
use oulipoly_config::ModelConfig;
use oulipoly_state::ResolvedResume;
use std::sync::Arc;

pub fn resolve_rotation_external_provider_identity(
    registry: &ProviderRegistry,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider: &str,
) -> Result<ExternalRotationIdentity, ExternalRotationError> {
    identity_validation::validate_external_model_identity(model, resolved, target_provider)?;
    let endpoint = registry_artifact_access::preflight_external_model_provider(
        registry,
        model,
        target_provider,
    )?;
    let settings_id = endpoint
        .settings_id()
        .map_err(registry_error_mapper::map_registry_identity_error)?;
    Ok(identity_mapper::map_external_rotation_identity(
        model,
        resolved,
        target_provider,
        endpoint.capabilities().clone(),
        settings_id,
    ))
}

pub(super) fn load_provider_artifact_and_capabilities(
    registry_handle: &ProviderRegistryHandle,
    account_name: &str,
    operation: &'static str,
) -> Result<Arc<PinnedProviderEndpoint>, ExternalRotationError> {
    let registry = registry_handle.current();
    let registry = registry.as_ref();
    let endpoint = registry
        .preflight_account(account_name)
        .map_err(registry_error_mapper::map_registry_dispatch_error)?;
    capability_predicates::supports_rotation_or_migration(endpoint.capabilities(), operation)?;
    Ok(endpoint)
}
