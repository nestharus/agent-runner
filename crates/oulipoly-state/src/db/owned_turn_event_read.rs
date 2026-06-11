//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - formatter
//!
//! Role set: { accessor, mapper, formatter }
//!
//! Owned turn/event reads and compact-summary evidence mapping.

use super::*;
use crate::StateDbError;

impl StateDb {
    pub fn owned_turn_event_rows_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<OwnedTurnEvent>, StateDbError> {
        Self::read_owned_turn_event_rows(&self.conn, session_id)
    }

    pub fn compact_summary_evidence(
        &self,
        session_id: &str,
    ) -> Result<CompactSummaryEvidence, StateDbError> {
        let rows = self.compact_summary_owned_turn_uuids(session_id)?;
        Ok(Self::map_compact_summary_evidence(session_id, rows))
    }

    pub(super) fn read_owned_turn_event_rows(
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<OwnedTurnEvent>, StateDbError> {
        let mut stmt = Self::prepare_owned_turn_event_read(conn)?;
        let rows = stmt
            .query_map(params![session_id], Self::map_owned_turn_event_row)
            .map_err(Self::format_owned_turn_event_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_owned_turn_event_row_read_error)
    }

    pub(super) fn owned_turn_event_read_sql() -> &'static str {
        "SELECT session_id,
                turn_uuid,
                is_compaction_boundary,
                summary_metadata_json,
                ingested_at
         FROM owned_turn_events
         WHERE session_id = ?1
         ORDER BY id"
    }

    pub(super) fn map_owned_turn_event_row(
        row: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<OwnedTurnEvent> {
        let is_compaction_boundary = row.get::<_, i64>(2)? != 0;
        Ok(OwnedTurnEvent {
            session_id: row.get(0)?,
            turn_uuid: row.get(1)?,
            is_compaction_boundary,
            summary_metadata_json: row.get(3)?,
            ingested_at: row.get(4)?,
        })
    }

    pub(super) fn compact_summary_owned_turn_uuids(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, StateDbError> {
        Self::read_compact_summary_owned_turn_uuids(&self.conn, session_id)
    }

    pub(super) fn read_compact_summary_owned_turn_uuids(
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<String>, StateDbError> {
        let mut stmt = Self::prepare_compact_summary_evidence_read(conn)?;
        let rows = stmt
            .query_map(params![session_id], |row| row.get::<_, String>(0))
            .map_err(Self::format_compact_summary_evidence_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_compact_summary_evidence_row_read_error)
    }

    pub(super) fn compact_summary_evidence_read_sql() -> &'static str {
        "SELECT turn_uuid
         FROM owned_turn_events
         WHERE session_id = ?1
           AND is_compaction_boundary = 1
         ORDER BY id"
    }

    pub(super) fn prepare_owned_turn_event_read(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::owned_turn_event_read_sql())
            .map_err(Self::format_owned_turn_event_read_prepare_error)
    }

    pub(super) fn prepare_compact_summary_evidence_read(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::compact_summary_evidence_read_sql())
            .map_err(Self::format_compact_summary_evidence_read_prepare_error)
    }

    pub(super) fn map_compact_summary_evidence(
        session_id: &str,
        compact_turn_uuids: Vec<String>,
    ) -> CompactSummaryEvidence {
        CompactSummaryEvidence {
            session_id: session_id.to_string(),
            compact_turn_uuids,
        }
    }

    pub(super) fn format_owned_turn_event_read_prepare_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to prepare owned turn/event read: {error}")
    }

    pub(super) fn format_owned_turn_event_query_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to query owned turn/event rows: {error}")
    }

    pub(super) fn format_owned_turn_event_row_read_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to read owned turn/event row: {error}")
    }

    pub(super) fn format_compact_summary_evidence_read_prepare_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to prepare compact-summary evidence read: {error}")
    }

    pub(super) fn format_compact_summary_evidence_query_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to query compact-summary evidence: {error}")
    }

    pub(super) fn format_compact_summary_evidence_row_read_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to read compact-summary evidence row: {error}")
    }
}
