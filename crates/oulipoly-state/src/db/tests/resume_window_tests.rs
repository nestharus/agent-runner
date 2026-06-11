//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn find_session_for_invocation_window_returns_fresh_in_window_candidate() {
    let db = test_db();
    let turns = vec![
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "old-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:00Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        },
        SessionTurnIngest {
            session_id: SESSION_B.to_string(),
            turn_id: "fresh-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:02Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        },
    ];
    db.ingest_session_turns_batch("claude", &turns).unwrap();

    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:03Z"),
        )
        .unwrap();

    assert_eq!(found.as_deref(), Some(SESSION_B));
}

#[test]
fn find_session_for_invocation_window_ranks_by_count_earliest_then_session_id() {
    fn turn(session_id: &str, turn_id: &str, timestamp: &str) -> SessionTurnIngest {
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

    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(SESSION_A),
        "higher in-window turn count outranks an earlier first turn"
    );

    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
            turn(SESSION_B, "b-2", "2026-04-17T08:00:06Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(SESSION_B),
        "earlier first in-window turn breaks equal counts"
    );

    let lexically_first = "11111111-1111-4111-8111-111111111111";
    let lexically_second = "22222222-2222-4222-8222-222222222222";
    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(lexically_second, "second-1", "2026-04-17T08:00:02Z"),
            turn(lexically_second, "second-2", "2026-04-17T08:00:05Z"),
            turn(lexically_first, "first-1", "2026-04-17T08:00:02Z"),
            turn(lexically_first, "first-2", "2026-04-17T08:00:06Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(lexically_first),
        "lexicographic session id breaks equal counts and equal earliest turns"
    );
}
