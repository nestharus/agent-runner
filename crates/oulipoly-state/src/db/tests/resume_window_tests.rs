//! ## Declared roles
//!
//! - mapper
//! - validator
//!
//! Role set: { mapper, validator }

use super::common::*;
use super::*;
#[test]
fn find_session_for_invocation_window_returns_fresh_in_window_candidate() {
    assert_eq!(fresh_in_window_candidate().as_deref(), Some(SESSION_B));
}

fn fresh_in_window_candidate() -> Option<String> {
    let db = test_db();
    let turns = vec![
        window_turn(SESSION_A, "old-turn", "2026-04-17T08:00:00Z"),
        window_turn(SESSION_B, "fresh-turn", "2026-04-17T08:00:02Z"),
    ];
    db.ingest_session_turns_batch("provider-a", &turns).unwrap();
    invocation_window_candidate(&db, "2026-04-17T08:00:01Z", "2026-04-17T08:00:03Z")
}

#[test]
fn find_session_for_invocation_window_ranks_by_count_earliest_then_session_id() {
    assert_eq!(
        ranked_window_candidate(&[
            window_turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            window_turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            window_turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
        ])
        .as_deref(),
        Some(SESSION_A),
        "higher in-window turn count outranks an earlier first turn"
    );

    assert_eq!(
        ranked_window_candidate(&[
            window_turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            window_turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            window_turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
            window_turn(SESSION_B, "b-2", "2026-04-17T08:00:06Z"),
        ])
        .as_deref(),
        Some(SESSION_B),
        "earlier first in-window turn breaks equal counts"
    );

    let lexically_first = "11111111-1111-4111-8111-111111111111";
    let lexically_second = "22222222-2222-4222-8222-222222222222";
    assert_eq!(
        ranked_window_candidate(&[
            window_turn(lexically_second, "second-1", "2026-04-17T08:00:02Z"),
            window_turn(lexically_second, "second-2", "2026-04-17T08:00:05Z"),
            window_turn(lexically_first, "first-1", "2026-04-17T08:00:02Z"),
            window_turn(lexically_first, "first-2", "2026-04-17T08:00:06Z"),
        ])
        .as_deref(),
        Some(lexically_first),
        "lexicographic session id breaks equal counts and equal earliest turns"
    );
}

fn ranked_window_candidate(turns: &[SessionTurnIngest]) -> Option<String> {
    let db = test_db();
    db.ingest_session_turns_batch("provider-a", turns).unwrap();
    invocation_window_candidate(&db, "2026-04-17T08:00:01Z", "2026-04-17T08:00:06Z")
}

fn invocation_window_candidate(db: &StateDb, start: &str, end: &str) -> Option<String> {
    db.find_session_for_invocation_window("provider-a", &ts(start), &ts(end))
        .unwrap()
}

fn window_turn(session_id: &str, turn_id: &str, timestamp: &str) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: session_id.to_string(),
        turn_id: turn_id.to_string(),
        timestamp: ts(timestamp),
        role: "assistant".to_string(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}
