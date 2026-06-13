//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - predicate
//!
//! Role set: { accessor, formatter, mapper, orchestration, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/chain_backfill.rs
//!     role: intrinsic-surface
//!     Domain: chain-backfill-persistence
//!     Owns:
//!       - StateDb chain-backfill persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, DbError, StateDb, Uuid, params, sqlite
//! ```
//!
//! Session chain backfill from legacy session turn rows.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillReport {
    pub skipped_existing: bool,
    pub chains_inserted: u64,
    pub segments_inserted: u64,
}

#[derive(Debug)]
struct SessionChainBackfillRow {
    provider: String,
    session: String,
    started_at: String,
    last_used_at: String,
    last_turn_id: String,
}

impl StateDb {
    pub fn backfill_session_chains(&self) -> Result<BackfillReport, DbError> {
        if Self::session_chains_backfill_exists(&self.conn)? {
            return Ok(Self::session_chain_backfill_skipped_report());
        }

        let rows = Self::load_session_chain_backfill_rows(&self.conn)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_session_chain_backfill_begin_error)?;
        let provider_session_expr = Self::provider_session_expr(&tx, None)?;
        let mut chains_inserted = 0;
        let mut segments_inserted = 0;
        for row in rows {
            let model_name = Self::infer_model_for_backfill_row(&tx, &provider_session_expr, &row)?;
            let chain_id = Self::new_session_chain_id();
            chains_inserted += Self::insert_backfill_chain(&tx, &chain_id, &row, &model_name)?;
            segments_inserted += Self::insert_backfill_segment(&tx, &chain_id, &row)?;
        }
        tx.commit()
            .map_err(Self::format_session_chain_backfill_commit_error)?;
        Ok(Self::session_chain_backfill_report(
            chains_inserted,
            segments_inserted,
        ))
    }

    fn session_chain_backfill_skipped_report() -> BackfillReport {
        BackfillReport {
            skipped_existing: true,
            chains_inserted: 0,
            segments_inserted: 0,
        }
    }

    fn session_chain_backfill_report(
        chains_inserted: u64,
        segments_inserted: u64,
    ) -> BackfillReport {
        BackfillReport {
            skipped_existing: false,
            chains_inserted,
            segments_inserted,
        }
    }

    fn new_session_chain_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn session_chains_backfill_exists(conn: &sqlite::Connection) -> Result<bool, DbError> {
        let exists = Self::read_session_chains_backfill_exists(conn)?;
        Ok(Self::session_chains_backfill_exists_value(exists))
    }

    fn read_session_chains_backfill_exists(conn: &sqlite::Connection) -> Result<i64, DbError> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_chains LIMIT 1)",
            [],
            Self::map_session_chains_backfill_exists_row,
        )
        .map_err(Self::format_session_chain_backfill_state_error)
    }

    fn map_session_chains_backfill_exists_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn session_chains_backfill_exists_value(exists: i64) -> bool {
        exists != 0
    }

    fn load_session_chain_backfill_rows(
        conn: &sqlite::Connection,
    ) -> Result<Vec<SessionChainBackfillRow>, DbError> {
        let mut stmt = conn
            .prepare(
                "SELECT st.provider_name,
                        st.session_id,
                        MIN(st.timestamp) AS started_at,
                        MAX(st.timestamp) AS last_used_at,
                        (
                            SELECT st2.turn_id
                            FROM session_turns st2
                            WHERE st2.provider_name = st.provider_name
                              AND st2.session_id = st.session_id
                            ORDER BY st2.timestamp DESC, st2.id DESC
                            LIMIT 1
                        ) AS last_turn_id
                 FROM session_turns st
                 GROUP BY st.provider_name, st.session_id",
            )
            .map_err(Self::format_session_chain_backfill_prepare_error)?;
        let iter = stmt
            .query_map([], Self::map_session_chain_backfill_row)
            .map_err(Self::format_session_chain_backfill_query_error)?;
        iter.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_session_chain_backfill_rows_read_error)
    }

    fn map_session_chain_backfill_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<SessionChainBackfillRow> {
        Ok(SessionChainBackfillRow {
            provider: row.get(0)?,
            session: row.get(1)?,
            started_at: row.get(2)?,
            last_used_at: row.get(3)?,
            last_turn_id: row.get(4)?,
        })
    }

    fn infer_model_for_backfill_row(
        conn: &sqlite::Connection,
        provider_session_expr: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<String, DbError> {
        let model_sql = Self::backfill_model_lookup_sql(provider_session_expr);
        let model_name = Self::lookup_model_for_backfill_row(conn, &model_sql, row)?;
        Ok(Self::default_backfill_model_name(model_name))
    }

    fn backfill_model_lookup_sql(provider_session_expr: &str) -> String {
        format!(
            "SELECT model_name
             FROM invocations
             WHERE {provider_session_expr} = ?1
             ORDER BY COALESCE(finished_at, created_at) DESC, id DESC
             LIMIT 1"
        )
    }

    fn lookup_model_for_backfill_row(
        conn: &sqlite::Connection,
        model_sql: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<Option<String>, DbError> {
        conn.query_row(
            model_sql,
            sqlite::params![row.session],
            Self::map_backfill_model_lookup_row,
        )
        .optional()
        .map_err(Self::format_backfill_model_inference_error)
    }

    fn map_backfill_model_lookup_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn default_backfill_model_name(model_name: Option<String>) -> String {
        model_name.unwrap_or_else(|| "<unknown>".to_string())
    }

    fn insert_backfill_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        row: &SessionChainBackfillRow,
        model_name: &str,
    ) -> Result<u64, DbError> {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?3, ?4)",
            sqlite::params![chain_id, row.started_at, row.last_used_at, model_name],
        )
        .map(|changed| changed as u64)
        .map_err(Self::format_backfill_chain_insert_error)
    }

    fn insert_backfill_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        row: &SessionChainBackfillRow,
    ) -> Result<u64, DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, ended_at, last_turn_id, transition_reason)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, 'imported')",
            sqlite::params![chain_id, row.provider, row.session, row.started_at, row.last_turn_id],
        )
        .map(|changed| changed as u64)
        .map_err(Self::format_backfill_segment_insert_error)
    }

    fn format_session_chain_backfill_begin_error(e: sqlite::Error) -> DbError {
        format!("Failed to begin session chain backfill: {e}")
    }

    fn format_session_chain_backfill_commit_error(e: sqlite::Error) -> DbError {
        format!("Failed to commit session chain backfill: {e}")
    }

    fn format_session_chain_backfill_state_error(e: sqlite::Error) -> DbError {
        format!("Failed to check session chain backfill state: {e}")
    }

    fn format_session_chain_backfill_prepare_error(e: sqlite::Error) -> DbError {
        format!("Failed to prepare session chain backfill: {e}")
    }

    fn format_session_chain_backfill_query_error(e: sqlite::Error) -> DbError {
        format!("Failed to query session chain backfill rows: {e}")
    }

    fn format_session_chain_backfill_rows_read_error(e: sqlite::Error) -> DbError {
        format!("Failed to read session chain backfill rows: {e}")
    }

    fn format_backfill_model_inference_error(e: sqlite::Error) -> DbError {
        format!("Failed to infer model during backfill: {e}")
    }

    fn format_backfill_chain_insert_error(e: sqlite::Error) -> DbError {
        format!("Failed to insert session chain during backfill: {e}")
    }

    fn format_backfill_segment_insert_error(e: sqlite::Error) -> DbError {
        format!("Failed to insert session chain segment during backfill: {e}")
    }
}
