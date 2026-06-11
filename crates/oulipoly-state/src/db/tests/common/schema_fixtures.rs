//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//!
//! Role set: { accessor, formatter, orchestration }

use super::super::*;
use super::*;
pub(in crate::db::tests) fn mark_current_schema_version(conn: &sqlite::Connection) {
    seed_current_drift_required_tables(conn);
    conn.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)
        .unwrap();
}

pub(in crate::db::tests) fn seed_current_drift_required_tables(conn: &sqlite::Connection) {
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

pub(in crate::db::tests) fn db_without_table(table: &str) -> StateDb {
    let db = test_db();
    db.conn
        .execute_batch(&format!("DROP TABLE {table};"))
        .unwrap();
    db
}
