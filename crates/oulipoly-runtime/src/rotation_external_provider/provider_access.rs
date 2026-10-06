//! ## Declared roles
//! orchestration, validator, accessor, predicate, mapper

mod capability_predicates;
mod identity_mapper;
mod identity_validation;
mod registry_artifact_access;
mod registry_error_mapper;

use super::{ExternalRotationError, ExternalRotationIdentity};
use crate::provider_registry::{DescribeHostOptions, PinnedProviderEndpoint, ProviderRegistry};
use oulipoly_config::ModelConfig;
use oulipoly_state::ResolvedResume;
use std::sync::Arc;

/// One acquisition shared by identity discovery and all steps of a rotation / migration.
/// It is neither serialized into recovery journals nor retained across operations.
#[derive(Debug, Clone)]
pub struct ExternalRotationProviderOperation {
    identity: Box<ExternalRotationIdentity>,
    endpoint: Arc<PinnedProviderEndpoint>,
    pub(super) host_options: DescribeHostOptions,
    pub(super) source_settings_id: String,
}

impl std::ops::Deref for ExternalRotationProviderOperation {
    type Target = ExternalRotationIdentity;
    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}

impl ExternalRotationProviderOperation {
    pub(super) fn endpoint(
        &self,
        operation: &'static str,
    ) -> Result<&PinnedProviderEndpoint, ExternalRotationError> {
        capability_predicates::supports_rotation_or_migration(
            self.endpoint.capabilities(),
            operation,
        )?;
        Ok(self.endpoint.as_ref())
    }
}

pub fn resolve_rotation_external_provider_identity(
    registry: &ProviderRegistry,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    target_provider: &str,
) -> Result<ExternalRotationProviderOperation, ExternalRotationError> {
    identity_validation::validate_external_model_identity(model, resolved, target_provider)?;
    let endpoint = registry_artifact_access::preflight_external_model_provider(
        registry,
        model,
        target_provider,
    )?;
    let settings_id = endpoint
        .settings_id()
        .map_err(registry_error_mapper::map_registry_identity_error)?;
    let identity = identity_mapper::map_external_rotation_identity(
        model,
        resolved,
        target_provider,
        endpoint.capabilities().clone(),
        settings_id,
    );
    Ok(ExternalRotationProviderOperation {
        identity: Box::new(identity),
        endpoint,
        host_options: registry.host_options().clone(),
        source_settings_id: registry
            .account_settings_id(&resolved.active_provider)
            .map_err(registry_error_mapper::map_registry_identity_error)?
            .to_string(),
    })
}
