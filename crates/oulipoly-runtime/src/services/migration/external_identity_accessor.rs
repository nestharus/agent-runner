//! ## Declared roles
//! accessor, validator, formatter, orchestration

use super::external_target_validator;
use crate::provider_registry::ProviderRegistryHandle;
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;

pub(super) fn select_external_target_provider(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<String, ExternalRotationError> {
    let registry = provider_registry
        .ok_or_else(crate::rotation_external_provider::error_formatter::missing_registry_handle)?;
    external_target_validator::select_external_rotation_target_provider(
        request,
        registry.current().as_ref(),
    )
}

pub(super) fn resolve_external_provider_identity_for_target(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
    target_provider: &str,
) -> Result<ExternalRotationIdentity, ExternalRotationError> {
    let registry = provider_registry
        .ok_or_else(crate::rotation_external_provider::error_formatter::missing_registry_handle)?;
    crate::rotation_external_provider::resolve_rotation_external_provider_identity(
        registry.current().as_ref(),
        request.migration_model,
        request.resolved,
        target_provider,
    )
}

pub(super) fn resolve_external_provider_identity(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<ExternalRotationIdentity, ExternalRotationError> {
    let target_provider = select_external_target_provider(request, provider_registry)?;
    resolve_external_provider_identity_for_target(request, provider_registry, &target_provider)
}
