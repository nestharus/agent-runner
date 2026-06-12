//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//! - validator
//!
//! Role set: { accessor, formatter, mapper, orchestration, parser, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/chain_resume_helpers.rs
//!     role: intrinsic-surface
//!     Domain: chain-resume-helpers-persistence
//!     Owns:
//!       - StateDb chain-resume-helpers persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, HashMap, InvocationStart, StateDb, TempDir, Uuid, params, sqlite
//! ```

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
    collect_model_store(parsed_model_fixtures(fixtures))
}

struct ParsedModelFixture {
    name: String,
    config: oulipoly_config::ModelConfig,
}

fn parsed_model_fixtures(fixtures: &[(&str, &str)]) -> Vec<ParsedModelFixture> {
    fixtures.iter().map(parsed_model_fixture).collect()
}

fn parsed_model_fixture((name, body): &(&str, &str)) -> ParsedModelFixture {
    ParsedModelFixture {
        name: model_fixture_name(name),
        config: parse_model_fixture(name, body),
    }
}

fn model_fixture_name(name: &str) -> String {
    name.to_string()
}

fn parse_model_fixture(name: &str, body: &str) -> oulipoly_config::ModelConfig {
    let result = parse_model_fixture_result(name, body);
    require_model_fixture(result)
}

fn parse_model_fixture_result(
    name: &str,
    body: &str,
) -> Result<oulipoly_config::ModelConfig, oulipoly_config::ModelError> {
    oulipoly_config::ModelConfig::from_toml_with_name(name, body, None)
}

fn require_model_fixture(
    result: Result<oulipoly_config::ModelConfig, oulipoly_config::ModelError>,
) -> oulipoly_config::ModelConfig {
    result.unwrap()
}

fn collect_model_store(
    fixtures: Vec<ParsedModelFixture>,
) -> std::collections::HashMap<String, oulipoly_config::ModelConfig> {
    fixtures
        .into_iter()
        .map(|fixture| (fixture.name, fixture.config))
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
        .start_invocation(&session_seed_invocation_start(model_name, provider_name))
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

fn session_seed_invocation_start(model_name: &str, provider_name: &str) -> InvocationStart {
    InvocationStart {
        invocation_uuid: Uuid::new_v4().to_string(),
        model_name: model_name.to_string(),
        provider_name: provider_name.to_string(),
        provider_index: 0,
        parent_invocation_id: None,
    }
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

pub(in crate::db::tests) fn active_segment_count_for_chain(db: &StateDb, chain_id: &str) -> i64 {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE chain_id = ?1 AND ended_at IS NULL",
            sqlite::params![chain_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn active_imported_segment_count(db: &StateDb) -> i64 {
    db.conn
        .query_row(
            "SELECT COUNT(*) FROM session_chain_segments WHERE transition_reason = 'imported' AND ended_at IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn session_chain_model_name(db: &StateDb) -> String {
    db.conn
        .query_row("SELECT model_name FROM session_chains", [], |row| {
            row.get(0)
        })
        .unwrap()
}

pub(in crate::db::tests) fn chain_segment_transition_reason(
    db: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> String {
    db.conn
        .query_row(
            "SELECT transition_reason FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2",
            sqlite::params![provider_name, session_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn chain_segment_started_at_raw(
    db: &StateDb,
    provider_name: &str,
    session_id: &str,
) -> String {
    db.conn
        .query_row(
            "SELECT started_at FROM session_chain_segments WHERE provider_name = ?1 AND session_id = ?2",
            sqlite::params![provider_name, session_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn chain_last_used_at_raw(db: &StateDb, chain_id: &str) -> String {
    db.conn
        .query_row(
            "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
            sqlite::params![chain_id],
            |row| row.get(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn invocation_count(db: &StateDb) -> i64 {
    db.conn
        .query_row("SELECT COUNT(*) FROM invocations", [], |row| row.get(0))
        .unwrap()
}

pub(in crate::db::tests) fn invocation_checksum(db: &StateDb) -> String {
    let dual_id_cols = invocation_checksum_has_dual_id_columns(db);
    let extra_cols = invocation_checksum_extra_columns(dual_id_cols);
    let sql = invocation_checksum_sql(extra_cols);
    query_invocation_checksum(db, &sql)
}

fn invocation_checksum_has_dual_id_columns(db: &StateDb) -> bool {
    StateDb::invocations_have_dual_id_columns(&db.conn).unwrap()
}

fn invocation_checksum_extra_columns(dual_id_cols: bool) -> &'static str {
    if dual_id_cols {
        " || '|' || COALESCE(session_capture_method, '') \
             || '|' || COALESCE(provider_session_id, '') \
             || '|' || COALESCE(resume_input_id, '') \
             || '|' || COALESCE(provider_session_capture_method, '')"
    } else {
        ""
    }
}

fn invocation_checksum_sql(extra_cols: &str) -> String {
    format!(
        "SELECT COALESCE(group_concat(line, char(10)), '')
             FROM (
                 SELECT id || '|' || invocation_uuid || '|' || status || '|' ||
                        COALESCE(session_id, ''){extra_cols} || '|' ||
                        COALESCE(finished_at, '') AS line
                 FROM invocations
                 ORDER BY id
             )"
    )
}

fn query_invocation_checksum(db: &StateDb, sql: &str) -> String {
    db.conn.query_row(sql, [], |row| row.get(0)).unwrap()
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
    let conn = open_legacy_v4_state_db(&dir);
    seed_legacy_v4_dual_id_schema(&conn);
    let row = legacy_v4_dual_id_insert(
        invocation_uuid,
        session_id,
        session_capture_method,
        status,
        terminal_reason,
        error_category,
    );
    insert_legacy_v4_dual_id_row(&conn, &row);
    mark_legacy_v4_schema_version(&conn);
    drop(conn);
    dir
}

fn open_legacy_v4_state_db(dir: &TempDir) -> sqlite::Connection {
    let db_path = dir.path().join("state.db");
    sqlite::Connection::open(&db_path).unwrap()
}

fn seed_legacy_v4_dual_id_schema(conn: &sqlite::Connection) {
    seed_current_drift_required_tables(conn);
    drop_legacy_v4_resolved_account_column(conn);
}

fn drop_legacy_v4_resolved_account_column(conn: &sqlite::Connection) {
    conn.execute(legacy_v4_drop_resolved_account_column_sql(), [])
        .unwrap();
}

fn legacy_v4_drop_resolved_account_column_sql() -> &'static str {
    "ALTER TABLE invocations DROP COLUMN provider_session_resolved_account"
}

struct LegacyV4DualIdInsert<'a> {
    invocation_uuid: &'a str,
    session_id: Option<&'a str>,
    session_capture_method: Option<&'a str>,
    status: &'a str,
    terminal_reason: Option<&'a str>,
    error_category: Option<&'a str>,
}

fn legacy_v4_dual_id_insert<'a>(
    invocation_uuid: &'a str,
    session_id: Option<&'a str>,
    session_capture_method: Option<&'a str>,
    status: &'a str,
    terminal_reason: Option<&'a str>,
    error_category: Option<&'a str>,
) -> LegacyV4DualIdInsert<'a> {
    LegacyV4DualIdInsert {
        invocation_uuid,
        session_id,
        session_capture_method,
        status,
        terminal_reason,
        error_category,
    }
}

fn insert_legacy_v4_dual_id_row(conn: &sqlite::Connection, row: &LegacyV4DualIdInsert<'_>) {
    conn.execute(
        legacy_v4_dual_id_insert_sql(),
        sqlite::params![
            row.invocation_uuid,
            row.status,
            row.error_category,
            row.terminal_reason,
            row.session_id,
            row.session_capture_method
        ],
    )
    .unwrap();
}

fn legacy_v4_dual_id_insert_sql() -> &'static str {
    "INSERT INTO invocations
            (invocation_uuid, model_name, provider_name, provider_index, status, success,
             exit_code, error_category, terminal_reason, session_id, session_capture_method,
             created_at, finished_at)
         VALUES (?1, 'provider-a-opus', 'provider-a', 0, ?2, NULL, NULL, ?3, ?4, ?5, ?6,
                 '2026-04-17T08:00:00Z', NULL)"
}

fn mark_legacy_v4_schema_version(conn: &sqlite::Connection) {
    conn.pragma_update(None, "user_version", 4).unwrap();
}
