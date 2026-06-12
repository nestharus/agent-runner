//! ## Declared roles
//!
//! - validator
//! - mapper
//! - accessor
//!
//! Role set: { validator, mapper, accessor }

use super::common::*;
use super::*;
#[test]
fn migration_backfills_resolved_and_legacy_rows() {
    // PP-001: the caller pushes the resolved `(model, provider_index) ->
    // provider_name` lookup; StateDb no longer discovers the app config layout.
    let mut provider_names = LegacyProviderNames::new();
    provider_names.insert(
        ("mapped-model".to_string(), 0),
        "fixture-provider".to_string(),
    );

    let dir = legacy_invocations_db(&[
        ("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
        (
            "missing-model",
            0,
            0,
            7,
            Some("rate_limit"),
            "2026-04-17T08:05:00Z",
        ),
    ]);
    let db =
        StateDb::open_with_legacy_provider_names(&dir.path().join("state.db"), &provider_names)
            .unwrap();

    let rows = migrated_invocation_rows(&db.conn);

    assert_eq!(rows[0].0, "mapped-model");
    assert_eq!(rows[0].1.as_deref(), Some("fixture-provider"));
    assert_eq!(rows[0].2, "succeeded");
    assert_eq!(rows[0].4, "2026-04-17T08:00:00Z");
    assert!(Uuid::parse_str(&rows[0].3).is_ok());

    // A model absent from the pushed lookup falls through to status='legacy'
    // with provider_name=NULL.
    assert_eq!(rows[1].0, "missing-model");
    assert_eq!(rows[1].1, None);
    assert_eq!(rows[1].2, "legacy");
    assert_eq!(rows[1].4, "2026-04-17T08:05:00Z");
    assert!(Uuid::parse_str(&rows[1].3).is_ok());
}

#[test]
fn migration_rolls_back_when_rebuild_fails() {
    let dir = legacy_invocations_db(&[("mapped-model", 0, 1, 0, None, "2026-04-17T08:00:00Z")]);
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE invocations_new (id INTEGER PRIMARY KEY);
             CREATE TABLE blocker (name TEXT);
             CREATE INDEX idx_invocations_uuid ON blocker(name);",
    )
    .unwrap();
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("migration should fail"),
        Err(err) => err,
    };
    assert!(!err.is_empty());

    let conn = sqlite::Connection::open(&path).unwrap();
    let columns = invocation_column_names(&conn);
    assert_eq!(
        columns,
        vec![
            "id",
            "model_name",
            "provider_index",
            "success",
            "exit_code",
            "error_category",
            "created_at",
        ]
    );
    let row_count = invocation_row_count(&conn);
    assert_eq!(row_count, 1);
}

#[test]
fn migration_with_empty_provider_lookup_marks_rows_legacy() {
    // When the caller cannot resolve any provider names (e.g. the app's models
    // config is missing or corrupt, so it pushes an empty lookup), the DB open
    // must still succeed and every legacy row degrades to status='legacy' with
    // provider_name=NULL (per V10 — observable degradation, not silent).
    let dir = legacy_invocations_db(&[
        ("any-model", 0, 1, 0, None, "2026-04-17T08:00:00Z"),
        (
            "other-model",
            1,
            0,
            1,
            Some("rate_limit"),
            "2026-04-17T08:05:00Z",
        ),
    ]);
    let path = dir.path().join("state.db");

    let db = StateDb::open_with_legacy_provider_names(&path, &LegacyProviderNames::new())
        .expect("DB open must not fail on an empty provider lookup");

    let conn = sqlite::Connection::open(&path).unwrap();
    let rows = migrated_invocation_rows(&conn);
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert!(r.1.is_none(), "provider_name must be NULL on empty lookup");
        assert_eq!(r.2, "legacy", "status must be legacy on empty lookup");
        assert!(Uuid::parse_str(&r.3).is_ok());
        assert!(!r.4.is_empty(), "finished_at must be backfilled");
    }
    drop(db);
}

type MigratedInvocationRow = (String, Option<String>, String, String, String);

fn migrated_invocation_rows(conn: &sqlite::Connection) -> Vec<MigratedInvocationRow> {
    conn.prepare(
        "SELECT model_name, provider_name, status, invocation_uuid, finished_at
             FROM invocations ORDER BY created_at",
    )
    .unwrap()
    .query_map([], migrated_invocation_row)
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn migrated_invocation_row(row: &sqlite::Row<'_>) -> sqlite::Result<MigratedInvocationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn invocation_column_names(conn: &sqlite::Connection) -> Vec<String> {
    conn.prepare("PRAGMA table_info(invocations)")
        .unwrap()
        .query_map([], invocation_column_name_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn invocation_column_name_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(1)
}
