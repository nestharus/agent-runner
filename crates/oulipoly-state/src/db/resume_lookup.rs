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
        let _ = Self::parse_resume_preview_uuid(input)?;
        self.chain_previews(input)
    }

    fn parse_resume_preview_uuid(input: &str) -> Result<Uuid, DbError> {
        Uuid::try_parse(input).map_err(|err| Self::format_invalid_resume_preview_uuid(input, err))
    }

    fn format_invalid_resume_preview_uuid(input: &str, err: uuid::Error) -> String {
        format!("Invalid UUID {input}: {err}")
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
                Self::map_chain_id_row,
            )
            .optional()
            .map_err(Self::format_session_chain_id_lookup_error)
    }

    fn map_chain_id_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn format_session_chain_id_lookup_error(err: sqlite::Error) -> String {
        format!("Failed to look up session chain id: {err}")
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
            .map_err(Self::format_resume_chain_lookup_prepare_error)?;
        let rows = stmt
            .query_map(sqlite::params![input], Self::map_chain_id_row)
            .map_err(Self::format_resume_chain_lookup_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_resume_chain_lookup_read_error)
    }

    fn format_resume_chain_lookup_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare resume chain lookup: {err}")
    }

    fn format_resume_chain_lookup_query_error(err: sqlite::Error) -> String {
        format!("Failed to query resume chain lookup: {err}")
    }

    fn format_resume_chain_lookup_read_error(err: sqlite::Error) -> String {
        format!("Failed to read resume chain lookup: {err}")
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
        Ok(Some(Self::map_wrong_id_kind_invocation_match(
            row, chain_id,
        )))
    }

    fn map_wrong_id_kind_invocation_match(
        row: WrongIdKindInvocationRow,
        chain_id: Option<String>,
    ) -> WrongIdKindInvocationMatch {
        WrongIdKindInvocationMatch {
            invocation_uuid: row.invocation_uuid,
            provider_name: row.provider_name,
            provider_session_id: row.provider_session_id,
            chain_id,
        }
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
        conn.query_row(
            sql,
            sqlite::params![input],
            Self::map_wrong_id_invocation_row,
        )
        .optional()
        .map_err(Self::format_wrong_id_invocation_match_query_error)
    }

    fn map_wrong_id_invocation_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<WrongIdKindInvocationRow> {
        Ok(WrongIdKindInvocationRow {
            invocation_uuid: row.get(0)?,
            provider_name: row.get(1)?,
            provider_session_id: row.get(2)?,
        })
    }

    fn format_wrong_id_invocation_match_query_error(err: sqlite::Error) -> String {
        format!("Failed to query invocation id-kind match: {err}")
    }

    pub(super) fn chain_id_for_wrong_id_match(
        &self,
        provider_name: Option<&str>,
        provider_session_id: Option<&str>,
    ) -> Result<Option<String>, String> {
        let Some((provider_name, provider_session_id)) =
            Self::wrong_id_match_segment(provider_name, provider_session_id)
        else {
            return Ok(None);
        };
        self.chain_id_for_segment(provider_name, provider_session_id)
            .map_err(Self::format_wrong_id_match_chain_resolution_error)
    }

    fn wrong_id_match_segment<'a>(
        provider_name: Option<&'a str>,
        provider_session_id: Option<&'a str>,
    ) -> Option<(&'a str, &'a str)> {
        provider_name.zip(provider_session_id)
    }

    fn format_wrong_id_match_chain_resolution_error(err: DbError) -> String {
        format!("Failed to resolve chain for wrong-id-kind match: {err}")
    }

    pub(super) fn choose_resume_chain(
        &self,
        _input: &str,
        mut chain_ids: Vec<String>,
    ) -> Result<Option<String>, String> {
        if let Some(chain_id) = Self::only_resume_chain(&mut chain_ids) {
            return Ok(Some(chain_id));
        }
        let mut rows = self.load_resume_chain_candidates(chain_ids)?;
        Ok(Self::select_resume_chain(&mut rows))
    }

    fn only_resume_chain(chain_ids: &mut Vec<String>) -> Option<String> {
        if chain_ids.len() == 1 {
            chain_ids.pop()
        } else {
            None
        }
    }

    fn load_resume_chain_candidates(
        &self,
        chain_ids: Vec<String>,
    ) -> Result<Vec<ResumeChainCandidate>, String> {
        let mut rows = Vec::new();
        for chain_id in chain_ids {
            rows.push(self.load_resume_chain_candidate(chain_id)?);
        }
        Ok(rows)
    }

    fn select_resume_chain(rows: &mut [ResumeChainCandidate]) -> Option<String> {
        Self::sort_resume_chain_candidates(rows);
        rows.first().map(Self::map_resume_chain_candidate_id)
    }

    fn map_resume_chain_candidate_id(row: &ResumeChainCandidate) -> String {
        row.chain_id.clone()
    }

    pub(super) fn load_resume_chain_candidate(
        &self,
        chain_id: String,
    ) -> Result<ResumeChainCandidate, String> {
        let last_used_at = self.read_chain_last_used_at(&chain_id)?;
        let latest_segment_started_at = self.read_latest_segment_started_at(&chain_id)?;
        Ok(Self::map_resume_chain_candidate(
            chain_id,
            last_used_at,
            latest_segment_started_at,
        ))
    }

    fn map_resume_chain_candidate(
        chain_id: String,
        last_used_at: DateTime<Utc>,
        latest_segment_started_at: DateTime<Utc>,
    ) -> ResumeChainCandidate {
        ResumeChainCandidate {
            chain_id,
            last_used_at,
            latest_segment_started_at,
        }
    }

    pub(super) fn read_chain_last_used_at(&self, chain_id: &str) -> Result<DateTime<Utc>, String> {
        let raw = self.read_chain_last_used_at_raw(chain_id)?;
        Self::parse_chain_last_used_at(&raw)
    }

    fn read_chain_last_used_at_raw(&self, chain_id: &str) -> Result<String, String> {
        self.conn
            .query_row(
                "SELECT last_used_at FROM session_chains WHERE chain_id = ?1",
                sqlite::params![chain_id],
                Self::map_chain_timestamp_row,
            )
            .map_err(Self::format_chain_last_used_at_read_error)
    }

    fn parse_chain_last_used_at(raw: &str) -> Result<DateTime<Utc>, String> {
        Self::strict_rfc3339_message(raw, "chain last_used_at")
    }

    fn format_chain_last_used_at_read_error(err: sqlite::Error) -> String {
        format!("Failed to read chain last_used_at: {err}")
    }

    pub(super) fn read_latest_segment_started_at(
        &self,
        chain_id: &str,
    ) -> Result<DateTime<Utc>, String> {
        let raw_started = self.read_latest_segment_started_at_raw(chain_id)?;
        Self::parse_latest_segment_started_at(&raw_started)
    }

    fn read_latest_segment_started_at_raw(&self, chain_id: &str) -> Result<String, String> {
        self.conn
            .query_row(
                "SELECT started_at
                 FROM session_chain_segments
                 WHERE chain_id = ?1
                 ORDER BY started_at DESC, id DESC
                 LIMIT 1",
                sqlite::params![chain_id],
                Self::map_chain_timestamp_row,
            )
            .map_err(Self::format_latest_segment_started_at_read_error)
    }

    fn parse_latest_segment_started_at(raw_started: &str) -> Result<DateTime<Utc>, String> {
        Self::strict_rfc3339_message(raw_started, "chain segment started_at")
    }

    fn map_chain_timestamp_row(row: &sqlite::Row<'_>) -> sqlite::Result<String> {
        row.get(0)
    }

    fn format_latest_segment_started_at_read_error(err: sqlite::Error) -> String {
        format!("Failed to read chain latest segment started_at: {err}")
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
