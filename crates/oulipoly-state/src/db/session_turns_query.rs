//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - predicate
//!
//! Role set: { accessor, mapper, predicate }
//!
//! Session-turn count and user-body query helpers.

use super::*;
use chrono::{DateTime, Utc};

impl StateDb {
    pub fn count_session_turns(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<SessionTurnCounts, String> {
        let (total, assistant, sidechain): (i64, i64, i64) = self
            .conn
            .query_row(
                "SELECT
                    COUNT(*) AS total,
                    COUNT(CASE WHEN role = 'assistant' THEN 1 END) AS assistant,
                    COUNT(CASE WHEN is_sidechain = 1 THEN 1 END) AS sidechain
                 FROM session_turns
                 WHERE provider_name = ?1 AND session_id = ?2",
                sqlite::params![provider_name, session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Failed to count session turns for trace: {e}"))?;

        Ok(SessionTurnCounts {
            total: total.max(0) as u64,
            assistant: assistant.max(0) as u64,
            sidechain: sidechain.max(0) as u64,
        })
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
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM session_turns
                 WHERE provider_name = ?1 AND role = 'assistant' AND timestamp > ?2",
                sqlite::params![provider_name, since.to_rfc3339()],
                |row| row.get(0),
            )
            .map_err(Self::session_turn_count_error)
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
                |row| row.get(0),
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
        let mut stmt = self
            .conn
            .prepare(
                "SELECT body
                 FROM session_turns
                 WHERE provider_name = ?1
                   AND session_id = ?2
                   AND role = 'user'
                   AND body IS NOT NULL",
            )
            .map_err(|e| format!("Failed to prepare session user turn lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![provider_name, session_id], |row| {
                Self::session_user_turn_body(row)
            })
            .map_err(|e| format!("Failed to query session user turns: {e}"))?;

        for row in rows {
            let body = row.map_err(|e| format!("Failed to read session user turn body: {e}"))?;
            if Self::session_user_turn_body_matches(&body, text) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn has_session_user_turn_containing(
        &self,
        provider_name: &str,
        session_id: &str,
        needle: &str,
    ) -> Result<bool, String> {
        if needle.is_empty() {
            return Ok(false);
        }
        let found: i64 = self
            .conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM session_turns
                    WHERE provider_name = ?1
                      AND session_id = ?2
                      AND role = 'user'
                      AND body IS NOT NULL
                      AND instr(body, ?3) > 0
                )",
                sqlite::params![provider_name, session_id, needle],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query session user turn substring: {e}"))?;
        Ok(found != 0)
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
        let mut canonical_text = String::new();
        let mut has_text = false;
        for chunk in chunks {
            if let Some(candidate) = chunk.get("text").and_then(serde_json::Value::as_str) {
                canonical_text.push_str(candidate);
                has_text = true;
            }
        }
        has_text.then_some(canonical_text)
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
