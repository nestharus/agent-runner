//! ## Declared roles
//!
//! - mapper
//! - orchestration
//! - formatter
//!
//! Role set: { mapper, orchestration, formatter }
//!
//! Owned turn/event DTOs and insert persistence.

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

pub(super) struct OwnedTurnEventInsertValues<'a> {
    pub(super) session_id: &'a str,
    pub(super) turn_uuid: &'a str,
    pub(super) is_compaction_boundary: i64,
    pub(super) summary_metadata_json: &'a Option<String>,
    pub(super) ingested_at: &'a str,
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

    pub(super) fn insert_owned_turn_event_rows_tx(
        conn: &Connection,
        rows: &[OwnedTurnEventRow],
        ingested_at: &str,
    ) -> Result<usize, StateDbError> {
        let mut stmt = Self::prepare_owned_turn_event_insert(conn)?;
        Self::insert_owned_turn_event_row_batch(&mut stmt, rows, ingested_at)
    }

    pub(super) fn insert_owned_turn_event_row(
        stmt: &mut rusqlite::Statement<'_>,
        row: &OwnedTurnEventRow,
        ingested_at: &str,
    ) -> Result<usize, StateDbError> {
        let values = Self::owned_turn_event_insert_values(row, ingested_at);
        Self::execute_owned_turn_event_insert(stmt, &values)
    }

    pub(super) fn prepare_owned_turn_event_insert(
        conn: &Connection,
    ) -> Result<rusqlite::Statement<'_>, StateDbError> {
        conn.prepare(Self::owned_turn_event_insert_sql())
            .map_err(Self::format_owned_turn_event_insert_prepare_error)
    }

    pub(super) fn insert_owned_turn_event_row_batch(
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

    pub(super) fn owned_turn_event_insert_values<'a>(
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

    pub(super) fn execute_owned_turn_event_insert(
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

    pub(super) fn owned_turn_event_insert_sql() -> &'static str {
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

    pub(super) fn owned_turn_event_rows_are_empty(rows: &[OwnedTurnEventRow]) -> bool {
        rows.is_empty()
    }

    pub(super) fn owned_turn_event_ingested_at() -> String {
        Utc::now().to_rfc3339()
    }

    pub(super) fn begin_owned_turn_event_insert(
        conn: &Connection,
    ) -> Result<Transaction<'_>, StateDbError> {
        conn.unchecked_transaction()
            .map_err(Self::format_owned_turn_event_insert_begin_error)
    }

    pub(super) fn commit_owned_turn_event_insert(tx: Transaction<'_>) -> Result<(), StateDbError> {
        tx.commit()
            .map_err(Self::format_owned_turn_event_insert_commit_error)
    }

    pub(super) fn format_owned_turn_event_insert_begin_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to begin owned turn/event insert: {error}")
    }

    pub(super) fn format_owned_turn_event_insert_commit_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to commit owned turn/event insert: {error}")
    }

    pub(super) fn format_owned_turn_event_insert_prepare_error(
        error: rusqlite::Error,
    ) -> StateDbError {
        format!("Failed to prepare owned turn/event insert: {error}")
    }

    pub(super) fn format_owned_turn_event_row_insert_error(error: rusqlite::Error) -> StateDbError {
        format!("Owned turn/event row insert failed: {error}")
    }
}
