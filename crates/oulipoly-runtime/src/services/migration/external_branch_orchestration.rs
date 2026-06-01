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

pub(super) fn model_declares_external_provider(request: &MigrationServiceRequest<'_>) -> bool {
    request.migration_model.provider.is_some()
}

pub(super) fn select_migration_branch(
    request: &MigrationServiceRequest<'_>,
    provider_registry: Option<&ProviderRegistryHandle>,
) -> Result<MigrationBranch, ServiceError> {
    if !model_declares_external_provider(request) {
        return Ok(MigrationBranch::BuiltIn);
    }
    match external_identity_accessor::resolve_external_provider_identity(request, provider_registry)
    {
        Ok(identity) => Ok(MigrationBranch::External { identity }),
        Err(error) => Err(error_formatter::construct_migration_service_error(error)),
    }
}
