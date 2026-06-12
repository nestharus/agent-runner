//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - mapper
//! - parser
//! - predicate
//!
//! Role set: { accessor, filter, formatter, mapper, parser, predicate }
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
//! ```
//!
//! Session-turn count and user-body query helpers.

use super::*;
use chrono::{DateTime, Utc};

const SESSION_TURN_COUNTS_SQL: &str = "SELECT
                    COUNT(*) AS total,
                    COUNT(CASE WHEN role = 'assistant' THEN 1 END) AS assistant,
                    COUNT(CASE WHEN is_sidechain = 1 THEN 1 END) AS sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2";

const SESSION_USER_TURN_BODIES_SQL: &str = "SELECT body
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND role = 'user'
                   AND body IS NOT NULL";

const SESSION_USER_TURN_SUBSTRING_SQL: &str = "SELECT EXISTS(
                    SELECT 1
                    FROM session_turns
                    WHERE provider_name = ?1
                      AND session_id = ?2
                      AND role = 'user'
                      AND body IS NOT NULL
                      AND instr(body, ?3) > 0
                )";

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
        Ok(count.max(0) as u64)
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

    pub fn has_session_user_text_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        text: &str,
    ) -> Result<bool, String> {
        let bodies = self.load_session_user_turn_bodies(provider_name, session_id)?;
        Ok(Self::session_user_turn_bodies_contain(&bodies, text))
    }

    fn load_session_user_turn_bodies(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(SESSION_USER_TURN_BODIES_SQL)
            .map_err(Self::format_session_user_turn_lookup_prepare_error)?;
        let rows = stmt
            .query_map(
                sqlite::params![provider_name, session_id],
                Self::session_user_turn_body,
            )
            .map_err(Self::format_session_user_turn_lookup_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_session_user_turn_body_read_error)
    }

    fn session_user_turn_bodies_contain(bodies: &[String], text: &str) -> bool {
        for body in bodies {
            if Self::session_user_turn_body_matches(body, text) {
                return true;
            }
        }
        false
    }

    fn format_session_user_turn_lookup_prepare_error(e: sqlite::Error) -> String {
        format!("Failed to prepare session user turn lookup: {e}")
    }

    fn format_session_user_turn_lookup_query_error(e: sqlite::Error) -> String {
        format!("Failed to query session user turns: {e}")
    }

    fn format_session_user_turn_body_read_error(e: sqlite::Error) -> String {
        format!("Failed to read session user turn body: {e}")
    }

    pub fn has_session_user_turn_containing(
        &self,
        provider_name: &str,
        session_id: &str,
        needle: &str,
    ) -> Result<bool, String> {
        if Self::session_user_turn_needle_is_empty(needle) {
            return Ok(false);
        }
        let found = self.session_user_turn_substring_exists(provider_name, session_id, needle)?;
        Ok(Self::sqlite_exists_value_is_true(found))
    }

    fn session_user_turn_needle_is_empty(needle: &str) -> bool {
        needle.is_empty()
    }

    fn session_user_turn_substring_exists(
        &self,
        provider_name: &str,
        session_id: &str,
        needle: &str,
    ) -> Result<i64, String> {
        self.conn
            .query_row(
                SESSION_USER_TURN_SUBSTRING_SQL,
                sqlite::params![provider_name, session_id, needle],
                Self::map_session_user_turn_substring_exists_row,
            )
            .map_err(Self::format_session_user_turn_substring_query_error)
    }

    fn map_session_user_turn_substring_exists_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn sqlite_exists_value_is_true(found: i64) -> bool {
        found != 0
    }

    fn format_session_user_turn_substring_query_error(e: sqlite::Error) -> String {
        format!("Failed to query session user turn substring: {e}")
    }

    pub(super) fn session_user_turn_body(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    pub(super) fn session_user_turn_body_matches(body: &str, text: &str) -> bool {
        Self::session_turn_body_has_exact_text(body, text)
    }

    pub(super) fn session_turn_body_has_exact_text(body: &str, text: &str) -> bool {
        Self::parse_session_turn_body(body)
            .as_ref()
            .is_some_and(|value| Self::parsed_session_turn_body_has_exact_text(value, text))
    }

    pub(super) fn parse_session_turn_body(body: &str) -> Option<serde_json::Value> {
        serde_json::from_str::<serde_json::Value>(body).ok()
    }

    pub(super) fn parsed_session_turn_body_has_exact_text(
        body: &serde_json::Value,
        text: &str,
    ) -> bool {
        Self::canonical_body_has_exact_text(body, text)
    }

    pub(super) fn canonical_body_has_exact_text(body: &serde_json::Value, text: &str) -> bool {
        let serde_json::Value::Array(chunks) = body else {
            return false;
        };
        let canonical_text =
            Self::canonical_text_from_chunks(Self::session_turn_text_chunks(chunks));
        Self::canonical_text_equals(canonical_text.as_deref(), text)
    }

    pub(super) fn session_turn_text_chunks(
        chunks: &[serde_json::Value],
    ) -> impl Iterator<Item = &serde_json::Value> + '_ {
        chunks
            .iter()
            .filter(|chunk| Self::session_turn_chunk_is_text(chunk))
    }

    pub(super) fn canonical_text_from_chunks<'a>(
        chunks: impl Iterator<Item = &'a serde_json::Value>,
    ) -> Option<String> {
        Self::format_canonical_text(Self::session_turn_text_values(chunks))
    }

    pub(super) fn session_turn_text_values<'a>(
        chunks: impl Iterator<Item = &'a serde_json::Value>,
    ) -> impl Iterator<Item = &'a str> {
        chunks.filter_map(Self::session_turn_chunk_text)
    }

    pub(super) fn session_turn_chunk_text(chunk: &serde_json::Value) -> Option<&str> {
        chunk.get("text").and_then(serde_json::Value::as_str)
    }

    pub(super) fn format_canonical_text<'a>(
        texts: impl Iterator<Item = &'a str>,
    ) -> Option<String> {
        let mut canonical_text = String::new();
        let mut text_count = 0;
        for text in texts {
            canonical_text.push_str(text);
            text_count += 1;
        }
        Self::canonical_text_has_chunks(text_count).then_some(canonical_text)
    }

    pub(super) fn canonical_text_has_chunks(text_count: usize) -> bool {
        text_count > 0
    }

    pub(super) fn canonical_text_equals(candidate: Option<&str>, text: &str) -> bool {
        candidate == Some(text)
    }

    pub(super) fn session_turn_chunk_is_text(chunk: &serde_json::Value) -> bool {
        chunk
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|chunk_type| chunk_type == "text")
    }
}
