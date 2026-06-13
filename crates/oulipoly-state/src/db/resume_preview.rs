//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - parser
//!
//! Role set: { accessor, filter, formatter, mapper, parser }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/resume_preview.rs
//!     role: intrinsic-surface
//!     Domain: resume-preview-persistence
//!     Owns:
//!       - StateDb resume-preview persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: ChainPreview, DateTime, ParsedTurnPreviewTimestamp, RecentTurnRow, StateDb, TurnPreview, Utc, params, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: DateTime, Utc
//! ```
//!
//! Resume preview query and turn-preview mapping helpers.

use super::*;
use chrono::{DateTime, Utc};

impl StateDb {
    pub(super) fn chain_previews(&self, input: &str) -> Result<Vec<ChainPreview>, String> {
        let chain_ids = self.candidate_chain_ids(input)?;
        let mut out = Vec::new();
        for chain_id in chain_ids {
            out.push(self.build_chain_preview(chain_id)?);
        }
        Self::sort_chain_previews(&mut out);
        Ok(out)
    }

    pub(super) fn build_chain_preview(&self, chain_id: String) -> Result<ChainPreview, String> {
        let last_used_at = self.read_chain_preview_last_used_at(&chain_id)?;
        let (active_provider, active_session_id) = self
            .active_segment_for_chain(&chain_id)?
            .unwrap_or_else(|| ("<none>".to_string(), "<none>".to_string()));
        let turn_count = self.preview_turn_count(&active_provider, &active_session_id);
        let recent_turns = self.recent_turn_previews(&active_provider, &active_session_id)?;
        Ok(ChainPreview {
            chain_id,
            last_used_at,
            active_provider,
            active_session_id,
            turn_count,
            recent_turns,
        })
    }

    pub(super) fn read_chain_preview_last_used_at(
        &self,
        chain_id: &str,
    ) -> Result<DateTime<Utc>, String> {
        let raw_last = self.read_chain_preview_last_used_at_raw(chain_id)?;
        Self::parse_chain_preview_last_used_at(&raw_last)
    }

    fn read_chain_preview_last_used_at_raw(&self, chain_id: &str) -> Result<String, String> {
        self.conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                Self::map_chain_preview_last_used_at_row,
            )
            .map_err(Self::format_chain_preview_read_error)
    }

    fn map_chain_preview_last_used_at_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn parse_chain_preview_last_used_at(raw_last: &str) -> Result<DateTime<Utc>, String> {
        Self::strict_rfc3339_message(raw_last, "chain preview timestamp")
    }

    fn format_chain_preview_read_error(err: sqlite::Error) -> String {
        format!("Failed to read chain preview: {err}")
    }

    pub(super) fn preview_turn_count(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> usize {
        Self::normalized_preview_turn_count(
            self.raw_preview_turn_count(active_provider, active_session_id),
        )
    }

    fn raw_preview_turn_count(&self, active_provider: &str, active_session_id: &str) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![active_provider, active_session_id],
                Self::map_preview_turn_count_row,
            )
            .unwrap_or(0)
    }

    fn map_preview_turn_count_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn normalized_preview_turn_count(turn_count: i64) -> usize {
        turn_count.max(0) as usize
    }

    pub(super) fn recent_turn_previews(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<TurnPreview>, String> {
        let rows = self.query_recent_turn_rows(active_provider, active_session_id)?;
        let parsed = Self::parse_turn_preview_timestamps(rows)?;
        let mut recent_turns = Self::map_recent_turn_previews(parsed);
        recent_turns.reverse();
        Ok(recent_turns)
    }

    pub(super) fn query_recent_turn_rows(
        &self,
        active_provider: &str,
        active_session_id: &str,
    ) -> Result<Vec<RecentTurnRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT role, timestamp
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY timestamp DESC, id DESC
                 LIMIT 3",
            )
            .map_err(Self::format_recent_turns_preview_prepare_error)?;
        let rows = stmt
            .query_map(
                sqlite::params![active_provider, active_session_id],
                Self::recent_turn_row_mapper,
            )
            .map_err(Self::format_recent_turns_preview_query_error)?;

        let mut recent_turns = Vec::new();
        for row in rows {
            recent_turns.push(row.map_err(Self::format_recent_turn_read_error)?);
        }
        Ok(recent_turns)
    }

    fn format_recent_turns_preview_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare recent turns preview: {err}")
    }

    fn format_recent_turns_preview_query_error(err: sqlite::Error) -> String {
        format!("Failed to query recent turns preview: {err}")
    }

    fn format_recent_turn_read_error(err: sqlite::Error) -> String {
        format!("Failed to read recent turn: {err}")
    }

    pub(super) fn recent_turn_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<RecentTurnRow> {
        Ok(RecentTurnRow {
            role: row.get(0)?,
            timestamp_raw: row.get(1)?,
        })
    }

    pub(super) fn parse_turn_preview_timestamps(
        rows: Vec<RecentTurnRow>,
    ) -> Result<Vec<ParsedTurnPreviewTimestamp>, String> {
        rows.into_iter()
            .map(|row| {
                Ok(ParsedTurnPreviewTimestamp {
                    role: row.role,
                    timestamp: Self::strict_rfc3339_message(
                        &row.timestamp_raw,
                        "recent turn timestamp",
                    )?,
                })
            })
            .collect()
    }

    pub(super) fn map_recent_turn_previews(
        rows: Vec<ParsedTurnPreviewTimestamp>,
    ) -> Vec<TurnPreview> {
        rows.into_iter()
            .map(|row| TurnPreview {
                role: row.role,
                timestamp: row.timestamp,
                snippet: None,
            })
            .collect()
    }

    pub(super) fn sort_chain_previews(out: &mut [ChainPreview]) {
        out.sort_by_key(|preview| std::cmp::Reverse(preview.last_used_at));
    }
}
