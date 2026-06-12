//! ## Declared roles
//!
//! - validator
//! - accessor
//! - mapper
//! - predicate
//!
//! Role set: { validator, accessor, mapper, predicate }

use super::common::*;
use super::*;
#[test]
fn session_turns_schema_creation_includes_sidechain_columns() {
    let db = test_db();
    let sql = session_turn_table_sql(&db);

    assert!(sql.contains("parent_turn_id TEXT"));
    assert!(sql.contains("is_sidechain INTEGER NOT NULL DEFAULT 0"));
    assert!(sql.contains("body TEXT"));
}

#[test]
fn session_turns_schema_migration_adds_parent_and_sidechain_columns() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let columns = session_turn_columns(&db);

    assert!(has_parent_turn_column(&columns));
    assert!(has_sidechain_column(&columns));
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

    let session_columns = session_turn_columns(&db);
    assert!(
        has_nullable_body_column(&session_columns),
        "legacy migration must add nullable body TEXT; columns={session_columns:?}"
    );
    let body = session_turn_body(&db, "legacy-turn");
    assert_eq!(body, None);

    let quota_columns = provider_quota_column_names(&db);
    assert!(
        has_column(&quota_columns, "topology_peak_live_window_count"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
    assert!(
        has_column(&quota_columns, "last_topology_probe_at"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
}

#[test]
fn session_turns_schema_creation_includes_resume_lookup_index() {
    let db = test_db();
    let indexes = session_turn_index_names(&db);

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must exist on fresh DB bootstrap: {indexes:?}"
    );
}

#[test]
fn session_turns_schema_migration_adds_resume_lookup_index() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let indexes = session_turn_index_names(&db);

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must be added on existing DB open: {indexes:?}"
    );
}

type SessionTurnColumn = (String, String, i64, Option<String>);

fn session_turn_columns(db: &StateDb) -> Vec<SessionTurnColumn> {
    db.conn
        .prepare("PRAGMA table_info(session_turns)")
        .unwrap()
        .query_map([], session_turn_column_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn session_turn_column_row(row: &sqlite::Row<'_>) -> sqlite::Result<SessionTurnColumn> {
    Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
}

fn has_parent_turn_column(columns: &[SessionTurnColumn]) -> bool {
    columns.iter().any(|column| {
        column.0 == "parent_turn_id" && column.1 == "TEXT" && column.2 == 0 && column.3.is_none()
    })
}

fn has_sidechain_column(columns: &[SessionTurnColumn]) -> bool {
    columns.iter().any(|column| {
        column.0 == "is_sidechain"
            && column.1 == "INTEGER"
            && column.2 == 1
            && column.3.as_deref() == Some("0")
    })
}

fn has_nullable_body_column(columns: &[SessionTurnColumn]) -> bool {
    columns
        .iter()
        .any(|(name, data_type, notnull, _)| name == "body" && data_type == "TEXT" && *notnull == 0)
}

fn provider_quota_column_names(db: &StateDb) -> Vec<String> {
    db.conn
        .prepare("PRAGMA table_info(provider_quotas)")
        .unwrap()
        .query_map([], provider_quota_column_name_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn provider_quota_column_name_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(1)
}

fn has_column(columns: &[String], expected: &str) -> bool {
    columns.iter().any(|column| column == expected)
}

fn session_turn_index_names(db: &StateDb) -> Vec<String> {
    db.conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], session_turn_index_name_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn session_turn_index_name_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(0)
}
