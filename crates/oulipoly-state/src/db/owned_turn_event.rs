//! ## Declared roles
//! accessor, formatter, mapper, orchestration
//!
//! Oulipoly-owned turn/event persistence for compact-summary evidence.
//!
//! This component uses a separate `owned_turn_events` table instead of adding
//! columns to `session_turns`: the existing table already mixes provider turn
//! storage with legacy compaction flags and has no dedicated metadata column.
//! A separate add-only table keeps the new Oulipoly-owned boundary independent
//! while preserving the old `session_turns` surface for stable reports.
//!
//! ## Adapter declarations
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/owned_turn_event.rs
//!     role: adapter
//!     Translates:
//!       - owned_turn_events SQLite table
//!       - StateDb owned turn/event facade methods

use super::*;
use crate::StateDbError;

/// Oulipoly-owned compact-summary evidence projected from provider transcripts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedTurnEventRow {
    pub session_id: String,
    pub turn_uuid: String,
    pub is_compaction_boundary: bool,
    pub summary_metadata_json: Option<String>,
}

/// Test-visible owned turn/event row read from the state boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedTurnEvent {
    pub session_id: String,
    pub turn_uuid: String,
    pub is_compaction_boundary: bool,
    pub summary_metadata_json: Option<String>,
    pub ingested_at: String,
}

struct OwnedTurnEventInsertValues<'a> {
    session_id: &'a str,
    turn_uuid: &'a str,
    is_compaction_boundary: i64,
    summary_metadata_json: &'a Option<String>,
    ingested_at: &'a str,
}

impl StateDb {
    pub fn insert_owned_turn_event_rows(
        &self,
        rows: &[OwnedTurnEventRow],
    ) -> Result<usize, StateDbError> {
        if Self::owned_turn_event_rows_are_empty(rows) {
            return Ok(0);
        }

        let now = Self::owned_turn_event_ingested_at();
        let tx = Self::begin_owned_turn_event_insert(&self.conn)?;
        let changed = Self::insert_owned_turn_event_rows_tx(&tx, rows, &now)?;
        Self::commit_owned_turn_event_insert(tx)?;
        Ok(changed)
    }

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

    fn insert_owned_turn_event_rows_tx(
        conn: &Connection,
        rows: &[OwnedTurnEventRow],
        ingested_at: &str,
    ) -> Result<usize, StateDbError> {
        let mut stmt = Self::prepare_owned_turn_event_insert(conn)?;
        Self::insert_owned_turn_event_row_batch(&mut stmt, rows, ingested_at)
    }

    fn insert_owned_turn_event_row(
        stmt: &mut rusqlite::Statement<'_>,
        row: &OwnedTurnEventRow,
        ingested_at: &str,
    ) -> Result<usize, StateDbError> {
        let values = Self::owned_turn_event_insert_values(row, ingested_at);
        Self::execute_owned_turn_event_insert(stmt, &values)
    }

    fn read_owned_turn_event_rows(
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

    fn prepare_owned_turn_event_insert(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::owned_turn_event_insert_sql())
            .map_err(Self::format_owned_turn_event_insert_prepare_error)
    }

    fn insert_owned_turn_event_row_batch(
        stmt: &mut rusqlite::Statement<'_>,
        rows: &[OwnedTurnEventRow],
        ingested_at: &str,
    ) -> Result<usize, StateDbError> {
        let mut changed = 0;
        for row in rows {
            changed += Self::insert_owned_turn_event_row(stmt, row, ingested_at)?;
        }
        Ok(changed)
    }

    fn owned_turn_event_insert_values<'a>(
        row: &'a OwnedTurnEventRow,
        ingested_at: &'a str,
    ) -> OwnedTurnEventInsertValues<'a> {
        OwnedTurnEventInsertValues {
            session_id: &row.session_id,
            turn_uuid: &row.turn_uuid,
            is_compaction_boundary: Self::sqlite_bool(row.is_compaction_boundary),
            summary_metadata_json: &row.summary_metadata_json,
            ingested_at,
        }
    }

    fn execute_owned_turn_event_insert(
        stmt: &mut rusqlite::Statement<'_>,
        values: &OwnedTurnEventInsertValues<'_>,
    ) -> Result<usize, StateDbError> {
        stmt.execute(params![
            values.session_id,
            values.turn_uuid,
            values.is_compaction_boundary,
            values.summary_metadata_json,
            values.ingested_at,
        ])
        .map_err(Self::format_owned_turn_event_row_insert_error)
    }

    fn owned_turn_event_insert_sql() -> &'static str {
        "INSERT INTO owned_turn_events
            (
                session_id,
                turn_uuid,
                is_compaction_boundary,
                summary_metadata_json,
                ingested_at
            )
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (session_id, turn_uuid) DO UPDATE SET
             is_compaction_boundary = CASE
                 WHEN excluded.is_compaction_boundary = 1 THEN 1
                 ELSE owned_turn_events.is_compaction_boundary
             END,
             summary_metadata_json = COALESCE(
                 excluded.summary_metadata_json,
                 owned_turn_events.summary_metadata_json
             ),
             ingested_at = excluded.ingested_at"
    }

    fn owned_turn_event_read_sql() -> &'static str {
        "SELECT session_id,
                turn_uuid,
                is_compaction_boundary,
                summary_metadata_json,
                ingested_at
         FROM owned_turn_events
         WHERE session_id = ?1
         ORDER BY id"
    }

    fn map_owned_turn_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OwnedTurnEvent> {
        let is_compaction_boundary = row.get::<_, i64>(2)? != 0;
        Ok(OwnedTurnEvent {
            session_id: row.get(0)?,
            turn_uuid: row.get(1)?,
            is_compaction_boundary,
            summary_metadata_json: row.get(3)?,
            ingested_at: row.get(4)?,
        })
    }

    fn compact_summary_owned_turn_uuids(
        &self,
        session_id: &str,
    ) -> Result<Vec<String>, StateDbError> {
        Self::read_compact_summary_owned_turn_uuids(&self.conn, session_id)
    }

    fn read_compact_summary_owned_turn_uuids(
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

    fn compact_summary_evidence_read_sql() -> &'static str {
        "SELECT turn_uuid
         FROM owned_turn_events
         WHERE session_id = ?1
           AND is_compaction_boundary = 1
         ORDER BY id"
    }

    fn prepare_owned_turn_event_read(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::owned_turn_event_read_sql())
            .map_err(Self::format_owned_turn_event_read_prepare_error)
    }

    fn prepare_compact_summary_evidence_read(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::compact_summary_evidence_read_sql())
            .map_err(Self::format_compact_summary_evidence_read_prepare_error)
    }

    fn owned_turn_event_rows_are_empty(rows: &[OwnedTurnEventRow]) -> bool {
        rows.is_empty()
    }

    fn owned_turn_event_ingested_at() -> String {
        Utc::now().to_rfc3339()
    }

    fn begin_owned_turn_event_insert(conn: &Connection) -> Result<Transaction<'_>, StateDbError> {
        conn.unchecked_transaction()
            .map_err(Self::format_owned_turn_event_insert_begin_error)
    }

    fn commit_owned_turn_event_insert(tx: Transaction<'_>) -> Result<(), StateDbError> {
        tx.commit()
            .map_err(Self::format_owned_turn_event_insert_commit_error)
    }

    fn map_compact_summary_evidence(
        session_id: &str,
        compact_turn_uuids: Vec<String>,
    ) -> CompactSummaryEvidence {
        CompactSummaryEvidence {
            session_id: session_id.to_string(),
            compact_turn_uuids,
        }
    }

    fn format_owned_turn_event_insert_begin_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to begin owned turn/event insert: {error}")
    }

    fn format_owned_turn_event_insert_commit_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to commit owned turn/event insert: {error}")
    }

    fn format_owned_turn_event_insert_prepare_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to prepare owned turn/event insert: {error}")
    }

    fn format_owned_turn_event_row_insert_error(error: rusqlite::Error) -> StateDbError {
        format!("Owned turn/event row insert failed: {error}")
    }

    fn format_owned_turn_event_read_prepare_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to prepare owned turn/event read: {error}")
    }

    fn format_owned_turn_event_query_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to query owned turn/event rows: {error}")
    }

    fn format_owned_turn_event_row_read_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to read owned turn/event row: {error}")
    }

    fn format_compact_summary_evidence_read_prepare_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to prepare compact-summary evidence read: {error}")
    }

    fn format_compact_summary_evidence_query_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to query compact-summary evidence: {error}")
    }

    fn format_compact_summary_evidence_row_read_error(error: rusqlite::Error) -> StateDbError {
        format!("Failed to read compact-summary evidence row: {error}")
    }
}
