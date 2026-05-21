//! ## Declared roles
//! orchestration, accessor, mapper, filter, predicate, validator
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/tests/age_61_row_version_migration_idempotent.rs
//!     role: intrinsic-surface
//!     Domain: state-db-row-version-idempotence-test-domain
//!     Owns:
//!       - representative seed query pair for invocations
//!       - representative seed query pair for providers
//!       - representative seed query pair for provider_quotas
//!       - representative seed query pair for provider_quota_windows
//!       - representative seed query pair for memory_nodes
//!       - representative seed query pair for memory_edges
//!       - representative seed query pair for setup_sessions
//!       - representative seed query pair for setup_turns
//!       - representative seed query pair for cli_providers
//!       - representative seed query pair for accounts
//!       - representative seed query pair for discovered_models
//!       - representative seed query pair for model_parameters
//!       - representative seed query pair for session_turns
//!       - representative seed query pair for session_chains
//!       - representative seed query pair for session_chain_segments
//!       - representative seed query pair for invocation_returned_artifacts
//!       - oulipoly_state row-version payload_hash_for_columns API
//!       - oulipoly_state migrations plan and run_with_db_path APIs

mod fixtures;

use fixtures::schema5_invocations::build_schema5_invocation_fixture;
use fixtures::{count_rows, seed_representative_state_rows, user_version};
use oulipoly_state::CURRENT_SCHEMA_VERSION;
use oulipoly_state::deployment::row_version::checksum::payload_hash_for_columns;
use oulipoly_state::migrations;
use rusqlite::Connection;
use rusqlite::types::Value;
use std::path::Path;

#[test]
fn ti_04_loader_level_idempotence_preserves_representative_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("state.db");
    build_representative_schema5_fixture(&db_path);

    let mut conn = Connection::open(&db_path).unwrap();
    let plan = migrations::current_plan_from(5).unwrap();
    assert_eq!(
        plan.iter()
            .map(|migration| migration.id)
            .collect::<Vec<_>>(),
        vec![
            "0006_age_58_dual_write_row_versions",
            "0007_age_123_resume_provider_identity",
            "0008_owned_turn_events",
            "0009_age163_working_set_and_round_robin",
        ]
    );

    migrations::run_with_db_path(&mut conn, &plan, db_path.clone()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    let after_first = capture_representative_seed(&conn);

    let empty_plan = migrations::current_plan_from(CURRENT_SCHEMA_VERSION).unwrap();
    assert!(
        empty_plan.is_empty(),
        "current-schema DBs must not replay completed migrations through the migration loader"
    );
    migrations::run_with_db_path(&mut conn, &empty_plan, db_path.clone()).unwrap();
    assert_eq!(user_version(&conn), CURRENT_SCHEMA_VERSION);
    let after_second = capture_representative_seed(&conn);

    assert_eq!(after_second, after_first);
}

// Declared role: orchestration
fn build_representative_schema5_fixture(path: &Path) {
    build_schema5_invocation_fixture(path);
    let conn = Connection::open(path).unwrap();
    seed_representative_state_rows(&conn);
    seed_schema5_age61_tables(&conn);
}

// Declared role: accessor
fn seed_schema5_age61_tables(conn: &Connection) {
    // This fixture covers the v5 path where the returned-artifacts table
    // already exists; the pragma test covers 0006 creating it when absent.
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS invocation_returned_artifacts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_id INTEGER NOT NULL REFERENCES invocations(id),
            ordinal INTEGER NOT NULL,
            version_id TEXT NOT NULL,
            name TEXT NOT NULL,
            workflow_run_id TEXT NOT NULL,
            artifact_name TEXT NOT NULL,
            version INTEGER NOT NULL CHECK(version > 0),
            sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
            content_len INTEGER NOT NULL CHECK(content_len >= 0),
            format_hint TEXT NULL,
            verdict_line TEXT NULL,
            source_kind TEXT NOT NULL,
            source_json TEXT NOT NULL,
            returned_at TEXT NOT NULL,
            UNIQUE(invocation_id, ordinal),
            UNIQUE(invocation_id, version_id)
        );

        INSERT INTO cli_providers
            (cli_name, display_name, installed, version, config_dir, last_synced)
        VALUES
            ('fixture-cli', 'Fixture CLI', 1, '1.2.3', '/tmp/fixture-cli', '2026-05-01T00:00:00Z');

        INSERT INTO accounts
            (id, provider, profile_name, auth_method, auth_status, created_at)
        VALUES
            ('fixture-account', 'fixture-cli', 'default', 'oauth', 'authenticated',
             '2026-05-01T00:00:00Z');

        INSERT INTO discovered_models
            (canonical_name, provider, discovered_at, cli_version)
        VALUES
            ('fixture-model', 'fixture-cli', '2026-05-01T00:00:00Z', '1.2.3');

        INSERT INTO model_parameters
            (model_name, provider, name, display_name, param_type, description, cli_mapping)
        VALUES
            ('fixture-model', 'fixture-cli', 'temperature', 'Temperature',
             '{\"kind\":\"number\"}', 'Sampling temperature', '{\"flag\":\"--temperature\"}');

        INSERT INTO invocation_returned_artifacts
            (invocation_id, ordinal, version_id, name, workflow_run_id, artifact_name, version,
             sha256, content_len, format_hint, verdict_line, source_kind, source_json, returned_at)
        VALUES
            (1, 0, 'artifact-version-1', 'summary.txt', 'return:11111111-1111-4111-8111-111111111111',
             'summary.txt', 1,
             'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
             42, 'text/plain', 'ok', 'scratchpad', '{}', '2026-05-01T00:00:02Z');
        ",
    )
    .unwrap();
}

#[derive(Debug, PartialEq, Eq)]
struct TableSeedSnapshot {
    table: &'static str,
    row_count: i64,
    payload_hash: [u8; 32],
}

// Declared role: accessor
fn capture_representative_seed(conn: &Connection) -> Vec<TableSeedSnapshot> {
    representative_seed_queries()
        .into_iter()
        .map(|(table, sql)| table_seed_snapshot(conn, table, sql))
        .collect()
}

// Declared role: mapper
fn table_seed_snapshot(
    conn: &Connection,
    table: &'static str,
    sql: &'static str,
) -> TableSeedSnapshot {
    TableSeedSnapshot {
        table,
        row_count: table_row_count(conn, table),
        payload_hash: payload_hash_for_query(conn, sql),
    }
}

// Declared role: accessor
fn table_row_count(conn: &Connection, table: &str) -> i64 {
    count_rows(conn, table)
}

// Declared role: mapper
fn payload_hash_for_query(conn: &Connection, sql: &str) -> [u8; 32] {
    payload_hash_for_values(&payload_values_for_query(conn, sql))
}

// Declared role: accessor
fn representative_seed_queries() -> [(&'static str, &'static str); 16] {
    [
        (
            "invocations",
            "SELECT invocation_uuid, model_name, provider_name, provider_index, status,
                    terminal_reason, provider_session_id, resume_input_id, created_at
             FROM invocations
             WHERE invocation_uuid = '11111111-1111-4111-8111-111111111111'",
        ),
        (
            "providers",
            "SELECT model_name, provider_name, invocation_count, error_count, last_error,
                    last_error_at, last_invoked_at
             FROM providers WHERE provider_name = 'fixture-provider'",
        ),
        (
            "provider_quotas",
            "SELECT provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at,
                    last_empty_refresh_at, exhausted_at, topology_peak_live_window_count,
                    last_topology_probe_at
             FROM provider_quotas WHERE provider_name = 'fixture-provider'",
        ),
        (
            "provider_quota_windows",
            "SELECT provider_name, window_id, used_percent, resets_at, last_delta_percent,
                    last_delta_calls
             FROM provider_quota_windows WHERE provider_name = 'fixture-provider'",
        ),
        (
            "memory_nodes",
            "SELECT id, node_type, label, data, created_at, updated_at
             FROM memory_nodes WHERE id = 'memory-a'",
        ),
        (
            "memory_edges",
            "SELECT source_id, target_id, edge_type, data, created_at
             FROM memory_edges WHERE source_id = 'memory-a'",
        ),
        (
            "setup_sessions",
            "SELECT id, started_at, ended_at, outcome, turn_count
             FROM setup_sessions WHERE id = 'setup-session'",
        ),
        (
            "setup_turns",
            "SELECT session_id, turn_number, agent_prompt, agent_response, events_emitted,
                    created_at
             FROM setup_turns WHERE session_id = 'setup-session'",
        ),
        (
            "cli_providers",
            "SELECT cli_name, display_name, installed, version, config_dir, last_synced
             FROM cli_providers WHERE cli_name = 'fixture-cli'",
        ),
        (
            "accounts",
            "SELECT id, provider, profile_name, auth_method, auth_status, created_at
             FROM accounts WHERE id = 'fixture-account'",
        ),
        (
            "discovered_models",
            "SELECT canonical_name, provider, discovered_at, cli_version
             FROM discovered_models WHERE canonical_name = 'fixture-model'",
        ),
        (
            "model_parameters",
            "SELECT model_name, provider, name, display_name, param_type, description, cli_mapping
             FROM model_parameters WHERE name = 'temperature'",
        ),
        (
            "session_turns",
            "SELECT provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
                    is_sidechain, is_compaction_boundary, source_file, ingested_at, body
             FROM session_turns WHERE turn_id = 'turn-root'",
        ),
        (
            "session_chains",
            "SELECT chain_id, created_at, last_used_at, model_name
             FROM session_chains WHERE chain_id = '44444444-4444-4444-8444-444444444444'",
        ),
        (
            "session_chain_segments",
            "SELECT chain_id, provider_name, session_id, started_at, ended_at, last_turn_id,
                    transition_reason
             FROM session_chain_segments
             WHERE chain_id = '44444444-4444-4444-8444-444444444444'",
        ),
        (
            "invocation_returned_artifacts",
            "SELECT invocation_id, ordinal, version_id, name, workflow_run_id, artifact_name,
                    version, sha256, content_len, format_hint, verdict_line, source_kind,
                    source_json, returned_at
             FROM invocation_returned_artifacts WHERE version_id = 'artifact-version-1'",
        ),
    ]
}

// Declared role: accessor
fn payload_values_for_query(conn: &Connection, sql: &str) -> Vec<Option<Value>> {
    let mut stmt = conn.prepare(sql).unwrap();
    stmt.query_row([], query_row_values).unwrap()
}

// Declared role: mapper
fn query_row_values(row: &rusqlite::Row<'_>) -> rusqlite::Result<Vec<Option<Value>>> {
    (0..row.as_ref().column_count())
        .map(|idx| row.get::<_, Value>(idx).map(Some))
        .collect()
}

// Declared role: mapper
fn payload_hash_for_values(values: &[Option<Value>]) -> [u8; 32] {
    payload_hash_for_columns(values)
}
