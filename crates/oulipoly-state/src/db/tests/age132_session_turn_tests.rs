//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/age132_session_turn_tests.rs
//!     role: intrinsic-surface
//!     Domain: age132-session-turn-tests-persistence
//!     Owns:
//!       - StateDb age132-session-turn-tests persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: SESSION_A, session_turn_count, session_turn_detail_row, session_turn_source_file, test_db, ts
//! ```

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
    let source_file = session_turn_source_file(&db, "single-turn");
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
    let row = session_turn_detail_row(&db, "turn-2");
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
    let persisted = session_turn_count(&failing);
    assert_eq!(persisted, 0);
}
