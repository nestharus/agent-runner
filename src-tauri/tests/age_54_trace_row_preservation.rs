#![cfg(unix)]
//! ## Declared roles
//! orchestration, accessor, mapper, filter, predicate, validator, formatter
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: src-tauri/tests/age_54_trace_row_preservation.rs
//!     role: intrinsic-surface
//!     Domain: trace-row-preservation-integration-test-domain
//!     Owns:
//!       - trace-related SQLite table invocations
//!       - trace-related SQLite table session_turns
//!       - trace-related SQLite table session_chains
//!       - trace-related SQLite table session_chain_segments
//!       - trace-related SQLite table invocation_returned_artifacts
//!       - trace-related SQLite table provider_quotas
//!       - trace-related SQLite table provider_quota_windows
//!       - state_fixtures::schema4_invocations trace fixture constants and builder
//!       - state_fixtures::schema5_invocations current-state fixture builder
//!       - state_fixtures count_rows, default_state_path, user_version helpers
//!       - oulipoly_state::schema::CURRENT_SCHEMA_VERSION
//!       - oulipoly-agent-runner trace --json command harness

#[path = "../../crates/oulipoly-state/tests/fixtures/mod.rs"]
mod state_fixtures;

use oulipoly_state::schema::CURRENT_SCHEMA_VERSION;
use rusqlite::Connection;
use serde_json::Value;
use state_fixtures::schema4_invocations::{
    PROVIDER_SESSION_A, SCHEMA4_ROOT_UUID, build_schema4_invocation_fixture,
};
use state_fixtures::schema5_invocations::build_schema5_invocation_fixture;
use state_fixtures::{count_rows, default_state_path, user_version};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct TraceFixture {
    _dir: tempfile::TempDir,
    config_home: PathBuf,
    data_home: PathBuf,
}

impl TraceFixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config_home = dir.path().join("config");
        let data_home = dir.path().join("data");
        let app_config_dir = config_home.join("oulipoly-agent-runner");
        let models_dir = app_config_dir.join("models");
        fs::create_dir_all(&models_dir).unwrap();

        let script = dir.path().join("fixture-provider.sh");
        fs::write(
            &script,
            "#!/usr/bin/env bash\nprintf 'fixture-response\\n'\n",
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();

        fs::write(
            models_dir.join("fixture-model.toml"),
            "[[providers]]\nname = \"fixture-provider\"\n",
        )
        .unwrap();
        fs::write(
            app_config_dir.join("providers.toml"),
            format!(
                "[fixture-provider]\ncommand = {:?}\nargs = []\nprompt_mode = \"arg\"\n",
                script.to_string_lossy()
            ),
        )
        .unwrap();

        Self {
            _dir: dir,
            config_home,
            data_home,
        }
    }

    fn db_path(&self) -> PathBuf {
        default_state_path(&self.data_home)
    }

    fn run_trace_json(&self, invocation_uuid: &str) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_oulipoly-agent-runner"));
        cmd.arg("trace").arg(invocation_uuid).arg("--json");
        cmd.env("XDG_CONFIG_HOME", &self.config_home);
        cmd.env("XDG_DATA_HOME", &self.data_home);
        cmd.env_remove("OULIPOLY_DATA_DIR");
        cmd.output().unwrap()
    }
}

// Risk: ticket acceptance / trace inherited writable open
// Source: proposal TI-agents trace --json row preservation; contract observable signals 4, 5
// Level: verifies trace preserves schema-4 invocations across repeated calls
#[test]
fn trace_json_preserves_schema4_invocation_rows_across_two_calls() {
    let fixture = TraceFixture::new();
    seed_schema4_trace_db(&fixture.db_path());

    let before_count = invocation_count(&fixture.db_path());

    let first = fixture.run_trace_json(SCHEMA4_ROOT_UUID);
    assert_eq!(first.status.code(), Some(0), "{first:?}");
    let first_json: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(first_json["root"]["invocation"]["id"], SCHEMA4_ROOT_UUID);
    assert_eq!(invocation_count(&fixture.db_path()), before_count);

    let second = fixture.run_trace_json(SCHEMA4_ROOT_UUID);
    assert_eq!(second.status.code(), Some(0), "{second:?}");
    assert_eq!(invocation_count(&fixture.db_path()), before_count);

    let conn = Connection::open(fixture.db_path()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    assert_root_uuid_resolves(&fixture.db_path());
}

// Risk: read command writes during open
// Source: proposal TI-run_trace_command current schema non-destructive; contract observable signal trace tables
// Level: verifies trace-related tables remain unchanged
#[test]
fn trace_json_current_schema_does_not_change_trace_related_tables() {
    let fixture = TraceFixture::new();
    seed_current_trace_db_with_prepopulated_chains(&fixture.db_path());

    let before = trace_table_snapshot(&fixture.db_path());
    let output = fixture.run_trace_json(SCHEMA4_ROOT_UUID);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let after = trace_table_snapshot(&fixture.db_path());

    assert_eq!(after, before);
}

// Risk: migrate --rebuild boundary confusion
// Source: proposal Duplicate 4 accept divergence; contract trace must not create state-backups
// Level: verifies trace does not emulate destructive rebuild
#[test]
fn trace_json_does_not_call_rebuild_or_create_state_backups() {
    let fixture = TraceFixture::new();
    seed_trace_rebuild_sentinel_db(&fixture.db_path());
    let backup_dir = fixture
        .data_home
        .join("oulipoly-agent-runner")
        .join("state-backups");
    assert!(!backup_dir.exists());
    let before_count = invocation_count(&fixture.db_path());

    let output = fixture.run_trace_json(SCHEMA4_ROOT_UUID);
    assert_eq!(output.status.code(), Some(0), "{output:?}");

    assert_eq!(invocation_count(&fixture.db_path()), before_count);
    assert!(
        !backup_dir.exists(),
        "trace must not run destructive rebuild"
    );
    assert_root_uuid_resolves(&fixture.db_path());
}

fn seed_schema4_trace_db(path: &Path) {
    build_schema4_invocation_fixture(path);
}

fn seed_current_trace_db_with_prepopulated_chains(path: &Path) {
    build_schema5_invocation_fixture(path);
    let _ = oulipoly_state::StateDb::open(path).unwrap();
    let conn = Connection::open(path).unwrap();
    seed_trace_read_tables(&conn);
}

fn seed_trace_rebuild_sentinel_db(path: &Path) {
    build_schema5_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "UPDATE invocations
            SET resume_acceptance_evidence = 'rebuild-sentinel'
          WHERE invocation_uuid = ?1",
        [SCHEMA4_ROOT_UUID],
    )
    .unwrap();
    seed_trace_read_tables(&conn);
}

fn seed_trace_read_tables(conn: &Connection) {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS invocation_returned_artifacts (
            invocation_id INTEGER NOT NULL,
            ordinal INTEGER NOT NULL,
            kind TEXT NOT NULL,
            path TEXT NOT NULL,
            mime_type TEXT,
            bytes INTEGER,
            created_at TEXT NOT NULL,
            PRIMARY KEY (invocation_id, ordinal)
        );
        INSERT OR IGNORE INTO provider_quotas
            (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at,
             topology_peak_live_window_count, last_topology_probe_at)
        VALUES
            ('fixture-provider', 0.25, '2026-05-05T00:00:00Z', 4,
             '2026-05-04T00:00:00Z', 1, '2026-05-04T00:00:00Z');
        INSERT OR IGNORE INTO provider_quota_windows
            (provider_name, window_id, used_percent, resets_at)
        VALUES
            ('fixture-provider', 0, 0.25, '2026-05-05T00:00:00Z');
        INSERT OR IGNORE INTO session_chains
            (chain_id, created_at, last_used_at, model_name)
        VALUES
            ('trace-chain', '2026-05-04T00:00:00Z', '2026-05-04T00:00:01Z', 'fixture-model');
        INSERT OR IGNORE INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id,
             transition_reason)
        VALUES
            ('trace-chain', 'fixture-provider', 'provider-session-a',
             '2026-05-04T00:00:00Z', NULL, 'trace-turn-1', 'initial');
        INSERT OR IGNORE INTO session_turns
            (provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
             is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
        VALUES
            ('fixture-provider', 'provider-session-a', 'trace-turn-1',
             '2026-05-04T00:00:00Z', 'assistant', NULL, 0, 0,
             '/tmp/trace.jsonl', '2026-05-04T00:00:01Z', '{\"role\":\"assistant\"}');
        ",
    )
    .unwrap();
    assert_eq!(PROVIDER_SESSION_A, "provider-session-a");
}

fn invocation_count(path: &Path) -> i64 {
    let conn = Connection::open(path).unwrap();
    count_rows(&conn, "invocations")
}

fn assert_root_uuid_resolves(path: &Path) {
    let conn = Connection::open(path).unwrap();
    let count = root_uuid_match_count(&conn);
    assert_root_uuid_count(count);
}

fn root_uuid_match_count(conn: &Connection) -> i64 {
    conn.query_row(root_uuid_count_sql(), [SCHEMA4_ROOT_UUID], |row| row.get(0))
        .unwrap()
}

fn root_uuid_count_sql() -> &'static str {
    "SELECT COUNT(*) FROM invocations WHERE invocation_uuid = ?1"
}

fn assert_root_uuid_count(count: i64) {
    assert_eq!(count, 1);
}

fn trace_table_snapshot(path: &Path) -> BTreeMap<String, (i64, String)> {
    let conn = Connection::open(path).unwrap();
    [
        "invocations",
        "session_turns",
        "session_chains",
        "session_chain_segments",
        "invocation_returned_artifacts",
        "provider_quotas",
        "provider_quota_windows",
    ]
    .into_iter()
    .map(|table| {
        let count = count_rows(&conn, table);
        let checksum = table_checksum(&conn, table);
        (table.to_string(), (count, checksum))
    })
    .collect()
}

fn table_checksum(conn: &Connection, table: &str) -> String {
    let columns = checksum_columns(conn, table);
    let sql = checksum_sql(table, &columns);
    conn.query_row(&sql, [], |row| row.get(0)).unwrap()
}

fn checksum_columns(conn: &Connection, table: &str) -> Vec<String> {
    query_checksum_columns(conn, &table_info_sql(table))
}

fn table_info_sql(table: &str) -> String {
    format!("PRAGMA table_info({table})")
}

fn query_checksum_columns(conn: &Connection, sql: &str) -> Vec<String> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn checksum_sql(table: &str, columns: &[String]) -> String {
    let payload_columns = checksum_payload_columns(columns);
    let expr = checksum_row_expression(&payload_columns);
    format!(
        "SELECT COALESCE(group_concat(row_data, char(10)), '') \
         FROM (SELECT {expr} AS row_data FROM {table} ORDER BY row_data)"
    )
}

fn checksum_payload_columns(columns: &[String]) -> Vec<&str> {
    columns
        .iter()
        .map(String::as_str)
        .filter(|column| *column != "row_version")
        .collect()
}

fn checksum_row_expression(columns: &[&str]) -> String {
    columns
        .iter()
        .map(|column| format!("quote({column})"))
        .collect::<Vec<_>>()
        .join(" || '|' || ")
}
