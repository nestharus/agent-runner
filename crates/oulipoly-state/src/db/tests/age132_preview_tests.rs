//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn age132_resume_previews_and_compaction_boundaries_preserve_ordering_contracts() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "provider-a",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "provider-a2",
        SESSION_A,
        "provider-a-opus",
        "2026-04-17T09:00:00Z",
    );
    let turns: Vec<_> = (0..4)
        .map(|i| SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: format!("turn-{i}"),
            timestamp: ts(&format!("2026-04-17T08:00:0{i}Z")),
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some(format!("body-{i}")),
        })
        .collect();
    db.ingest_session_turns_batch("provider-a2", &turns)
        .unwrap();

    let previews = db.resume_previews(SESSION_A).unwrap();
    assert_eq!(previews.len(), 2);
    assert_eq!(previews[0].chain_id, CHAIN_B);
    assert_eq!(previews[0].active_provider, "provider-a2");
    assert_eq!(previews[0].turn_count, 4);
    assert_eq!(previews[0].recent_turns.len(), 3);
    assert_eq!(
        previews[0].recent_turns[0].timestamp,
        ts("2026-04-17T08:00:01Z")
    );
    assert_eq!(
        previews[0].recent_turns[1].timestamp,
        ts("2026-04-17T08:00:02Z")
    );
    assert_eq!(
        previews[0].recent_turns[2].timestamp,
        ts("2026-04-17T08:00:03Z")
    );
    assert_eq!(previews[0].recent_turns[0].snippet, None);
    assert_eq!(previews[1].chain_id, CHAIN_A);

    let boundary_db = test_db();
    boundary_db
        .ingest_session_turns_batch(
            "provider-a",
            &[
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "old-boundary".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "tie-first".to_string(),
                    timestamp: ts("2026-04-17T08:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "tie-second".to_string(),
                    timestamp: ts("2026-04-17T08:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "not-yet-boundary".to_string(),
                    timestamp: ts("2026-04-17T08:02:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        )
        .unwrap();
    let latest = boundary_db
        .latest_compaction_boundary("provider-a", SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(latest.0, "tie-second");
    assert_eq!(latest.1, ts("2026-04-17T08:01:00Z"));
    assert!(
        boundary_db
            .flag_compaction_boundary("provider-a", SESSION_A, "not-yet-boundary")
            .unwrap()
    );
    assert!(
        !boundary_db
            .flag_compaction_boundary("provider-a", SESSION_A, "not-yet-boundary")
            .unwrap()
    );
    assert!(
        !boundary_db
            .flag_compaction_boundary("provider-a", SESSION_A, "missing-turn")
            .unwrap()
    );
    let latest = boundary_db
        .latest_compaction_boundary("provider-a", SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(latest.0, "not-yet-boundary");
    assert_eq!(latest.1, ts("2026-04-17T08:02:00Z"));
    assert_eq!(
        test_db()
            .latest_compaction_boundary("provider-a", SESSION_A)
            .unwrap(),
        None
    );
}
