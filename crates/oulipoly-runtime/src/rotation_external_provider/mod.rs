//! ## Declared roles
//! orchestration, mapper, formatter
//!
//! ## Adapter declarations
//! adapter_declarations:
//!   - component: s7c-provider-identity-registry-adapter
//!     role: adapter
//!   - component: s7c-provider-subprocess-contract-adapter
//!     role: adapter

#![allow(dead_code)]

pub mod error_formatter;
mod provider_access;
mod provider_dispatch;
mod request_mapper;

use crate::provider_registry::ProviderRegistryHandle;
use crate::services::{MigrationServiceOutput, MigrationServiceRequest};
use oulipoly_provider::generated::{
    MigrationApplyResult, MigrationPlanResult, RotationAssessResult, RotationMaterializeResult,
};

pub use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
pub use provider_access::resolve_rotation_external_provider_identity;

pub fn assess_rotation(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<RotationAssessResult, ExternalRotationError> {
    let registry = registry_handle.current();
    let client = provider_access::load_provider_artifact_and_capabilities(
        registry_handle,
        &identity.model_name,
        "rotation.assess",
    )?;
    let payload = request_mapper::rotation_request(
        &identity,
        request,
        registry.host_options(),
        "rotation.assess",
    )?;
    provider_dispatch::invoke_provider_contract(&client, "rotation.assess", payload)
}

pub fn materialize_rotation(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationServiceOutput, ExternalRotationError> {
    let result = invoke_rotation_materialize(registry_handle, &identity, request)?;
    if !result.changed {
        crate::rotation_host_apply::validate_no_change_host_state_plan(
            &result.host_state_plan,
            &result.artifacts,
            request,
            &identity,
        )?;
        return Ok(MigrationServiceOutput::Stay);
    }
    crate::rotation_journal::publish_after_artifact_record(request, &identity, &result)?;
    crate::rotation_host_apply::verify_rotation_artifacts(&result.artifacts)
        .map_err(error_formatter::artifact_verification_failure)?;
    crate::rotation_host_apply::validate_host_state_plan(
        &result.host_state_plan,
        &result.artifacts,
        request,
        &identity,
    )?;
    crate::rotation_journal::publish_during_apply_record(request, &identity, &result)?;
    let segment =
        crate::rotation_host_apply::apply_chain_segment_transaction(request, &identity, &result)?;
    crate::rotation_journal::cleanup_rotation_journal(request.effective_cwd)?;
    Ok(MigrationServiceOutput::Migrated { segment })
}

pub fn plan_migration(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationPlanResult, ExternalRotationError> {
    let registry = registry_handle.current();
    let client = provider_access::load_provider_artifact_and_capabilities(
        registry_handle,
        &identity.model_name,
        "migration.plan",
    )?;
    provider_dispatch::invoke_provider_contract(
        &client,
        "migration.plan",
        request_mapper::migration_request(
            &identity,
            request,
            registry.host_options(),
            "migration.plan",
        )?,
    )
}

pub fn apply_migration(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationApplyResult, ExternalRotationError> {
    let registry = registry_handle.current();
    let client = provider_access::load_provider_artifact_and_capabilities(
        registry_handle,
        &identity.model_name,
        "migration.apply",
    )?;
    provider_dispatch::invoke_provider_contract(
        &client,
        "migration.apply",
        request_mapper::migration_request(
            &identity,
            request,
            registry.host_options(),
            "migration.apply",
        )?,
    )
}

fn invoke_rotation_materialize(
    registry_handle: &ProviderRegistryHandle,
    identity: &ExternalRotationIdentity,
    request: &MigrationServiceRequest<'_>,
) -> Result<RotationMaterializeResult, ExternalRotationError> {
    let registry = registry_handle.current();
    let client = provider_access::load_provider_artifact_and_capabilities(
        registry_handle,
        &identity.model_name,
        "rotation.materialize",
    )?;
    let payload = request_mapper::rotation_request(
        identity,
        request,
        registry.host_options(),
        "rotation.materialize",
    )?;
    provider_dispatch::invoke_provider_contract(&client, "rotation.materialize", payload)
}
