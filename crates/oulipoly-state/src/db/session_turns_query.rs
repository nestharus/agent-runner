//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - orchestration
//! - predicate
//!
//! Role set: { accessor, filter, formatter, mapper, orchestration, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/session_turns_query.rs
//!     role: intrinsic-surface
//!     Domain: session-turns-query-persistence
//!     Owns:
//!       - StateDb session-turns-query persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: DateTime, SessionTurnCounts, StateDb, Utc, params, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: DateTime, Utc
//! ```
//!
//! Session-turn count query helpers.

use super::*;
use chrono::{DateTime, Utc};

const SESSION_TURN_COUNTS_SQL: &str = "SELECT
                    COUNT(*) AS total,
                    COUNT(CASE WHEN role = 'assistant' THEN 1 END) AS assistant,
                    COUNT(CASE WHEN is_sidechain = 1 THEN 1 END) AS sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2";

impl StateDb {
    pub fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String> {
        self.query_session_turn_count_tuple(provider_name, session_id)
            .map(Self::map_session_turn_counts)
    }

    fn query_session_turn_count_tuple(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<(i64, i64, i64), String> {
        self.conn
            .query_row(
                SESSION_TURN_COUNTS_SQL,
                sqlite::params![provider_name, session_id],
                Self::map_session_turn_count_tuple,
            )
            .map_err(Self::format_trace_session_turn_count_error)
    }

    fn map_session_turn_count_tuple(row: &sqlite::Row<'_>) -> sqlite::Result<(i64, i64, i64)> {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    }

    fn format_trace_session_turn_count_error(e: sqlite::Error) -> String {
        format!("Failed to count session turns for trace: {e}")
    }

    fn map_session_turn_counts(
        (total, assistant, sidechain): (i64, i64, i64),
    ) -> SessionTurnCounts {
        SessionTurnCounts {
            total: Self::nonnegative_turn_count(total),
            assistant: Self::nonnegative_turn_count(assistant),
            sidechain: Self::nonnegative_turn_count(sidechain),
        }
    }

    fn nonnegative_turn_count(value: i64) -> u64 {
        value.max(0) as u64
    }

    /// Count assistant turns ingested for a provider since `since` (exclusive).
    /// `None` means count everything we've ever ingested for that provider.
    pub fn count_assistant_turns_since(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<u64, String> {
        let count = self.query_assistant_turn_count(provider_name, since)?;
        Ok(Self::nonnegative_turn_count(count))
    }

    pub(super) fn query_assistant_turn_count(
        &self,
        provider_name: &str,
        since: Option<&DateTime<Utc>>,
    ) -> Result<i64, String> {
        match since {
            Some(ts) => self.query_assistant_turn_count_after(provider_name, ts),
            None => self.query_all_assistant_turn_count(provider_name),
        }
    }

    pub(super) fn query_assistant_turn_count_after(
        &self,
        provider_name: &str,
        since: &DateTime<Utc>,
    ) -> Result<i64, String> {
        let since = Self::assistant_turn_count_cutoff(since);
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant' AND timestamp > ?2",
                sqlite::params![provider_name, since],
                Self::map_assistant_turn_count_row,
            )
            .map_err(Self::session_turn_count_error)
    }

    fn map_assistant_turn_count_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn assistant_turn_count_cutoff(since: &DateTime<Utc>) -> String {
        since.to_rfc3339()
    }

    pub(super) fn query_all_assistant_turn_count(
        &self,
        provider_name: &str,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant'",
                sqlite::params![provider_name],
                Self::map_assistant_turn_count_row,
            )
            .map_err(Self::session_turn_count_error)
    }

    pub(super) fn session_turn_count_error(e: sqlite::Error) -> String {
        format!("Failed to count session turns: {e}")
    }
}
