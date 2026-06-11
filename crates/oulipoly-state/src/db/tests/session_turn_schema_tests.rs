//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn session_turns_schema_creation_includes_sidechain_columns() {
    let db = test_db();
    let sql: String = db
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'session_turns'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(sql.contains("parent_turn_id TEXT"));
    assert!(sql.contains("is_sidechain INTEGER NOT NULL DEFAULT 0"));
    assert!(sql.contains("body TEXT"));
}

#[test]
fn session_turns_schema_migration_adds_parent_and_sidechain_columns() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let columns: Vec<(String, String, i64, Option<String>)> = db
        .conn
        .prepare("PRAGMA table_info(session_turns)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(columns.iter().any(|column| {
        column.0 == "parent_turn_id" && column.1 == "TEXT" && column.2 == 0 && column.3.is_none()
    }));
    assert!(columns.iter().any(|column| {
        column.0 == "is_sidechain"
            && column.1 == "INTEGER"
            && column.2 == 1
            && column.3.as_deref() == Some("0")
    }));
}

#[test]
fn session_turns_schema_migration_adds_nullable_body_to_legacy_db() {
    // risk: legacy-DB upgrade; level: unit; source: contract §4 T5 / proposal A2,A8.
    let dir = legacy_session_turns_db();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES ('fixture-provider', 'session-a', 'legacy-turn', '2026-04-17T08:00:00Z', 'assistant', '', '2026-04-17T08:00:00Z')",
            [],
        )
        .unwrap();
    drop(conn);

    let db = StateDb::open(&path).unwrap();

    let session_columns: Vec<(String, String, i64)> = db
        .conn
        .prepare("PRAGMA table_info(session_turns)")
        .unwrap()
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        session_columns
            .iter()
            .any(|(name, data_type, notnull)| name == "body"
                && data_type == "TEXT"
                && *notnull == 0),
        "legacy migration must add nullable body TEXT; columns={session_columns:?}"
    );
    let body: Option<String> = db
        .conn
        .query_row(
            "SELECT body FROM session_turns WHERE turn_id = 'legacy-turn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, None);

    let quota_columns: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(provider_quotas)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        quota_columns
            .iter()
            .any(|column| column == "topology_peak_live_window_count"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
    assert!(
        quota_columns
            .iter()
            .any(|column| column == "last_topology_probe_at"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
}

#[test]
fn session_turns_schema_creation_includes_resume_lookup_index() {
    let db = test_db();
    let indexes: Vec<String> = db
        .conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must exist on fresh DB bootstrap: {indexes:?}"
    );
}

#[test]
fn session_turns_schema_migration_adds_resume_lookup_index() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let indexes: Vec<String> = db
        .conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must be added on existing DB open: {indexes:?}"
    );
}
