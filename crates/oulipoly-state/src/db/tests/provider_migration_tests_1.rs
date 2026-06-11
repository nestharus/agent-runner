//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn quota_tight_routing_column_dropped_after_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER REFERENCES invocations(id),
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                quota_tight_routing BOOLEAN NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let db = StateDb::open(&path).unwrap();

    let columns: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(invocations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        !columns.iter().any(|column| column == "quota_tight_routing"),
        "quota_tight_routing should be removed by migration: {columns:?}"
    );
}

#[test]
fn providers_migration_rebuilds_aggregate_from_invocations_by_provider_name() {
    let dir = provider_rebuild_fixture_db();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();

    let columns = table_columns_with_pk(&db.conn, "providers");
    assert!(
        columns
            .iter()
            .any(|(name, pk)| name == "provider_name" && *pk == 2),
        "providers must be keyed by provider_name after migration: {columns:?}"
    );
    assert!(
        !columns.iter().any(|(name, _)| name == "provider_index"),
        "providers.provider_index must be removed after migration: {columns:?}"
    );

    let rows = provider_aggregate_snapshot(&db.conn);
    assert_eq!(
        rows,
        vec![
            ProviderAggregateSnapshot {
                model_name: "routing-model".to_string(),
                provider_name: "claude".to_string(),
                invocation_count: 1,
                error_count: 0,
                last_error: None,
                last_error_at: None,
                last_invoked_at: Some("2026-04-20T12:00:01+00:00".to_string()),
            },
            ProviderAggregateSnapshot {
                model_name: "routing-model".to_string(),
                provider_name: "claude2".to_string(),
                invocation_count: 2,
                error_count: 1,
                last_error: Some("rate_limit".to_string()),
                last_error_at: Some("2026-04-20T11:00:01+00:00".to_string()),
                last_invoked_at: Some("2026-04-20T11:00:01+00:00".to_string()),
            },
        ]
    );
}

#[test]
fn quota_schema_remains_name_keyed_after_provider_migration() {
    let dir = provider_rebuild_fixture_db();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();

    let quota_columns = table_columns_with_pk(&db.conn, "provider_quotas");
    assert!(
        quota_columns
            .iter()
            .any(|(name, pk)| name == "provider_name" && *pk == 1),
        "provider_quotas must remain keyed only by provider_name: {quota_columns:?}"
    );
    assert!(
        !quota_columns
            .iter()
            .any(|(name, _)| name == "model_name" || name == "provider_index"),
        "provider_quotas must not gain aggregate identity columns: {quota_columns:?}"
    );

    let window_columns = table_columns_with_pk(&db.conn, "provider_quota_windows");
    assert!(
        window_columns
            .iter()
            .any(|(name, pk)| name == "provider_name" && *pk == 1),
        "provider_quota_windows must remain provider-name keyed: {window_columns:?}"
    );
    assert!(
        !window_columns
            .iter()
            .any(|(name, _)| name == "model_name" || name == "provider_index"),
        "provider_quota_windows must not gain aggregate identity columns: {window_columns:?}"
    );
}

#[test]
fn providers_migration_rejects_unexpected_shape_without_mutating_source_tables() {
    let dir = malformed_providers_shape_db();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    let providers_before = malformed_providers_snapshot(&conn);
    let invocations_before = invocations_snapshot(&conn);
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("unexpected providers shape should fail StateDb::open"),
        Err(err) => err,
    };
    let err_lower = err.to_ascii_lowercase();
    assert!(
        err_lower.contains("providers") && err_lower.contains("unexpected"),
        "unexpected-shape error should name providers and unexpected shape; got {err}"
    );

    let conn = sqlite::Connection::open(&path).unwrap();
    assert_eq!(malformed_providers_snapshot(&conn), providers_before);
    assert_eq!(invocations_snapshot(&conn), invocations_before);
    conn.execute_batch("DROP TABLE providers").unwrap();
    drop(conn);

    let recovered = StateDb::open(&path).unwrap();
    let columns = table_columns_with_pk(&recovered.conn, "providers");
    assert!(
        columns
            .iter()
            .any(|(name, pk)| name == "provider_name" && *pk == 2),
        "operator cleanup should let missing-table branch create post-fix providers: {columns:?}"
    );
    assert!(
        !columns.iter().any(|(name, _)| name == "provider_index"),
        "operator cleanup must not recreate provider_index: {columns:?}"
    );
}

#[test]
fn providers_migration_rejects_wrong_affinity_shape() {
    let dir = malformed_providers_affinity_db();
    let path = dir.path().join("state.db");

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("wrong providers affinity should fail StateDb::open"),
        Err(err) => err,
    };

    assert!(
        err.contains("provider_index(type=TEXT"),
        "unexpected-shape error should describe the wrong affinity; got {err}"
    );
}

#[test]
fn providers_migration_rejects_non_table_object_named_providers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(StateDb::invocations_schema_sql())
        .unwrap();
    // SQLite shares table/view namespace; create a VIEW named providers.
    conn.execute_batch(
        "CREATE TABLE providers_source (
                 model_name TEXT NOT NULL,
                 provider_name TEXT NOT NULL,
                 invocation_count INTEGER NOT NULL DEFAULT 0,
                 error_count INTEGER NOT NULL DEFAULT 0,
                 last_error TEXT,
                 last_error_at TEXT,
                 last_invoked_at TEXT,
                 PRIMARY KEY (model_name, provider_name)
             );
             CREATE VIEW providers AS
                 SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                   FROM providers_source;",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let err = match StateDb::open(&path) {
        Ok(_) => panic!("non-table object named providers should fail StateDb::open"),
        Err(err) => err,
    };
    assert!(
        err.contains("object type=view"),
        "object-type rejection should name the unexpected type; got {err}"
    );

    let conn = sqlite::Connection::open(&path).unwrap();
    let mut stmt = conn
        .prepare("SELECT type FROM sqlite_master WHERE name = 'providers'")
        .unwrap();
    let observed_type: String = stmt
        .query_row([], |row| row.get(0))
        .expect("providers object should still exist after rejected open");
    assert_eq!(
        observed_type, "view",
        "rejected open must not mutate the providers object"
    );
}
