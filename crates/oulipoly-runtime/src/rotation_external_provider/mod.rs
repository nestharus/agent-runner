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
mod source_ingest;

use crate::provider_registry::ProviderRegistryHandle;
use crate::services::{MigrationServiceOutput, MigrationServiceRequest};
use oulipoly_provider::generated::{
    MigrationApplyResult, MigrationPlanResult, RotationAssessResult, RotationMaterializeResult,
};

pub use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
pub use provider_access::{
    ExternalRotationProviderOperation, resolve_rotation_external_provider_identity,
};

pub fn assess_rotation(
    _registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
) -> Result<RotationAssessResult, ExternalRotationError> {
    let endpoint = identity.endpoint("rotation.assess")?;
    let payload = request_mapper::rotation_request(
        &identity,
        request,
        &identity.host_options,
        "rotation.assess",
        identity.source_settings_id()?,
    )?;
    provider_dispatch::invoke_provider_contract(endpoint.client(), "rotation.assess", payload)
}

pub fn materialize_rotation(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationServiceOutput, ExternalRotationError> {
    let migration_fence = request
        .state
        .begin_completed_turn_migration(
            request.resolved,
            &identity.target_provider,
            None,
            oulipoly_state::CompletedTurnMigrationScope::ExternalProviderWide,
            oulipoly_state::CompletedTurnMigrationStage::ExternalBeforeProvider,
        )
        .map_err(crate::rotation_domain::host_apply_conflict)?;
    materialize_rotation_with_fence(registry_handle, identity, request, &migration_fence)
}

pub(crate) fn materialize_rotation_with_fence(
    registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
    migration_fence: &oulipoly_state::CompletedTurnMigrationFence,
) -> Result<MigrationServiceOutput, ExternalRotationError> {
    identity.source_settings_id()?;
    source_ingest::settle_source_ingestion(registry_handle, &identity, request)?;
    let result = invoke_rotation_materialize(&identity, request)?;
    if !result.changed {
        crate::rotation_host_apply::validate_no_change_host_state_plan(
            &crate::rotation_host_apply::host_state_plan_value(&result),
            &result.artifacts,
            request,
            &identity,
        )?;
        return Ok(MigrationServiceOutput::Stay);
    }
    crate::rotation_journal::publish_after_artifact_record(request, &identity, &result)?;
    crate::rotation_host_apply::verify_rotation_artifacts(&result.artifacts)
        .map_err(error_formatter::artifact_verification_failure)?;
    crate::rotation_host_apply::validate_host_state_plan_with_fence(
        &crate::rotation_host_apply::host_state_plan_value(&result),
        &result.artifacts,
        request,
        &identity,
        migration_fence,
    )?;
    crate::rotation_journal::publish_during_apply_record(request, &identity, &result)?;
    let segment =
        crate::rotation_host_apply::apply_chain_segment_transaction(request, &identity, &result)?;
    crate::rotation_journal::cleanup_rotation_journal(request.effective_cwd)?;
    Ok(MigrationServiceOutput::Migrated { segment })
}

pub fn plan_migration(
    _registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationPlanResult, ExternalRotationError> {
    let endpoint = identity.endpoint("migration.plan")?;
    provider_dispatch::invoke_provider_contract(
        endpoint.client(),
        "migration.plan",
        request_mapper::migration_request(
            &identity,
            request,
            &identity.host_options,
            "migration.plan",
        )?,
    )
}

pub fn apply_migration(
    _registry_handle: &ProviderRegistryHandle,
    identity: ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
) -> Result<MigrationApplyResult, ExternalRotationError> {
    let endpoint = identity.endpoint("migration.apply")?;
    provider_dispatch::invoke_provider_contract(
        endpoint.client(),
        "migration.apply",
        request_mapper::migration_request(
            &identity,
            request,
            &identity.host_options,
            "migration.apply",
        )?,
    )
}

fn invoke_rotation_materialize(
    identity: &ExternalRotationProviderOperation,
    request: &MigrationServiceRequest<'_>,
) -> Result<RotationMaterializeResult, ExternalRotationError> {
    let endpoint = identity.endpoint("rotation.materialize")?;
    let payload = request_mapper::rotation_request(
        identity,
        request,
        &identity.host_options,
        "rotation.materialize",
        identity.source_settings_id()?,
    )?;
    provider_dispatch::invoke_provider_contract(endpoint.client(), "rotation.materialize", payload)
}
