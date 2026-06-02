//! ## Declared roles
//! accessor, formatter, mapper, predicate

use super::types::ChainSegmentMutations;
use super::{error_formatter, host_apply_conflict, semantic_host_plan_rejection};
use crate::rotation_domain::{ExternalRotationError, ExternalRotationIdentity};
use crate::services::MigrationServiceRequest;

pub(super) fn active_chain_segment_snapshot(
    request: &MigrationServiceRequest<'_>,
) -> Result<Option<oulipoly_state::ActiveChainSegmentSnapshot>, ExternalRotationError> {
    request
        .state
        .active_chain_segment_snapshot(&request.resolved.chain_id)
        .map_err(|error| semantic_host_plan_rejection(error_formatter::snapshot_read_error(error)))
}

pub(super) fn rotate_chain_segment_transactionally(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    mutations: &ChainSegmentMutations,
    now: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ExternalRotationError> {
    request
        .state
        .rotate_chain_segment_transactionally(oulipoly_state::ChainSegmentRotationInput {
            chain_id: &request.resolved.chain_id,
            source_provider_name: &identity.source_provider,
            source_session_id: &identity.source_session_id,
            target_provider_name: &identity.target_provider,
            target_session_id: &mutations.target_session_id,
            changed_at: now,
            reason: mutations.reason,
        })
        .map(|_| ())
        .map_err(host_apply_conflict)
}

pub(super) fn active_segment_id_for_chain_provider_session(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    mutations: &ChainSegmentMutations,
) -> Result<Option<i64>, ExternalRotationError> {
    request
        .state
        .active_segment_id_for_chain_provider_session(
            &request.resolved.chain_id,
            &identity.target_provider,
            &mutations.target_session_id,
        )
        .map_err(host_apply_conflict)
}

pub(super) fn find_conflicting_active_segment(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    target_session_id: &str,
) -> Result<Option<String>, ExternalRotationError> {
    request
        .state
        .find_conflicting_active_segment(
            &identity.target_provider,
            target_session_id,
            &request.resolved.chain_id,
        )
        .map_err(host_apply_conflict)
}

pub(super) fn close_active_segment_returning(
    request: &MigrationServiceRequest<'_>,
    ended_at: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ExternalRotationError> {
    request
        .state
        .close_active_segment_returning(&request.resolved.chain_id, ended_at)
        .map_err(host_apply_conflict)?
        .ok_or_else(|| host_apply_conflict("active segment was already closed"))?;
    Ok(())
}

pub(super) fn open_chain_segment(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    mutations: &ChainSegmentMutations,
    started_at: &chrono::DateTime<chrono::Utc>,
) -> Result<(), ExternalRotationError> {
    request
        .state
        .open_chain_segment(
            &request.resolved.chain_id,
            &identity.target_provider,
            &mutations.target_session_id,
            started_at,
            mutations.reason,
        )
        .map_err(host_apply_conflict)?;
    Ok(())
}
