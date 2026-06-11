//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn migration_returning_clause_aborts_on_concurrent_close() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );

    let first = db
        .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:00Z"))
        .unwrap();
    let second = db
        .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:01Z"))
        .unwrap();

    assert!(first.is_some(), "first close should win RETURNING guard");
    assert_eq!(second, None, "concurrent loser must abort");
    let active: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            sqlite::params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 0);
}
