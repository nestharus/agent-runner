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
//! Imported chain minting and active segment closing helpers.

use super::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;

const CLOSE_MATCHING_ACTIVE_SEGMENT_RETURNING_SQL: &str = "UPDATE session_chain_segments
             SET ended_at = ?2,
                 last_turn_id = (
                    SELECT st.turn_id
                    FROM session_turns st
                    WHERE st.provider_name = session_chain_segments.provider_name
                      AND st.session_id = session_chain_segments.session_id
                    ORDER BY st.timestamp DESC, st.id DESC
                    LIMIT 1
                 )
             WHERE chain_id = ?1
               AND ended_at IS NULL
               AND (?3 IS NULL OR provider_name = ?3)
               AND (?4 IS NULL OR session_id = ?4)
             RETURNING id";

impl StateDb {
    pub fn mint_imported_chain_if_absent(
        &self,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        model_name: &str,
    ) -> Result<(), DbError> {
        if Self::session_chain_segment_exists(&self.conn, provider_name, session_id)? {
            return Ok(());
        }
        let chain_id = Self::new_imported_chain_id();
        let ts = Self::segment_start_timestamp(started_at);
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_imported_chain_mint_begin_error)?;
        Self::insert_imported_chain(&tx, &chain_id, &ts, model_name)?;
        Self::insert_imported_segment(&tx, &chain_id, provider_name, session_id, &ts)?;
        tx.commit()
            .map_err(Self::format_imported_chain_mint_commit_error)?;
        Ok(())
    }

    fn new_imported_chain_id() -> String {
        Uuid::new_v4().to_string()
    }

    fn segment_start_timestamp(started_at: &DateTime<Utc>) -> String {
        started_at.to_rfc3339()
    }

    pub(super) fn session_chain_segment_exists(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let exists = Self::read_session_chain_segment_exists(conn, provider_name, session_id)?;
        Ok(Self::session_chain_segment_exists_value(exists))
    }

    fn read_session_chain_segment_exists(
        conn: &sqlite::Connection,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<i64>, DbError> {
        conn.query_row(
            "SELECT 1 FROM session_chain_segments
             WHERE provider_name = ?1 AND session_id = ?2
             LIMIT 1",
            sqlite::params![provider_name, session_id],
            Self::map_session_chain_segment_exists_row,
        )
        .optional()
        .map_err(Self::format_session_chain_segment_exists_error)
    }

    fn map_session_chain_segment_exists_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn session_chain_segment_exists_value(exists: Option<i64>) -> bool {
        exists.is_some()
    }

    pub(super) fn insert_imported_chain(
        conn: &sqlite::Connection,
        chain_id: &str,
        started_at: &str,
        model_name: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
             VALUES (?1, ?2, ?2, ?3)
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, started_at, model_name],
        )
        .map_err(Self::format_imported_chain_mint_error)?;
        Ok(())
    }

    pub(super) fn insert_imported_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, 'imported')
             ON CONFLICT DO NOTHING",
            sqlite::params![chain_id, provider_name, session_id, started_at],
        )
        .map_err(Self::format_imported_segment_mint_error)?;
        Ok(())
    }

    pub fn close_active_segment_returning(
        &self,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_active_segment_returning_on(&self.conn, chain_id, ended_at)
    }

    pub(super) fn close_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(conn, chain_id, None, None, ended_at)
    }

    pub(super) fn close_expected_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        Self::close_matching_active_segment_returning_on(
            conn,
            chain_id,
            Some(provider_name),
            Some(session_id),
            ended_at,
        )
    }

    pub(super) fn close_matching_active_segment_returning_on(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: Option<&str>,
        session_id: Option<&str>,
        ended_at: &DateTime<Utc>,
    ) -> Result<Option<i64>, DbError> {
        let ended_at = Self::segment_end_timestamp(ended_at);
        conn.query_row(
            CLOSE_MATCHING_ACTIVE_SEGMENT_RETURNING_SQL,
            sqlite::params![chain_id, ended_at, provider_name, session_id],
            Self::map_returned_chain_segment_id,
        )
        .optional()
        .map_err(Self::format_close_active_segment_error)
    }

    fn segment_end_timestamp(ended_at: &DateTime<Utc>) -> String {
        ended_at.to_rfc3339()
    }

    fn map_returned_chain_segment_id(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn format_close_active_segment_error(e: sqlite::Error) -> DbError {
        format!("Failed to close active session chain segment: {e}")
    }

    fn format_imported_chain_mint_begin_error(e: sqlite::Error) -> DbError {
        format!("Failed to begin imported chain mint: {e}")
    }

    fn format_imported_chain_mint_commit_error(e: sqlite::Error) -> DbError {
        format!("Failed to commit imported chain mint: {e}")
    }

    fn format_session_chain_segment_exists_error(e: sqlite::Error) -> DbError {
        format!("Failed to check existing session chain segment: {e}")
    }

    fn format_imported_chain_mint_error(e: sqlite::Error) -> DbError {
        format!("Failed to mint imported session chain: {e}")
    }

    fn format_imported_segment_mint_error(e: sqlite::Error) -> DbError {
        format!("Failed to mint imported session chain segment: {e}")
    }
}
