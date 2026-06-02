//! ## Declared roles
//! orchestration, validator, accessor, predicate, mapper

mod capability_predicates;
mod identity_mapper;
mod identity_validation;
mod registry_artifact_access;
mod registry_error_mapper;

use super::{ExternalRotationError, ExternalRotationIdentity};
use crate::provider_registry::{ProviderRegistry, ProviderRegistryHandle};
use oulipoly_config::ModelConfig;
use oulipoly_provider::client::ProviderClient;
use oulipoly_state::ResolvedResume;

pub fn resolve_rotation_external_provider_identity(
    registry: &ProviderRegistry,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider: &str,
) -> Result<ExternalRotationIdentity, ExternalRotationError> {
    identity_validation::validate_external_model_identity(model, resolved, target_provider)?;
    let describe = registry_artifact_access::describe_external_model_provider(registry, model)?;
    Ok(identity_mapper::map_external_rotation_identity(
        model,
        resolved,
        target_provider,
        describe,
    ))
}

pub(super) fn load_provider_artifact_and_capabilities(
    registry_handle: &ProviderRegistryHandle,
    model_name: &str,
    operation: &'static str,
) -> Result<ProviderClient, ExternalRotationError> {
    let registry = registry_handle.current();
    let registry = registry.as_ref();
    let describe = registry_artifact_access::describe_external_model_provider_for_dispatch(
        registry, model_name,
    )?;
    capability_predicates::supports_rotation_or_migration(&describe, operation)?;
    let artifact = registry_artifact_access::enabled_artifact_for_model(registry, model_name)?;
    Ok(registry.client_factory().client_for(artifact))
}
