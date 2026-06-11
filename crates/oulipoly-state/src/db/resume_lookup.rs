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
//! Resume chain and wrong-id lookup helpers.

use super::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;

impl StateDb {
    pub fn resume_previews(&self, input: &str) -> Result<Vec<ChainPreview>, DbError> {
        Uuid::try_parse(input).map_err(|e| format!("Invalid UUID {input}: {e}"))?;
        self.chain_previews(input)
    }

    pub fn chain_id_for_segment(
        &self,
        provider_name: &str,
        session_id: &str,
    ) -> Result<Option<String>, DbError> {
        self.conn
            .query_row(
                "SELECT chain_id
                 FROM session_chain_segments
                 WHERE provider_name = ?1 AND session_id = ?2
                 ORDER BY ended_at IS NULL DESC, started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![provider_name, session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to look up session chain id: {e}"))
    }

    pub(super) fn candidate_chain_ids(&self, input: &str) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT chain_id
                 FROM session_chain_segments
                 WHERE session_id = ?1 OR chain_id = ?1
                 ORDER BY chain_id",
            )
            .map_err(|e| format!("Failed to prepare resume chain lookup: {e}"))?;
        let rows = stmt
            .query_map(sqlite::params![input], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query resume chain lookup: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read resume chain lookup: {e}"))
    }

    pub(super) fn wrong_id_kind_invocation_match(
        &self,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationMatch>, String> {
        let sql = Self::wrong_id_invocation_match_sql(&self.conn)?;
        let row = Self::load_wrong_id_invocation_match_row(&self.conn, &sql, input)?;

        let Some(row) = row else {
            return Ok(None);
        };
        let chain_id = self.chain_id_for_wrong_id_match(
            row.provider_name.as_deref(),
            row.provider_session_id.as_deref(),
        )?;
        Ok(Some(WrongIdKindInvocationMatch {
            invocation_uuid: row.invocation_uuid,
            provider_name: row.provider_name,
            provider_session_id: row.provider_session_id,
            chain_id,
        }))
    }

    pub(super) fn wrong_id_invocation_match_sql(
        conn: &sqlite::Connection,
    ) -> Result<String, String> {
        let provider_session_select = Self::wrong_id_provider_session_select(conn)?;
        Ok(format!(
            "SELECT invocation_uuid, provider_name, {provider_session_select}
             FROM invocations
             WHERE invocation_uuid = ?1"
        ))
    }

    pub(super) fn wrong_id_provider_session_select(
        conn: &sqlite::Connection,
    ) -> Result<&'static str, String> {
        if Self::invocations_have_dual_id_columns(conn)? {
            Ok("provider_session_id")
        } else {
            Ok("NULL AS provider_session_id")
        }
    }

    pub(super) fn load_wrong_id_invocation_match_row(
        conn: &sqlite::Connection,
        sql: &str,
        input: &str,
    ) -> Result<Option<WrongIdKindInvocationRow>, String> {
        conn.query_row(sql, sqlite::params![input], |row| {
            Ok(WrongIdKindInvocationRow {
                invocation_uuid: row.get(0)?,
                provider_name: row.get(1)?,
                provider_session_id: row.get(2)?,
            })
        })
        .optional()
        .map_err(|e| format!("Failed to query invocation id-kind match: {e}"))
    }

    pub(super) fn chain_id_for_wrong_id_match(
        &self,
        provider_name: Option<&str>,
        provider_session_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        match (provider_name, provider_session_id) {
            (Some(provider_name), Some(provider_session_id)) => self
                .chain_id_for_segment(provider_name, provider_session_id)
                .map_err(|e| format!("Failed to resolve chain for wrong-id-kind match: {e}")),
            _ => Ok(None),
        }
    }

    pub(super) fn choose_resume_chain(
        &self,
        _input: &str,
        mut chain_ids: Vec<String>,
    ) -> Result<Option<String>, String> {
        if chain_ids.len() == 1 {
            return Ok(chain_ids.pop());
        }
        let mut rows = Vec::new();
        for chain_id in chain_ids {
            rows.push(self.load_resume_chain_candidate(chain_id)?);
        }
        Self::sort_resume_chain_candidates(&mut rows);
        Ok(rows.into_iter().next().map(|row| row.chain_id))
    }

    pub(super) fn load_resume_chain_candidate(
        &self,
        chain_id: String,
    ) -> Result<ResumeChainCandidate, String> {
        let last_used_at = self.read_chain_last_used_at(&chain_id)?;
        let latest_segment_started_at = self.read_latest_segment_started_at(&chain_id)?;
        Ok(ResumeChainCandidate {
            chain_id,
            last_used_at,
            latest_segment_started_at,
        })
    }

    pub(super) fn read_chain_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw: String = self
            .conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain last_used_at: {e}"))?;
        Self::strict_rfc3339_message(&raw, "chain last_used_at")
    }

    pub(super) fn read_latest_segment_started_at(
        &self,
        chain_id: &str,
    ) -> Result<DateTime<Utc>, String> {
        let raw_started: String = self
            .conn
            .query_row(
                "SELECT started_at
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read chain latest segment started_at: {e}"))?;
        Self::strict_rfc3339_message(&raw_started, "chain segment started_at")
    }

    pub(super) fn sort_resume_chain_candidates(rows: &mut [ResumeChainCandidate]) {
        rows.sort_by(|a, b| {
            b.last_used_at
                .cmp(&a.last_used_at)
                .then_with(|| {
                    b.latest_segment_started_at
                        .cmp(&a.latest_segment_started_at)
                })
                .then_with(|| a.chain_id.cmp(&b.chain_id))
        });
    }
}
