//! ## Declared roles
//! orchestration, predicate, accessor, formatter

use super::error_formatter;
use super::external_identity_accessor;
use crate::provider_registry::ProviderRegistryHandle;
use crate::rotation_domain::ExternalRotationIdentity;
use crate::services::{MigrationServiceRequest, ServiceError};

pub(super) enum MigrationBranch {
    BuiltIn,
    External { identity: ExternalRotationIdentity },
}

pub(super) fn model_declares_external_provider(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> bool {
    request.migration_model.provider.is_some()
        || (request.manual_target.is_some()
            && provider_registry.is_some_and(|handle| {
                handle
                    .current()
                    .has_account_endpoint(&request.resolved.active_provider)
            }))
}

fn account_owns_rotation(
    request: &MigrationServiceRequest<'_>,
    handle: &ProviderRegistryHandle,
) -> Result<bool, ServiceError> {
    let registry = handle.current();
    let endpoint = registry
        .preflight_account(&request.resolved.active_provider)
        .map_err(|error| ServiceError::Dependency {
            message: error.to_string(),
        })?;
    // Legacy account endpoints can provide session reads without owning rotation.
    // Once rotation is advertised, failures remain on the external path.
    Ok(endpoint.capabilities().capabilities.rotation)
}

pub(super) fn external_rotation_selected(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<bool, ServiceError> {
    if !model_declares_external_provider(request, provider_registry) {
        return Ok(false);
    }
    if request.migration_model.provider.is_none()
        && !account_owns_rotation(
            request,
            provider_registry.expect("account endpoint requires registry"),
        )?
    {
        return Ok(false);
    }
    Ok(true)
}

pub(super) fn select_migration_branch(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationBranch, ServiceError> {
    if !external_rotation_selected(request, provider_registry)? {
        return Ok(MigrationBranch::BuiltIn);
    }
    match external_identity_accessor::resolve_external_provider_identity(request, provider_registry)
    {
        Ok(identity) => Ok(MigrationBranch::External { identity }),
        Err(error) => Err(error_formatter::construct_migration_service_error(error)),
    }
}
