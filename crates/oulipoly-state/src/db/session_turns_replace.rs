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
        let updated_segments = tx
            .execute(
                "UPDATE session_chain_segments
                 SET last_turn_id = ?2
                 WHERE id = ?1 AND chain_id = ?3 AND provider_name = ?4 AND session_id = ?5",
                params![
                    input.active_segment_id,
                    last.turn_id,
                    input.chain_id,
                    input.provider_name,
                    input.session_id,
                ],
            )
            .map_err(|error| format!("failed to refresh active segment: {error}"))?;
        if updated_segments != 1 {
            return Err(format!(
                "failed to refresh active segment: expected 1 updated row, got {updated_segments}"
            ));
        }
        let updated_chains = tx
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                params![input.chain_id, last.timestamp],
            )
            .map_err(|error| format!("failed to refresh chain: {error}"))?;
        if updated_chains != 1 {
            return Err(format!(
                "failed to refresh chain: expected 1 updated row, got {updated_chains}"
            ));
        }
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
        let updated_segments = tx
            .execute(
                "UPDATE session_chain_segments
                 SET last_turn_id = ?2
                 WHERE id = ?1 AND chain_id = ?3 AND provider_name = ?4 AND session_id = ?5",
                params![
                    input.active_segment_id,
                    input.last_turn_id,
                    input.chain_id,
                    input.provider_name,
                    input.session_id,
                ],
            )
            .map_err(|error| format!("failed to restore active segment: {error}"))?;
        if updated_segments != 1 {
            return Err(format!(
                "failed to restore active segment: expected 1 updated row, got {updated_segments}"
            ));
        }
        let updated_chains = tx
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                params![input.chain_id, input.last_used_at],
            )
            .map_err(|error| format!("failed to restore chain: {error}"))?;
        if updated_chains != 1 {
            return Err(format!(
                "failed to restore chain: expected 1 updated row, got {updated_chains}"
            ));
        }
        tx.commit()
            .map_err(|error| format!("failed to commit session-turn restoration: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_chain_and_segment(
        state: &StateDb,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> i64 {
        state
            .conn
            .execute(
                "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-08-13T00:00:00Z', '2026-08-13T00:00:00Z', 'model')",
                params![chain_id],
            )
            .unwrap();
        state
            .conn
            .execute(
                "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, last_turn_id, transition_reason)
                 VALUES (?1, ?2, ?3, '2026-08-13T00:00:00Z', 'current', 'initial')",
                params![chain_id, provider_name, session_id],
            )
            .unwrap();
        state.conn.last_insert_rowid()
    }

    fn insert_turn(state: &StateDb, turn_id: &str) {
        state
            .conn
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, body)
                 VALUES ('provider', 'session', ?1, '2026-08-13T00:00:00Z', 'user',
                         '/tmp/session.jsonl', '2026-08-13T00:00:00Z', ?1)",
                params![turn_id],
            )
            .unwrap();
    }

    fn turn_ids(state: &StateDb) -> Vec<String> {
        let mut statement = state
            .conn
            .prepare(
                "SELECT turn_id FROM session_turns
                 WHERE provider_name = 'provider' AND session_id = 'session' ORDER BY turn_id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn replacement_rolls_back_when_active_segment_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = StateDb::open(&directory.path().join("state.db")).unwrap();
        insert_chain_and_segment(&state, "chain", "provider", "session");
        insert_turn(&state, "old");
        let input = SessionTurnsReplacement {
            provider_name: "provider".to_string(),
            session_id: "session".to_string(),
            chain_id: "chain".to_string(),
            active_segment_id: i64::MAX,
            source_file: "/tmp/session.jsonl".to_string(),
            turns: vec![SessionTurnReplacement {
                turn_id: "new".to_string(),
                timestamp: "2026-08-14T00:00:00Z".to_string(),
                role: "assistant".to_string(),
                body: "new".to_string(),
            }],
        };

        assert_eq!(
            state.replace_session_turns(&input).unwrap_err(),
            "failed to refresh active segment: expected 1 updated row, got 0"
        );
        assert_eq!(turn_ids(&state), vec!["old"]);
    }

    #[test]
    fn restoration_rolls_back_when_chain_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = StateDb::open(&directory.path().join("state.db")).unwrap();
        let segment_id = insert_chain_and_segment(&state, "chain", "provider", "session");
        insert_turn(&state, "replacement");
        let input = SessionTurnsRestore {
            provider_name: "provider".to_string(),
            session_id: "session".to_string(),
            chain_id: "missing-chain".to_string(),
            active_segment_id: segment_id,
            last_turn_id: Some("old".to_string()),
            last_used_at: "2026-08-13T00:00:00Z".to_string(),
            turns: vec![SessionTurnRestoreRow {
                provider_name: "provider".to_string(),
                session_id: "session".to_string(),
                turn_id: "old".to_string(),
                timestamp: "2026-08-13T00:00:00Z".to_string(),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: 0,
                is_compaction_boundary: 0,
                source_file: "/tmp/session.jsonl".to_string(),
                body: Some("old".to_string()),
            }],
        };

        assert_eq!(
            state.restore_session_turns(&input).unwrap_err(),
            "failed to restore active segment: expected 1 updated row, got 0"
        );
        assert_eq!(turn_ids(&state), vec!["replacement"]);
    }

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

    #[test]
    fn replacement_rejects_cross_resource_segment_chain_and_session_targets() {
        for (active_segment, chain_id, provider_name, session_id) in [
            ("other", "chain", "provider", "session"),
            ("target", "other-chain", "provider", "session"),
            ("target", "chain", "other-provider", "session"),
            ("target", "chain", "provider", "other-session"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut state = StateDb::open(&directory.path().join("state.db")).unwrap();
            let target_segment = insert_chain_and_segment(&state, "chain", "provider", "session");
            let other_segment =
                insert_chain_and_segment(&state, "other-chain", "other-provider", "other-session");
            insert_turn(&state, "old");
            let input = SessionTurnsReplacement {
                provider_name: provider_name.to_string(),
                session_id: session_id.to_string(),
                chain_id: chain_id.to_string(),
                active_segment_id: if active_segment == "target" {
                    target_segment
                } else {
                    other_segment
                },
                source_file: "/tmp/session.jsonl".to_string(),
                turns: vec![SessionTurnReplacement {
                    turn_id: "new".to_string(),
                    timestamp: "2026-08-14T00:00:00Z".to_string(),
                    role: "assistant".to_string(),
                    body: "new".to_string(),
                }],
            };

            let error = state.replace_session_turns(&input).unwrap_err();

            assert!(error.contains("expected 1 updated row, got 0"), "{error}");
            assert_eq!(turn_ids(&state), vec!["old"]);
        }
    }

    #[test]
    fn restoration_rejects_cross_resource_segment_chain_and_session_targets() {
        for (active_segment, chain_id, provider_name, session_id) in [
            ("other", "chain", "provider", "session"),
            ("target", "other-chain", "provider", "session"),
            ("target", "chain", "other-provider", "session"),
            ("target", "chain", "provider", "other-session"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut state = StateDb::open(&directory.path().join("state.db")).unwrap();
            let target_segment = insert_chain_and_segment(&state, "chain", "provider", "session");
            let other_segment =
                insert_chain_and_segment(&state, "other-chain", "other-provider", "other-session");
            insert_turn(&state, "replacement");
            let input = SessionTurnsRestore {
                provider_name: provider_name.to_string(),
                session_id: session_id.to_string(),
                chain_id: chain_id.to_string(),
                active_segment_id: if active_segment == "target" {
                    target_segment
                } else {
                    other_segment
                },
                last_turn_id: Some("old".to_string()),
                last_used_at: "2026-08-13T00:00:00Z".to_string(),
                turns: vec![SessionTurnRestoreRow {
                    provider_name: provider_name.to_string(),
                    session_id: session_id.to_string(),
                    turn_id: "old".to_string(),
                    timestamp: "2026-08-13T00:00:00Z".to_string(),
                    role: "user".to_string(),
                    parent_turn_id: None,
                    is_sidechain: 0,
                    is_compaction_boundary: 0,
                    source_file: "/tmp/session.jsonl".to_string(),
                    body: Some("old".to_string()),
                }],
            };

            let error = state.restore_session_turns(&input).unwrap_err();

            assert!(error.contains("expected 1 updated row, got 0"), "{error}");
            assert_eq!(turn_ids(&state), vec!["replacement"]);
        }
    }
}
