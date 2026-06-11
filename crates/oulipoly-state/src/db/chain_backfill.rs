//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, mapper, orchestration }
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
            return Ok(BackfillReport {
                skipped_existing: true,
                chains_inserted: 0,
                segments_inserted: 0,
            });
        }

        let rows = Self::load_session_chain_backfill_rows(&self.conn)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin session chain backfill: {e}"))?;
        let provider_session_expr = Self::provider_session_expr(&tx, None)?;
        let mut chains_inserted = 0;
        let mut segments_inserted = 0;
        for row in rows {
            let model_name = Self::infer_model_for_backfill_row(&tx, &provider_session_expr, &row)?;
            let chain_id = Uuid::new_v4().to_string();
            chains_inserted += Self::insert_backfill_chain(&tx, &chain_id, &row, &model_name)?;
            segments_inserted += Self::insert_backfill_segment(&tx, &chain_id, &row)?;
        }
        tx.commit()
            .map_err(|e| format!("Failed to commit session chain backfill: {e}"))?;
        Ok(BackfillReport {
            skipped_existing: false,
            chains_inserted,
            segments_inserted,
        })
    }

    fn session_chains_backfill_exists(conn: &sqlite::Connection) -> Result<bool, DbError> {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_chains LIMIT 1)",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check session chain backfill state: {e}"))?;
        Ok(exists != 0)
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
            .map_err(|e| format!("Failed to prepare session chain backfill: {e}"))?;
        let iter = stmt
            .query_map([], Self::map_session_chain_backfill_row)
            .map_err(|e| format!("Failed to query session chain backfill rows: {e}"))?;
        iter.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read session chain backfill rows: {e}"))
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
        conn.query_row(model_sql, sqlite::params![row.session], |r| {
            r.get::<_, String>(0)
        })
        .optional()
        .map_err(|e| format!("Failed to infer model during backfill: {e}"))
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
        .map_err(|e| format!("Failed to insert session chain during backfill: {e}"))
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
        .map_err(|e| format!("Failed to insert session chain segment during backfill: {e}"))
    }
}
