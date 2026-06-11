use super::opening::{classify_read_only_open_error, shm_path, wal_path};
use super::*;
use crate::test_support::env_lock;
use tempfile::TempDir;
use uuid::Uuid;

mod failing_migration {
    include!("../../tests/fixtures/failing_migration.rs");
}

fn test_db() -> StateDb {
    StateDb::open(Path::new(":memory:")).unwrap()
}

#[test]
fn state_db_open_sets_busy_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");

    let db = StateDb::open(&path).unwrap();
    let busy_timeout = db
        .connection()
        .query_row("PRAGMA busy_timeout", [], |row| row.get::<_, i64>(0))
        .unwrap();

    assert!(
        busy_timeout >= 5000,
        "StateDb::open should configure busy_timeout >= 5000ms, got {busy_timeout}ms"
    );
}

fn mark_current_schema_version(conn: &sqlite::Connection) {
    seed_current_drift_required_tables(conn);
    conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)
        .unwrap();
}

fn seed_current_drift_required_tables(conn: &sqlite::Connection) {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS invocations (
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
                terminal_reason TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                provider_session_id TEXT,
                resume_input_id TEXT,
                provider_session_capture_method TEXT,
                provider_session_resolved_account TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT,
                row_version INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS provider_quotas (
                provider_name TEXT PRIMARY KEY,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT,
                calls_since_refresh INTEGER NOT NULL DEFAULT 0,
                refreshed_at TEXT,
                last_empty_refresh_at TEXT,
                exhausted_at TEXT NULL,
                topology_peak_live_window_count INTEGER NOT NULL DEFAULT 0,
                last_topology_probe_at TEXT
            );
            CREATE TABLE IF NOT EXISTS provider_quota_windows (
                provider_name TEXT NOT NULL,
                window_id INTEGER NOT NULL,
                used_percent REAL NOT NULL DEFAULT 0,
                resets_at TEXT NOT NULL,
                last_delta_percent REAL,
                last_delta_calls INTEGER,
                PRIMARY KEY (provider_name, window_id)
            );
            CREATE TABLE IF NOT EXISTS session_turns (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                turn_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                role TEXT NOT NULL,
                parent_turn_id TEXT,
                is_sidechain INTEGER NOT NULL DEFAULT 0,
                is_compaction_boundary INTEGER NOT NULL DEFAULT 0,
                source_file TEXT NOT NULL,
                ingested_at TEXT NOT NULL,
                body TEXT,
                UNIQUE (provider_name, session_id, turn_id)
            );
            CREATE TABLE IF NOT EXISTS session_chains (
                chain_id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                last_used_at TEXT NOT NULL,
                model_name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS session_chain_segments (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
                provider_name TEXT NOT NULL,
                session_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT,
                last_turn_id TEXT,
                transition_reason TEXT NOT NULL CHECK (transition_reason IN
                    ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
                UNIQUE(chain_id, provider_name, session_id)
            );",
    )
    .unwrap();
}

fn db_without_table(table: &str) -> StateDb {
    let db = test_db();
    db.conn
        .execute_batch(&format!("DROP TABLE {table};"))
        .unwrap();
    db
}

fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn quota_input(used_percent: f64, resets_at: &str) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent,
        resets_at: ts(resets_at),
    }
}

fn quota_window_rows(db: &StateDb, provider_name: &str) -> Vec<(u32, f64, String)> {
    db.get_windows(provider_name)
        .unwrap()
        .into_iter()
        .map(|window| {
            (
                window.window_id,
                window.used_percent,
                window.resets_at.to_rfc3339(),
            )
        })
        .collect()
}

type QuotaWindowDetailRow = (u32, f64, String, Option<f64>, Option<u64>);

fn quota_window_detail_rows(db: &StateDb, provider_name: &str) -> Vec<QuotaWindowDetailRow> {
    db.get_windows(provider_name)
        .unwrap()
        .into_iter()
        .map(|window| {
            (
                window.window_id,
                window.used_percent,
                window.resets_at.to_rfc3339(),
                window.last_delta_percent,
                window.last_delta_calls,
            )
        })
        .collect()
}

fn insert_assistant_turns_after(
    db: &StateDb,
    provider_name: &str,
    since: DateTime<Utc>,
    count: usize,
    id_prefix: &str,
) {
    let turns: Vec<_> = (0..count)
        .map(|i| SessionTurnIngest {
            session_id: format!("{id_prefix}-session"),
            turn_id: format!("{id_prefix}-turn-{i}"),
            timestamp: since + chrono::Duration::seconds((i + 1) as i64),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        })
        .collect();
    db.ingest_session_turns_batch(provider_name, &turns)
        .unwrap();
}

fn last_empty_refresh_at(db: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
    db.conn
        .query_row(
            "SELECT last_empty_refresh_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .unwrap()
                .with_timezone(&Utc)
        })
}

fn last_topology_probe_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
    db.conn
        .query_row(
            "SELECT last_topology_probe_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
}

fn calls_since_refresh(db: &StateDb, provider_name: &str) -> u64 {
    db.conn
        .query_row(
            "SELECT calls_since_refresh
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap() as u64
}

fn exhausted_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
    db.conn
        .query_row(
            "SELECT exhausted_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
}

fn exhausted_at(db: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
    exhausted_at_raw(db, provider_name).map(|value| {
        DateTime::parse_from_rfc3339(&value)
            .unwrap()
            .with_timezone(&Utc)
    })
}

fn insert_invocation_fixture(
    db: &StateDb,
    invocation_uuid: &str,
    parent_invocation_id: Option<i64>,
    created_at: &str,
) -> i64 {
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "fixture-model".to_string(),
            provider_name: "fixture-provider".to_string(),
            provider_index: 0,
            parent_invocation_id,
        })
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET created_at = ?1 WHERE id = ?2",
            sqlite::params![created_at, id],
        )
        .unwrap();
    id
}

fn seed_running_invocation(db: &StateDb) -> i64 {
    db.start_invocation(&InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    })
    .unwrap()
}

fn record_provider_invocation(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    provider_index: usize,
    success: bool,
    error_category: Option<&str>,
    stderr_snippet: Option<&str>,
) -> i64 {
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        })
        .unwrap();
    db.finalize_invocation(
        id,
        success,
        if success { 0 } else { 1 },
        error_category,
        stderr_snippet,
    )
    .unwrap();
    id
}

fn with_models_config(model_name: &str, body: &str, test: impl FnOnce()) {
    let _guard = env_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let app_dir = dir.path().join("oulipoly-agent-runner");
    let models_dir = app_dir.join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(models_dir.join(format!("{model_name}.toml")), body).unwrap();

    let old = std::env::var_os("XDG_CONFIG_HOME");
    // Tests need to isolate config-driven provider-name resolution.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
    match old {
        Some(value) => unsafe {
            std::env::set_var("XDG_CONFIG_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        },
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

type LegacyInvocationFixtureRow<'a> = (&'a str, i64, i64, i64, Option<&'a str>, &'a str);

fn legacy_invocations_db(rows: &[LegacyInvocationFixtureRow<'_>]) -> TempDir {
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

struct ProviderMigrationInvocationFixture<'a> {
    model_name: &'a str,
    provider_name: Option<&'a str>,
    provider_index: i64,
    status: &'a str,
    success: Option<i64>,
    exit_code: Option<i64>,
    error_category: Option<&'a str>,
    created_at: &'a str,
    finished_at: Option<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
struct ProviderAggregateSnapshot {
    model_name: String,
    provider_name: String,
    invocation_count: i64,
    error_count: i64,
    last_error: Option<String>,
    last_error_at: Option<String>,
    last_invoked_at: Option<String>,
}

fn legacy_providers_db(rows: &[ProviderMigrationInvocationFixture<'_>]) -> TempDir {
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

fn provider_rebuild_fixture_db() -> TempDir {
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

fn provider_last_error_fixture_db() -> TempDir {
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

fn provider_last_error_tie_fixture_db() -> TempDir {
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

fn malformed_providers_shape_db() -> TempDir {
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

fn malformed_providers_affinity_db() -> TempDir {
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

fn legacy_invocations_with_malformed_providers_db() -> TempDir {
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

fn table_columns_with_pk(conn: &sqlite::Connection, table_name: &str) -> Vec<(String, i64)> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table_name})"))
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
        })
        .unwrap();
    rows.map(|row| row.unwrap()).collect()
}

fn provider_aggregate_snapshot(conn: &sqlite::Connection) -> Vec<ProviderAggregateSnapshot> {
    let mut stmt = conn
        .prepare(
            "SELECT model_name, provider_name, invocation_count, error_count,
                        last_error, last_error_at, last_invoked_at
                   FROM providers
                  ORDER BY model_name, provider_name",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            Ok(ProviderAggregateSnapshot {
                model_name: row.get(0)?,
                provider_name: row.get(1)?,
                invocation_count: row.get(2)?,
                error_count: row.get(3)?,
                last_error: row.get(4)?,
                last_error_at: row.get(5)?,
                last_invoked_at: row.get(6)?,
            })
        })
        .unwrap();
    rows.map(|row| row.unwrap()).collect()
}

fn quoted_snapshot(conn: &sqlite::Connection, schema_sql: &str, rows_sql: &str) -> Vec<String> {
    let mut snapshot = Vec::new();
    snapshot.push(
        conn.query_row(schema_sql, [], |row| row.get::<_, String>(0))
            .unwrap(),
    );
    let mut stmt = conn.prepare(rows_sql).unwrap();
    let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
    snapshot.extend(rows.map(|row| row.unwrap()));
    snapshot
}

fn malformed_providers_snapshot(conn: &sqlite::Connection) -> Vec<String> {
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

fn invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
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

fn legacy_invocations_snapshot(conn: &sqlite::Connection) -> Vec<String> {
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

fn legacy_session_turns_db() -> TempDir {
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

fn invocation_table_sql(db: &StateDb) -> String {
    db.conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'invocations'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
}

fn invocation_columns(db: &StateDb) -> Vec<String> {
    db.conn
        .prepare("PRAGMA table_info(invocations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn mark_exhausted_writes_timestamp_on_existing_quota_row() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();

    let before = Utc::now();
    db.mark_exhausted(provider).unwrap();
    let after = Utc::now();

    let exhausted = exhausted_at(&db, provider).expect("exhausted_at should be set");
    assert!(
        exhausted >= before - chrono::Duration::seconds(1)
            && exhausted <= after + chrono::Duration::seconds(1),
        "exhausted_at {exhausted} should be near mark_exhausted call"
    );
}

#[test]
fn mark_exhausted_creates_row_when_missing() {
    // CodeRabbit pass 1 finding: a plain UPDATE silently dropped the
    // write when a provider had no quota row yet (e.g. misconfigured
    // quota_script that only ever fails, or first-call quota rejection
    // before any refresh succeeded). mark_exhausted must upsert so the
    // flag always lands — otherwise the balancer routes to a known-bad
    // account on the next invocation and we get a guaranteed
    // re-failure that the reactive model is meant to prevent.
    let db = test_db();
    let provider = "never-refreshed";

    let before = Utc::now();
    db.mark_exhausted(provider).unwrap();
    let after = Utc::now();

    let row_count: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM provider_quotas WHERE provider_name = ?1",
            sqlite::params![provider],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1, "mark_exhausted must upsert the quota row");

    let exhausted = exhausted_at(&db, provider).expect("exhausted_at set");
    assert!(
        exhausted >= before - chrono::Duration::seconds(1)
            && exhausted <= after + chrono::Duration::seconds(1)
    );
}

#[test]
fn clear_exhausted_nulls_the_flag() {
    let db = test_db();
    let provider = "a";

    db.mark_exhausted(provider).unwrap();
    assert!(exhausted_at_raw(&db, provider).is_some());

    db.clear_exhausted(provider).unwrap();
    assert_eq!(exhausted_at_raw(&db, provider), None);

    db.clear_exhausted(provider).unwrap();
    assert_eq!(exhausted_at_raw(&db, provider), None);

    db.clear_exhausted("nonexistent-provider").unwrap();
}

#[test]
fn record_provider_unavailable_writes_and_round_trips_next_available_at() {
    let db = test_db();
    let provider = "wu-a1-record";
    let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:23:45Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts), "RollingWindow5h")
        .unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.next_available_at, Some(ts));
    assert_eq!(quota.failure_class.as_deref(), Some("RollingWindow5h"));
}

#[test]
fn record_provider_unavailable_idempotent_under_repeat_calls() {
    let db = test_db();
    let provider = "wu-a1-repeat";
    let ts1 = chrono::DateTime::parse_from_rfc3339("2026-05-21T01:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let ts2 = chrono::DateTime::parse_from_rfc3339("2026-05-21T02:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts1), "RollingWindow5h")
        .unwrap();
    db.record_provider_unavailable(provider, Some(ts2), "WeeklyOrLonger")
        .unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.next_available_at, Some(ts2));
    assert_eq!(quota.failure_class.as_deref(), Some("WeeklyOrLonger"));
}

#[test]
fn touch_provider_refresh_updates_last_refresh_at_only() {
    let db = test_db();
    let provider = "wu-a1-touch";
    let now = chrono::DateTime::parse_from_rfc3339("2026-05-21T03:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.touch_provider_refresh(provider, now).unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row written");
    assert_eq!(quota.last_refresh_at, Some(now));
    assert_eq!(quota.next_available_at, None);
    assert_eq!(quota.failure_class, None);
}

#[test]
fn next_round_robin_index_for_model_returns_none_on_unknown_model() {
    let db = test_db();
    assert_eq!(db.next_round_robin_index_for_model("nope").unwrap(), None);
}

#[test]
fn advance_round_robin_index_persists_across_db_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let now = Utc::now();
    {
        let db = StateDb::open(&path).unwrap();
        db.advance_round_robin_index("claude-opus", 2, now).unwrap();
    }
    let db = StateDb::open(&path).unwrap();
    assert_eq!(
        db.next_round_robin_index_for_model("claude-opus").unwrap(),
        Some(2)
    );

    db.advance_round_robin_index("claude-opus", 5, now).unwrap();
    assert_eq!(
        db.next_round_robin_index_for_model("claude-opus").unwrap(),
        Some(5)
    );
}

#[test]
fn clear_provider_unavailable_nulls_next_available_at_and_failure_class() {
    let db = test_db();
    let provider = "wu-a1-clear";
    let ts = chrono::DateTime::parse_from_rfc3339("2026-05-21T04:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    db.record_provider_unavailable(provider, Some(ts), "UpstreamApiDown")
        .unwrap();
    db.clear_provider_unavailable(provider).unwrap();

    let quota = db.get_quota(provider).unwrap().expect("row exists");
    assert_eq!(quota.next_available_at, None);
    assert_eq!(quota.failure_class, None);
}

#[test]
fn upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.mark_exhausted(provider).unwrap();
    assert!(exhausted_at_raw(&db, provider).is_some());

    db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-23T00:00:00Z")])
        .unwrap();

    assert_eq!(exhausted_at_raw(&db, provider), None);
}

#[test]
fn upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.mark_exhausted(provider).unwrap();
    let exhausted_before = exhausted_at_raw(&db, provider).expect("exhausted_at should be set");

    db.upsert_quota_refresh(provider, &[]).unwrap();

    assert_eq!(
        exhausted_at_raw(&db, provider).as_deref(),
        Some(exhausted_before.as_str())
    );
}

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

// Risk: Providers migration from pre-fix aggregate shape | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Quota path unchanged regression | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration error contract — unexpected shape rejected | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration error contract rejects malformed provider column metadata | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration error contract — providers as non-table object rejected | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration error contract — providers with foreign keys rejected | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration error contract rejects before source-table mutation | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §2 Migration helper
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

// Risk: Migration ensure_providers_schema is idempotent across reopens | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration last_error_at reflects most recent failed invocation | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
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

// Risk: Migration last_error_at deterministic tie-break | level: particular-integration
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §2 Migration helper
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

#[test]
fn schema_creation() {
    let db = test_db();
    let sql = invocation_table_sql(&db);
    assert!(sql.contains("invocation_uuid TEXT NOT NULL UNIQUE"));
    assert!(sql.contains("provider_name TEXT"));
    assert!(sql.contains("parent_invocation_id INTEGER REFERENCES invocations(id)"));
    assert!(sql.contains(
        "status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy'))"
    ));
    assert!(sql.contains("success INTEGER"));
    assert!(sql.contains("finished_at TEXT"));
    assert!(sql.contains("session_id TEXT"));
    assert!(sql.contains("session_capture_method TEXT"));
    assert!(sql.contains("resume_acceptance_status TEXT"));
    assert!(sql.contains("resume_acceptance_evidence TEXT"));

    let indexes: Vec<String> = db
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'invocations' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
    assert_eq!(
        indexes,
        vec![
            "idx_invocations_parent".to_string(),
            "idx_invocations_provider_created".to_string(),
            "idx_invocations_provider_provider_session".to_string(),
            "idx_invocations_provider_session".to_string(),
            "idx_invocations_uuid".to_string(),
            "sqlite_autoindex_invocations_1".to_string(),
        ]
    );
}

// RISK: fresh schema path could omit terminal_reason (proposal §test-intent "schema cascade tests", assumption A5)
// LEVEL: unit
// SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-FRESH)
#[test]
fn t_schema_fresh_invocations_schema_includes_nullable_terminal_reason() {
    let db = test_db();
    let columns = invocation_columns(&db);

    assert!(
        columns.iter().any(|column| column == "terminal_reason"),
        "fresh invocations schema must expose terminal_reason: {columns:?}"
    );

    let nullable: i64 = db
        .conn
        .query_row(
            "SELECT [notnull] FROM pragma_table_info('invocations') WHERE name = 'terminal_reason'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(nullable, 0, "terminal_reason must be nullable");
}

// RISK: incremental ALTER path could miss terminal_reason or destroy existing invocation data (proposal §test-intent "schema cascade tests", assumption A5)
// LEVEL: unit
// SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-INCREMENTAL)
#[test]
fn t_schema_incremental_adds_terminal_reason_without_losing_rows() {
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
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );
            INSERT INTO invocations (
                invocation_uuid, model_name, provider_name, provider_index,
                status, success, exit_code, error_category, created_at, finished_at
            ) VALUES (
                '11111111-1111-1111-1111-111111111111',
                'fixture-model', 'fixture-provider', 0,
                'failed', 0, 7, 'fixture_error',
                '2026-04-17T08:00:00Z', '2026-04-17T08:00:01Z'
            );",
    )
    .unwrap();
    mark_current_schema_version(&conn);
    drop(conn);

    let db = StateDb::open(&path).unwrap();
    let columns = invocation_columns(&db);
    assert!(
        columns.iter().any(|column| column == "terminal_reason"),
        "incremental migration must add terminal_reason: {columns:?}"
    );

    let row = db
        .get_invocation_by_uuid("11111111-1111-1111-1111-111111111111")
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category.as_deref(), Some("fixture_error"));
    assert_eq!(row.terminal_reason, None);
}

// RISK: legacy rebuild path could omit terminal_reason or synthesize historical terminal meaning (proposal §test-intent "schema cascade tests", assumption A5)
// LEVEL: unit
// SOURCE: contracts/nes-250-contract.md § Test catalog § Schema cascade (T-SCHEMA-LEGACY)
#[test]
fn t_schema_legacy_rebuild_adds_terminal_reason_and_migrates_null() {
    let dir = legacy_invocations_db(&[(
        "mapped-model",
        0,
        0,
        7,
        Some("rate_limit"),
        "2026-04-17T08:00:00Z",
    )]);

    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let columns = invocation_columns(&db);
    assert!(
        columns.iter().any(|column| column == "terminal_reason"),
        "legacy rebuild must add terminal_reason: {columns:?}"
    );

    let terminal_reason: Option<String> = db
        .conn
        .query_row(
            "SELECT terminal_reason FROM invocations WHERE model_name = 'mapped-model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terminal_reason, None);
}

#[test]
fn update_resume_acceptance_persists_status_and_evidence() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_resume_acceptance(id, "accepted", Some("matched session id"))
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.resume_acceptance_status.as_deref(), Some("accepted"));
    assert_eq!(
        row.resume_acceptance_evidence.as_deref(),
        Some("matched session id")
    );
}

#[test]
fn session_turns_schema_creation_includes_sidechain_columns() {
    let db = test_db();
    let sql: String = db
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'session_turns'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(sql.contains("parent_turn_id TEXT"));
    assert!(sql.contains("is_sidechain INTEGER NOT NULL DEFAULT 0"));
    assert!(sql.contains("body TEXT"));
}

#[test]
fn session_turns_schema_migration_adds_parent_and_sidechain_columns() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let columns: Vec<(String, String, i64, Option<String>)> = db
        .conn
        .prepare("PRAGMA table_info(session_turns)")
        .unwrap()
        .query_map([], |row| {
            Ok((row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(columns.iter().any(|column| {
        column.0 == "parent_turn_id" && column.1 == "TEXT" && column.2 == 0 && column.3.is_none()
    }));
    assert!(columns.iter().any(|column| {
        column.0 == "is_sidechain"
            && column.1 == "INTEGER"
            && column.2 == 1
            && column.3.as_deref() == Some("0")
    }));
}

#[test]
fn session_turns_schema_migration_adds_nullable_body_to_legacy_db() {
    // risk: legacy-DB upgrade; level: unit; source: contract §4 T5 / proposal A2,A8.
    let dir = legacy_session_turns_db();
    let path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&path).unwrap();
    conn.execute(
            "INSERT INTO session_turns
                (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
             VALUES ('fixture-provider', 'session-a', 'legacy-turn', '2026-04-17T08:00:00Z', 'assistant', '', '2026-04-17T08:00:00Z')",
            [],
        )
        .unwrap();
    drop(conn);

    let db = StateDb::open(&path).unwrap();

    let session_columns: Vec<(String, String, i64)> = db
        .conn
        .prepare("PRAGMA table_info(session_turns)")
        .unwrap()
        .query_map([], |row| Ok((row.get(1)?, row.get(2)?, row.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        session_columns
            .iter()
            .any(|(name, data_type, notnull)| name == "body"
                && data_type == "TEXT"
                && *notnull == 0),
        "legacy migration must add nullable body TEXT; columns={session_columns:?}"
    );
    let body: Option<String> = db
        .conn
        .query_row(
            "SELECT body FROM session_turns WHERE turn_id = 'legacy-turn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, None);

    let quota_columns: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(provider_quotas)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        quota_columns
            .iter()
            .any(|column| column == "topology_peak_live_window_count"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
    assert!(
        quota_columns
            .iter()
            .any(|column| column == "last_topology_probe_at"),
        "body migration must coexist with WU-13 quota topology migration; columns={quota_columns:?}"
    );
}

#[test]
fn session_turns_schema_creation_includes_resume_lookup_index() {
    let db = test_db();
    let indexes: Vec<String> = db
        .conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must exist on fresh DB bootstrap: {indexes:?}"
    );
}

#[test]
fn session_turns_schema_migration_adds_resume_lookup_index() {
    let dir = legacy_session_turns_db();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let indexes: Vec<String> = db
        .conn
        .prepare(
            "SELECT name FROM sqlite_master
                 WHERE type = 'index' AND tbl_name = 'session_turns'
                 ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();

    assert!(
        indexes.contains(&"idx_session_turns_session_lookup".to_string()),
        "resume lookup index must be added on existing DB open: {indexes:?}"
    );
}

#[test]
fn migration_backfills_resolved_and_legacy_rows() {
    with_models_config(
        "mapped-model",
        r#"
[[providers]]
name = "fixture-provider"
"#,
        || {
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
            let db = StateDb::open(&dir.path().join("state.db")).unwrap();

            let rows: Vec<(String, Option<String>, String, String, String)> = db
                .conn
                .prepare(
                    "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                         FROM invocations ORDER BY created_at",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })
                .unwrap()
                .map(Result::unwrap)
                .collect();

            assert_eq!(rows[0].0, "mapped-model");
            assert_eq!(rows[0].1.as_deref(), Some("fixture-provider"));
            assert_eq!(rows[0].2, "succeeded");
            assert_eq!(rows[0].4, "2026-04-17T08:00:00Z");
            assert!(Uuid::parse_str(&rows[0].3).is_ok());

            assert_eq!(rows[1].0, "missing-model");
            assert_eq!(rows[1].1, None);
            assert_eq!(rows[1].2, "legacy");
            assert_eq!(rows[1].4, "2026-04-17T08:05:00Z");
            assert!(Uuid::parse_str(&rows[1].3).is_ok());
        },
    );
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
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(invocations)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
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
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap();
    assert_eq!(row_count, 1);
}

/// Per V10 (failures observable, never silent): if the models config
/// is unloadable mid-migration, the rebuild must still succeed and
/// degrade rows to `status='legacy'` / `provider_name=NULL`. Opening
/// the DB MUST NOT fail just because the config is corrupt.
#[test]
fn migration_succeeds_with_corrupt_models_config_and_marks_rows_legacy() {
    let _guard = env_lock().lock().unwrap();
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

    // Plant a corrupt models/ directory at XDG_CONFIG_HOME so the
    // load_models() call inside migration fails.
    let config_root = dir.path().join("oulipoly-agent-runner");
    let models_dir = config_root.join("models");
    std::fs::create_dir_all(&models_dir).unwrap();
    std::fs::write(
        models_dir.join("broken.toml"),
        "this = is = not = valid = toml",
    )
    .unwrap();

    let old = std::env::var_os("XDG_CONFIG_HOME");
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The DB open must succeed despite the corrupt config.
        let db = StateDb::open(&path).expect("DB open must not fail on corrupt models config");

        // Verify both legacy rows migrated cleanly with provider_name=NULL
        // and status='legacy' since the lookup couldn't resolve anything.
        let conn = sqlite::Connection::open(&path).unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT model_name, provider_name, status, invocation_uuid, finished_at
                     FROM invocations ORDER BY created_at",
            )
            .unwrap();
        let rows: Vec<(String, Option<String>, String, String, String)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(rows.len(), 2);
        for r in &rows {
            assert!(
                r.1.is_none(),
                "provider_name must be NULL on corrupt config"
            );
            assert_eq!(r.2, "legacy", "status must be legacy on corrupt config");
            assert!(Uuid::parse_str(&r.3).is_ok());
            assert!(!r.4.is_empty(), "finished_at must be backfilled");
        }
        drop(db);
    }));
    match old {
        Some(value) => unsafe {
            std::env::set_var("XDG_CONFIG_HOME", value);
        },
        None => unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        },
    }
    if let Err(payload) = result {
        std::panic::resume_unwind(payload);
    }
}

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

/// Risk: Migration might omit columns or leave legacy rows with no usable peak count.
/// Level: particular-integration.
/// Source: proposal §Test-intent track row 10; Assumption A6.
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

    let columns: Vec<String> = db
        .conn
        .prepare("PRAGMA table_info(provider_quotas)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(
        columns
            .iter()
            .any(|column| column == "topology_peak_live_window_count"),
        "provider_quotas topology peak column missing after migration: {columns:?}"
    );
    assert!(
        columns
            .iter()
            .any(|column| column == "last_topology_probe_at"),
        "provider_quotas probe timestamp column missing after migration: {columns:?}"
    );

    let quota = db.get_quota("p").unwrap().unwrap();
    assert_eq!(quota.topology_peak_live_window_count, 2);
    assert!(quota.last_topology_probe_at.is_none());

    let empty_quota = db.get_quota("empty").unwrap().unwrap();
    assert_eq!(empty_quota.topology_peak_live_window_count, 0);
    assert!(empty_quota.last_topology_probe_at.is_none());
}

/// Risk: Migration backfill could clobber an existing higher
/// `topology_peak_live_window_count` column when a partial legacy
/// row already includes the column without the probe-timestamp
/// column.
/// Level: particular-integration.
/// Source: contract §4 (Migration helper); CodeRabbit pass 1
/// finding R1-F06 (idempotent self-healing backfill).
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

/// Risk: Non-empty incomplete refresh could erase peak topology memory.
/// Level: unit.
/// Source: proposal §Test-intent track row 11; Assumptions A2, A6.
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

/// Risk: Malformed state files could wrap a negative learned topology
/// count into a huge `usize`.
/// Level: unit.
/// Source: CodeRabbit pass 1 finding R1-F03.
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

/// Risk: Cooldown marker could mutate quota windows or reset learning data.
/// Level: unit.
/// Source: proposal §Test-intent track row 12; Assumptions A2, A6.
#[test]
fn record_topology_probe_sets_timestamp_without_changing_windows() {
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
    db.set_window_delta_for_test(provider, 0, 0.01, 40).unwrap();
    db.set_window_delta_for_test(provider, 1, 0.02, 40).unwrap();
    let before_windows = quota_window_detail_rows(&db, provider);
    let before = Utc::now();

    db.record_topology_probe(provider).unwrap();

    let after = Utc::now();
    let probe_at_raw =
        last_topology_probe_at_raw(&db, provider).expect("probe timestamp should be set");
    let probe_at = DateTime::parse_from_rfc3339(&probe_at_raw)
        .unwrap()
        .with_timezone(&Utc);
    assert!(
        probe_at >= before - chrono::Duration::seconds(1)
            && probe_at <= after + chrono::Duration::seconds(1),
        "last_topology_probe_at {probe_at} should be near record_topology_probe call"
    );
    assert_eq!(
        quota_window_detail_rows(&db, provider),
        before_windows,
        "record_topology_probe must not mutate window rows or learning deltas"
    );
}

#[test]
fn upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();

    let replacement = [quota_input(0.30, "2026-04-23T12:00:00Z")];
    db.upsert_quota_refresh(provider, &replacement).unwrap();

    assert_eq!(
        quota_window_rows(&db, provider),
        vec![(0, 0.30, "2026-04-23T12:00:00+00:00".to_string())]
    );
}

#[test]
fn upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();

    let before = Utc::now();
    db.upsert_quota_refresh(provider, &[]).unwrap();
    let after = Utc::now();

    let last_empty = last_empty_refresh_at(&db, provider).unwrap();
    assert!(
        last_empty >= before - chrono::Duration::seconds(1)
            && last_empty <= after + chrono::Duration::seconds(1),
        "last_empty_refresh_at {last_empty} should be near empty refresh"
    );
}

#[test]
fn upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row() {
    let db = test_db();
    let provider = "p";

    db.upsert_quota_refresh(provider, &[]).unwrap();

    let quota = db.get_quota(provider).unwrap().unwrap();
    assert!(quota.refreshed_at.is_some());
    assert!(last_empty_refresh_at(&db, provider).is_some());
    assert!(db.get_windows(provider).unwrap().is_empty());
    assert!(quota.refreshed_at.is_some());
}

#[test]
fn upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();
    for _ in 0..5 {
        db.increment_calls_since_refresh(provider).unwrap();
    }
    assert_eq!(calls_since_refresh(&db, provider), 5);

    db.upsert_quota_refresh(provider, &[]).unwrap();

    assert_eq!(calls_since_refresh(&db, provider), 5);
}

#[test]
fn upsert_quota_refresh_writes_per_window_delta_for_matching_window_id() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.20, "2026-04-22T00:00:00Z"),
            quota_input(0.30, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let refreshed_at = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, refreshed_at, 50, "delta-n1");

    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.38, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let windows = db.get_windows(provider).unwrap();
    assert_eq!(windows.len(), 2);
    assert!((windows[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(windows[0].last_delta_calls, Some(50));
    assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
    assert_eq!(windows[1].last_delta_calls, Some(50));
}

#[test]
fn upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.20, "2026-04-22T00:00:00Z"),
            quota_input(0.30, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let first_refreshed_at = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &first_refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, first_refreshed_at, 50, "delta-n1");
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.38, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let second_refreshed_at = ts("2026-04-21T12:00:00Z");
    db.set_refreshed_at_for_test(provider, &second_refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, second_refreshed_at, 20, "delta-n2");
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.05, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let windows = db.get_windows(provider).unwrap();
    assert_eq!(windows.len(), 2);
    assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
    assert_eq!(windows[1].last_delta_calls, Some(50));
}

#[test]
fn upsert_quota_refresh_rejects_pathological_burn_rate_sample() {
    // Regression: an upstream API spike (used_percent briefly reported as
    // 1.0) paired with a small turn count would previously learn a
    // pathological per-turn rate (~0.05/turn), carry it forward across
    // every subsequent no-change refresh, and permanently project every
    // provider near the ceiling. The sanity cap at
    // MAX_LEARNABLE_BURN_RATE = 0.1/turn rejects this sample and carries
    // the prior learn forward instead, so the pool stays usable.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior learn (0.05 / 100 calls = 5e-4 per turn).
    db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 100, "prior-learn");
    db.upsert_quota_refresh(provider, &[quota_input(0.25, "2026-04-22T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(100));

    // Now feed a pathological sample: used_percent jumps from 0.25 to
    // 0.95 over just 5 turns. dp = 0.70, dc = 5, so new_rate = 0.14/turn,
    // which exceeds MAX_LEARNABLE_BURN_RATE (0.1/turn).
    let t1 = ts("2026-04-21T06:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 5, "spike");
    db.upsert_quota_refresh(provider, &[quota_input(0.95, "2026-04-22T00:00:00Z")])
        .unwrap();

    let after_spike = db.get_windows(provider).unwrap();
    // Pathological sample rejected: delta is still the prior 0.05/100.
    assert!(
        (after_spike[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9,
        "spike sample should not overwrite prior learn; got {:?}",
        after_spike[0].last_delta_percent
    );
    assert_eq!(after_spike[0].last_delta_calls, Some(100));
    // used_percent still reflects the incoming sample — we only reject
    // the delta learn, not the quota observation itself.
    assert!((after_spike[0].used_percent - 0.95).abs() < 1e-9);
}

#[test]
fn upsert_quota_refresh_learns_sample_at_cap_boundary() {
    // A plausible-high rate just below the cap DOES get learned,
    // confirming the cap doesn't accidentally reject real workloads.
    // dp=0.90 over 25 turns → 0.036/turn. Below MAX_LEARNABLE_BURN_RATE
    // (0.1), above MIN_LEARN_SAMPLE_CALLS (20), below
    // NEAR_EXHAUSTED_USED_PERCENT (0.99). All three gates pass.
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.0, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 25, "boundary");
    db.upsert_quota_refresh(provider, &[quota_input(0.90, "2026-04-22T00:00:00Z")])
        .unwrap();

    let w = db.get_windows(provider).unwrap();
    assert!((w[0].last_delta_percent.unwrap() - 0.90).abs() < 1e-9);
    assert_eq!(w[0].last_delta_calls, Some(25));
}

#[test]
fn upsert_quota_refresh_rejects_learn_when_new_sample_near_rail() {
    // Regression: live observation 2026-04-21 had codex2's 7-day window
    // briefly read used_percent=1.0 from an upstream ChatGPT API spike,
    // paired with 34 turns since prior refresh. The learner computed
    // rate ≈ 0.029/turn on WEEKLY (real weekly rates are ~6e-5/turn;
    // the 100% sample was a cap-hit trajectory, not a natural fill),
    // which then projected every future invocation near the ceiling.
    // User framing: "turns barely budge weekly" —
    // so a weekly sample that moves 100 points in one interval is
    // distrusted. The marker we key on is "new used_percent at the
    // rail (>= 0.99)"; this test pins that gate.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior weekly rate: 0.02 over 300 turns → 6.7e-5/turn.
    db.upsert_quota_refresh(provider, &[quota_input(0.50, "2026-04-28T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 300, "prior-weekly");
    db.upsert_quota_refresh(provider, &[quota_input(0.52, "2026-04-28T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(300));

    // Upstream spike: new sample arrives at used_percent = 1.0 after
    // 34 turns. MIN_LEARN_SAMPLE_CALLS and MAX_LEARNABLE_BURN_RATE
    // alone would have let this through (34 > 20, 0.48/34 = 0.014/turn
    // < 0.1). The NEAR_EXHAUSTED_USED_PERCENT gate catches it.
    let t1 = ts("2026-04-21T12:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 34, "spike");
    db.upsert_quota_refresh(provider, &[quota_input(1.0, "2026-04-28T00:00:00Z")])
        .unwrap();

    let after = db.get_windows(provider).unwrap();
    assert!(
        (after[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9,
        "near-rail sample must not overwrite prior weekly learn"
    );
    assert_eq!(after[0].last_delta_calls, Some(300));
    // used_percent still reflects the spike — we only distrust the rate.
    assert!((after[0].used_percent - 1.0).abs() < 1e-9);
}

#[test]
fn upsert_quota_refresh_rejects_small_sample_delta_as_noise() {
    // Regression: live observation 2026-04-21 had claude2 with a learned
    // delta of 0.01/6 (rate 0.00167/turn). Paired with 193 turns since
    // refresh at scoring time, that projected 0.65 → 0.97, hard-blocking
    // the whole claude-opus pool. Sample-size floor of MIN_LEARN_SAMPLE_CALLS
    // rejects any delta learn below 20 turns and carries the prior
    // learn forward. At claude2 scale, this would have kept the pool
    // usable for the next invocation.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior learn (0.01 over 50 calls = 2e-4/turn).
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 50, "prior-learn");
    db.upsert_quota_refresh(provider, &[quota_input(0.11, "2026-04-22T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(50));

    // Now a small-sample observation: dp=0.01 over just 6 turns. Well
    // below the MAX_LEARNABLE_BURN_RATE cap (rate ≈ 0.00167), but
    // the sample size is too small to trust.
    let t1 = ts("2026-04-21T06:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 6, "small-sample");
    db.upsert_quota_refresh(provider, &[quota_input(0.12, "2026-04-22T00:00:00Z")])
        .unwrap();

    let after = db.get_windows(provider).unwrap();
    // Small-sample rejected: prior 0.01/50 carried forward.
    assert!(
        (after[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9,
        "small-sample delta should not overwrite prior learn"
    );
    assert_eq!(after[0].last_delta_calls, Some(50));
}

// RISK: start_invocation could write terminal metadata on a running row (proposal §test-intent "terminal-reason absence characterization", assumption A5)
// LEVEL: unit
// SOURCE: contracts/nes-250-contract.md § Test catalog § Run-row null contract (T-RUN-NULL)
#[test]
fn start_invocation_inserts_running_row_with_null_terminal_fields() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    let id = db.start_invocation(&start).unwrap();
    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();

    assert_eq!(row.id, id);
    assert_eq!(row.status, InvocationStatus::Running);
    assert_eq!(row.provider_name.as_deref(), Some("fixture-provider"));
    assert_eq!(row.parent_invocation_id, None);
    assert_eq!(row.success, None);
    assert_eq!(row.exit_code, None);
    assert_eq!(row.terminal_reason, None);
    assert_eq!(row.finished_at, None);
}

#[test]
fn running_invocation_provider_session_id() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Running);
    assert_eq!(row.finished_at, None);
    assert_eq!(
        row.provider_session_id.as_deref(),
        Some(provider_session_id.as_str())
    );
    assert_eq!(
        row.provider_session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );
}

#[test]
fn running_invocation_chain_minted() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    let chain_id = db
        .chain_id_for_segment("fixture-provider", &provider_session_id)
        .unwrap()
        .expect("chain segment must be minted");
    assert!(Uuid::parse_str(&chain_id).is_ok());
}

#[test]
fn bind_invocation_provider_session_start_same_id_is_idempotent() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: provider_session_id.clone(),
        capture_method: "forced_flag_verified",
        resume_input_id: None,
        provider_session_resolved_account: None,
    };

    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();
    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();

    assert_eq!(segment_count(&db), 1);
    assert!(
        db.chain_id_for_segment("fixture-provider", &provider_session_id)
            .unwrap()
            .is_some()
    );
}

#[test]
fn bind_invocation_provider_session_start_conflicting_rebind_rejects_without_mutation() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "forced_flag_verified",
            resume_input_id: None,
            provider_session_resolved_account: None,
        },
    )
    .unwrap();
    let before_segments = segment_count(&db);

    let err = db
        .bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: Uuid::new_v4().to_string(),
                capture_method: "forced_flag_verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap_err();

    assert!(
        err.contains("already bound") || err.contains("refusing"),
        "{err}"
    );
    assert_eq!(segment_count(&db), before_segments);
    let stored: Option<String> = db
        .conn
        .query_row(
            "SELECT provider_session_id FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored.as_deref(), Some(provider_session_id.as_str()));
}

#[test]
fn bind_invocation_provider_session_start_matching_resume_input_does_not_mint_duplicate_chain() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();

    db.bind_invocation_provider_session_start(
        id,
        &ProviderSessionBinding {
            provider_session_id: provider_session_id.clone(),
            capture_method: "resumed",
            resume_input_id: Some(provider_session_id.clone()),
            provider_session_resolved_account: None,
        },
    )
    .unwrap();

    assert_eq!(segment_count(&db), 0);
    let row: (Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT provider_session_id, resume_input_id FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some(provider_session_id.as_str()));
    assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
}

#[test]
fn bind_then_record_legacy_then_rebind_preserves_legacy_resume_session_id() {
    let db = test_db();
    let id = seed_running_invocation(&db);
    let provider_session_id = Uuid::new_v4().to_string();
    let legacy_resume_input = Uuid::new_v4().to_string();
    let binding = ProviderSessionBinding {
        provider_session_id: provider_session_id.clone(),
        capture_method: "resumed",
        resume_input_id: Some(legacy_resume_input.clone()),
        provider_session_resolved_account: None,
    };

    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();
    db.record_legacy_resume_input_session_id(id, &legacy_resume_input)
        .unwrap();
    db.bind_invocation_provider_session_start(id, &binding)
        .unwrap();

    let row: (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id
                 FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some(legacy_resume_input.as_str()));
    assert_eq!(row.1.as_deref(), Some(provider_session_id.as_str()));
    assert_eq!(row.2.as_deref(), Some(legacy_resume_input.as_str()));
}

#[test]
fn start_invocation_rejects_duplicate_uuid() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    db.start_invocation(&start).unwrap();
    let err = db.start_invocation(&start).unwrap_err();
    assert!(err.contains("invocation"));
}

#[test]
fn start_invocation_accepts_parent_rowid() {
    let db = test_db();
    let parent = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let parent_id = db.start_invocation(&parent).unwrap();

    let child = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: Some(parent_id),
    };
    db.start_invocation(&child).unwrap();

    let row = db
        .get_invocation_by_uuid(&child.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.parent_invocation_id, Some(parent_id));
}

// RISK: finalize_invocation could fail to persist caller-provided terminal_reason separately from error_category (proposal §test-intent "terminal-reason absence characterization", assumption A5)
// LEVEL: unit
// SOURCE: contracts/nes-250-contract.md § Schema § StateDb::finalize_invocation
#[test]
fn finalize_invocation_sets_terminal_fields() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.finalize_invocation(id, false, 7, None, Some("exit_nonzero"))
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Failed);
    assert_eq!(row.success, Some(false));
    assert_eq!(row.exit_code, Some(7));
    assert_eq!(row.error_category, None);
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_nonzero"));
    assert!(row.finished_at.is_some());
}

#[test]
fn finalize_invocation_updates_provider_aggregate_stats() {
    let db = test_db();
    let failed = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let succeeded = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };

    let failed_id = db.start_invocation(&failed).unwrap();
    db.finalize_invocation(
        failed_id,
        false,
        1,
        Some("rate_limit"),
        Some("429 Too Many Requests"),
    )
    .unwrap();
    let succeeded_id = db.start_invocation(&succeeded).unwrap();
    db.finalize_invocation(succeeded_id, true, 0, None, None)
        .unwrap();

    let provider = db
        .get_provider("test-model", "fixture-provider")
        .unwrap()
        .unwrap();
    assert_eq!(provider.invocation_count, 2);
    assert_eq!(provider.error_count, 1);
    assert_eq!(
        provider.last_error.as_deref(),
        Some("429 Too Many Requests")
    );
    assert!(provider.last_invoked_at.is_some());
}

// Risk: Null-provider legacy rows must not synthesize aggregate identity | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track; ~/projects/agent-runner/planning/trunk/research/10-routing-claude-skipped-contract.md §5 finalize_invocation
#[test]
fn finalize_invocation_skips_provider_aggregate_for_null_provider_name() {
    let db = test_db();

    let mut ids = Vec::new();
    for provider_index in [0, 1] {
        db.conn
            .execute(
                "INSERT INTO invocations (
                        invocation_uuid, model_name, provider_name, provider_index,
                        status, created_at
                     ) VALUES (?1, 'legacy-model', NULL, ?2, 'running', ?3)",
                sqlite::params![
                    Uuid::new_v4().to_string(),
                    provider_index,
                    Utc::now().to_rfc3339(),
                ],
            )
            .unwrap();
        ids.push(db.conn.last_insert_rowid());
    }

    db.finalize_invocation(ids[0], true, 0, None, None).unwrap();
    db.finalize_invocation(ids[1], false, 1, Some("rate_limit"), Some("429"))
        .unwrap();

    let provider_rows: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM providers WHERE model_name = 'legacy-model'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(provider_rows, 0);
}

#[test]
fn finalize_invocation_errors_for_missing_row() {
    let db = test_db();
    let err = db
        .finalize_invocation(99, false, 1, Some("rate_limit"), None)
        .unwrap_err();
    assert!(err.contains("99"));
}

#[test]
fn finalize_invocation_errors_when_called_twice() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    db.finalize_invocation(id, true, 0, None, Some("exit_zero"))
        .unwrap();

    let err = db
        .finalize_invocation(
            id,
            false,
            -1,
            None,
            Some("supervisor_observed_unknown_exit"),
        )
        .unwrap_err();
    assert!(err.contains("already"));

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.status, InvocationStatus::Succeeded);
    assert_eq!(row.success, Some(true));
    assert_eq!(row.exit_code, Some(0));
    assert_eq!(row.terminal_reason.as_deref(), Some("exit_zero"));
}

#[test]
fn update_session_capture_persists_verified_session_id_and_method() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_session_capture(
        id,
        Some("5169694d-de0f-40d1-890c-6e28e55bab27"),
        "forced_flag_verified",
    )
    .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(
        row.session_id.as_deref(),
        Some("5169694d-de0f-40d1-890c-6e28e55bab27")
    );
    assert_eq!(
        row.session_capture_method.as_deref(),
        Some("forced_flag_verified")
    );
}

/// Per V10 (failures observable, never silent): a completed
/// invocation with no capture configured must persist `"none"`
/// explicitly so trace can distinguish "no capture attempted" from
/// "still running" (NULL). Calling
/// `update_session_capture(id, None, "none")` must write the
/// column, NOT no-op.
#[test]
fn update_session_capture_none_none_persists_none_marker() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    // Before any update: column is NULL (start_invocation doesn't set it).
    let before = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(before.session_capture_method, None);

    db.update_session_capture(id, None, "none").unwrap();

    let after = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(after.session_id, None);
    assert_eq!(
        after.session_capture_method.as_deref(),
        Some("none"),
        "completed-no-capture rows must record 'none' explicitly per V10"
    );
}

/// Per contract: update_session_capture is safe to call multiple
/// times (idempotency for retries). The latest call wins.
#[test]
fn update_session_capture_safe_to_call_multiple_times() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "test-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();

    db.update_session_capture(id, Some("first"), "forced_flag_verified")
        .unwrap();
    db.update_session_capture(id, Some("second"), "stdout_json_event")
        .unwrap();
    db.update_session_capture(id, Some("third"), "failed")
        .unwrap();

    let row = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(row.session_id.as_deref(), Some("third"));
    assert_eq!(row.session_capture_method.as_deref(), Some("failed"));
}

/// "Leaves others alone" — update_session_capture must NOT clobber
/// fields outside session_id/session_capture_method (e.g.
/// invocation_uuid, model_name, status).
#[test]
fn update_session_capture_leaves_other_columns_alone() {
    let db = test_db();
    let start = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "specific-model".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 7,
        parent_invocation_id: None,
    };
    let id = db.start_invocation(&start).unwrap();
    let before = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();

    db.update_session_capture(id, Some("sid"), "forced_flag_verified")
        .unwrap();

    let after = db
        .get_invocation_by_uuid(&start.invocation_uuid)
        .unwrap()
        .unwrap();
    assert_eq!(after.invocation_uuid, before.invocation_uuid);
    assert_eq!(after.model_name, before.model_name);
    assert_eq!(after.provider_index, before.provider_index);
    assert_eq!(after.status, before.status);
    assert_eq!(after.created_at, before.created_at);
}

#[test]
fn update_session_capture_dual_id_semantics_for_non_resumed_and_resumed_rows() {
    let db = test_db();
    let non_resumed = seed_running_invocation(&db);
    let resumed = seed_running_invocation(&db);
    db.conn
        .execute(
            "UPDATE invocations
                 SET provider_session_id = 'active-provider-session'
                 WHERE id = ?1",
            sqlite::params![resumed],
        )
        .unwrap();

    db.update_session_capture(non_resumed, Some("new-provider-session"), "stdout")
        .unwrap();
    db.update_session_capture(resumed, Some("attempted-resume-id"), "resumed")
        .unwrap();

    let non_resumed_row: (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT provider_session_id, resume_input_id, provider_session_capture_method
                 FROM invocations WHERE id = ?1",
            sqlite::params![non_resumed],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(non_resumed_row.0.as_deref(), Some("new-provider-session"));
    assert_eq!(non_resumed_row.1, None);
    assert_eq!(non_resumed_row.2.as_deref(), Some("stdout"));

    let resumed_row: (Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT provider_session_id, resume_input_id, provider_session_capture_method
                 FROM invocations WHERE id = ?1",
            sqlite::params![resumed],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(resumed_row.0.as_deref(), Some("active-provider-session"));
    assert_eq!(resumed_row.1.as_deref(), Some("attempted-resume-id"));
    assert_eq!(resumed_row.2, None);
    assert_eq!(invocation_count(&db), 2);
}

#[test]
fn record_legacy_resume_input_session_id_updates_only_resumed_row() {
    let db = test_db();
    let resumed = seed_running_invocation(&db);
    let non_resumed = seed_running_invocation(&db);
    db.update_session_capture(resumed, Some("active-session"), "resumed")
        .unwrap();
    db.update_session_capture(non_resumed, Some("provider-session"), "stdout")
        .unwrap();

    db.record_legacy_resume_input_session_id(resumed, "attempted-resume")
        .unwrap();
    db.record_legacy_resume_input_session_id(non_resumed, "must-not-apply")
        .unwrap();

    let resumed_session: Option<String> = db
        .conn
        .query_row(
            "SELECT session_id FROM invocations WHERE id = ?1",
            sqlite::params![resumed],
            |row| row.get(0),
        )
        .unwrap();
    let non_resumed_session: Option<String> = db
        .conn
        .query_row(
            "SELECT session_id FROM invocations WHERE id = ?1",
            sqlite::params![non_resumed],
            |row| row.get(0),
        )
        .unwrap();

    assert_eq!(resumed_session.as_deref(), Some("attempted-resume"));
    assert_eq!(non_resumed_session.as_deref(), Some("provider-session"));
    assert_eq!(invocation_count(&db), 2);
}

#[test]
fn recent_errors() {
    let db = test_db();
    let failed = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "m".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let succeeded = InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: "m".to_string(),
        provider_name: "fixture-provider".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    };
    let failed_id = db.start_invocation(&failed).unwrap();
    db.finalize_invocation(failed_id, false, 1, None, None)
        .unwrap();
    let succeeded_id = db.start_invocation(&succeeded).unwrap();
    db.finalize_invocation(succeeded_id, true, 0, None, None)
        .unwrap();

    let count = db.recent_error_count("m", "fixture-provider", 60).unwrap();
    assert_eq!(count, 1);
}

// Risk: recent_error_count identity drift | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
#[test]
fn recent_error_count_uses_provider_name_not_reused_index_history() {
    let db = test_db();

    for _ in 0..3 {
        record_provider_invocation(
            &db,
            "routing-model",
            "claude-old",
            0,
            false,
            Some("rate_limit"),
            None,
        );
    }

    assert_eq!(
        db.recent_error_count("routing-model", "claude", 60)
            .unwrap(),
        0,
        "current provider name must not inherit recent failures from a prior occupant of index 0"
    );
    assert_eq!(
        db.recent_error_count("routing-model", "claude-old", 60)
            .unwrap(),
        3,
        "the failed provider name still owns its own recent failures"
    );
}

// Risk: Aggregate writer/reader round-trip after provider reorder | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
#[test]
fn provider_aggregate_round_trip_follows_name_after_reorder() {
    let db = test_db();
    record_provider_invocation(&db, "routing-model", "claude2", 0, true, None, None);

    let claude2 = db
        .get_provider("routing-model", "claude2")
        .unwrap()
        .expect("claude2 aggregate should exist by provider name");
    assert_eq!(claude2.provider_name, "claude2");
    assert_eq!(claude2.invocation_count, 1);
    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "claude must not inherit claude2 history after taking index 0"
    );

    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "fallback scoring should treat the current claude provider as unused"
    );
}

// Risk: Aggregate writer/reader round-trip after provider rename | level: unit
// Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
#[test]
fn provider_aggregate_round_trip_does_not_inherit_renamed_provider_history() {
    let db = test_db();
    record_provider_invocation(&db, "routing-model", "claude-old", 0, true, None, None);

    let old = db
        .get_provider("routing-model", "claude-old")
        .unwrap()
        .expect("old provider name should retain its aggregate");
    assert_eq!(old.provider_name, "claude-old");
    assert_eq!(old.invocation_count, 1);
    assert!(
        db.get_provider("routing-model", "claude")
            .unwrap()
            .is_none(),
        "renamed provider claude starts without aggregate history unless invocations use that name"
    );
}

#[test]
fn ingest_session_turns_batch_persists_parent_and_sidechain_columns() {
    let db = test_db();

    let inserted = db
        .ingest_session_turns_batch(
            "fixture-provider",
            &[SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "child-turn".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("root-turn".to_string()),
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            }],
        )
        .unwrap();

    assert_eq!(inserted, 1);
    let row: (Option<String>, i64) = db
        .conn
        .query_row(
            "SELECT parent_turn_id, is_sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3",
            sqlite::params!["fixture-provider", "session-a", "child-turn"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("root-turn"));
    assert_eq!(row.1, 1);
}

#[test]
fn count_session_turns_reports_total_assistant_and_sidechain_counts() {
    let db = test_db();

    db.ingest_session_turns_batch(
        "fixture-provider",
        &[
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "root".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-main".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("root".to_string()),
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-side".to_string(),
                timestamp: ts("2026-04-17T08:00:02Z"),
                role: "assistant".to_string(),
                parent_turn_id: Some("assistant-main".to_string()),
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: "session-b".to_string(),
                turn_id: "other-session".to_string(),
                timestamp: ts("2026-04-17T08:00:03Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: true,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    db.ingest_session_turns_batch(
        "other-provider",
        &[SessionTurnIngest {
            session_id: "session-a".to_string(),
            turn_id: "other-provider-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:04Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: true,
            is_compaction_boundary: false,
            body: None,
        }],
    )
    .unwrap();

    let counts: SessionTurnCounts = db
        .count_session_turns("fixture-provider", "session-a")
        .unwrap();

    assert_eq!(counts.total, 3);
    assert_eq!(counts.assistant, 2);
    assert_eq!(counts.sidechain, 1);
}

#[test]
fn has_session_user_text_turn_requires_exact_user_body_match() {
    let db = test_db();
    let expected = "[OULIPOLY NOTIFICATIONS]\nhandle: h-exact\n";

    db.ingest_session_turns_batch(
        "fixture-provider",
        &[
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "user-exact".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(serde_json::json!([{ "type": "text", "text": expected }]).to_string()),
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-same-text".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(
                    serde_json::json!([{ "type": "text", "text": "assistant text" }]).to_string(),
                ),
            },
        ],
    )
    .unwrap();

    let extra_text_body = serde_json::json!([
        { "type": "text", "text": expected },
        { "type": "text", "text": "extra" }
    ])
    .to_string();

    assert!(
        db.has_session_user_text_turn("fixture-provider", "session-a", expected)
            .unwrap()
    );
    assert!(
        !db.has_session_user_text_turn("fixture-provider", "session-a", "handle: h")
            .unwrap(),
        "partial text must not confirm delivery"
    );
    assert!(
        !StateDb::session_turn_body_has_exact_text(&extra_text_body, expected),
        "multi-chunk turns must match the submitted payload exactly"
    );
    assert!(StateDb::session_turn_body_has_exact_text(
        &extra_text_body,
        &format!("{expected}extra")
    ));
    assert!(
        !db.has_session_user_text_turn("other-provider", "session-a", expected)
            .unwrap(),
        "provider identity must match"
    );
}

#[test]
fn has_session_user_turn_containing_matches_user_body_substring() {
    let db = test_db();
    let nonce = "11111111-2222-4333-8444-555555555555";

    db.ingest_session_turns_batch(
        "fixture-provider",
        &[
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "user-quoted-delivery".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(
                    serde_json::json!([
                        {
                            "type": "text",
                            "text": format!(
                                "\"[OULIPOLY NOTIFICATIONS]\n[OULIPOLY-DELIVERY {nonce}]\nbody\""
                            )
                        }
                    ])
                    .to_string(),
                ),
            },
            SessionTurnIngest {
                session_id: "session-a".to_string(),
                turn_id: "assistant-same-nonce".to_string(),
                timestamp: ts("2026-04-17T08:00:01Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: Some(
                    serde_json::json!([
                        {
                            "type": "text",
                            "text": format!("assistant [OULIPOLY-DELIVERY {nonce}]")
                        }
                    ])
                    .to_string(),
                ),
            },
        ],
    )
    .unwrap();

    assert!(
        db.has_session_user_turn_containing("fixture-provider", "session-a", nonce)
            .unwrap(),
        "the delivery nonce should match inside a non-exact quote-wrapped user body"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "session-a", "missing-nonce")
            .unwrap(),
        "missing nonce must not confirm delivery"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "session-a", "")
            .unwrap(),
        "empty needles must not match every body"
    );
    assert!(
        !db.has_session_user_turn_containing("other-provider", "session-a", nonce)
            .unwrap(),
        "provider identity must match"
    );
    assert!(
        !db.has_session_user_turn_containing("fixture-provider", "other-session", nonce)
            .unwrap(),
        "session identity must match"
    );
}

#[test]
fn composite_invocation_id_formats_and_round_trips() {
    let composite = CompositeInvocationId {
        source: "fixture-provider".to_string(),
        id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
    };
    let line = composite.stderr_line();
    assert_eq!(
        line,
        r#"OULIPOLY_INVOCATION={"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );

    let parsed =
        CompositeInvocationId::parse_env_value(line.strip_prefix("OULIPOLY_INVOCATION=").unwrap())
            .unwrap();
    assert_eq!(parsed, composite);
}

#[test]
fn composite_invocation_id_parses_shell_mangled_env_values() {
    let parsed = CompositeInvocationId::parse_env_value(
        "{source:fixture-provider,id:7ad2916c-38dd-49e6-a1f7-3ef22766ff70}",
    )
    .unwrap();

    assert_eq!(
        parsed,
        CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
        }
    );
}

#[test]
fn composite_invocation_id_parses_quoted_shell_mangled_env_values() {
    let parsed = CompositeInvocationId::parse_env_value(
        r#"{source:"fixture-provider",id:"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#,
    )
    .unwrap();

    assert_eq!(
        parsed,
        CompositeInvocationId {
            source: "fixture-provider".to_string(),
            id: "7ad2916c-38dd-49e6-a1f7-3ef22766ff70".to_string(),
        }
    );
}

#[test]
fn composite_invocation_id_rejects_malformed_env_values() {
    for raw in [
        "not-json",
        r#"{"source":"fixture-provider"}"#,
        r#"{"source":"fixture-provider","id":"not-a-uuid"}"#,
        r#"{"source":"fixture-provider","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70","extra":true}"#,
    ] {
        assert!(
            CompositeInvocationId::parse_env_value(raw).is_err(),
            "{raw}"
        );
    }
}

#[test]
fn invocation_status_round_trips_through_strings() {
    for status in [
        InvocationStatus::Running,
        InvocationStatus::Succeeded,
        InvocationStatus::Failed,
        InvocationStatus::Legacy,
    ] {
        // Inherent contracted API: Option<Self>.
        assert_eq!(InvocationStatus::from_str(status.as_str()), Some(status));
        // FromStr trait surface: Result<Self, _>. Both must work.
        assert_eq!(
            status.as_str().parse::<InvocationStatus>().ok(),
            Some(status)
        );
    }
    assert_eq!(InvocationStatus::from_str("unknown"), None);
    assert!("unknown".parse::<InvocationStatus>().is_err());
}

#[test]
fn get_invocation_by_uuid_returns_matching_and_missing_rows() {
    with_models_config(
        "legacy-model",
        r#"
[[providers]]
name = "fixture-provider"
"#,
        || {
            let db = test_db();
            let start = InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: "legacy-model".to_string(),
                provider_name: "fixture-provider".to_string(),
                provider_index: 0,
                parent_invocation_id: None,
            };
            db.start_invocation(&start).unwrap();
            let running = db
                .get_invocation_by_uuid(&start.invocation_uuid)
                .unwrap()
                .unwrap();
            assert_eq!(running.invocation_uuid, start.invocation_uuid);

            let dir =
                legacy_invocations_db(&[("missing-model", 0, 0, 7, None, "2026-04-17T08:05:00Z")]);
            let migrated = StateDb::open(&dir.path().join("state.db")).unwrap();
            let legacy_uuid: String = migrated
                .conn
                .query_row(
                    "SELECT invocation_uuid FROM invocations LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let legacy = migrated
                .get_invocation_by_uuid(&legacy_uuid)
                .unwrap()
                .unwrap();
            assert_eq!(legacy.status, InvocationStatus::Legacy);
            assert!(
                migrated
                    .get_invocation_by_uuid("00000000-0000-0000-0000-000000000000")
                    .unwrap()
                    .is_none()
            );
        },
    );
}

#[test]
fn list_invocation_children_returns_empty_for_unknown_parent() {
    let db = test_db();

    let children = db.list_invocation_children(999).unwrap();

    assert!(children.is_empty());
}

#[test]
fn list_invocation_children_orders_by_created_at_then_row_id() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "10000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    insert_invocation_fixture(
        &db,
        "30000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:02:00Z",
    );
    insert_invocation_fixture(
        &db,
        "20000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    insert_invocation_fixture(
        &db,
        "40000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );

    let children = db.list_invocation_children(root_id).unwrap();
    let ordered: Vec<&str> = children
        .iter()
        .map(|record| record.invocation_uuid.as_str())
        .collect();

    assert_eq!(
        ordered,
        vec![
            "20000000-0000-0000-0000-000000000000",
            "40000000-0000-0000-0000-000000000000",
            "30000000-0000-0000-0000-000000000000",
        ]
    );
}

#[test]
fn list_invocation_children_returns_only_direct_children() {
    let db = test_db();
    let root_id = insert_invocation_fixture(
        &db,
        "50000000-0000-0000-0000-000000000000",
        None,
        "2026-04-17T08:00:00Z",
    );
    let child_id = insert_invocation_fixture(
        &db,
        "60000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:01:00Z",
    );
    insert_invocation_fixture(
        &db,
        "70000000-0000-0000-0000-000000000000",
        Some(child_id),
        "2026-04-17T08:02:00Z",
    );
    insert_invocation_fixture(
        &db,
        "80000000-0000-0000-0000-000000000000",
        Some(root_id),
        "2026-04-17T08:03:00Z",
    );

    let children = db.list_invocation_children(root_id).unwrap();
    let uuids: Vec<&str> = children
        .iter()
        .map(|record| record.invocation_uuid.as_str())
        .collect();

    assert_eq!(
        uuids,
        vec![
            "60000000-0000-0000-0000-000000000000",
            "80000000-0000-0000-0000-000000000000",
        ]
    );
}

#[test]
fn missing_provider_returns_none() {
    let db = test_db();
    assert!(db.get_provider("nonexistent", "missing").unwrap().is_none());
}

// --- CLI Provider & Account tests ---

fn sample_provider() -> CliProviderRecord {
    CliProviderRecord {
        cli_name: "claude".to_string(),
        display_name: "Anthropic".to_string(),
        installed: true,
        version: Some("1.2.3".to_string()),
        config_dir: Some("/home/user/.claude".to_string()),
        last_synced: None,
    }
}

#[test]
fn upsert_and_list_cli_providers() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let mut p2 = sample_provider();
    p2.cli_name = "codex".to_string();
    p2.display_name = "OpenAI".to_string();
    db.upsert_cli_provider(&p2).unwrap();

    let providers = db.list_cli_providers().unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].cli_name, "claude");
    assert_eq!(providers[1].cli_name, "codex");
}

#[test]
fn upsert_cli_provider_updates_existing() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let mut updated = sample_provider();
    updated.version = Some("2.0.0".to_string());
    updated.last_synced = Some("2026-02-19T00:00:00Z".to_string());
    db.upsert_cli_provider(&updated).unwrap();

    let p = db.get_cli_provider("claude").unwrap().unwrap();
    assert_eq!(p.version.as_deref(), Some("2.0.0"));
    assert!(p.last_synced.is_some());
}

#[test]
fn get_cli_provider_missing() {
    let db = test_db();
    assert!(db.get_cli_provider("nonexistent").unwrap().is_none());
}

#[test]
fn insert_and_list_accounts() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let acct = AccountRecord {
        id: "work".to_string(),
        provider: "claude".to_string(),
        profile_name: "work-profile".to_string(),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Valid,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct).unwrap();

    let acct2 = AccountRecord {
        id: "personal".to_string(),
        provider: "claude".to_string(),
        profile_name: "personal-profile".to_string(),
        auth_method: AuthMethod::ApiKey {
            env_var: "ANTHROPIC_API_KEY".to_string(),
            config_path: None,
        },
        auth_status: AuthStatus::Unknown,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct2).unwrap();

    // List all
    let all = db.list_accounts(None).unwrap();
    assert_eq!(all.len(), 2);

    // List by provider
    let claude_accounts = db.list_accounts(Some("claude")).unwrap();
    assert_eq!(claude_accounts.len(), 2);
    assert_eq!(claude_accounts[0].id, "personal");
    assert_eq!(claude_accounts[1].id, "work");

    let empty = db.list_accounts(Some("codex")).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn delete_account() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();

    let acct = AccountRecord {
        id: "temp".to_string(),
        provider: "claude".to_string(),
        profile_name: "temp-profile".to_string(),
        auth_method: AuthMethod::ConfigFile {
            path: "~/.claude/config".to_string(),
        },
        auth_status: AuthStatus::NoAuth,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&acct).unwrap();
    assert_eq!(db.list_accounts(None).unwrap().len(), 1);

    let deleted = db.delete_account("temp", "claude").unwrap();
    assert!(deleted);
    assert!(db.list_accounts(None).unwrap().is_empty());

    // Deleting again returns false
    let deleted_again = db.delete_account("temp", "claude").unwrap();
    assert!(!deleted_again);
}

#[test]
fn auth_method_roundtrip() {
    let methods = vec![
        AuthMethod::OAuth,
        AuthMethod::ApiKey {
            env_var: "MY_KEY".to_string(),
            config_path: Some("/path/to/key".to_string()),
        },
        AuthMethod::ConfigFile {
            path: "~/.config/file".to_string(),
        },
    ];
    for method in methods {
        let serialized = method.to_db_string();
        let deserialized = AuthMethod::from_db_string(&serialized);
        assert_eq!(method, deserialized);
    }
}

// --- Discovered model & parameter tests ---

fn sample_discovered_model(name: &str, provider: &str) -> DiscoveredModel {
    DiscoveredModel {
        canonical_name: name.to_string(),
        provider: provider.to_string(),
        discovered_at: "2026-02-19T00:00:00Z".to_string(),
        cli_version: "1.0.0".to_string(),
    }
}

#[test]
fn upsert_and_list_discovered_models() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("claude-sonnet-4", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("gpt-5.3", "codex"))
        .unwrap();

    // List all
    let all = db.list_discovered_models(None).unwrap();
    assert_eq!(all.len(), 3);

    // List by provider
    let claude_models = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(claude_models.len(), 2);
    assert_eq!(claude_models[0].canonical_name, "claude-opus-4");
    assert_eq!(claude_models[1].canonical_name, "claude-sonnet-4");

    let codex_models = db.list_discovered_models(Some("codex")).unwrap();
    assert_eq!(codex_models.len(), 1);
    assert_eq!(codex_models[0].canonical_name, "gpt-5.3");

    let empty = db.list_discovered_models(Some("gemini")).unwrap();
    assert!(empty.is_empty());
}

#[test]
fn upsert_discovered_model_updates_existing() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("claude-opus-4", "claude"))
        .unwrap();

    let mut updated = sample_discovered_model("claude-opus-4", "claude");
    updated.cli_version = "2.0.0".to_string();
    updated.discovered_at = "2026-02-20T00:00:00Z".to_string();
    db.upsert_discovered_model(&updated).unwrap();

    let models = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].cli_version, "2.0.0");
    assert_eq!(models[0].discovered_at, "2026-02-20T00:00:00Z");
}

#[test]
fn delete_stale_models() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("model-b", "claude"))
        .unwrap();

    let mut newer = sample_discovered_model("model-c", "claude");
    newer.cli_version = "2.0.0".to_string();
    db.upsert_discovered_model(&newer).unwrap();

    // Delete models with cli_version != "2.0.0"
    let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
    assert_eq!(deleted, 2);

    let remaining = db.list_discovered_models(Some("claude")).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].canonical_name, "model-c");
}

#[test]
fn delete_stale_models_different_provider() {
    let db = test_db();
    db.upsert_discovered_model(&sample_discovered_model("model-a", "claude"))
        .unwrap();
    db.upsert_discovered_model(&sample_discovered_model("model-b", "codex"))
        .unwrap();

    // Only delete stale models for "claude", "codex" should be untouched
    let deleted = db.delete_stale_models("claude", "2.0.0").unwrap();
    assert_eq!(deleted, 1);

    let codex = db.list_discovered_models(Some("codex")).unwrap();
    assert_eq!(codex.len(), 1);
}

#[test]
fn upsert_and_list_model_parameters() {
    let db = test_db();

    let temp_param = ModelParameter {
        name: "temperature".to_string(),
        display_name: "Temperature".to_string(),
        param_type: ParamType::Number {
            min: Some(0.0),
            max: Some(2.0),
        },
        description: "Controls randomness".to_string(),
        cli_mapping: CliMapping {
            flag: "--temperature".to_string(),
            value_template: "{value}".to_string(),
        },
    };

    let model_param = ModelParameter {
        name: "model".to_string(),
        display_name: "Model".to_string(),
        param_type: ParamType::Enum {
            options: vec!["opus-4".to_string(), "sonnet-4".to_string()],
        },
        description: "Model variant to use".to_string(),
        cli_mapping: CliMapping {
            flag: "-m".to_string(),
            value_template: "{value}".to_string(),
        },
    };

    db.upsert_model_parameter("claude-opus-4", "claude", &temp_param)
        .unwrap();
    db.upsert_model_parameter("claude-opus-4", "claude", &model_param)
        .unwrap();

    let params = db.list_model_parameters("claude-opus-4", "claude").unwrap();
    assert_eq!(params.len(), 2);
    // Ordered by name
    assert_eq!(params[0].name, "model");
    assert_eq!(params[1].name, "temperature");

    // Verify ParamType round-trip
    match &params[0].param_type {
        ParamType::Enum { options } => {
            assert_eq!(options.len(), 2);
            assert_eq!(options[0], "opus-4");
        }
        other => panic!("Expected Enum, got {:?}", other),
    }

    match &params[1].param_type {
        ParamType::Number { min, max } => {
            assert_eq!(*min, Some(0.0));
            assert_eq!(*max, Some(2.0));
        }
        other => panic!("Expected Number, got {:?}", other),
    }

    // Verify CliMapping round-trip
    assert_eq!(params[1].cli_mapping.flag, "--temperature");
    assert_eq!(params[1].cli_mapping.value_template, "{value}");
}

#[test]
fn upsert_model_parameter_updates_existing() {
    let db = test_db();

    let param = ModelParameter {
        name: "verbose".to_string(),
        display_name: "Verbose".to_string(),
        param_type: ParamType::Boolean,
        description: "Enable verbose output".to_string(),
        cli_mapping: CliMapping {
            flag: "--verbose".to_string(),
            value_template: "".to_string(),
        },
    };
    db.upsert_model_parameter("gpt-5.3", "codex", &param)
        .unwrap();

    // Update description
    let mut updated = param.clone();
    updated.description = "Toggle verbose mode".to_string();
    db.upsert_model_parameter("gpt-5.3", "codex", &updated)
        .unwrap();

    let params = db.list_model_parameters("gpt-5.3", "codex").unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].description, "Toggle verbose mode");
}

#[test]
fn list_model_parameters_empty() {
    let db = test_db();
    let params = db
        .list_model_parameters("nonexistent", "nonexistent")
        .unwrap();
    assert!(params.is_empty());
}

#[test]
fn param_type_string_variant() {
    let db = test_db();
    let param = ModelParameter {
        name: "system_prompt".to_string(),
        display_name: "System Prompt".to_string(),
        param_type: ParamType::String,
        description: "The system prompt".to_string(),
        cli_mapping: CliMapping {
            flag: "--system".to_string(),
            value_template: "{value}".to_string(),
        },
    };
    db.upsert_model_parameter("m", "p", &param).unwrap();
    let params = db.list_model_parameters("m", "p").unwrap();
    assert_eq!(params[0].param_type, ParamType::String);
}

const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const CHAIN_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

fn model_store_from_toml(
    fixtures: &[(&str, &str)],
) -> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
    fixtures
        .iter()
        .map(|(name, body)| {
            (
                (*name).to_string(),
                oulipoly_config::ModelConfig::from_toml_with_name(name, body, None).unwrap(),
            )
        })
        .collect()
}

fn resolver_model_store() -> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
    model_store_from_toml(&[
        (
            "claude-opus",
            r#"
[[providers]]
name = "claude"
interactive_args = ["launch"]

[[providers]]
name = "claude2"
interactive_args = ["launch"]
"#,
        ),
        (
            "claude-haiku",
            r#"
[[providers]]
name = "claude"
interactive_args = ["launch"]
"#,
        ),
    ])
}

fn seed_chain_row(db: &StateDb, chain_id: &str, model_name: &str, last_used_at: &str) {
    db.conn
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, ?2, ?2, ?3)",
            sqlite::params![chain_id, last_used_at, model_name],
        )
        .unwrap();
}

fn seed_segment_row(
    db: &StateDb,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
    started_at: &str,
    ended_at: Option<&str>,
    reason: &str,
) {
    db.conn
        .execute(
            "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, ended_at, transition_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            sqlite::params![
                chain_id,
                provider_name,
                session_id,
                started_at,
                ended_at,
                reason
            ],
        )
        .unwrap();
}

pub(crate) fn seed_test_chain(
    db: &StateDb,
    chain_id: &str,
    provider_name: &str,
    session_id: &str,
    model_name: &str,
    last_used_at: &str,
) {
    seed_chain_row(db, chain_id, model_name, last_used_at);
    seed_segment_row(
        db,
        chain_id,
        provider_name,
        session_id,
        last_used_at,
        None,
        "initial",
    );
}

fn seed_invocation_for_session(
    db: &StateDb,
    model_name: &str,
    provider_name: &str,
    session_id: &str,
    created_at: &str,
) {
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(session_id), "fixture")
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET created_at = ?1, finished_at = ?1 WHERE id = ?2",
            sqlite::params![created_at, id],
        )
        .unwrap();
}

fn pre_chain_db_with_turns(rows: &[(&str, &str, &str, &str, &str)]) -> TempDir {
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
            );
            CREATE TABLE invocations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                invocation_uuid TEXT NOT NULL UNIQUE,
                model_name TEXT NOT NULL,
                provider_name TEXT,
                provider_index INTEGER NOT NULL,
                parent_invocation_id INTEGER,
                status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
                success INTEGER,
                exit_code INTEGER,
                error_category TEXT,
                session_id TEXT,
                session_capture_method TEXT,
                resume_acceptance_status TEXT,
                resume_acceptance_evidence TEXT,
                created_at TEXT NOT NULL,
                finished_at TEXT
            );",
    )
    .unwrap();
    for (provider, session, turn, timestamp, role) in rows {
        conn.execute(
            "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, '', ?4)",
            sqlite::params![provider, session, turn, timestamp, role],
        )
        .unwrap();
    }
    mark_current_schema_version(&conn);
    dir
}

fn chain_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
        .unwrap()
}

fn segment_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM session_chain_segments", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn invocation_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

fn invocation_checksum(db: &StateDb) -> String {
    let dual_id_cols = StateDb::invocations_have_dual_id_columns(&db.conn).unwrap();
    let extra_cols = if dual_id_cols {
        " || '|' || COALESCE(session_capture_method, '') \
             || '|' || COALESCE(provider_session_id, '') \
             || '|' || COALESCE(resume_input_id, '') \
             || '|' || COALESCE(provider_session_capture_method, '')"
    } else {
        ""
    };
    let sql = format!(
        "SELECT COALESCE(group_concat(line, char(10)), '')
             FROM (
                 SELECT id || '|' || invocation_uuid || '|' || status || '|' ||
                        COALESCE(session_id, ''){extra_cols} || '|' ||
                        COALESCE(finished_at, '') AS line
                 FROM invocations
                 ORDER BY id
             )"
    );
    db.conn.query_row(&sql, [], |row| row.get(0)).unwrap()
}

// risk: Schema migration and backfill; level: particular-integration; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn backfill_creates_one_chain_per_provider_session_pair() {
    let dir = pre_chain_db_with_turns(&[
        (
            "claude",
            SESSION_A,
            "turn-a1",
            "2026-04-17T08:00:00Z",
            "assistant",
        ),
        (
            "claude",
            SESSION_A,
            "turn-a2",
            "2026-04-17T08:00:01Z",
            "assistant",
        ),
        (
            "claude2",
            SESSION_B,
            "turn-b1",
            "2026-04-17T09:00:00Z",
            "assistant",
        ),
    ]);

    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    assert_eq!(chain_count(&db), 2);
    assert_eq!(segment_count(&db), 2);
    let imported: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM session_chain_segments WHERE transition_reason = 'imported' AND ended_at IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(imported, 2);
}

// risk: Schema migration and backfill; level: particular-integration; source: proposal §11.1 Schema migration and backfill / A5.
#[test]
fn backfill_idempotent_on_second_open() {
    let dir = pre_chain_db_with_turns(&[(
        "claude",
        SESSION_A,
        "turn-a1",
        "2026-04-17T08:00:00Z",
        "assistant",
    )]);
    let path = dir.path().join("state.db");

    let first = StateDb::open(&path).unwrap();
    let first_count = chain_count(&first);
    let first_invocation_checksum = invocation_checksum(&first);
    drop(first);
    let second = StateDb::open(&path).unwrap();

    assert_eq!(chain_count(&second), first_count);
    assert_eq!(segment_count(&second), 1);
    assert_eq!(invocation_checksum(&second), first_invocation_checksum);
}

fn legacy_v4_invocation_dual_id_fixture(
    invocation_uuid: &str,
    session_id: Option<&str>,
    session_capture_method: Option<&str>,
    status: &str,
    terminal_reason: Option<&str>,
    error_category: Option<&str>,
) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    let conn = sqlite::Connection::open(&db_path).unwrap();
    seed_current_drift_required_tables(&conn);
    conn.execute(
        "ALTER TABLE invocations DROP COLUMN provider_session_resolved_account",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO invocations
                (invocation_uuid, model_name, provider_name, provider_index, status, success,
                 exit_code, error_category, terminal_reason, session_id, session_capture_method,
                 created_at, finished_at)
             VALUES (?1, 'claude-opus', 'claude', 0, ?2, NULL, NULL, ?3, ?4, ?5, ?6,
                     '2026-04-17T08:00:00Z', NULL)",
        sqlite::params![
            invocation_uuid,
            status,
            error_category,
            terminal_reason,
            session_id,
            session_capture_method
        ],
    )
    .unwrap();
    conn.pragma_update(None, "user_version", 4).unwrap();
    drop(conn);
    dir
}

#[test]
fn migration_backfill_null_null_preserves_running_rows() {
    let invocation_uuid = "11111111-1111-4111-8111-111111111111";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        None,
        None,
        "running",
        Some("still_running"),
        Some("unknown"),
    );
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    type MigrationBackfillRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
    );

    let row: MigrationBackfillRow = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method, terminal_reason, status, error_category
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .unwrap();

    assert_eq!(row.0, None);
    assert_eq!(row.1, None);
    assert_eq!(row.2, None);
    assert_eq!(row.3, None);
    assert_eq!(row.4.as_deref(), Some("still_running"));
    assert_eq!(row.5, "running");
    assert_eq!(row.6.as_deref(), Some("unknown"));
}

#[test]
fn migration_backfill_resumed_chain_id_safe() {
    let invocation_uuid = "22222222-2222-4222-8222-222222222222";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        Some(CHAIN_A),
        Some("resumed"),
        "succeeded",
        None,
        None,
    );
    {
        let conn = sqlite::Connection::open(dir.path().join("state.db")).unwrap();
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, '2026-04-17T08:00:00Z', '2026-04-17T08:00:00Z', 'claude-opus')",
            sqlite::params![CHAIN_A],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_chain_segments
                    (chain_id, provider_name, session_id, started_at, transition_reason)
                 VALUES (?1, 'claude', ?2, '2026-04-17T08:00:00Z', 'initial')",
            sqlite::params![CHAIN_A, SESSION_A],
        )
        .unwrap();
    }
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let row: (String, Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    let models = resolver_model_store();
    let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

    assert_eq!(row.0, CHAIN_A);
    assert_eq!(row.1, None);
    assert_eq!(row.2.as_deref(), Some(CHAIN_A));
    assert_eq!(row.3, None);
    assert_eq!(resolved.active_session_id, SESSION_A);
}

#[test]
fn migration_backfill_non_resumed_with_session_id() {
    let invocation_uuid = "33333333-3333-4333-8333-333333333333";
    let dir = legacy_v4_invocation_dual_id_fixture(
        invocation_uuid,
        Some(SESSION_A),
        Some("forced_flag_verified"),
        "succeeded",
        None,
        None,
    );
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();

    let row: (String, Option<String>, Option<String>, Option<String>) = db
        .conn
        .query_row(
            "SELECT session_id, provider_session_id, resume_input_id,
                        provider_session_capture_method
                 FROM invocations
                 WHERE invocation_uuid = ?1",
            sqlite::params![invocation_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();

    assert_eq!(row.0, SESSION_A);
    assert_eq!(row.1.as_deref(), Some(SESSION_A));
    assert_eq!(row.2, None);
    assert_eq!(row.3.as_deref(), Some("forced_flag_verified"));
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn mint_chain_no_op_on_resume_of_existing_chain() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T08:00:00Z");

    let first_id = db
        .open_chain_segment(
            CHAIN_A,
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap();
    let second_id = db
        .open_chain_segment(
            CHAIN_A,
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:01:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap();

    assert_eq!(first_id, second_id);
    assert_eq!(segment_count(&db), 1);
    let active: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            sqlite::params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 1);
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn agent_session_chain_records_initial_reason_even_if_ingestion_minted_first() {
    let db = test_db();
    db.mint_imported_chain_if_absent(
        "claude",
        SESSION_A,
        &ts("2026-04-17T08:00:00Z"),
        "<unknown>",
    )
    .unwrap();
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "fixture")
        .unwrap();

    db.mint_chain_for_invocation_session(id).unwrap();

    let reason: String = db
        .conn
        .query_row(
            "SELECT transition_reason FROM session_chain_segments
                 WHERE provider_name = 'claude' AND session_id = ?1",
            sqlite::params![SESSION_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "initial");
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn imported_session_stays_imported_when_no_agent_mint_fires() {
    let db = test_db();

    db.mint_imported_chain_if_absent(
        "claude",
        SESSION_A,
        &ts("2026-04-17T08:00:00Z"),
        "<unknown>",
    )
    .unwrap();

    let reason: String = db
        .conn
        .query_row(
            "SELECT transition_reason FROM session_chain_segments
                 WHERE provider_name = 'claude' AND session_id = ?1",
            sqlite::params![SESSION_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "imported");
}

#[test]
fn find_session_for_invocation_window_returns_fresh_in_window_candidate() {
    let db = test_db();
    let turns = vec![
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "old-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:00Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        },
        SessionTurnIngest {
            session_id: SESSION_B.to_string(),
            turn_id: "fresh-turn".to_string(),
            timestamp: ts("2026-04-17T08:00:02Z"),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        },
    ];
    db.ingest_session_turns_batch("claude", &turns).unwrap();

    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:03Z"),
        )
        .unwrap();

    assert_eq!(found.as_deref(), Some(SESSION_B));
}

#[test]
fn find_session_for_invocation_window_ranks_by_count_earliest_then_session_id() {
    fn turn(session_id: &str, turn_id: &str, timestamp: &str) -> SessionTurnIngest {
        SessionTurnIngest {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            timestamp: ts(timestamp),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }
    }

    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(SESSION_A),
        "higher in-window turn count outranks an earlier first turn"
    );

    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(SESSION_A, "a-1", "2026-04-17T08:00:03Z"),
            turn(SESSION_A, "a-2", "2026-04-17T08:00:05Z"),
            turn(SESSION_B, "b-1", "2026-04-17T08:00:02Z"),
            turn(SESSION_B, "b-2", "2026-04-17T08:00:06Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(SESSION_B),
        "earlier first in-window turn breaks equal counts"
    );

    let lexically_first = "11111111-1111-4111-8111-111111111111";
    let lexically_second = "22222222-2222-4222-8222-222222222222";
    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            turn(lexically_second, "second-1", "2026-04-17T08:00:02Z"),
            turn(lexically_second, "second-2", "2026-04-17T08:00:05Z"),
            turn(lexically_first, "first-1", "2026-04-17T08:00:02Z"),
            turn(lexically_first, "first-2", "2026-04-17T08:00:06Z"),
        ],
    )
    .unwrap();
    let found = db
        .find_session_for_invocation_window(
            "claude",
            &ts("2026-04-17T08:00:01Z"),
            &ts("2026-04-17T08:00:06Z"),
        )
        .unwrap();
    assert_eq!(
        found.as_deref(),
        Some(lexically_first),
        "lexicographic session id breaks equal counts and equal earliest turns"
    );
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_returns_active_segment_for_single_chain() {
    let db = test_db();
    seed_chain_row(&db, CHAIN_A, "claude-opus", "2026-04-17T09:00:00Z");
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude2",
        SESSION_B,
        "2026-04-17T08:31:00Z",
        None,
        "quota_threshold",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, CHAIN_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "claude2");
    assert_eq!(resolved.active_session_id, SESSION_B);
    assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_chooses_most_recent_chain_when_two_chains_share_session_id() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T09:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_B);
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_chooses_most_recent_chain_without_ambiguous_halt() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "claude2",
        SESSION_A,
        "claude-opus",
        "2026-04-17T09:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_C,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T10:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_C);
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_breaks_equal_last_used_tie_by_latest_segment_start() {
    let db = test_db();
    let last_used_at = "2026-04-17T10:00:00Z";
    seed_chain_row(&db, CHAIN_A, "claude-opus", last_used_at);
    seed_segment_row(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        None,
        "initial",
    );
    seed_chain_row(&db, CHAIN_B, "claude-opus", last_used_at);
    seed_segment_row(
        &db,
        CHAIN_B,
        "claude2",
        SESSION_A,
        "2026-04-17T09:00:00Z",
        None,
        "initial",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_B);
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_infers_model_from_latest_invocation() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "<unknown>",
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "claude-haiku",
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
    );
    seed_invocation_for_session(
        &db,
        "claude-opus",
        "claude",
        SESSION_A,
        "2026-04-17T09:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("claude-opus"));
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_falls_back_to_chain_model_name_when_no_invocations() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-haiku",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name.as_deref(), Some("claude-haiku"));
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference / A8.
#[test]
fn resolve_resume_returns_none_model_when_no_inference_source() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "<unknown>",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let resolved = db.resolve_resume(&models, SESSION_A, None).unwrap();

    assert_eq!(resolved.model_name, None);
    assert!(resolved.model.is_none());
}

// risk: Resolver disambiguation and model inference; level: particular-integration; source: proposal §11.1 Resolver disambiguation and model inference.
#[test]
fn resolve_resume_validates_provider_in_model_pool() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude2",
        SESSION_A,
        "claude-haiku",
        "2026-04-17T08:00:00Z",
    );
    let models = resolver_model_store();

    let err = db.resolve_resume(&models, SESSION_A, None).unwrap_err();

    match err {
        ResumeError::ProviderModelMismatch {
            model_name,
            active_provider,
            suggestions,
        } => {
            assert_eq!(model_name, "claude-haiku");
            assert_eq!(active_provider, "claude2");
            assert!(suggestions.contains(&"claude-opus".to_string()));
        }
        other => panic!("expected provider/model mismatch, got {other:?}"),
    }
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn chain_last_used_at_updates_after_successful_invocation() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );

    let before = Utc::now();
    db.update_chain_last_used(CHAIN_A).unwrap();
    let after = Utc::now();

    let last_used_raw: String = db
        .conn
        .query_row(
            "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
            sqlite::params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    let last_used = DateTime::parse_from_rfc3339(&last_used_raw)
        .unwrap()
        .with_timezone(&Utc);
    assert!(last_used >= before - chrono::Duration::seconds(1));
    assert!(last_used <= after + chrono::Duration::seconds(1));
}

// risk: Chain identity write paths; level: particular-integration; source: proposal §11.1 Chain identity write paths / A3, A8.
#[test]
fn chain_identity_helpers_report_sql_errors() {
    let segmentless = db_without_table("session_chain_segments");
    let segment_open_err = segmentless
        .open_chain_segment(
            CHAIN_A,
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            oulipoly_core::TransitionReason::Initial,
        )
        .unwrap_err();
    assert!(
        segment_open_err.contains("session chain segment"),
        "{segment_open_err}"
    );

    let mint_err = db_without_table("session_chain_segments")
        .mint_imported_chain_if_absent(
            "claude",
            SESSION_A,
            &ts("2026-04-17T08:00:00Z"),
            "claude-opus",
        )
        .unwrap_err();
    assert!(
        mint_err.contains("existing session chain segment"),
        "{mint_err}"
    );

    let update_err = db_without_table("session_chains")
        .update_chain_last_used(CHAIN_A)
        .unwrap_err();
    assert!(update_err.contains("last_used_at"), "{update_err}");

    let chain_lookup_err = db_without_table("session_chain_segments")
        .chain_id_for_segment("claude", SESSION_A)
        .unwrap_err();
    assert!(
        chain_lookup_err.contains("session chain id"),
        "{chain_lookup_err}"
    );
}

// risk: Migration mechanic: compaction-aware Claude target build; level: particular-integration; source: proposal §11.1 Migration mechanic: compaction-aware Claude target build / A3, A6.
#[test]
fn compaction_and_preview_helpers_report_negative_paths() {
    let malformed_uuid = test_db().resume_previews("not-a-uuid").unwrap_err();
    assert!(malformed_uuid.contains("Invalid UUID"), "{malformed_uuid}");

    let db = test_db();
    db.conn
            .execute(
                "INSERT INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, source_file, ingested_at, is_compaction_boundary)
                 VALUES ('claude', ?1, 'bad-boundary', 'not-a-timestamp', 'assistant', '', '2026-04-17T08:00:00Z', 1)",
                sqlite::params![SESSION_A],
            )
            .unwrap();

    let boundary_err = db
        .latest_compaction_boundary("claude", SESSION_A)
        .unwrap_err();
    assert!(
        boundary_err.contains("Bad compaction boundary timestamp"),
        "{boundary_err}"
    );
}

// risk: Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races; level: particular-integration; source: proposal §11.1 Migration mechanic / A3.
#[test]
fn migration_returning_clause_aborts_on_concurrent_close() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );

    let first = db
        .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:00Z"))
        .unwrap();
    let second = db
        .close_active_segment_returning(CHAIN_A, &ts("2026-04-17T09:00:01Z"))
        .unwrap();

    assert!(first.is_some(), "first close should win RETURNING guard");
    assert_eq!(second, None, "concurrent loser must abort");
    let active: i64 = db
        .conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            sqlite::params![CHAIN_A],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(active, 0);
}

#[test]
fn age132_invocation_projection_maps_full_row_and_rejects_bad_values() {
    let db = test_db();
    let invocation_uuid = "44444444-4444-4444-8444-444444444444";
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 7,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "verified")
        .unwrap();
    db.update_resume_acceptance(id, "accepted", Some("matched"))
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations
                 SET status = 'succeeded',
                     success = 1,
                     exit_code = 0,
                     terminal_reason = 'exit_zero',
                     created_at = '2026-04-17T08:00:00Z',
                     finished_at = '2026-04-17T08:00:02Z'
                 WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();

    let record = db.get_invocation_by_uuid(invocation_uuid).unwrap().unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.invocation_uuid, invocation_uuid);
    assert_eq!(record.model_name, "claude-opus");
    assert_eq!(record.provider_name.as_deref(), Some("claude"));
    assert_eq!(record.provider_index, 7);
    assert_eq!(record.parent_invocation_id, None);
    assert_eq!(record.status, InvocationStatus::Succeeded);
    assert_eq!(record.success, Some(true));
    assert_eq!(record.exit_code, Some(0));
    assert_eq!(record.terminal_reason.as_deref(), Some("exit_zero"));
    assert_eq!(record.session_id.as_deref(), Some(SESSION_A));
    assert_eq!(record.provider_session_id.as_deref(), Some(SESSION_A));
    assert_eq!(
        record.provider_session_capture_method.as_deref(),
        Some("verified")
    );
    assert_eq!(record.resume_acceptance_status.as_deref(), Some("accepted"));
    assert_eq!(
        record.resume_acceptance_evidence.as_deref(),
        Some("matched")
    );
    assert_eq!(record.created_at, ts("2026-04-17T08:00:00Z"));
    assert_eq!(record.finished_at, Some(ts("2026-04-17T08:00:02Z")));

    let child_uuid = "55555555-5555-5555-8555-555555555555";
    let child_id = insert_invocation_fixture(&db, child_uuid, Some(id), "2026-04-17T08:00:01Z");
    let children = db.list_invocation_children(id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].id, child_id);
    assert_eq!(children[0].invocation_uuid, child_uuid);
    assert_eq!(children[0].parent_invocation_id, Some(id));
    assert_eq!(children[0].created_at, ts("2026-04-17T08:00:01Z"));

    db.conn
        .pragma_update(None, "ignore_check_constraints", true)
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET status = 'paused' WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();
    let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
    assert!(err.contains("Unknown invocation status: paused"), "{err}");
    db.conn
            .execute(
                "UPDATE invocations SET status = 'running', created_at = 'not-a-timestamp' WHERE id = ?1",
                sqlite::params![id],
            )
            .unwrap();
    let err = db.get_invocation_by_uuid(invocation_uuid).unwrap_err();
    assert!(err.contains("Conversion error"), "{err}");
}

#[test]
fn age132_backfill_infers_model_from_latest_matching_invocation() {
    let db = test_db();
    db.ingest_session_turns_batch(
        "claude",
        &[
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-a1".to_string(),
                timestamp: ts("2026-04-17T08:00:00Z"),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "turn-a2".to_string(),
                timestamp: ts("2026-04-17T08:01:00Z"),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    seed_invocation_for_session(
        &db,
        "claude-haiku",
        "claude",
        SESSION_A,
        "2026-04-17T08:00:30Z",
    );
    seed_invocation_for_session(
        &db,
        "claude-opus",
        "claude",
        SESSION_A,
        "2026-04-17T08:01:30Z",
    );

    let report = db.backfill_session_chains().unwrap();
    assert_eq!(
        report,
        BackfillReport {
            skipped_existing: false,
            chains_inserted: 1,
            segments_inserted: 1
        }
    );
    let model_name: String = db
        .conn
        .query_row("SELECT model_name FROM session_chains", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(model_name, "claude-opus");
}

#[test]
fn age132_resolve_resume_rejections_and_wrong_id_context_are_typed() {
    let models = resolver_model_store();
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "not-a-uuid", None)
            .unwrap_err(),
        ResumeError::InvalidUuid { .. }
    ));
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "ses_ab", None)
            .unwrap_err(),
        ResumeError::InvalidUuid { .. }
    ));
    assert!(matches!(
        test_db()
            .resolve_resume(&models, "77777777-7777-4777-8777-777777777777", None)
            .unwrap_err(),
        ResumeError::NoChainFound { .. }
    ));

    let unknown_model_db = test_db();
    seed_test_chain(
        &unknown_model_db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "missing-model",
        "2026-04-17T08:00:00Z",
    );
    assert!(matches!(
        unknown_model_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
        ResumeError::UnknownModel { ref model_name } if model_name == "missing-model"
    ));

    let missing_segment_db = test_db();
    seed_chain_row(
        &missing_segment_db,
        CHAIN_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_segment_row(
        &missing_segment_db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "2026-04-17T08:00:00Z",
        Some("2026-04-17T08:30:00Z"),
        "initial",
    );
    assert!(matches!(
        missing_segment_db.resolve_resume(&models, SESSION_A, None).unwrap_err(),
        ResumeError::ActiveSegmentMissing { ref chain_id } if chain_id == CHAIN_A
    ));

    let wrong_id_db = test_db();
    let invocation_uuid = "88888888-8888-4888-8888-888888888888";
    let id = wrong_id_db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    wrong_id_db
        .bind_invocation_provider_session_start(
            id,
            &ProviderSessionBinding {
                provider_session_id: SESSION_A.to_string(),
                capture_method: "verified",
                resume_input_id: None,
                provider_session_resolved_account: None,
            },
        )
        .unwrap();
    match wrong_id_db
        .resolve_resume(&models, invocation_uuid, None)
        .unwrap_err()
    {
        ResumeError::WrongIdKind {
            provider_session_id,
            chain_id,
            provider_name,
            agent_runner_invocation_id,
            ..
        } => {
            assert_eq!(provider_session_id.as_deref(), Some(SESSION_A));
            assert!(chain_id.is_some());
            assert_eq!(provider_name.as_deref(), Some("claude"));
            assert_eq!(agent_runner_invocation_id, invocation_uuid);
        }
        other => panic!("expected wrong-id-kind rejection, got {other:?}"),
    }
}

#[test]
fn resolve_resume_accepts_opencode_provider_session_id() {
    let db = test_db();
    let models = model_store_from_toml(&[(
        "gpt-high",
        r#"
[[providers]]
name = "opencode"
interactive_args = ["run"]
"#,
    )]);
    seed_test_chain(
        &db,
        CHAIN_A,
        "opencode",
        "ses_fixture",
        "gpt-high",
        "2026-06-04T08:00:00Z",
    );

    let resolved = db.resolve_resume(&models, "ses_fixture", None).unwrap();

    assert_eq!(resolved.chain_id, CHAIN_A);
    assert_eq!(resolved.active_provider, "opencode");
    assert_eq!(resolved.active_session_id, "ses_fixture");
    assert_eq!(resolved.model_name.as_deref(), Some("gpt-high"));
}

#[test]
fn age132_timestamp_policies_preserve_strict_forgiving_and_fallback_callers() {
    let db = test_db();
    db.upsert_quota_refresh("claude", &[quota_input(0.40, "2026-04-22T00:00:00Z")])
        .unwrap();
    db.conn
        .execute(
            "UPDATE provider_quotas
                 SET refreshed_at = 'bad-refreshed',
                     exhausted_at = 'bad-exhausted',
                     last_topology_probe_at = 'bad-probe'
                 WHERE provider_name = 'claude'",
            [],
        )
        .unwrap();
    let quota = db.get_quota("claude").unwrap().unwrap();
    assert_eq!(quota.refreshed_at, None);
    assert_eq!(quota.exhausted_at, None);
    assert_eq!(quota.last_topology_probe_at, None);
    db.conn
            .execute(
                "UPDATE provider_quota_windows SET resets_at = 'bad-window' WHERE provider_name = 'claude'",
                [],
            )
            .unwrap();
    assert!(db.get_windows("claude").is_err());

    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    db.update_session_capture(id, Some(SESSION_A), "verified")
        .unwrap();
    db.conn
        .execute(
            "UPDATE invocations SET created_at = 'not-a-timestamp' WHERE id = ?1",
            sqlite::params![id],
        )
        .unwrap();
    let before = Utc::now();
    db.mint_chain_for_invocation_session(id).unwrap();
    let after = Utc::now();
    let raw_started: String = db
            .conn
            .query_row(
                "SELECT started_at FROM session_chain_segments WHERE provider_name = 'claude' AND session_id = ?1",
                sqlite::params![SESSION_A],
                |row| row.get(0),
            )
            .unwrap();
    let started_at = DateTime::parse_from_rfc3339(&raw_started)
        .unwrap()
        .with_timezone(&Utc);
    assert!(started_at >= before - chrono::Duration::seconds(1));
    assert!(started_at <= after + chrono::Duration::seconds(1));
}

#[test]
fn age132_invocation_artifact_contract_and_warning_only_failure_paths() {
    let memory = StateDb::open(Path::new(":memory:")).unwrap();
    let memory_id = memory
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    memory
        .finalize_invocation(memory_id, true, 0, None, None)
        .unwrap();
    let memory_status: String = memory
        .connection()
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            sqlite::params![memory_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(memory_status, "succeeded");

    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(&dir.path().join("state.db")).unwrap();
    let invocation_uuid = "99999999-9999-4999-8999-999999999999";
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let invocation_path = dir
        .path()
        .join("invocations")
        .join(format!("{invocation_uuid}.invocation"));
    assert!(invocation_path.exists());
    assert!(!invocation_path.with_extension("invocation.tmp").exists());
    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&invocation_path).unwrap()).unwrap();
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "running");
    assert_eq!(payload["model_name"], "claude-opus");
    assert_eq!(payload["provider_name"], "claude");
    assert!(payload["pid"].as_u64().is_some());
    assert!(DateTime::parse_from_rfc3339(payload["started_at"].as_str().unwrap()).is_ok());

    db.finalize_invocation(id, false, 42, Some("rate_limit"), Some("limited"))
        .unwrap();
    let result_path = dir
        .path()
        .join("invocations")
        .join(format!("{invocation_uuid}.result"));
    assert!(result_path.exists());
    assert!(!result_path.with_extension("result.tmp").exists());
    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result_path).unwrap()).unwrap();
    assert_eq!(payload["id"], invocation_uuid);
    assert_eq!(payload["status"], "failed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["exit_code"], 42);
    assert_eq!(payload["error_category"], "rate_limit");
    assert_eq!(payload["terminal_reason"], "limited");
    assert!(DateTime::parse_from_rfc3339(payload["finished_at"].as_str().unwrap()).is_ok());

    let failing_dir = tempfile::tempdir().unwrap();
    let failing = StateDb::open(&failing_dir.path().join("state.db")).unwrap();
    std::fs::write(failing_dir.path().join("invocations"), b"not a directory").unwrap();
    let id = failing
        .start_invocation(&InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    failing
        .finalize_invocation(id, true, 0, None, None)
        .unwrap();
    let status: String = failing
        .conn
        .query_row(
            "SELECT status FROM invocations WHERE id = ?1",
            sqlite::params![id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "succeeded");
}

fn returned_artifact_ref(
    invocation_uuid: Uuid,
    artifact_name: &str,
    version: u64,
) -> ReturnedArtifactRef {
    let workflow_run_id = format!("return:{invocation_uuid}");
    let version_id = format!("store://return/{invocation_uuid}/{artifact_name}/{version}");
    ReturnedArtifactRef {
        version_id,
        name: artifact_name.to_string(),
        store_address: oulipoly_agent_messenger::StoreAddress {
            workflow_run_id,
            artifact_name: artifact_name.to_string(),
            version,
        },
        sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        content_len: 123,
        format_hint: Some("text/plain".to_string()),
        verdict_line: Some("ok".to_string()),
        source: oulipoly_agent_messenger::ReturnedArtifactSource::Scratchpad {
            name: "notes".to_string(),
            version: 1,
        },
        producer_invocation_uuid: invocation_uuid,
        returned_at: ts("2026-04-17T08:00:00Z"),
    }
}

#[test]
fn age132_returned_artifacts_validate_identity_bounds_and_rollback_failed_retry() {
    let db = test_db();
    let invocation_uuid = Uuid::new_v4();
    let id = db
        .start_invocation(&InvocationStart {
            invocation_uuid: invocation_uuid.to_string(),
            model_name: "claude-opus".to_string(),
            provider_name: "claude".to_string(),
            provider_index: 0,
            parent_invocation_id: None,
        })
        .unwrap();
    let good = returned_artifact_ref(invocation_uuid, "alpha.txt", 1);
    db.record_returned_artifacts(id, std::slice::from_ref(&good))
        .unwrap();

    let mut bad_workflow = returned_artifact_ref(invocation_uuid, "bad-workflow.txt", 1);
    bad_workflow.store_address.workflow_run_id = "not-return-namespace".to_string();
    assert!(
        db.record_returned_artifacts(id, &[bad_workflow])
            .unwrap_err()
            .contains("workflow_run_id")
    );

    let mut bad_version = returned_artifact_ref(invocation_uuid, "bad-version.txt", 1);
    bad_version.version_id = "store://wrong-version".to_string();
    assert!(
        db.record_returned_artifacts(id, &[bad_version])
            .unwrap_err()
            .contains("version_id mismatch")
    );

    let mut overflow = returned_artifact_ref(invocation_uuid, "overflow.txt", 1);
    overflow.content_len = u64::MAX;
    assert!(
        db.record_returned_artifacts(id, &[overflow])
            .unwrap_err()
            .contains("content_len exceeds SQLite INTEGER range")
    );
    assert_eq!(db.list_returned_artifacts(id).unwrap(), vec![good]);
}

#[test]
fn age132_session_turn_ingest_batch_and_single_paths_preserve_mapping_and_atomicity() {
    let db = test_db();
    let timestamp = ts("2026-04-17T08:00:00Z");
    assert!(
        db.ingest_session_turn(
            "claude",
            SESSION_A,
            "single-turn",
            &timestamp,
            "assistant",
            "/tmp/session.jsonl",
        )
        .unwrap()
    );
    assert!(
        !db.ingest_session_turn(
            "claude",
            SESSION_A,
            "single-turn",
            &timestamp,
            "assistant",
            "/tmp/session.jsonl",
        )
        .unwrap()
    );
    let source_file: String = db
        .conn
        .query_row(
            "SELECT source_file FROM session_turns WHERE turn_id = 'single-turn'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(source_file, "/tmp/session.jsonl");

    let turns = vec![
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "turn-1".to_string(),
            timestamp,
            role: "user".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some("hello".to_string()),
        },
        SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: "turn-2".to_string(),
            timestamp: timestamp + chrono::Duration::seconds(1),
            role: "assistant".to_string(),
            parent_turn_id: Some("turn-1".to_string()),
            is_sidechain: true,
            is_compaction_boundary: true,
            body: Some("world".to_string()),
        },
    ];
    assert_eq!(db.ingest_session_turns_batch("claude", &turns).unwrap(), 2);
    assert_eq!(db.ingest_session_turns_batch("claude", &turns).unwrap(), 0);
    let row: (Option<String>, i64, i64, Option<String>) = db
        .conn
        .query_row(
            "SELECT parent_turn_id, is_sidechain, is_compaction_boundary, body
                 FROM session_turns WHERE turn_id = 'turn-2'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        row,
        (Some("turn-1".to_string()), 1, 1, Some("world".to_string()))
    );

    let failing = test_db();
    failing
        .conn
        .execute_batch(
            "CREATE TRIGGER fail_bad_turn
                 BEFORE INSERT ON session_turns
                 WHEN NEW.turn_id = 'bad'
                 BEGIN
                   SELECT RAISE(ABORT, 'bad turn');
                 END;",
        )
        .unwrap();
    assert!(
        failing
            .ingest_session_turns_batch(
                "claude",
                &[
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "good-before-error".to_string(),
                        timestamp,
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: false,
                        body: None,
                    },
                    SessionTurnIngest {
                        session_id: SESSION_A.to_string(),
                        turn_id: "bad".to_string(),
                        timestamp: timestamp + chrono::Duration::seconds(1),
                        role: "assistant".to_string(),
                        parent_turn_id: None,
                        is_sidechain: false,
                        is_compaction_boundary: false,
                        body: None,
                    },
                ],
            )
            .unwrap_err()
            .contains("bad turn")
    );
    let persisted: i64 = failing
        .conn
        .query_row("SELECT COUNT(*) FROM session_turns", [], |row| row.get(0))
        .unwrap();
    assert_eq!(persisted, 0);
}

#[test]
fn age132_resume_previews_and_compaction_boundaries_preserve_ordering_contracts() {
    let db = test_db();
    seed_test_chain(
        &db,
        CHAIN_A,
        "claude",
        SESSION_A,
        "claude-opus",
        "2026-04-17T08:00:00Z",
    );
    seed_test_chain(
        &db,
        CHAIN_B,
        "claude2",
        SESSION_A,
        "claude-opus",
        "2026-04-17T09:00:00Z",
    );
    let turns: Vec<_> = (0..4)
        .map(|i| SessionTurnIngest {
            session_id: SESSION_A.to_string(),
            turn_id: format!("turn-{i}"),
            timestamp: ts(&format!("2026-04-17T08:00:0{i}Z")),
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: Some(format!("body-{i}")),
        })
        .collect();
    db.ingest_session_turns_batch("claude2", &turns).unwrap();

    let previews = db.resume_previews(SESSION_A).unwrap();
    assert_eq!(previews.len(), 2);
    assert_eq!(previews[0].chain_id, CHAIN_B);
    assert_eq!(previews[0].active_provider, "claude2");
    assert_eq!(previews[0].turn_count, 4);
    assert_eq!(previews[0].recent_turns.len(), 3);
    assert_eq!(
        previews[0].recent_turns[0].timestamp,
        ts("2026-04-17T08:00:01Z")
    );
    assert_eq!(
        previews[0].recent_turns[1].timestamp,
        ts("2026-04-17T08:00:02Z")
    );
    assert_eq!(
        previews[0].recent_turns[2].timestamp,
        ts("2026-04-17T08:00:03Z")
    );
    assert_eq!(previews[0].recent_turns[0].snippet, None);
    assert_eq!(previews[1].chain_id, CHAIN_A);

    let boundary_db = test_db();
    boundary_db
        .ingest_session_turns_batch(
            "claude",
            &[
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "old-boundary".to_string(),
                    timestamp: ts("2026-04-17T08:00:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "tie-first".to_string(),
                    timestamp: ts("2026-04-17T08:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "tie-second".to_string(),
                    timestamp: ts("2026-04-17T08:01:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: true,
                    body: None,
                },
                SessionTurnIngest {
                    session_id: SESSION_A.to_string(),
                    turn_id: "not-yet-boundary".to_string(),
                    timestamp: ts("2026-04-17T08:02:00Z"),
                    role: "assistant".to_string(),
                    parent_turn_id: None,
                    is_sidechain: false,
                    is_compaction_boundary: false,
                    body: None,
                },
            ],
        )
        .unwrap();
    let latest = boundary_db
        .latest_compaction_boundary("claude", SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(latest.0, "tie-second");
    assert_eq!(latest.1, ts("2026-04-17T08:01:00Z"));
    assert!(
        boundary_db
            .flag_compaction_boundary("claude", SESSION_A, "not-yet-boundary")
            .unwrap()
    );
    assert!(
        !boundary_db
            .flag_compaction_boundary("claude", SESSION_A, "not-yet-boundary")
            .unwrap()
    );
    assert!(
        !boundary_db
            .flag_compaction_boundary("claude", SESSION_A, "missing-turn")
            .unwrap()
    );
    let latest = boundary_db
        .latest_compaction_boundary("claude", SESSION_A)
        .unwrap()
        .unwrap();
    assert_eq!(latest.0, "not-yet-boundary");
    assert_eq!(latest.1, ts("2026-04-17T08:02:00Z"));
    assert_eq!(
        test_db()
            .latest_compaction_boundary("claude", SESSION_A)
            .unwrap(),
        None
    );
}

#[test]
fn age132_read_only_error_classifier_and_sidecar_paths_map_documented_variants() {
    let missing_dir = tempfile::tempdir().unwrap();
    let missing_path = missing_dir.path().join("missing-state.db");
    match StateDb::open_read_only(&missing_path) {
        Err(ReadOnlyOpenError::Missing { path }) => assert_eq!(path, missing_path),
        Ok(_) => panic!("expected Missing, got successful read-only open"),
        Err(other) => panic!("expected Missing, got {other:?}"),
    }

    let malformed_dir = tempfile::tempdir().unwrap();
    let malformed_path = malformed_dir.path().join("state.db");
    std::fs::write(&malformed_path, b"not sqlite").unwrap();
    match StateDb::open_read_only(&malformed_path) {
        Err(ReadOnlyOpenError::NotADatabase { path: p, .. }) => {
            assert_eq!(p, malformed_path);
        }
        Ok(_) => panic!("expected NotADatabase, got successful read-only open"),
        Err(other) => panic!("expected NotADatabase, got {other:?}"),
    }

    let valid_dir = tempfile::tempdir().unwrap();
    let valid_path = valid_dir.path().join("state.db");
    drop(StateDb::open(&valid_path).unwrap());
    drop(StateDb::open_read_only(&valid_path).unwrap());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let denied_dir = tempfile::tempdir().unwrap();
        let denied_path = denied_dir.path().join("state.db");
        drop(StateDb::open(&denied_path).unwrap());
        let mut denied_permissions = std::fs::metadata(&denied_path).unwrap().permissions();
        denied_permissions.set_mode(0o000);
        std::fs::set_permissions(&denied_path, denied_permissions).unwrap();
        match StateDb::open_read_only(&denied_path) {
            Err(ReadOnlyOpenError::PermissionDenied { path }) => assert_eq!(path, denied_path),
            Ok(_) => panic!("expected PermissionDenied, got successful read-only open"),
            Err(other) => panic!("expected PermissionDenied, got {other:?}"),
        }

        let sidecar_dir = tempfile::tempdir().unwrap();
        let sidecar_path = sidecar_dir.path().join("state.db");
        let sidecar_conn = sqlite::Connection::open(&sidecar_path).unwrap();
        sidecar_conn
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                     CREATE TABLE sidecar_probe (value TEXT);
                     INSERT INTO sidecar_probe (value) VALUES ('kept open');",
            )
            .unwrap();
        let sidecar_file = std::fs::read_dir(sidecar_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path != &sidecar_path && path.is_file())
            .expect("WAL mode should create at least one SQLite sidecar file");
        let mut sidecar_permissions = std::fs::metadata(&sidecar_file).unwrap().permissions();
        sidecar_permissions.set_mode(0o000);
        std::fs::set_permissions(&sidecar_file, sidecar_permissions).unwrap();
        match StateDb::open_read_only(&sidecar_path) {
            Err(ReadOnlyOpenError::WalSidecarError { path, message }) => {
                assert_eq!(path, sidecar_path);
                assert!(message.contains("sidecar"), "{message}");
            }
            Ok(_) => panic!("expected WalSidecarError, got successful read-only open"),
            Err(other) => panic!("expected WalSidecarError, got {other:?}"),
        }
        drop(sidecar_conn);
    }

    match StateDb::open_read_only(valid_dir.path()) {
        Err(ReadOnlyOpenError::Operational { message }) => {
            assert!(!message.is_empty());
        }
        Ok(_) => panic!("expected Operational, got successful read-only open"),
        Err(other) => panic!("expected operational mapping, got {other:?}"),
    }
}

#[test]
fn age132_setup_crud_count_and_call_counter_edge_contracts() {
    let db = test_db();
    db.upsert_cli_provider(&sample_provider()).unwrap();
    let expired = AccountRecord {
        id: "expired".to_string(),
        provider: "claude".to_string(),
        profile_name: "expired-profile".to_string(),
        auth_method: AuthMethod::OAuth,
        auth_status: AuthStatus::Expired,
        created_at: "2026-02-19T00:00:00Z".to_string(),
    };
    db.insert_account(&expired).unwrap();
    db.conn
        .execute(
            "UPDATE accounts SET auth_status = 'surprise' WHERE id = 'expired'",
            [],
        )
        .unwrap();
    let accounts = db.list_accounts(Some("claude")).unwrap();
    assert_eq!(accounts[0].auth_status, AuthStatus::Unknown);
    assert!(!db.delete_account("missing", "claude").unwrap());
    assert_eq!(
        db.delete_stale_models("claude", "missing-version").unwrap(),
        0
    );

    let since = ts("2026-04-17T08:00:00Z");
    db.ingest_session_turns_batch(
        "claude",
        &[
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "at-boundary".to_string(),
                timestamp: since,
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "after-boundary".to_string(),
                timestamp: since + chrono::Duration::seconds(1),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
            SessionTurnIngest {
                session_id: SESSION_A.to_string(),
                turn_id: "user-after-boundary".to_string(),
                timestamp: since + chrono::Duration::seconds(2),
                role: "user".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            },
        ],
    )
    .unwrap();
    assert_eq!(db.count_assistant_turns_since("claude", None).unwrap(), 2);
    assert_eq!(
        db.count_assistant_turns_since("claude", Some(&since))
            .unwrap(),
        1
    );
    assert_eq!(db.count_assistant_turns_since("codex", None).unwrap(), 0);
    db.increment_calls_since_refresh("claude").unwrap();
    db.increment_calls_since_refresh("claude").unwrap();
    assert_eq!(calls_since_refresh(&db, "claude"), 2);
}

// TI-04, TI-12, TI-24: ordered migration steps must fail with actionable
// rebuild guidance and roll back both schema effects and user_version.
#[test]
fn ti_04_ti_12_ti_24_ordered_migration_failure_rolls_back_and_reports_rebuild() {
    use crate::migrations::{self, Migration, MigrationError};

    let (target_version, id, sql) = failing_migration::failing_migration_parts();
    let failing = Migration {
        target_version,
        id,
        sql,
        post_sql_hook: None,
    };

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("state.db");
    let mut conn = sqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "
            PRAGMA user_version = 3;
            CREATE TABLE preserved_rows (id INTEGER PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO preserved_rows (id, value) VALUES (1, 'before');
            ",
    )
    .unwrap();

    let err = migrations::run_with_db_path(&mut conn, &[&failing], db_path.clone()).unwrap_err();
    let message = err.to_string();

    assert!(
        message.contains(failing_migration::FAILING_MIGRATION_ID),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "target_version={}",
            failing_migration::FAILING_MIGRATION_TARGET_VERSION
        )),
        "{message}"
    );
    assert!(message.contains("agents migrate --rebuild"), "{message}");
    assert!(
        message.contains(&format!("db={}", db_path.display())),
        "{message}"
    );
    assert!(
        matches!(err, MigrationError::StepFailed { .. }),
        "expected StepFailed"
    );

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);
    let preserved: String = conn
        .query_row("SELECT value FROM preserved_rows WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(preserved, "before");
    let marker_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = 'age32_failure_marker'",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(marker_exists, 0, "failed migration left partial schema");
}

fn age160_sqlite_failure(
    code: sqlite::ffi::ErrorCode,
    extended_code: i32,
    message: &str,
) -> sqlite::Error {
    sqlite::Error::SqliteFailure(
        sqlite::ffi::Error {
            code,
            extended_code,
        },
        Some(message.to_string()),
    )
}

fn age160_assert_not_database(error: ReadOnlyOpenError, expected_path: &Path) {
    match error {
        ReadOnlyOpenError::NotADatabase { path, .. } => assert_eq!(path, expected_path),
        other => panic!("expected NotADatabase, got {other:?}"),
    }
}

fn age160_assert_permission_denied(error: ReadOnlyOpenError, expected_path: &Path) {
    match error {
        ReadOnlyOpenError::PermissionDenied { path } => assert_eq!(path, expected_path),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

fn age160_assert_operational(error: ReadOnlyOpenError) {
    match error {
        ReadOnlyOpenError::Operational { message } => assert!(!message.is_empty()),
        other => panic!("expected Operational, got {other:?}"),
    }
}

fn age160_assert_wal_sidecar(error: ReadOnlyOpenError, expected_path: &Path) {
    match error {
        ReadOnlyOpenError::WalSidecarError { path, message } => {
            assert_eq!(path, expected_path);
            assert!(!message.is_empty());
        }
        other => panic!("expected WalSidecarError, got {other:?}"),
    }
}

/// AGE-160 risk: PP-001 push-pull / PM-01 typed read-only SQLite error projection.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track; validates A1 and
/// the "do not parse diagnostic strings" forbidden behavior.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_not_database_permission_and_plain_unknown()
 {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_not_database(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::NotADatabase,
                sqlite::ffi::ErrorCode::NotADatabase as i32,
                "private diagnostic mentions wal but code is not-a-database",
            ),
        ),
        &path,
    );
    age160_assert_not_database(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::DatabaseCorrupt,
                sqlite::ffi::ErrorCode::DatabaseCorrupt as i32,
                "private diagnostic mentions shared memory but code is corrupt",
            ),
        ),
        &path,
    );
    age160_assert_permission_denied(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::PermissionDenied,
                sqlite::ffi::ErrorCode::PermissionDenied as i32,
                "permission denied",
            ),
        ),
        &path,
    );

    for (code, message) in [
        (
            sqlite::ffi::ErrorCode::SystemIoFailure,
            "plain SystemIoFailure must ignore wal/-shm diagnostic tokens",
        ),
        (
            sqlite::ffi::ErrorCode::ReadOnly,
            "read only database with wal-shaped private text",
        ),
        (
            sqlite::ffi::ErrorCode::CannotOpen,
            "cannot open database with shared memory-shaped private text",
        ),
    ] {
        age160_assert_operational(classify_read_only_open_error(
            &path,
            age160_sqlite_failure(code, code as i32, message),
        ));
    }
}

/// AGE-160 risk: PP-001 push-pull + A2 sidecar evidence.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track; validates A2.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_wal_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    std::fs::write(&path, b"placeholder").unwrap();
    std::fs::write(wal_path(&path), b"owned wal sidecar").unwrap();

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::SystemIoFailure,
                sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                "plain io failure text intentionally lacks sidecar tokens",
            ),
        ),
        &path,
    );

    let dirty_wal_path = temp.path().join("dirty-wal-state.db");
    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &dirty_wal_path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::CannotOpen,
                sqlite::ffi::SQLITE_CANTOPEN_DIRTYWAL,
                "dirty WAL extended code without diagnostic-token dependency",
            ),
        ),
        &dirty_wal_path,
    );
}

/// AGE-160 risk: PP-001 push-pull + A2 READONLY_CANTLOCK projection.
/// Selected level: unit.
/// Source: Phase 8 PR-review remediation; covers the typed extended-code branch.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_readonly_cantlock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::ReadOnly,
                sqlite::ffi::SQLITE_READONLY_CANTLOCK,
                "readonly cantlock extended code without diagnostic-token dependency",
            ),
        ),
        &path,
    );
}

/// AGE-160 risk: PP-001 push-pull + A2 READONLY_RECOVERY projection.
/// Selected level: unit.
/// Source: Phase 8 PR-review remediation; covers the typed extended-code branch.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_readonly_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::ReadOnly,
                sqlite::ffi::SQLITE_READONLY_RECOVERY,
                "readonly recovery extended code without diagnostic-token dependency",
            ),
        ),
        &path,
    );
}

/// AGE-160 risk: PP-001 push-pull + A2 owned SHM sidecar probe evidence.
/// Selected level: unit.
/// Source: Phase 8 PR-review remediation; validates the `shm_exists` probe path.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_probe_path_branch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    std::fs::write(&path, b"placeholder").unwrap();
    std::fs::write(shm_path(&path), b"owned shm sidecar").unwrap();

    assert!(
        !wal_path(&path).exists(),
        "fixture should exercise only the shm sidecar probe branch"
    );
    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::SystemIoFailure,
                sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                "plain io failure text intentionally lacks sidecar tokens",
            ),
        ),
        &path,
    );
}

/// AGE-160 risk: PP-001 push-pull + A2 SHM extended-code projection.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track; validates A2/A3.
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_extended_codes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    for extended_code in [
        sqlite::ffi::SQLITE_IOERR_SHMOPEN,
        sqlite::ffi::SQLITE_IOERR_SHMSIZE,
        sqlite::ffi::SQLITE_IOERR_SHMLOCK,
        sqlite::ffi::SQLITE_IOERR_SHMMAP,
    ] {
        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::SystemIoFailure,
                    extended_code,
                    "typed SHM sidecar evidence; message intentionally generic",
                ),
            ),
            &path,
        );
    }
}

/// AGE-160 risk: A6 db.rs↔SQLite namespace contraction.
/// Selected level: unit + compile.
/// Source: the AGE-160 proposal § Test-intent track; validates A7.
///
#[test]
fn age160_sqlite_adapter_read_only_projection_and_namespace_contract() {
    use crate::db::sqlite_adapter::{
        Connection as AdapterConnection, OpenFlags as AdapterOpenFlags,
        OptionalExtension as AdapterOptionalExtension, ReadOnlyOpenFailure, Row as AdapterRow,
        SidecarProbe, SqliteFailureProjection, Statement as AdapterStatement,
        Transaction as AdapterTransaction, params as adapter_params,
    };

    fn _accept_row(_: &AdapterRow<'_>) {}
    fn _accept_statement(_: &mut AdapterStatement<'_>) {}
    fn _accept_transaction(_: &AdapterTransaction<'_>) {}
    fn _accept_optional<T: AdapterOptionalExtension>(value: T) -> T {
        value
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    let conn = AdapterConnection::open_with_flags(
        &path,
        AdapterOpenFlags::SQLITE_OPEN_READ_WRITE | AdapterOpenFlags::SQLITE_OPEN_CREATE,
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE contract_probe (id INTEGER PRIMARY KEY)",
        adapter_params![],
    )
    .unwrap();

    let projection = SqliteFailureProjection::from(&age160_sqlite_failure(
        sqlite::ffi::ErrorCode::NotADatabase,
        sqlite::ffi::ErrorCode::NotADatabase as i32,
        "not db",
    ));
    assert!(matches!(
        ReadOnlyOpenFailure::from_projection(&path, projection, SidecarProbe::for_db(&path)),
        ReadOnlyOpenFailure::PlainDb { .. }
    ));
    let _ = _accept_optional(Ok::<Option<i64>, sqlite::Error>(Some(1)));
}

/// AGE-160 risk: PP-004 declared marker grammar.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_composite_invocation_id_declared_grammar_canonical_json_round_trip() {
    let known_uuid = Uuid::parse_str("7ad2916c-38dd-49e6-a1f7-3ef22766ff70").unwrap();
    let composite = CompositeInvocationId {
        source: "codex2".to_string(),
        id: known_uuid.to_string(),
    };

    let stderr_line = composite.stderr_line();
    assert!(stderr_line.starts_with("OULIPOLY_INVOCATION="));
    let payload = stderr_line
        .strip_prefix("OULIPOLY_INVOCATION=")
        .expect("stderr marker prefix");
    assert!(!payload.starts_with("OULIPOLY_INVOCATION="));
    assert_eq!(
        payload,
        r#"{"source":"codex2","id":"7ad2916c-38dd-49e6-a1f7-3ef22766ff70"}"#
    );

    let parsed = CompositeInvocationId::parse_env_value(payload).unwrap();
    assert_eq!(parsed.source, "codex2");
    assert_eq!(parsed.id.to_string(), known_uuid.to_string());

    let parent_env = serde_json::to_string(&composite).unwrap();
    assert!(!parent_env.starts_with("OULIPOLY_INVOCATION="));
    assert_eq!(
        CompositeInvocationId::parse_env_value(&parent_env)
            .unwrap()
            .id
            .to_string(),
        known_uuid.to_string()
    );
}

/// AGE-160 risk: PP-004 push-pull + A4 legacy compatibility grammar.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track; validates A4.
#[test]
fn age160_composite_invocation_id_declared_grammar_legacy_shell_mangled_compatibility() {
    let known_uuid = "7ad2916c-38dd-49e6-a1f7-3ef22766ff70";

    for payload in [
        format!("{{source:\"codex2\",id:\"{known_uuid}\",extra:\"ignored\"}}"),
        format!("{{source:'codex2',id:'{known_uuid}',extra:'ignored'}}"),
    ] {
        assert!(
            !payload.starts_with("OULIPOLY_INVOCATION="),
            "legacy compatibility payloads are raw payloads, not marker lines"
        );
        let parsed = CompositeInvocationId::parse_env_value(&payload).unwrap();
        assert_eq!(parsed.source, "codex2");
        assert_eq!(parsed.id.to_string(), known_uuid);
    }

    assert!(CompositeInvocationId::parse_env_value("{source:'codex2',id:'not-a-uuid'}").is_err());
}

#[derive(Clone, Default)]
struct Age160LifecycleSink {
    records: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

impl LifecycleEventSink for Age160LifecycleSink {
    fn forward(&mut self, record: &serde_json::Value) {
        self.records.lock().unwrap().push(record.clone());
    }
}

fn age160_lifecycle_fixture() -> (
    StateDb,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Age160LifecycleSink {
        records: records.clone(),
    };
    let db = StateDb::open_with_sink(Path::new(":memory:"), Box::new(sink)).unwrap();
    (db, records)
}

fn age160_invocation_start(uuid: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: uuid.to_string(),
        model_name: "codex~high".to_string(),
        provider_name: "codex2".to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
}

fn age160_lifecycle_records(
    records: &std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) -> Vec<serde_json::Value> {
    records.lock().unwrap().clone()
}

fn age160_record_keys(record: &serde_json::Value) -> Vec<&str> {
    record
        .as_object()
        .expect("record object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// AGE-160 risk: A6 db.rs↔lifecycle_log facade narrowing.
/// Selected level: unit + integration.
/// Source: the AGE-160 proposal § Test-intent track; validates A6.
#[test]
fn age160_lifecycle_log_facade_start_finalize_session_capture_preserves_records() {
    let (db, sink) = age160_lifecycle_fixture();
    let invocation_uuid = "16000000-0000-4000-8000-000000000001";

    let row_id = db
        .start_invocation(&age160_invocation_start(invocation_uuid))
        .unwrap();
    db.update_session_capture(row_id, Some("session-age160"), "resumed")
        .unwrap();
    db.finalize_invocation(row_id, true, 0, None, Some("done"))
        .unwrap();

    let records = age160_lifecycle_records(&sink);
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["event_name"], "invocation.started");
    assert_eq!(records[1]["event_name"], "invocation.session_captured");
    assert_eq!(records[2]["event_name"], "invocation.finalized");

    assert_eq!(
        age160_record_keys(&records[0]),
        vec![
            "chain_id",
            "error_chain",
            "event_name",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "model",
            "operation_result",
            "parent_invocation_uuid",
            "provider",
            "provider_source",
            "session_id",
        ]
    );
    assert_eq!(
        age160_record_keys(&records[1]),
        vec![
            "capture_method",
            "chain_id",
            "error_chain",
            "event_name",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "marker_emitted",
            "operation_result",
            "provider_source",
            "resume_input_id",
            "session_id",
        ]
    );
    assert_eq!(
        age160_record_keys(&records[2]),
        vec![
            "chain_id",
            "error_category",
            "error_chain",
            "event_name",
            "exit_code",
            "invocation_row_id",
            "invocation_uuid",
            "latency_us",
            "operation_result",
            "provider_source",
            "raw_artifact_paths",
            "session_id",
            "terminal_reason",
            "terminal_status",
        ]
    );

    assert_eq!(records[0]["invocation_uuid"], invocation_uuid);
    assert_eq!(records[0]["operation_result"], "ok");
    assert_eq!(records[0]["invocation_row_id"], serde_json::json!(row_id));
    assert_eq!(records[1]["capture_method"], "resumed");
    assert_eq!(records[1]["marker_emitted"], true);
    assert_eq!(records[1]["resume_input_id"], "session-age160");
    assert_eq!(records[2]["terminal_status"], "success");
    assert_eq!(records[2]["exit_code"], 0);
    assert_eq!(records[2]["terminal_reason"], "done");
}

fn age160_direct_symbol_count(haystack: &str, needles: &[&str]) -> usize {
    needles
        .iter()
        .map(|needle| haystack.match_indices(needle).count())
        .sum()
}

/// AGE-160 risk: A6 MEDIUM dispositions for db.rs↔serde_json/schema/chrono.
/// Selected level: unit.
/// Source: the AGE-160 proposal § Test-intent track.
#[test]
fn age160_post_cleanup_a6_medium_rows_resolved_or_declared() {
    let db_rs = include_str!("../db.rs");
    let serde_direct_symbols = age160_direct_symbol_count(
        db_rs,
        &[
            "serde_json::to_string",
            "serde_json::from_str",
            "serde_json::json!",
            "serde_json::to_vec",
            "serde_json::Value",
        ],
    );
    assert!(
        serde_direct_symbols < 12 || db_rs.contains("AGE-160 serde_json residual disposition"),
        "db.rs direct serde_json symbol count must fall below the A6 MEDIUM threshold or carry a local residual disposition; count={serde_direct_symbols}"
    );
    assert!(
        db_rs.contains("crate::schema")
            && db_rs.contains("AGE-160 intrinsic schema-version carrier"),
        "db.rs must declare crate::schema as the intrinsic StateDb schema-version carrier"
    );
    assert!(
        db_rs.contains("use chrono") && db_rs.contains("AGE-160 intrinsic timestamp carrier"),
        "db.rs must declare chrono as the intrinsic StateDb timestamp carrier"
    );
}
