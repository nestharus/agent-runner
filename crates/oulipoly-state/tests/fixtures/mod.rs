#![allow(dead_code)]

use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub mod failing_migration;
pub mod future_db;
pub mod invocation_shapes;
pub mod schema4_invocations;
pub mod schema5_drift;
pub mod schema5_invocations;
pub mod v3_full_state_db;
pub mod v3_setup_only_db;
pub mod versionless_drifted_setup;
pub mod versionless_unrecognized;

pub const ROOT_INVOCATION_UUID: &str = "11111111-1111-4111-8111-111111111111";
pub const CHILD_INVOCATION_UUID: &str = "22222222-2222-4222-8222-222222222222";
pub const PROVIDER_NAME: &str = "fixture-provider";
pub const MODEL_NAME: &str = "fixture-model";
pub const SESSION_ID: &str = "33333333-3333-4333-8333-333333333333";
pub const CHAIN_ID: &str = "44444444-4444-4444-8444-444444444444";
pub const ROOT_TURN_ID: &str = "turn-root";
pub const CHILD_TURN_ID: &str = "turn-child";
pub const MEMORY_NODE_A: &str = "memory-a";
pub const MEMORY_NODE_B: &str = "memory-b";

pub fn default_state_path(root: &Path) -> PathBuf {
    root.join("oulipoly-agent-runner").join("state.db")
}

pub fn open(path: &Path) -> Connection {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    Connection::open(path).unwrap()
}

pub fn user_version(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

pub fn table_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

pub fn schema_fingerprint(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT type, name, tbl_name, COALESCE(sql, '') FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .unwrap();
    stmt.query_map([], |row| {
        Ok(format!(
            "{}|{}|{}|{}",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?
        ))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

pub fn count_rows(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

pub fn create_full_state_schema(conn: &Connection, user_version: i32) {
    conn.execute_batch(&format!(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA user_version = {user_version};

        CREATE TABLE invocations (
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
            resume_acceptance_status TEXT,
            resume_acceptance_evidence TEXT,
            created_at TEXT NOT NULL,
            finished_at TEXT
        );
        CREATE INDEX idx_invocations_uuid ON invocations (invocation_uuid);
        CREATE INDEX idx_invocations_parent ON invocations (parent_invocation_id, created_at);
        CREATE INDEX idx_invocations_provider_created ON invocations (provider_name, created_at);
        CREATE INDEX idx_invocations_provider_session ON invocations (provider_name, session_id)
            WHERE session_id IS NOT NULL;

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

        CREATE TABLE memory_nodes (
            id TEXT PRIMARY KEY,
            node_type TEXT NOT NULL,
            label TEXT NOT NULL,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE memory_edges (
            source_id TEXT NOT NULL REFERENCES memory_nodes(id),
            target_id TEXT NOT NULL REFERENCES memory_nodes(id),
            edge_type TEXT NOT NULL,
            data TEXT,
            created_at TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, edge_type)
        );

        CREATE TABLE setup_sessions (
            id TEXT PRIMARY KEY,
            started_at TEXT NOT NULL,
            ended_at TEXT,
            outcome TEXT,
            turn_count INTEGER DEFAULT 0
        );

        CREATE TABLE setup_turns (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            turn_number INTEGER NOT NULL,
            agent_prompt TEXT NOT NULL,
            agent_response TEXT NOT NULL,
            events_emitted TEXT NOT NULL,
            created_at TEXT NOT NULL
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
        CREATE INDEX idx_session_turns_provider_ts ON session_turns (provider_name, role, timestamp);
        CREATE INDEX idx_session_turns_session_ts ON session_turns (provider_name, session_id, timestamp);
        CREATE INDEX idx_session_turns_session_lookup ON session_turns (session_id, timestamp);
        CREATE INDEX idx_session_turns_parent ON session_turns (provider_name, session_id, parent_turn_id, timestamp);

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
            transition_reason TEXT NOT NULL CHECK (transition_reason IN
                ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
            UNIQUE(chain_id, provider_name, session_id)
        );
        CREATE INDEX idx_segments_session ON session_chain_segments(session_id);
        CREATE INDEX idx_segments_chain_active ON session_chain_segments(chain_id, ended_at);
        "
    ))
    .unwrap();
}

pub fn seed_representative_state_rows(conn: &Connection) {
    conn.execute_batch(&format!(
        "
        CREATE TABLE IF NOT EXISTS cli_providers (
            cli_name TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            installed INTEGER NOT NULL DEFAULT 0,
            version TEXT,
            config_dir TEXT,
            last_synced TEXT
        );
        CREATE TABLE IF NOT EXISTS accounts (
            id TEXT NOT NULL,
            provider TEXT NOT NULL REFERENCES cli_providers(cli_name),
            profile_name TEXT NOT NULL,
            auth_method TEXT NOT NULL,
            auth_status TEXT NOT NULL DEFAULT 'unknown',
            created_at TEXT NOT NULL,
            PRIMARY KEY (id, provider)
        );
        CREATE TABLE IF NOT EXISTS discovered_models (
            canonical_name TEXT NOT NULL,
            provider TEXT NOT NULL,
            discovered_at TEXT NOT NULL,
            cli_version TEXT NOT NULL,
            PRIMARY KEY (canonical_name, provider)
        );
        CREATE TABLE IF NOT EXISTS model_parameters (
            model_name TEXT NOT NULL,
            provider TEXT NOT NULL,
            name TEXT NOT NULL,
            display_name TEXT NOT NULL,
            param_type TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            cli_mapping TEXT NOT NULL,
            PRIMARY KEY (model_name, provider, name)
        );

        DELETE FROM model_parameters;
        DELETE FROM discovered_models;
        DELETE FROM accounts;
        DELETE FROM cli_providers;
        DELETE FROM session_chain_segments;
        DELETE FROM session_chains;
        DELETE FROM session_turns;
        DELETE FROM setup_turns;
        DELETE FROM setup_sessions;
        DELETE FROM memory_edges;
        DELETE FROM memory_nodes;
        DELETE FROM provider_quota_windows;
        DELETE FROM provider_quotas;
        DELETE FROM providers;
        DELETE FROM invocations;

        INSERT INTO invocations
            (id, invocation_uuid, model_name, provider_name, provider_index, parent_invocation_id,
             status, success, exit_code, terminal_reason, session_id, session_capture_method,
             resume_acceptance_status, resume_acceptance_evidence, created_at, finished_at)
        VALUES
            (1, '{ROOT_INVOCATION_UUID}', '{MODEL_NAME}', '{PROVIDER_NAME}', 0, NULL,
             'succeeded', 1, 0, 'exit_zero', '{SESSION_ID}', 'stdout',
             'accepted', 'session id emitted', '2026-05-01T00:00:00Z', '2026-05-01T00:00:01Z'),
            (2, '{CHILD_INVOCATION_UUID}', '{MODEL_NAME}', '{PROVIDER_NAME}', 0, 1,
             'failed', 0, 7, 'exit_nonzero', '{SESSION_ID}', 'stdout',
             'accepted', 'session id emitted', '2026-05-01T00:01:00Z', '2026-05-01T00:01:01Z');

        INSERT INTO providers
            (model_name, provider_name, invocation_count, error_count, last_error, last_error_at, last_invoked_at)
        VALUES
            ('{MODEL_NAME}', '{PROVIDER_NAME}', 2, 1, 'fixture_error', '2026-05-01T00:01:01Z',
             '2026-05-01T00:01:00Z');

        INSERT INTO provider_quotas
            (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at,
             topology_peak_live_window_count, last_topology_probe_at)
        VALUES
            ('{PROVIDER_NAME}', 0.42, '2026-05-01T05:00:00Z', 12, '2026-05-01T00:00:00Z',
             2, '2026-05-01T00:02:00Z');

        INSERT INTO provider_quota_windows
            (provider_name, window_id, used_percent, resets_at, last_delta_percent, last_delta_calls)
        VALUES
            ('{PROVIDER_NAME}', 0, 0.42, '2026-05-01T05:00:00Z', 0.02, 24);

        INSERT INTO memory_nodes
            (id, node_type, label, data, created_at, updated_at)
        VALUES
            ('{MEMORY_NODE_A}', 'cli', 'Provider', '{{\"name\":\"fixture\"}}',
             '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z'),
            ('{MEMORY_NODE_B}', 'account', 'Account', '{{\"status\":\"ok\"}}',
             '2026-05-01T00:00:00Z', '2026-05-01T00:00:00Z');

        INSERT INTO memory_edges
            (source_id, target_id, edge_type, data, created_at)
        VALUES
            ('{MEMORY_NODE_A}', '{MEMORY_NODE_B}', 'owns', NULL, '2026-05-01T00:00:00Z');

        INSERT INTO setup_sessions
            (id, started_at, ended_at, outcome, turn_count)
        VALUES
            ('setup-session', '2026-05-01T00:00:00Z', '2026-05-01T00:05:00Z', 'completed', 1);

        INSERT INTO setup_turns
            (session_id, turn_number, agent_prompt, agent_response, events_emitted, created_at)
        VALUES
            ('setup-session', 1, 'detect', 'done', '[]', '2026-05-01T00:00:01Z');

        INSERT INTO session_turns
            (provider_name, session_id, turn_id, timestamp, role, parent_turn_id,
             is_sidechain, is_compaction_boundary, source_file, ingested_at, body)
        VALUES
            ('{PROVIDER_NAME}', '{SESSION_ID}', '{ROOT_TURN_ID}', '2026-05-01T00:00:00Z',
             'user', NULL, 0, 0, '/tmp/session.jsonl', '2026-05-01T00:00:02Z', '{{\"role\":\"user\"}}'),
            ('{PROVIDER_NAME}', '{SESSION_ID}', '{CHILD_TURN_ID}', '2026-05-01T00:00:01Z',
             'assistant', '{ROOT_TURN_ID}', 0, 0, '/tmp/session.jsonl', '2026-05-01T00:00:03Z',
             '{{\"role\":\"assistant\"}}');

        INSERT INTO session_chains
            (chain_id, created_at, last_used_at, model_name)
        VALUES
            ('{CHAIN_ID}', '2026-05-01T00:00:00Z', '2026-05-01T00:00:01Z', '{MODEL_NAME}');

        INSERT INTO session_chain_segments
            (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
        VALUES
            ('{CHAIN_ID}', '{PROVIDER_NAME}', '{SESSION_ID}', '2026-05-01T00:00:00Z',
             NULL, '{CHILD_TURN_ID}', 'initial');
        "
    ))
    .unwrap();
}

#[derive(Debug, PartialEq, Eq)]
pub struct RepresentativeSnapshot {
    pub user_version: i32,
    pub invocations: i64,
    pub providers: i64,
    pub provider_quotas: i64,
    pub provider_quota_windows: i64,
    pub memory_nodes: i64,
    pub memory_edges: i64,
    pub setup_sessions: i64,
    pub setup_turns: i64,
    pub session_turns: i64,
    pub session_chains: i64,
    pub session_chain_segments: i64,
    pub root_status: String,
    pub child_parent_id: i64,
    pub provider_invocation_count: i64,
    pub quota_calls_since_refresh: i64,
    pub assistant_body: Option<String>,
    pub segment_last_turn_id: Option<String>,
}

pub fn representative_snapshot(conn: &Connection) -> RepresentativeSnapshot {
    RepresentativeSnapshot {
        user_version: user_version(conn),
        invocations: count_rows(conn, "invocations"),
        providers: count_rows(conn, "providers"),
        provider_quotas: count_rows(conn, "provider_quotas"),
        provider_quota_windows: count_rows(conn, "provider_quota_windows"),
        memory_nodes: count_rows(conn, "memory_nodes"),
        memory_edges: count_rows(conn, "memory_edges"),
        setup_sessions: count_rows(conn, "setup_sessions"),
        setup_turns: count_rows(conn, "setup_turns"),
        session_turns: count_rows(conn, "session_turns"),
        session_chains: count_rows(conn, "session_chains"),
        session_chain_segments: count_rows(conn, "session_chain_segments"),
        root_status: conn
            .query_row(
                "SELECT status FROM invocations WHERE invocation_uuid = ?1",
                [ROOT_INVOCATION_UUID],
                |row| row.get(0),
            )
            .unwrap(),
        child_parent_id: conn
            .query_row(
                "SELECT parent_invocation_id FROM invocations WHERE invocation_uuid = ?1",
                [CHILD_INVOCATION_UUID],
                |row| row.get(0),
            )
            .unwrap(),
        provider_invocation_count: conn
            .query_row(
                "SELECT invocation_count FROM providers WHERE provider_name = ?1",
                [PROVIDER_NAME],
                |row| row.get(0),
            )
            .unwrap(),
        quota_calls_since_refresh: conn
            .query_row(
                "SELECT calls_since_refresh FROM provider_quotas WHERE provider_name = ?1",
                [PROVIDER_NAME],
                |row| row.get(0),
            )
            .unwrap(),
        assistant_body: conn
            .query_row(
                "SELECT body FROM session_turns WHERE turn_id = ?1",
                [CHILD_TURN_ID],
                |row| row.get(0),
            )
            .unwrap(),
        segment_last_turn_id: conn
            .query_row(
                "SELECT last_turn_id FROM session_chain_segments WHERE chain_id = ?1",
                [CHAIN_ID],
                |row| row.get(0),
            )
            .unwrap(),
    }
}
