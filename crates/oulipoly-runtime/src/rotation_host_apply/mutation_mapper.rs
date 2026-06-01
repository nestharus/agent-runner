//! ## Declared roles
//! mapper

use super::types::{ChainSegmentMutations, ChainSegmentSnapshot, ValidatedMutationInputs};
use crate::migration::MigratedSegment;
use crate::rotation_domain::ExternalRotationIdentity;
use crate::services::MigrationServiceRequest;

pub(super) fn compute_chain_segment_mutations(
    inputs: ValidatedMutationInputs,
) -> ChainSegmentMutations {
    ChainSegmentMutations {
        target_provider_index: inputs.target_provider_index,
        target_session_id: inputs.target_session_id,
        target_jsonl_path: inputs.target_jsonl_path,
        reason: inputs.reason,
        changed_at: inputs.changed_at,
    }
}

pub(super) fn map_chain_segment_snapshot(
    snapshot: oulipoly_state::ActiveChainSegmentSnapshot,
) -> ChainSegmentSnapshot {
    ChainSegmentSnapshot {
        chain_id: snapshot.chain_id,
        active_provider: snapshot.active_provider,
        active_session_id: snapshot.active_session_id,
        active_started_at: snapshot.active_started_at,
        active_ended_at: snapshot.active_ended_at,
        active_last_turn_id: snapshot.active_last_turn_id,
        latest_turn_at: snapshot.latest_turn_at,
    }
}

pub(super) fn map_migrated_segment(
    request: &MigrationServiceRequest<'_>,
    identity: &ExternalRotationIdentity,
    mutations: ChainSegmentMutations,
) -> MigratedSegment {
    MigratedSegment {
        chain_id: request.resolved.chain_id.clone(),
        source_provider: identity.source_provider.clone(),
        source_session_id: identity.source_session_id.clone(),
        target_provider: identity.target_provider.clone(),
        target_provider_index: mutations.target_provider_index,
        target_session_id: mutations.target_session_id,
        target_jsonl_path: mutations.target_jsonl_path,
        reason: mutations.reason,
    }
}
