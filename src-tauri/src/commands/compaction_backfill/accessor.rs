//! Declared role: accessor

use oulipoly_state::{CompactSummaryEvidence, StateDb};

pub(super) fn segments(state: &StateDb) -> Result<Vec<(String, String)>, String> {
    state.distinct_chain_segments()
}

pub(super) fn backfill_session(
    state: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> Result<u64, String> {
    let evidence = state
        .compact_summary_evidence(session_id)
        .map_err(|e| e.to_string())?;
    flag_compaction_boundaries_from_evidence(state, provider_name, &evidence)
}

fn flag_compaction_boundaries_from_evidence(
    state: &StateDb,
    provider_name: &str,
    evidence: &CompactSummaryEvidence,
) -> Result<u64, String> {
    let mut flagged = 0u64;
    for turn_uuid in &evidence.compact_turn_uuids {
        if state.flag_compaction_boundary(provider_name, &evidence.session_id, turn_uuid)? {
            flagged += 1;
        }
    }
    Ok(flagged)
}
