//! ## Declared roles
//! mapper

use super::types::RotationJournalPreimage;

pub(super) fn map_rotation_preimage(
    snapshot: crate::rotation_host_apply::ChainSegmentSnapshot,
) -> RotationJournalPreimage {
    RotationJournalPreimage {
        chain_id: snapshot.chain_id,
        active_provider: snapshot.active_provider,
        active_session_id: snapshot.active_session_id,
        active_started_at: snapshot.active_started_at,
        active_ended_at: snapshot.active_ended_at,
        active_last_turn_id: snapshot.active_last_turn_id,
        latest_turn_at: snapshot.latest_turn_at,
    }
}
