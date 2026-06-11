//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn age132_session_turn_ingest_batch_and_single_paths_preserve_mapping_and_atomicity() {
    let db = test_db();
    let timestamp = ts("2026-04-17T08:00:00Z");
    assert!(
        db.ingest_session_turn(
            "provider-a",
            SESSION_A,
            "single-turn",
            &timestamp,
            "assistant",
            "/tmp/session.jsonl",
        )
        .unwrap()
    );
    assert!(
        !db.ingest_session_turn(
            "provider-a",
            SESSION_A,
            "single-turn",
            &timestamp,
            "assistant",
            "/tmp/session.jsonl",
        )
        .unwrap()
    );
    let source_file: String = db
        .conn
        .query_row(
            "SELECT source_file FROM session_turns WHERE turn_id = 'single-turn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_file, "/tmp/session.jsonl");

    let turns = vec![
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "turn-1".to_string(),
            timestamp,
            role: "user".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some("hello".to_string()),
        },
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "turn-2".to_string(),
            timestamp: timestamp + chrono::Duration::seconds(1),
            role: "assistant".to_string(),
            parent_turn_id: Some("turn-1".to_string()),
            is_sidechain: true,
            is_compaction_boundary: true,
            body: Some("world".to_string()),
        },
    ];
    assert_eq!(
        db.ingest_session_turns_batch("provider-a", &turns).unwrap(),
        2
    );
    assert_eq!(
        db.ingest_session_turns_batch("provider-a", &turns).unwrap(),
        0
    );
    let row: (Option<String>, i64, i64, Option<String>) = db
        .conn
        .query_row(
            "SELECT parent_turn_id, is_sidechain, is_compaction_boundary, body
                 FROM session_turns WHERE turn_id = 'turn-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (Some("turn-1".to_string()), 1, 1, Some("world".to_string()))
    );

    let failing = test_db();
    failing
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_bad_turn
                 BEFORE INSERT ON session_turns
                 WHEN NEW.turn_id = 'bad'
                 BEGIN
                   SELECT RAISE(ABORT, 'bad turn');
                 END;",
        )
        .unwrap();
    assert!(
        failing
            .ingest_session_turns_batch(
                "provider-a",
                &[
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "good-before-error".to_string(),
                        timestamp,
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: false,
                        body: None,
                    },
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "bad".to_string(),
                        timestamp: timestamp + chrono::Duration::seconds(1),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: false,
                        body: None,
                    },
                ],
            )
            .unwrap_err()
            .contains("bad turn")
    );
    let persisted: i64 = failing
        .conn
        .query_row("SELECT COUNT(*) FROM session_turns", [], |row| row.get(0))
        .unwrap();
    assert_eq!(persisted, 0);
}
