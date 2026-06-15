use super::super::types::{SessionProviderError, SessionProviderTurn};
use oulipoly_state::{SessionTurnIngest, StateDb};
use serde_json::Value;

pub(super) fn provider_turns_to_ingest(
    turns: &[SessionProviderTurn],
) -> Result<Vec<SessionTurnIngest>, SessionProviderError> {
    turns.iter().map(provider_turn_to_ingest).collect()
}

pub(super) fn persist_owned_turn_batch(
    db: &StateDb,
    provider_name: &str,
    batch: &[SessionTurnIngest],
) -> Result<u64, SessionProviderError> {
    db.ingest_session_turns_batch(provider_name, batch)
        .map_err(provider_turn_ingest_failed)
}

fn provider_turn_ingest_failed(error: String) -> SessionProviderError {
    SessionProviderError::new("provider_turn_ingest_failed", error)
}

fn provider_turn_to_ingest(
    turn: &SessionProviderTurn,
) -> Result<SessionTurnIngest, SessionProviderError> {
    Ok(SessionTurnIngest {
        session_id: turn.session_id.clone(),
        turn_id: turn.turn_id.clone(),
        timestamp: turn.timestamp,
        role: turn.role.clone(),
        parent_turn_id: turn.parent_turn_id.clone(),
        is_sidechain: turn.is_sidechain,
        is_compaction_boundary: turn.is_compaction_boundary,
        body: serialize_optional_body(turn.body.as_ref())?,
    })
}

fn serialize_optional_body(body: Option<&Value>) -> Result<Option<String>, SessionProviderError> {
    body.map(serde_json::to_string).transpose().map_err(|err| {
        SessionProviderError::new("provider_turn_body_serialize_failed", err.to_string())
    })
}

pub(super) fn mint_imported_chains(
    db: &StateDb,
    provider_name: &str,
    batch: &[SessionTurnIngest],
) -> Result<(), SessionProviderError> {
    for turn in batch {
        db.mint_imported_chain_if_absent(
            provider_name,
            &turn.session_id,
            &turn.timestamp,
            "<unknown>",
        )
        .map_err(provider_turn_chain_mint_failed)?;
    }
    Ok(())
}

fn provider_turn_chain_mint_failed(error: String) -> SessionProviderError {
    SessionProviderError::new("provider_turn_chain_mint_failed", error)
}
