//! ## Declared roles
//!
//! - accessor
//!
//! Role set: { accessor }

use super::super::*;
use super::*;
pub(in crate::db::tests) const SESSION_A: &str = "5169694d-de0f-40d1-890c-6e28e55bab27";
pub(in crate::db::tests) const SESSION_B: &str = "8f0a6a1f-9cd2-4c91-b6c6-1f0a0a8c9e22";
pub(in crate::db::tests) const CHAIN_A: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
pub(in crate::db::tests) const CHAIN_B: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
pub(in crate::db::tests) const CHAIN_C: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";

pub(in crate::db::tests) fn model_store_from_toml(
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

pub(in crate::db::tests) fn resolver_model_store()
-> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
    model_store_from_toml(&[
        (
            "provider-a-opus",
            r#"
[[providers]]
name = "provider-a"
interactive_args = ["launch"]

[[providers]]
name = "provider-a2"
interactive_args = ["launch"]
"#,
        ),
        (
            "provider-a-haiku",
            r#"
[[providers]]
name = "provider-a"
interactive_args = ["launch"]
"#,
        ),
    ])
}

pub(in crate::db::tests) fn seed_chain_row(
    db: &StateDb,
    chain_id: &str,
    model_name: &str,
    last_used_at: &str,
) {
    db.conn
        .execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
                 VALUES (?1, ?2, ?2, ?3)",
            sqlite::params![chain_id, last_used_at, model_name],
        )
        .unwrap();
}

pub(in crate::db::tests) fn seed_segment_row(
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

pub(in crate::db::tests) fn seed_invocation_for_session(
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

pub(in crate::db::tests) fn pre_chain_db_with_turns(
    rows: &[(&str, &str, &str, &str, &str)],
) -> TempDir {
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

pub(in crate::db::tests) fn chain_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM session_chains", [], |row| row.get(0))
        .unwrap()
}

pub(in crate::db::tests) fn segment_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM session_chain_segments", [], |row| {
            row.get(0)
        })
        .unwrap()
}

pub(in crate::db::tests) fn invocation_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

pub(in crate::db::tests) fn invocation_checksum(db: &StateDb) -> String {
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

pub(in crate::db::tests) fn legacy_v4_invocation_dual_id_fixture(
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
             VALUES (?1, 'provider-a-opus', 'provider-a', 0, ?2, NULL, NULL, ?3, ?4, ?5, ?6,
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
