//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, mapper, orchestration }

use super::super::*;
use super::*;
pub(in crate::db::tests) type LegacyInvocationFixtureRow<'a> =
    (&'a str, i64, i64, i64, Option<&'a str>, &'a str);

pub(in crate::db::tests) fn legacy_invocations_db(
    rows: &[LegacyInvocationFixtureRow<'_>],
) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                success INTEGER NOT NULL,
                exit_code INTEGER NOT NULL,
                error_category TEXT,
                created_at TEXT NOT NULL
            );",
    )
    .unwrap();
    for (model_name, provider_index, success, exit_code, error_category, created_at) in rows {
        conn.execute(
                "INSERT INTO invocations (model_name, provider_index, success, exit_code, error_category, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                sqlite::params![
                    model_name,
                    provider_index,
                    success,
                    exit_code,
                    error_category,
                    created_at
                ],
            )
            .unwrap();
    }
    mark_current_schema_version(&conn);
    dir
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );
            INSERT INTO providers (
                model_name, provider_index, invocation_count, error_count,
                last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', 0, 99, 88,
                'stale-index-aggregate', '2026-04-01T00:00:00+00:00',
                '2026-04-01T00:00:00+00:00'
            );",
    )
    .unwrap();
    for row in rows {
        conn.execute(
            "INSERT INTO invocations (
                    invocation_uuid, model_name, provider_name, provider_index,
                    status, success, exit_code, error_category, created_at, finished_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
    mark_current_schema_version(&conn);
    dir
}
