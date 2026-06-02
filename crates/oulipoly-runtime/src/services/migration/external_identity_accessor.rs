//! ## Declared roles
//! accessor, validator, formatter, orchestration

use super::external_target_validator;
use crate::provider_registry::ProviderRegistryHandle;
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;

pub(super) fn resolve_external_provider_identity(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<ExternalRotationIdentity, ExternalRotationError> {
    let target_provider =
        external_target_validator::select_external_rotation_target_provider(request)?;
    let registry = provider_registry
        .ok_or_else(crate::rotation_external_provider::error_formatter::missing_registry_handle)?;
    let registry = registry.current();
    let identity = crate::rotation_external_provider::resolve_rotation_external_provider_identity(
        registry.as_ref(),
        request.migration_model,
        request.resolved,
        &target_provider,
    )?;
    Ok(identity)
}
