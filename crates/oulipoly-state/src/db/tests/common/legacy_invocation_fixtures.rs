//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/legacy_invocation_fixtures.rs
//!     role: intrinsic-surface
//!     Domain: legacy-invocation-fixtures-persistence
//!     Owns:
//!       - StateDb legacy-invocation-fixtures persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, StateDb, TempDir, Uuid, params, sqlite
//! ```

use super::super::*;
use super::*;
pub(in crate::db::tests) type LegacyInvocationFixtureRow<'a> =
    (&'a str, i64, i64, i64, Option<&'a str>, &'a str);

pub(in crate::db::tests) fn legacy_invocations_db(
    rows: &[LegacyInvocationFixtureRow<'_>],
) -> TempDir {
    let (dir, conn) = temp_state_connection();
    create_legacy_invocations_table(&conn);
    insert_legacy_invocation_rows(&conn, rows);
    mark_current_schema_version(&conn);
    dir
}

fn temp_state_connection() -> (TempDir, sqlite::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    (dir, conn)
}

fn create_legacy_invocations_table(conn: &sqlite::Connection) {
    conn.execute_batch(legacy_invocations_schema_sql()).unwrap();
}

fn legacy_invocations_schema_sql() -> &'static str {
    "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
            );"
}

fn insert_legacy_invocation_rows(
    conn: &sqlite::Connection,
    rows: &[LegacyInvocationFixtureRow<'_>],
) {
    for row in rows.iter().map(legacy_invocation_insert_from_row) {
        insert_legacy_invocation_row(conn, &row);
    }
}

struct LegacyInvocationInsert<'a> {
    model_name: &'a str,
    provider_index: i64,
    success: i64,
    exit_code: i64,
    error_category: Option<&'a str>,
    created_at: &'a str,
}

fn legacy_invocation_insert_from_row<'a>(
    row: &'a LegacyInvocationFixtureRow<'a>,
) -> LegacyInvocationInsert<'a> {
    let (model_name, provider_index, success, exit_code, error_category, created_at) = *row;
    LegacyInvocationInsert {
        model_name,
        provider_index,
        success,
        exit_code,
        error_category,
        created_at,
    }
}

fn insert_legacy_invocation_row(conn: &sqlite::Connection, row: &LegacyInvocationInsert<'_>) {
    conn.execute(
        legacy_invocation_insert_sql(),
        sqlite::params![
            row.model_name,
            row.provider_index,
            row.success,
            row.exit_code,
            row.error_category,
            row.created_at
        ],
    )
    .unwrap();
}

fn legacy_invocation_insert_sql() -> &'static str {
    "INSERT INTO invocations (model_name, provider_index, success, exit_code, error_category, created_at)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
}

pub(in crate::db::tests) struct ProviderMigrationInvocationFixture<'a> {
    pub(in crate::db::tests) model_name: &'a str,
    pub(in crate::db::tests) provider_name: Option<&'a str>,
    pub(in crate::db::tests) provider_index: i64,
    pub(in crate::db::tests) status: &'a str,
    pub(in crate::db::tests) success: Option<i64>,
    pub(in crate::db::tests) exit_code: Option<i64>,
    pub(in crate::db::tests) error_category: Option<&'a str>,
    pub(in crate::db::tests) created_at: &'a str,
    pub(in crate::db::tests) finished_at: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::db::tests) struct ProviderAggregateSnapshot {
    pub(in crate::db::tests) model_name: String,
    pub(in crate::db::tests) provider_name: String,
    pub(in crate::db::tests) invocation_count: i64,
    pub(in crate::db::tests) error_count: i64,
    pub(in crate::db::tests) last_error: Option<String>,
    pub(in crate::db::tests) last_error_at: Option<String>,
    pub(in crate::db::tests) last_invoked_at: Option<String>,
}

pub(in crate::db::tests) fn legacy_providers_db(
    rows: &[ProviderMigrationInvocationFixture<'_>],
) -> TempDir {
    let (dir, conn) = temp_state_connection();
    create_current_invocations_table(&conn);
    create_legacy_providers_table(&conn);
    seed_stale_provider_aggregate(&conn);
    insert_provider_migration_invocations(&conn, rows);
    mark_current_schema_version(&conn);
    dir
}

fn create_current_invocations_table(conn: &sqlite::Connection) {
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
}

fn create_legacy_providers_table(conn: &sqlite::Connection) {
    conn.execute_batch(legacy_providers_schema_sql()).unwrap();
}

fn legacy_providers_schema_sql() -> &'static str {
    "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );"
}

fn seed_stale_provider_aggregate(conn: &sqlite::Connection) {
    conn.execute_batch(stale_provider_aggregate_sql()).unwrap();
}

fn stale_provider_aggregate_sql() -> &'static str {
    "INSERT INTO providers (
                model_name, provider_index, invocation_count, error_count,
                last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', 0, 99, 88,
                'stale-index-aggregate', '2026-04-01T00:00:00+00:00',
                '2026-04-01T00:00:00+00:00'
            );"
}

fn insert_provider_migration_invocations(
    conn: &sqlite::Connection,
    rows: &[ProviderMigrationInvocationFixture<'_>],
) {
    for row in rows {
        insert_provider_migration_invocation(conn, row);
    }
}

fn insert_provider_migration_invocation(
    conn: &sqlite::Connection,
    row: &ProviderMigrationInvocationFixture<'_>,
) {
    conn.execute(
        provider_migration_invocation_insert_sql(),
        sqlite::params![
            Uuid::new_v4().to_string(),
            row.model_name,
            row.provider_name,
            row.provider_index,
            row.status,
            row.success,
            row.exit_code,
            row.error_category,
            row.created_at,
            row.finished_at,
        ],
    )
    .unwrap();
}

fn provider_migration_invocation_insert_sql() -> &'static str {
    "INSERT INTO invocations (
            invocation_uuid, model_name, provider_name, provider_index,
            status, success, exit_code, error_category, created_at, finished_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
}
