//! AGE-163 WU-A.1 — schema v9 migration adds working-set columns on
//! `provider_quotas` and creates the `model_round_robin_cursor` table.

mod fixtures;

use fixtures::{create_full_state_schema, user_version};
use oulipoly_state::{CURRENT_SCHEMA_VERSION, StateDb};
use rusqlite::Connection;

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .unwrap()
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .any(|name| name.unwrap() == column)
}

#[test]
fn fresh_open_has_current_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
}

#[test]
fn fresh_open_creates_round_robin_cursor_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    assert!(table_exists(&conn, "model_round_robin_cursor"));
}

#[test]
fn fresh_open_has_working_set_columns_on_provider_quotas() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    assert!(column_exists(&conn, "provider_quotas", "next_available_at"));
    assert!(column_exists(&conn, "provider_quotas", "last_refresh_at"));
    assert!(column_exists(&conn, "provider_quotas", "failure_class"));
}

#[test]
fn schema_v8_db_migrates_forward_to_current() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = Connection::open(&path).unwrap();
    create_full_state_schema(&conn, 8);
    conn.execute_batch("INSERT INTO provider_quotas (provider_name) VALUES ('claude');")
        .unwrap();
    drop(conn);

    let db = StateDb::open(&path).unwrap();
    let conn = Connection::open(db.path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    assert!(table_exists(&conn, "model_round_robin_cursor"));
    assert!(column_exists(&conn, "provider_quotas", "next_available_at"));
    assert!(column_exists(&conn, "provider_quotas", "last_refresh_at"));
    assert!(column_exists(&conn, "provider_quotas", "failure_class"));

    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM provider_quotas", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "v8 rows must survive migration to v9");
}
