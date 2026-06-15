mod parse;
mod persist;

use super::types::{SessionProviderError, SessionProviderReadTurnsResult, SessionProviderTurn};
use oulipoly_provider::generated::SessionReadTurnsResult as ProviderReadTurnsResult;
use oulipoly_state::StateDb;
use std::collections::HashSet;

pub(super) fn map_read_turns_result(
    result: ProviderReadTurnsResult,
) -> Result<SessionProviderReadTurnsResult, SessionProviderError> {
    let turns = parse::provider_turns_from_values(result.turns)?;
    validate_unique_provider_turns(&turns)?;
    Ok(session_provider_read_turns_result(
        turns,
        result.turn_count,
        result.complete,
    ))
}

fn session_provider_read_turns_result(
    turns: Vec<SessionProviderTurn>,
    turn_count: u64,
    complete: bool,
) -> SessionProviderReadTurnsResult {
    SessionProviderReadTurnsResult {
        turns,
        turn_count,
        complete,
    }
}

fn validate_unique_provider_turns(
    turns: &[SessionProviderTurn],
) -> Result<(), SessionProviderError> {
    let mut seen = HashSet::new();
    for turn in turns {
        if !seen.insert((turn.session_id.clone(), turn.turn_id.clone())) {
            return Err(SessionProviderError::new(
                "provider_turn_duplicate",
                "provider returned duplicate turn id for a session",
            ));
        }
    }
    Ok(())
}

pub fn ingest_owned_turns(
    db: &StateDb,
    provider_name: &str,
    result: &SessionProviderReadTurnsResult,
) -> Result<u64, SessionProviderError> {
    let batch = persist::provider_turns_to_ingest(&result.turns)?;
    let inserted = persist::persist_owned_turn_batch(db, provider_name, &batch)?;
    persist::mint_imported_chains(db, provider_name, &batch)?;
    Ok(inserted)
}

pub fn assert_turn_count_diagnostic(
    result: &SessionProviderReadTurnsResult,
) -> Result<(), SessionProviderError> {
    if result.turn_count == result.turns.len() as u64 {
        return Err(SessionProviderError::new(
            "provider_turn_count_matches",
            "turn_count matched accepted turn length",
        ));
    }
    Ok(())
}
