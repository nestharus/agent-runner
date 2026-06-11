//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, validator }

use super::super::*;
use super::*;
pub(in crate::db::tests) fn table_columns_with_pk(
    conn: &sqlite::Connection,
    table_name: &str,
) -> Vec<(String, i64)> {
    require_sqlite_rows(read_table_columns_with_pk(conn, table_name))
}

fn read_table_columns_with_pk(
    conn: &sqlite::Connection,
    table_name: &str,
) -> sqlite::Result<Vec<(String, i64)>> {
    let mut stmt = conn.prepare(&table_info_sql(table_name))?;
    let rows = stmt.query_map([], table_column_with_pk_row)?;
    rows.collect()
}

fn table_info_sql(table_name: &str) -> String {
    format!("PRAGMA table_info({table_name})")
}

fn table_column_with_pk_row(row: &sqlite::Row<'_>) -> sqlite::Result<(String, i64)> {
    Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
}

pub(in crate::db::tests) fn provider_aggregate_snapshot(
    conn: &sqlite::Connection,
) -> Vec<ProviderAggregateSnapshot> {
    require_sqlite_rows(read_provider_aggregate_snapshot(conn))
}

fn read_provider_aggregate_snapshot(
    conn: &sqlite::Connection,
) -> sqlite::Result<Vec<ProviderAggregateSnapshot>> {
    let mut stmt = conn.prepare(
        "SELECT model_name, provider_name, invocation_count, error_count,
                    last_error, last_error_at, last_invoked_at
               FROM providers
              ORDER BY model_name, provider_name",
    )?;
    let rows = stmt.query_map([], provider_aggregate_snapshot_row)?;
    rows.collect()
}

fn provider_aggregate_snapshot_row(
    row: &sqlite::Row<'_>,
) -> sqlite::Result<ProviderAggregateSnapshot> {
    Ok(ProviderAggregateSnapshot {
        model_name: row.get(0)?,
        provider_name: row.get(1)?,
        invocation_count: row.get(2)?,
        error_count: row.get(3)?,
        last_error: row.get(4)?,
        last_error_at: row.get(5)?,
        last_invoked_at: row.get(6)?,
    })
}

pub(in crate::db::tests) fn quoted_snapshot(
    conn: &sqlite::Connection,
    schema_sql: &str,
    rows_sql: &str,
) -> Vec<String> {
    let mut snapshot = quoted_schema_snapshot(conn, schema_sql);
    snapshot.extend(quoted_row_snapshot(conn, rows_sql));
    snapshot
}

fn quoted_schema_snapshot(conn: &sqlite::Connection, schema_sql: &str) -> Vec<String> {
    vec![conn.query_row(schema_sql, [], quoted_string_row).unwrap()]
}

fn quoted_row_snapshot(conn: &sqlite::Connection, rows_sql: &str) -> Vec<String> {
    require_sqlite_rows(read_quoted_row_snapshot(conn, rows_sql))
}

fn read_quoted_row_snapshot(
    conn: &sqlite::Connection,
    rows_sql: &str,
) -> sqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(rows_sql)?;
    let rows = stmt.query_map([], quoted_string_row)?;
    rows.collect()
}

fn quoted_string_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get::<_, String>(0)
}

pub(in crate::db::tests) fn malformed_providers_snapshot(conn: &sqlite::Connection) -> Vec<String> {
    quoted_snapshot(
        conn,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'providers'",
        "SELECT quote(model_name) || '|' || quote(provider_index) || '|' ||
                    quote(provider_name) || '|' || quote(invocation_count) || '|' ||
                    quote(error_count) || '|' || quote(last_error) || '|' ||
                    quote(last_error_at) || '|' || quote(last_invoked_at)
               FROM providers
              ORDER BY model_name, provider_index, provider_name",
    )
}

pub(in crate::db::tests) fn invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
    quoted_snapshot(
        conn,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
        "SELECT quote(invocation_uuid) || '|' || quote(model_name) || '|' ||
                    quote(provider_name) || '|' || quote(provider_index) || '|' ||
                    quote(status) || '|' || quote(success) || '|' ||
                    quote(exit_code) || '|' || quote(error_category) || '|' ||
                    quote(created_at) || '|' || quote(finished_at)
               FROM invocations
              ORDER BY id",
    )
}

pub(in crate::db::tests) fn legacy_invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
    quoted_snapshot(
        conn,
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
        "SELECT quote(model_name) || '|' || quote(provider_index) || '|' ||
                    quote(success) || '|' || quote(exit_code) || '|' ||
                    quote(error_category) || '|' || quote(created_at)
               FROM invocations
              ORDER BY id",
    )
}

pub(in crate::db::tests) fn legacy_session_turns_db() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                UNIQUE (provider_name, session_id, turn_id)
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    dir
}

pub(in crate::db::tests) fn invocation_table_sql(db: &StateDb) -> String {
    db.conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_columns(db: &StateDb) -> Vec<String> {
    require_sqlite_rows(read_invocation_columns(db))
}

fn read_invocation_columns(db: &StateDb) -> sqlite::Result<Vec<String>> {
    let mut stmt = db.conn.prepare("PRAGMA table_info(invocations)")?;
    let rows = stmt.query_map([], invocation_column_row)?;
    rows.collect()
}

fn require_sqlite_rows<T>(result: sqlite::Result<Vec<T>>) -> Vec<T> {
    result.unwrap()
}

fn invocation_column_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get::<_, String>(1)
}
