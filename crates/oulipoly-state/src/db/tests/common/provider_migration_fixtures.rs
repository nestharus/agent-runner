//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }

use super::super::*;
use super::*;
pub(in crate::db::tests) fn provider_rebuild_fixture_db() -> TempDir {
    legacy_providers_db(&[
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a2"),
            provider_index: 0,
            status: "succeeded",
            success: Some(1),
            exit_code: Some(0),
            error_category: None,
            created_at: "2026-04-20T10:00:00+00:00",
            finished_at: Some("2026-04-20T10:00:01+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a2"),
            provider_index: 2,
            status: "failed",
            success: Some(0),
            exit_code: Some(1),
            error_category: Some("rate_limit"),
            created_at: "2026-04-20T11:00:00+00:00",
            finished_at: Some("2026-04-20T11:00:01+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 1,
            status: "succeeded",
            success: Some(1),
            exit_code: Some(0),
            error_category: None,
            created_at: "2026-04-20T12:00:00+00:00",
            finished_at: Some("2026-04-20T12:00:01+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: None,
            provider_index: 0,
            status: "succeeded",
            success: Some(1),
            exit_code: Some(0),
            error_category: None,
            created_at: "2026-04-20T13:00:00+00:00",
            finished_at: Some("2026-04-20T13:00:01+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a3"),
            provider_index: 3,
            status: "running",
            success: None,
            exit_code: None,
            error_category: None,
            created_at: "2026-04-20T14:00:00+00:00",
            finished_at: None,
        },
    ])
}

pub(in crate::db::tests) fn provider_last_error_fixture_db() -> TempDir {
    legacy_providers_db(&[
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 0,
            status: "failed",
            success: Some(0),
            exit_code: Some(1),
            error_category: Some("rate_limit"),
            created_at: "2026-04-20T10:00:00+00:00",
            finished_at: Some("2026-04-20T10:00:10+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 0,
            status: "succeeded",
            success: Some(1),
            exit_code: Some(0),
            error_category: None,
            created_at: "2026-04-20T11:00:00+00:00",
            finished_at: Some("2026-04-20T11:00:10+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 0,
            status: "failed",
            success: Some(0),
            exit_code: Some(1),
            error_category: Some("auth_error"),
            created_at: "2026-04-20T10:30:00+00:00",
            finished_at: Some("2026-04-20T10:30:10+00:00"),
        },
    ])
}

pub(in crate::db::tests) fn provider_last_error_tie_fixture_db() -> TempDir {
    legacy_providers_db(&[
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 0,
            status: "failed",
            success: Some(0),
            exit_code: Some(1),
            error_category: Some("rate_limit"),
            created_at: "2026-04-20T10:00:00+00:00",
            finished_at: Some("2026-04-20T10:00:10+00:00"),
        },
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("provider-a"),
            provider_index: 0,
            status: "failed",
            success: Some(0),
            exit_code: Some(1),
            error_category: Some("auth_error"),
            created_at: "2026-04-20T10:00:01+00:00",
            finished_at: Some("2026-04-20T10:00:10+00:00"),
        },
    ])
}

pub(in crate::db::tests) fn malformed_providers_shape_db() -> TempDir {
    let (dir, conn) = malformed_provider_fixture_connection();
    create_current_invocations_table_for_provider_fixture(&conn);
    create_malformed_providers_shape_table(&conn);
    insert_malformed_providers_shape_invocation(&conn);
    mark_current_schema_version(&conn);
    dir
}

fn malformed_provider_fixture_connection() -> (TempDir, sqlite::Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    (dir, conn)
}

fn create_current_invocations_table_for_provider_fixture(conn: &sqlite::Connection) {
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
}

fn create_malformed_providers_shape_table(conn: &sqlite::Connection) {
    conn.execute_batch(malformed_providers_shape_sql()).unwrap();
}

fn malformed_providers_shape_sql() -> &'static str {
    "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                provider_name TEXT NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );
            INSERT INTO providers (
                model_name, provider_index, provider_name,
                invocation_count, error_count, last_error, last_error_at, last_invoked_at
            ) VALUES (
                'routing-model', 0, 'provider-a', 7, 1,
                'do-not-touch', '2026-04-20T10:00:00+00:00',
                '2026-04-20T10:00:00+00:00'
            );"
}

fn insert_malformed_providers_shape_invocation(conn: &sqlite::Connection) {
    conn.execute(
        malformed_providers_shape_invocation_sql(),
        sqlite::params![Uuid::new_v4().to_string()],
    )
    .unwrap();
}

fn malformed_providers_shape_invocation_sql() -> &'static str {
    "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
             ) VALUES (?1, 'routing-model', 'provider-a', 0, 'failed', 0, 1,
                       'rate_limit', '2026-04-20T10:00:00+00:00',
                       '2026-04-20T10:00:01+00:00')"
}

pub(in crate::db::tests) fn malformed_providers_affinity_db() -> TempDir {
    let (dir, conn) = malformed_provider_fixture_connection();
    create_current_invocations_table_for_provider_fixture(&conn);
    create_malformed_providers_affinity_table(&conn);
    mark_current_schema_version(&conn);
    dir
}

fn create_malformed_providers_affinity_table(conn: &sqlite::Connection) {
    conn.execute_batch(malformed_providers_affinity_sql())
        .unwrap();
}

fn malformed_providers_affinity_sql() -> &'static str {
    "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index TEXT NOT NULL,
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
                'routing-model', '0', 7, 1,
                'do-not-touch', '2026-04-20T10:00:00+00:00',
                '2026-04-20T10:00:00+00:00'
            );"
}

pub(in crate::db::tests) fn legacy_invocations_with_malformed_providers_db() -> TempDir {
    let dir = legacy_invocations_db(&[("routing-model", 0, 0, 1, Some("rate_limit"), "created-a")]);
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    create_malformed_legacy_providers_table(&conn);
    mark_current_schema_version(&conn);
    dir
}

fn create_malformed_legacy_providers_table(conn: &sqlite::Connection) {
    conn.execute_batch(malformed_legacy_providers_sql())
        .unwrap();
}

fn malformed_legacy_providers_sql() -> &'static str {
    "CREATE TABLE providers (
                model_name TEXT NOT NULL,
                provider_index INTEGER NOT NULL,
                provider_name TEXT NOT NULL,
                invocation_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                last_error_at TEXT,
                last_invoked_at TEXT,
                PRIMARY KEY (model_name, provider_index)
            );"
}
