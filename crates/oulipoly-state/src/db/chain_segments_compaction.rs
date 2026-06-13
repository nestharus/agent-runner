//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//! - parser
//! - predicate
//!
//! Role set: { accessor, formatter, mapper, orchestration, parser, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/chain_segments_compaction.rs
//!     role: intrinsic-surface
//!     Domain: chain-segments-compaction-persistence
//!     Owns:
//!       - StateDb chain-segments-compaction persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: DateTime, DbError, StateDb, Utc, params, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: DateTime, Utc
//! ```
//!
//! Compaction boundary and chain segment list helpers.

use super::*;
use chrono::{DateTime, Utc};

impl StateDb {
    pub fn update_chain_last_used(&self, chain_id: &str) -> Result<(), DbError> {
        self.conn
            .execute(
                "UPDATE session_chains SET last_used_at = ?2 WHERE chain_id = ?1",
                sqlite::params![chain_id, Self::current_chain_last_used_at()],
            )
            .map_err(Self::format_update_chain_last_used_error)?;
        Ok(())
    }

    fn current_chain_last_used_at() -> String {
        Utc::now().to_rfc3339()
    }

    fn format_update_chain_last_used_error(err: sqlite::Error) -> String {
        format!("Failed to update session chain last_used_at: {err}")
    }

    pub fn latest_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, DateTime<Utc>)>, DbError> {
        let row = self.latest_compaction_boundary_raw(provider_name, session_id)?;
        row.map(Self::map_compaction_boundary_raw).transpose()
    }

    fn latest_compaction_boundary_raw(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<(String, String)>, DbError> {
        let row = self
            .conn
            .query_row(
                "SELECT turn_id, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND is_compaction_boundary = 1
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                Self::map_latest_compaction_boundary_raw_row,
            )
            .optional()
            .map_err(Self::format_latest_compaction_boundary_query_error)?;
        Ok(row)
    }

    fn map_latest_compaction_boundary_raw_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<(String, String)> {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }

    fn format_latest_compaction_boundary_query_error(err: sqlite::Error) -> String {
        format!("Failed to query latest compaction boundary: {err}")
    }

    fn parse_compaction_boundary_timestamp(raw_ts: &str) -> Result<DateTime<Utc>, DbError> {
        Self::strict_rfc3339_message(raw_ts, "compaction boundary timestamp")
    }

    fn map_compaction_boundary_raw(
        row: (String, String),
    ) -> Result<(String, DateTime<Utc>), DbError> {
        let (turn_id, raw_ts) = row;
        let timestamp = Self::parse_compaction_boundary_timestamp(&raw_ts)?;
        Ok(Self::map_compaction_boundary(turn_id, timestamp))
    }

    fn map_compaction_boundary(
        turn_id: String,
        timestamp: DateTime<Utc>,
    ) -> (String, DateTime<Utc>) {
        (turn_id, timestamp)
    }

    pub fn distinct_chain_segments(&self) -> Result<Vec<(String, String)>, DbError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT provider_name, session_id
                 FROM session_chain_segments
                 ORDER BY provider_name, session_id",
            )
            .map_err(Self::format_chain_segment_list_prepare_error)?;
        let rows = stmt
            .query_map([], Self::map_distinct_chain_segment_row)
            .map_err(Self::format_chain_segment_list_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_chain_segment_list_read_error)
    }

    fn map_distinct_chain_segment_row(row: &sqlite::Row<'_>) -> sqlite::Result<(String, String)> {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }

    fn format_chain_segment_list_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare chain segment list: {err}")
    }

    fn format_chain_segment_list_query_error(err: sqlite::Error) -> String {
        format!("Failed to query chain segment list: {err}")
    }

    fn format_chain_segment_list_read_error(err: sqlite::Error) -> String {
        format!("Failed to read chain segment list: {err}")
    }

    pub fn flag_compaction_boundary(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
    ) -> Result<bool, DbError> {
        let changed = self
            .conn
            .execute(
                "UPDATE session_turns
                 SET is_compaction_boundary = 1
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND turn_id = ?3
                   AND is_compaction_boundary = 0",
                sqlite::params![provider_name, session_id, turn_id],
            )
            .map_err(Self::format_flag_compaction_boundary_error)?;
        Ok(Self::has_changed_rows(changed))
    }

    fn format_flag_compaction_boundary_error(err: sqlite::Error) -> String {
        format!("Failed to flag compaction boundary: {err}")
    }

    fn has_changed_rows(changed: usize) -> bool {
        changed > 0
    }
}
