use super::{create_full_state_schema, open};
use rusqlite::{Connection, params};
use std::path::Path;

pub const MODERN_SHAPE_UUID: &str = "54aaaaaa-0000-4000-8000-000000000001";
pub const PARTIAL_MODERN_SHAPE_UUID: &str = "54bbbbbb-0000-4000-8000-000000000001";
pub const UNKNOWN_SHAPE_MARKER: &str = "unknown-populated-marker";

pub fn build_modern_invocations_missing_repair_column(path: &Path) {
    let conn = open(path);
    create_minimal_support_tables(&conn, 5);
    conn.execute_batch(
        "
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_name TEXT,
            provider_index INTEGER NOT NULL,
            parent_invocation_id INTEGER,
            status TEXT NOT NULL,
            success INTEGER,
            exit_code INTEGER,
            error_category TEXT,
            session_id TEXT,
            session_capture_method TEXT,
            provider_session_id TEXT,
            resume_input_id TEXT,
            provider_session_capture_method TEXT,
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );
        ",
    )
    .unwrap();
    insert_modern_row(&conn, MODERN_SHAPE_UUID);
}

pub fn build_modern_invocations_shape(path: &Path) {
    let conn = open(path);
    create_full_state_schema(&conn, 5);
    conn.execute_batch(
        "
        ALTER TABLE invocations ADD COLUMN provider_session_id TEXT;
        ALTER TABLE invocations ADD COLUMN resume_input_id TEXT;
        ALTER TABLE invocations ADD COLUMN provider_session_capture_method TEXT;
        CREATE INDEX IF NOT EXISTS idx_invocations_provider_provider_session
            ON invocations(provider_name, provider_index, provider_session_id)
            WHERE provider_session_id IS NOT NULL;
        ",
    )
    .unwrap();
    insert_modern_row(&conn, MODERN_SHAPE_UUID);
}

pub fn build_partial_modern_invocations_shape(path: &Path) {
    let conn = open(path);
    create_minimal_support_tables(&conn, 5);
    conn.execute_batch(
        "
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            invocation_uuid TEXT NOT NULL UNIQUE,
            model_name TEXT NOT NULL,
            provider_index INTEGER NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        ",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO invocations
            (invocation_uuid, model_name, provider_index, status, created_at)
         VALUES (?1, 'fixture-model', 0, 'running', '2026-05-04T00:00:00Z')",
        [PARTIAL_MODERN_SHAPE_UUID],
    )
    .unwrap();
}

pub fn build_unknown_populated_invocations_shape(path: &Path) {
    let conn = open(path);
    create_minimal_support_tables(&conn, 5);
    conn.execute_batch(
        "
        CREATE TABLE invocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            model_name TEXT NOT NULL,
            provider_index INTEGER NOT NULL,
            success INTEGER NOT NULL,
            exit_code INTEGER NOT NULL,
            error_category TEXT,
            created_at TEXT NOT NULL,
            unexpected_hand_edit TEXT NOT NULL
        );
        ",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO invocations
            (model_name, provider_index, success, exit_code, error_category,
             created_at, unexpected_hand_edit)
         VALUES
            ('fixture-model', 0, 1, 0, NULL, '2026-05-04T00:00:00Z', ?1)",
        [UNKNOWN_SHAPE_MARKER],
    )
    .unwrap();
}

fn insert_modern_row(conn: &Connection, invocation_uuid: &str) {
    conn.execute(
        "INSERT INTO invocations
            (invocation_uuid, model_name, provider_name, provider_index, parent_invocation_id,
             status, success, exit_code, error_category, session_id, session_capture_method,
             created_at, finished_at)
         VALUES
            (?1, 'fixture-model', 'fixture-provider', 0, NULL, 'succeeded', 1, 0, NULL,
             'fixture-session', 'stdout', '2026-05-04T00:00:00Z',
             '2026-05-04T00:00:01Z')",
        params![invocation_uuid],
    )
    .unwrap();
}

fn create_minimal_support_tables(conn: &Connection, user_version: i32) {
    conn.execute_batch(&format!(
        "
        PRAGMA user_version = {user_version};
        CREATE TABLE providers (
            model_name TEXT NOT NULL,
            provider_name TEXT NOT NULL,
            invocation_count INTEGER NOT NULL DEFAULT 0,
            error_count INTEGER NOT NULL DEFAULT 0,
            last_error TEXT,
            last_error_at TEXT,
            last_invoked_at TEXT,
            PRIMARY KEY (model_name, provider_name)
        );
        CREATE TABLE provider_quotas (
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
        CREATE TABLE provider_quota_windows (
            provider_name TEXT NOT NULL,
            window_id INTEGER NOT NULL,
            used_percent REAL NOT NULL DEFAULT 0,
            resets_at TEXT NOT NULL,
            last_delta_percent REAL,
            last_delta_calls INTEGER,
            PRIMARY KEY (provider_name, window_id)
        );
        CREATE TABLE session_turns (
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
        CREATE TABLE session_chains (
            chain_id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            last_used_at TEXT NOT NULL,
            model_name TEXT NOT NULL
        );
        CREATE TABLE session_chain_segments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            chain_id TEXT NOT NULL REFERENCES session_chains(chain_id),
            provider_name TEXT NOT NULL,
            session_id TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            last_turn_id TEXT,
            transition_reason TEXT NOT NULL
        );
        "
    ))
    .unwrap();
}
