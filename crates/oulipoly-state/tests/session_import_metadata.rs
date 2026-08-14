use chrono::{DateTime, Utc};
use oulipoly_state::{
    CURRENT_SCHEMA_VERSION, ImportedSessionDisplayMetadataUpsert, StateDb, migrations,
};
use rusqlite::Connection;

fn ts(value: &str) -> DateTime<Utc> {
    value.parse::<DateTime<Utc>>().unwrap()
}

fn user_version(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, bool>(0),
    )
    .unwrap()
}

#[test]
fn fresh_open_creates_imported_session_display_metadata_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let connection = Connection::open(db.path()).unwrap();

    assert_eq!(user_version(&connection), CURRENT_SCHEMA_VERSION);
    assert!(table_exists(
        &connection,
        "imported_session_display_metadata"
    ));
}

#[test]
fn imported_session_display_metadata_upsert_preserves_first_seen_and_refreshes_display_fields() {
    let db = StateDb::open(std::path::Path::new(":memory:")).unwrap();
    let first_seen = ts("2026-06-01T00:00:00Z");
    let second_seen = ts("2026-06-01T00:05:00Z");

    db.upsert_imported_session_display_metadata(&ImportedSessionDisplayMetadataUpsert {
        provider_name: "provider-a".to_string(),
        provider_session_id: "session-a".to_string(),
        title: Some("Original".to_string()),
        cwd: Some("/tmp/original".to_string()),
        turn_count: Some(3),
        provider_updated_at: Some(ts("2026-05-31T23:59:00Z")),
        seen_at: first_seen,
    })
    .unwrap();
    db.upsert_imported_session_display_metadata(&ImportedSessionDisplayMetadataUpsert {
        provider_name: "provider-a".to_string(),
        provider_session_id: "session-a".to_string(),
        title: Some("Refreshed".to_string()),
        cwd: Some("/tmp/refreshed".to_string()),
        turn_count: Some(7),
        provider_updated_at: Some(ts("2026-06-01T00:04:00Z")),
        seen_at: second_seen,
    })
    .unwrap();

    let row = db
        .imported_session_display_metadata("provider-a", "session-a")
        .unwrap()
        .unwrap();
    assert_eq!(row.first_seen_at, first_seen);
    assert_eq!(row.last_seen_at, second_seen);
    assert_eq!(row.title.as_deref(), Some("Refreshed"));
    assert_eq!(row.cwd.as_deref(), Some("/tmp/refreshed"));
    assert_eq!(row.turn_count, Some(7));
    assert_eq!(row.provider_updated_at, Some(ts("2026-06-01T00:04:00Z")));
}

#[test]
fn schema_v9_to_current_metadata_migration_is_additive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let mut conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE preserved_rows (id INTEGER PRIMARY KEY, label TEXT NOT NULL);
         INSERT INTO preserved_rows (id, label) VALUES (1, 'keep-me');
         PRAGMA user_version = 9;",
    )
    .unwrap();

    let plan = migrations::plan(9, CURRENT_SCHEMA_VERSION).unwrap();
    migrations::run_with_db_path(&mut conn, &plan, path).unwrap();

    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    assert!(table_exists(&conn, "imported_session_display_metadata"));
    let label: String = conn
        .query_row("SELECT label FROM preserved_rows WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(label, "keep-me");
}
