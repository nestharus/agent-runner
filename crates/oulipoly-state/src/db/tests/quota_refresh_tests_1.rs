//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
use super::*;
#[test]
fn upsert_quota_refresh_preserves_windows_on_empty_input() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();
    let before = quota_window_rows(&db, provider);

    db.upsert_quota_refresh(provider, &[]).unwrap();

    assert_eq!(quota_window_rows(&db, provider), before);
}

#[test]
fn provider_quotas_topology_columns_created_and_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL
            );
            CREATE TABLE provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
            VALUES
                ('p', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z'),
                ('empty', 0.00, NULL, 0, '2026-04-21T00:00:00Z');
            INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at)
            VALUES
                ('p', 0, 0.20, '2026-04-22T00:00:00Z'),
                ('p', 1, 0.30, '2026-04-28T00:00:00Z');",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let db = StateDb::open(&path).unwrap();

    let columns = provider_quota_column_names(&db);
    assert!(
        has_provider_quota_column(&columns, "topology_peak_live_window_count"),
        "provider_quotas topology peak column missing after migration: {columns:?}"
    );
    assert!(
        has_provider_quota_column(&columns, "last_topology_probe_at"),
        "provider_quotas probe timestamp column missing after migration: {columns:?}"
    );

    let quota = db.get_quota("p").unwrap().unwrap();
    assert_eq!(quota.topology_peak_live_window_count, 2);
    assert!(quota.last_topology_probe_at.is_none());

    let empty_quota = db.get_quota("empty").unwrap().unwrap();
    assert_eq!(empty_quota.topology_peak_live_window_count, 0);
    assert!(empty_quota.last_topology_probe_at.is_none());
}

fn provider_quota_column_names(db: &StateDb) -> Vec<String> {
    db.conn
        .prepare("PRAGMA table_info(provider_quotas)")
        .unwrap()
        .query_map([], provider_quota_column_name_row)
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn provider_quota_column_name_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
    row.get(1)
}

fn has_provider_quota_column(columns: &[String], expected: &str) -> bool {
    columns.iter().any(|column| column == expected)
}

#[test]
fn provider_quotas_topology_backfill_recovers_when_column_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
            "CREATE TABLE provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL,
                topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, topology_peak_live_window_count)
            VALUES
                ('p', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z', 0),
                ('already-high', 0.20, '2026-04-28T00:00:00Z', 3, '2026-04-21T00:00:00Z', 4);
            INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at)
            VALUES
                ('p', 0, 0.20, '2026-04-22T00:00:00Z'),
                ('p', 1, 0.30, '2026-04-28T00:00:00Z'),
                ('already-high', 0, 0.20, '2026-04-22T00:00:00Z');",
        )
        .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let db = StateDb::open(&path).unwrap();

    assert_eq!(
        db.get_quota("p")
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        2
    );
    assert_eq!(
        db.get_quota("already-high")
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        4,
        "schema repair must not lower a previously learned topology peak"
    );
}

#[test]
fn upsert_quota_refresh_updates_topology_peak_without_lowering_on_shrink() {
    let db = test_db();
    let provider = "p";

    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();
    assert_eq!(
        db.get_quota(provider)
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        2
    );

    db.upsert_quota_refresh(provider, &[quota_input(0.30, "2026-04-23T12:00:00Z")])
        .unwrap();

    assert_eq!(db.get_windows(provider).unwrap().len(), 1);
    assert_eq!(
        db.get_quota(provider)
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        2,
        "topology peak should preserve the prior complete topology after a non-empty shrink"
    );

    db.upsert_quota_refresh(provider, &[]).unwrap();
    assert_eq!(
        db.get_quota(provider)
            .unwrap()
            .unwrap()
            .topology_peak_live_window_count,
        2,
        "empty refreshes should not lower topology peak"
    );
}

#[test]
fn get_quota_rejects_negative_topology_peak_count() {
    let db = test_db();

    db.conn
        .execute(
            "INSERT INTO provider_quotas
                    (provider_name, topology_peak_live_window_count)
                 VALUES (?1, ?2)",
            sqlite::params!["p", -1],
        )
        .unwrap();

    let error = db.get_quota("p").unwrap_err();

    assert!(
        error.contains("negative topology_peak_live_window_count"),
        "unexpected error: {error}"
    );
}
