//! ## Declared roles
//! orchestration, validator, accessor, mapper, formatter, parser, predicate

use super::mutation_mapper::{
    compute_chain_segment_mutations, map_chain_segment_snapshot, map_migrated_segment,
};
use super::plan_validation::validate_mutation_inputs;
use super::state_access;
use super::types::{ChainSegmentMutations, ChainSegmentSnapshot};
use super::{error_formatter, host_apply_conflict, semantic_host_plan_rejection};
use crate::migration::MigratedSegment;
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;
use oulipoly_provider::generated::RotationMaterializeResult;

pub(super) fn load_chain_segment_snapshot(
    request: &MigrationServiceRequest<'_>,
) -> Result<ChainSegmentSnapshot, ExternalRotationError> {
    if request.resolved.chain_id.is_empty() {
        return Err(semantic_host_plan_rejection("chain_id is empty"));
    }
    state_access::active_chain_segment_snapshot(request)?
        .map(map_chain_segment_snapshot)
        .ok_or_else(|| semantic_host_plan_rejection("active chain segment snapshot is missing"))
}

pub(super) fn compute_validated_chain_segment_mutations(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<ChainSegmentMutations, ExternalRotationError> {
    let plan = result.host_state_plan.as_object().ok_or_else(|| {
        semantic_host_plan_rejection("host_state_plan must be an object before host apply")
    })?;
    let inputs = validate_mutation_inputs(plan, request, identity, result)?;
    Ok(compute_chain_segment_mutations(inputs))
}

pub(super) fn apply_chain_segment_transaction(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<MigratedSegment, ExternalRotationError> {
    let mutations = compute_validated_chain_segment_mutations(request, identity, result)?;
    state_access::rotate_chain_segment_transactionally(
        request,
        identity,
        &mutations,
        &mutations.changed_at,
    )?;
    Ok(map_migrated_segment(request, identity, mutations))
}

pub(super) fn rotation_already_applied(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    result: &RotationMaterializeResult,
) -> Result<Option<MigratedSegment>, ExternalRotationError> {
    let mutations = compute_validated_chain_segment_mutations(request, identity, result)?;
    let active =
        state_access::active_segment_id_for_chain_provider_session(request, identity, &mutations)?;
    Ok(active.map(|_| map_migrated_segment(request, identity, mutations)))
}

pub(super) fn ensure_no_conflicting_active_segment(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    target_session_id: &str,
) -> Result<(), ExternalRotationError> {
    if let Some(conflict) =
        state_access::find_conflicting_active_segment(request, identity, target_session_id)?
    {
        return Err(host_apply_conflict(
            error_formatter::target_active_conflict_message(&conflict),
        ));
    }
    Ok(())
}
