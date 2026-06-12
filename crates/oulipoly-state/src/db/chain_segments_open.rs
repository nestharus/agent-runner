//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - orchestration
//! - accessor
//! - validator
//!
//! Role set: { formatter, mapper, orchestration, accessor, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/chain_segments_open.rs
//!     role: intrinsic-surface
//!     Domain: chain-segments-open-persistence
//!     Owns:
//!       - StateDb chain-segments-open persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, DateTime, DbError, StateDb, Utc, params, sqlite
//! ```
//!
//! Session-chain segment open, rotate, and active snapshot operations.

use super::*;
use chrono::{DateTime, Utc};
use oulipoly_core::TransitionReason;

const ACTIVE_CHAIN_SEGMENT_SNAPSHOT_SQL: &str = "SELECT sc.chain_id,
                        s.provider_name,
                        s.session_id,
                        s.started_at,
                        s.ended_at,
                        s.last_turn_id,
                        (
                            SELECT st.timestamp
                            FROM session_turns st
                            WHERE st.provider_name = s.provider_name
                              AND st.session_id = s.session_id
                            ORDER BY st.timestamp DESC, st.id DESC
                            LIMIT 1
                        )
                 FROM session_chains sc
                 JOIN session_chain_segments s ON s.chain_id = sc.chain_id
                 WHERE sc.chain_id = ?1 AND s.ended_at IS NULL
                 ORDER BY s.started_at DESC, s.id DESC
                 LIMIT 1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveChainSegmentSnapshot {
    pub chain_id: String,
    pub active_provider: String,
    pub active_session_id: String,
    pub active_started_at: String,
    pub active_ended_at: Option<String>,
    pub active_last_turn_id: Option<String>,
    pub latest_turn_at: Option<String>,
}

pub struct ChainSegmentRotationInput<'a> {
    pub chain_id: &'a str,
    pub source_provider_name: &'a str,
    pub source_session_id: &'a str,
    pub target_provider_name: &'a str,
    pub target_session_id: &'a str,
    pub changed_at: &'a DateTime<Utc>,
    pub reason: TransitionReason,
}

/// Compact-summary evidence consumed by `migrate-db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactSummaryEvidence {
    pub session_id: String,
    pub compact_turn_uuids: Vec<String>,
}

impl StateDb {
    pub fn open_chain_segment(
        &self,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &DateTime<Utc>,
        reason: TransitionReason,
    ) -> Result<i64, DbError> {
        let started_at = Self::open_segment_started_at(started_at);
        Self::upsert_open_chain_segment(
            &self.conn,
            chain_id,
            provider_name,
            session_id,
            &started_at,
            reason,
        )?;
        Self::read_open_chain_segment_id(&self.conn, chain_id, provider_name, session_id)
    }

    pub fn rotate_chain_segment_transactionally(
        &self,
        input: ChainSegmentRotationInput<'_>,
    ) -> Result<(i64, i64), DbError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_chain_segment_rotation_begin_error)?;
        let changed_at = Self::rotation_segment_changed_at(input.changed_at);
        let closed_id =
            Self::require_active_source_segment(Self::close_expected_active_segment_returning_on(
                &tx,
                input.chain_id,
                input.source_provider_name,
                input.source_session_id,
                input.changed_at,
            )?)?;
        Self::upsert_open_chain_segment(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
            &changed_at,
            input.reason,
        )?;
        let opened_id = Self::read_open_chain_segment_id(
            &tx,
            input.chain_id,
            input.target_provider_name,
            input.target_session_id,
        )?;
        tx.commit()
            .map_err(Self::format_chain_segment_rotation_commit_error)?;
        Ok(Self::chain_segment_rotation_ids(closed_id, opened_id))
    }

    fn open_segment_started_at(started_at: &DateTime<Utc>) -> String {
        started_at.to_rfc3339()
    }

    fn rotation_segment_changed_at(changed_at: &DateTime<Utc>) -> String {
        changed_at.to_rfc3339()
    }

    fn chain_segment_rotation_ids(closed_id: i64, opened_id: i64) -> (i64, i64) {
        (closed_id, opened_id)
    }

    pub fn active_chain_segment_snapshot(
        &self,
        chain_id: &str,
    ) -> Result<Option<ActiveChainSegmentSnapshot>, DbError> {
        self.conn
            .query_row(
                ACTIVE_CHAIN_SEGMENT_SNAPSHOT_SQL,
                sqlite::params![chain_id],
                Self::map_active_chain_segment_snapshot_row,
            )
            .optional()
            .map_err(Self::format_active_chain_segment_snapshot_error)
    }

    fn map_active_chain_segment_snapshot_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<ActiveChainSegmentSnapshot> {
        Ok(ActiveChainSegmentSnapshot {
            chain_id: row.get(0)?,
            active_provider: row.get(1)?,
            active_session_id: row.get(2)?,
            active_started_at: row.get(3)?,
            active_ended_at: row.get(4)?,
            active_last_turn_id: row.get(5)?,
            latest_turn_at: row.get(6)?,
        })
    }

    fn format_active_chain_segment_snapshot_error(e: sqlite::Error) -> DbError {
        format!("Failed to read active chain segment snapshot: {e}")
    }

    pub(super) fn upsert_open_chain_segment(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
        started_at: &str,
        reason: TransitionReason,
    ) -> Result<(), DbError> {
        conn.execute(
            "INSERT INTO session_chain_segments
                (chain_id, provider_name, session_id, started_at, transition_reason)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (chain_id, provider_name, session_id)
             DO UPDATE SET
                started_at = excluded.started_at,
                ended_at = NULL,
                last_turn_id = NULL,
                transition_reason = excluded.transition_reason",
            sqlite::params![
                chain_id,
                provider_name,
                session_id,
                started_at,
                reason.as_str()
            ],
        )
        .map_err(Self::format_open_chain_segment_error)?;
        Ok(())
    }

    pub(super) fn read_open_chain_segment_id(
        conn: &sqlite::Connection,
        chain_id: &str,
        provider_name: &str,
        session_id: &str,
    ) -> Result<i64, DbError> {
        conn.query_row(
            "SELECT id FROM session_chain_segments
                 WHERE chain_id = ?1 AND provider_name = ?2 AND session_id = ?3
                 ORDER BY id DESC LIMIT 1",
            sqlite::params![chain_id, provider_name, session_id],
            Self::map_open_chain_segment_id_row,
        )
        .map_err(Self::format_open_chain_segment_id_read_error)
    }

    fn map_open_chain_segment_id_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    pub fn find_conflicting_active_segment(
        &self,
        provider_name: &str,
        session_id: &str,
        own_chain_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND chain_id != ?3
                   AND ended_at IS NULL
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id, own_chain_id],
                Self::map_conflicting_active_segment_row,
            )
            .optional()
            .map_err(Self::format_conflicting_active_segment_check_error)
    }

    fn map_conflicting_active_segment_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn require_active_source_segment(segment_id: Option<i64>) -> Result<i64, DbError> {
        segment_id.ok_or_else(Self::format_inactive_source_segment_error)
    }

    fn format_inactive_source_segment_error() -> DbError {
        "validated source segment was no longer active".to_string()
    }

    fn format_chain_segment_rotation_begin_error(e: sqlite::Error) -> DbError {
        format!("Failed to begin session chain rotation transaction: {e}")
    }

    fn format_chain_segment_rotation_commit_error(e: sqlite::Error) -> DbError {
        format!("Failed to commit session chain rotation transaction: {e}")
    }

    fn format_open_chain_segment_error(e: sqlite::Error) -> DbError {
        format!("Failed to open session chain segment: {e}")
    }

    fn format_open_chain_segment_id_read_error(e: sqlite::Error) -> DbError {
        format!("Failed to read session chain segment id: {e}")
    }

    fn format_conflicting_active_segment_check_error(e: sqlite::Error) -> DbError {
        format!("Failed to check conflicting active session segment: {e}")
    }
}
