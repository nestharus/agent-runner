//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn providers_migration_rejects_table_with_foreign_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
    conn.execute_batch(
        "CREATE TABLE models (
                 name TEXT NOT NULL PRIMARY KEY
             );
             INSERT INTO models (name) VALUES ('routing-model');
             CREATE TABLE providers (
                 model_name TEXT NOT NULL REFERENCES models(name),
                 provider_index INTEGER NOT NULL,
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
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("providers with foreign keys should fail StateDb::open"),
        Err(err) => err,
    };
    assert!(
        err.contains("foreign-key constraints present"),
        "foreign-key rejection should name foreign keys; got {err}"
    );
}

#[test]
fn providers_preflight_rejects_malformed_shape_before_invocations_migration() {
    let dir = legacy_invocations_with_malformed_providers_db();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    let invocations_before = legacy_invocations_snapshot(&conn);
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("malformed providers shape should fail before invocations migration"),
        Err(err) => err,
    };

    assert!(
        err.contains("Unexpected providers schema shape"),
        "unexpected-shape error should come from providers preflight; got {err}"
    );

    let conn = sqlite::Connection::open(&path).unwrap();
    assert_eq!(legacy_invocations_snapshot(&conn), invocations_before);
}

#[test]
fn providers_migration_is_idempotent_across_reopens() {
    let dir = provider_rebuild_fixture_db();
    let path = dir.path().join("state.db");

    let first = StateDb::open(&path).unwrap();
    let first_rows = provider_aggregate_snapshot(&first.conn);
    drop(first);

    let second = StateDb::open(&path).unwrap();
    let second_rows = provider_aggregate_snapshot(&second.conn);

    assert_eq!(second_rows, first_rows);
}

#[test]
fn providers_migration_last_error_at_uses_most_recent_failure_not_later_success() {
    let dir = provider_last_error_fixture_db();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();
    let rows = provider_aggregate_snapshot(&db.conn);

    assert_eq!(
        rows,
        vec![ProviderAggregateSnapshot {
            model_name: "routing-model".to_string(),
            provider_name: "claude".to_string(),
            invocation_count: 3,
            error_count: 2,
            last_error: Some("auth_error".to_string()),
            last_error_at: Some("2026-04-20T10:30:10+00:00".to_string()),
            last_invoked_at: Some("2026-04-20T11:00:10+00:00".to_string()),
        }]
    );
}

#[test]
fn providers_migration_last_error_ties_use_highest_invocation_id() {
    let dir = provider_last_error_tie_fixture_db();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();
    let rows = provider_aggregate_snapshot(&db.conn);

    assert_eq!(
        rows,
        vec![ProviderAggregateSnapshot {
            model_name: "routing-model".to_string(),
            provider_name: "claude".to_string(),
            invocation_count: 2,
            error_count: 2,
            last_error: Some("auth_error".to_string()),
            last_error_at: Some("2026-04-20T10:00:10+00:00".to_string()),
            last_invoked_at: Some("2026-04-20T10:00:10+00:00".to_string()),
        }]
    );
}
