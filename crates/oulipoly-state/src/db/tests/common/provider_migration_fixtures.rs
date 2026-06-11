//! ## Declared roles
//!
//! - accessor
//!
//! Role set: { accessor }

use super::super::*;
use super::*;
pub(in crate::db::tests) fn provider_rebuild_fixture_db() -> TempDir {
    legacy_providers_db(&[
        ProviderMigrationInvocationFixture {
            model_name: "routing-model",
            provider_name: Some("claude2"),
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
            provider_name: Some("claude2"),
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
            provider_name: Some("claude"),
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
            provider_name: Some("claude3"),
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
            provider_name: Some("claude"),
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
            provider_name: Some("claude"),
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
            provider_name: Some("claude"),
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
            provider_name: Some("claude"),
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
            provider_name: Some("claude"),
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
    conn.execute_batch(
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
                'routing-model', 0, 'claude', 7, 1,
                'do-not-touch', '2026-04-20T10:00:00+00:00',
                '2026-04-20T10:00:00+00:00'
            );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
             ) VALUES (?1, 'routing-model', 'claude', 0, 'failed', 0, 1,
                       'rate_limit', '2026-04-20T10:00:00+00:00',
                       '2026-04-20T10:00:01+00:00')",
        sqlite::params![Uuid::new_v4().to_string()],
    )
    .unwrap();
    mark_current_schema_version(&conn);
    dir
}

pub(in crate::db::tests) fn malformed_providers_affinity_db() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
    conn.execute_batch(
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
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    dir
}

pub(in crate::db::tests) fn legacy_invocations_with_malformed_providers_db() -> TempDir {
    let dir = legacy_invocations_db(&[("routing-model", 0, 0, 1, Some("rate_limit"), "created-a")]);
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
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
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    dir
}
