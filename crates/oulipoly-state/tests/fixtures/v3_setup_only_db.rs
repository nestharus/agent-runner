use super::{MEMORY_NODE_A, MEMORY_NODE_B, open};
use oulipoly_state::schema::MINIMUM_SUPPORTED_SCHEMA_VERSION;
use rusqlite::Connection;
use std::path::Path;

pub fn build_v3_setup_only_db(path: &Path) {
    build_setup_only_db(path, MINIMUM_SUPPORTED_SCHEMA_VERSION, true);
}

pub fn build_versionless_setup_only_db(path: &Path) {
    build_setup_only_db(path, 0, true);
}

pub fn build_setup_only_db(path: &Path, user_version: i32, with_foreign_keys: bool) {
    let conn = open(path);
    let edge_fk = if with_foreign_keys {
        "REFERENCES memory_nodes(id)"
    } else {
        ""
    };
    conn.execute_batch(&format!(
        "
        PRAGMA user_version = {user_version};

        CREATE TABLE memory_nodes (
            id TEXT PRIMARY KEY,
            node_type TEXT NOT NULL,
            label TEXT NOT NULL,
            data TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE memory_edges (
            source_id TEXT NOT NULL {edge_fk},
            target_id TEXT NOT NULL {edge_fk},
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
        "
    ))
    .unwrap();
    seed_setup_rows(&conn);
}

pub fn seed_setup_rows(conn: &Connection) {
    conn.execute_batch(&format!(
        "
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
        "
    ))
    .unwrap();
}
