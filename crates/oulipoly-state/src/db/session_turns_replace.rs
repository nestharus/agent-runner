//! Typed session-turn replacement writes.
//!
//! ## Declared roles
//!
//! `orchestration`

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnReplacement {
    pub turn_id: String,
    pub timestamp: String,
    pub role: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnsReplacement {
    pub provider_name: String,
    pub session_id: String,
    pub chain_id: String,
    pub active_segment_id: i64,
    pub source_file: String,
    pub turns: Vec<SessionTurnReplacement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnRestoreRow {
    pub provider_name: String,
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: String,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: i64,
    pub is_compaction_boundary: i64,
    pub source_file: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnsRestore {
    pub provider_name: String,
    pub session_id: String,
    pub chain_id: String,
    pub active_segment_id: i64,
    pub last_turn_id: Option<String>,
    pub last_used_at: String,
    pub turns: Vec<SessionTurnRestoreRow>,
}

impl StateDb {
    pub fn replace_session_turns(&mut self, input: &SessionTurnsReplacement) -> Result<(), String> {
        let last = input
            .turns
            .last()
            .ok_or_else(|| "cannot replace db with empty records".to_string())?;
        let tx = self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to start session-turn replacement: {error}"))?;
        tx.execute(
            "DELETE FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
            params![input.provider_name, input.session_id],
        )
        .map_err(|error| format!("failed to delete old turns: {error}"))?;
        let now = StateDb::current_rfc3339_timestamp();
        for turn in &input.turns {
            tx.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 0, ?6, ?7, ?8)",
                params![
                    input.provider_name,
                    input.session_id,
                    turn.turn_id,
                    turn.timestamp,
                    turn.role,
                    input.source_file,
                    now,
                    turn.body,
                ],
            )
            .map_err(|error| format!("failed to insert replacement turn: {error}"))?;
        }
        tx.execute(
            "UPDATE session_chain_segments SET last_turn_id = ?2 WHERE id = ?1",
            params![input.active_segment_id, last.turn_id],
        )
        .map_err(|error| format!("failed to refresh active segment: {error}"))?;
        tx.execute(
            "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
            params![input.chain_id, last.timestamp],
        )
        .map_err(|error| format!("failed to refresh chain: {error}"))?;
        tx.commit()
            .map_err(|error| format!("failed to commit session-turn replacement: {error}"))
    }

    pub fn restore_session_turns(&mut self, input: &SessionTurnsRestore) -> Result<(), String> {
        if input.turns.iter().any(|turn| {
            turn.provider_name != input.provider_name || turn.session_id != input.session_id
        }) {
            return Err(
                "session-turn restoration rows do not match the target identity".to_string(),
            );
        }
        let tx = self
            .conn
            .transaction_with_behavior(sqlite::TransactionBehavior::Immediate)
            .map_err(|error| format!("failed to start session-turn restoration: {error}"))?;
        tx.execute(
            "DELETE FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
            params![input.provider_name, input.session_id],
        )
        .map_err(|error| format!("failed to delete replacement turns: {error}"))?;
        let now = StateDb::current_rfc3339_timestamp();
        for turn in &input.turns {
            tx.execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role,
                     parent_turn_id, is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    turn.provider_name,
                    turn.session_id,
                    turn.turn_id,
                    turn.timestamp,
                    turn.role,
                    turn.parent_turn_id,
                    turn.is_sidechain,
                    turn.is_compaction_boundary,
                    turn.source_file,
                    now,
                    turn.body,
                ],
            )
            .map_err(|error| format!("failed to restore preimage turn: {error}"))?;
        }
        tx.execute(
            "UPDATE session_chain_segments SET last_turn_id = ?2 WHERE id = ?1",
            params![input.active_segment_id, input.last_turn_id],
        )
        .map_err(|error| format!("failed to restore active segment: {error}"))?;
        tx.execute(
            "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
            params![input.chain_id, input.last_used_at],
        )
        .map_err(|error| format!("failed to restore chain: {error}"))?;
        tx.commit()
            .map_err(|error| format!("failed to commit session-turn restoration: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restoration_rejects_rows_outside_the_target_identity() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = StateDb::open(&directory.path().join("state.db")).unwrap();
        let input = SessionTurnsRestore {
            provider_name: "target-provider".to_string(),
            session_id: "target-session".to_string(),
            chain_id: "target-chain".to_string(),
            active_segment_id: 1,
            last_turn_id: Some("turn-1".to_string()),
            last_used_at: "2026-08-13T00:00:00Z".to_string(),
            turns: vec![SessionTurnRestoreRow {
                provider_name: "other-provider".to_string(),
                session_id: "target-session".to_string(),
                turn_id: "turn-1".to_string(),
                timestamp: "2026-08-13T00:00:00Z".to_string(),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: 0,
                is_compaction_boundary: 0,
                source_file: "/tmp/target.jsonl".to_string(),
                body: None,
            }],
        };

        assert_eq!(
            state.restore_session_turns(&input).unwrap_err(),
            "session-turn restoration rows do not match the target identity"
        );
    }
}
