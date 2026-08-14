//! ## Declared roles
//!
//! accessor, mapper, formatter
//!
//! Session-chain segment read surface for resume fallback resolution.

use crate::db::{DbError, StateDb};
use rusqlite::params;

/// One segment of a session chain, for newest-first fallback resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainSegmentRow {
    pub provider_name: String,
    pub session_id: String,
    pub started_at: String,
}

impl StateDb {
    /// All segments of a chain, newest first, for fallback when the active
    /// segment's provider is no longer registered.
    pub fn list_chain_segments_desc(
        &self,
        chain_id: &str,
    ) -> Result<Vec<ChainSegmentRow>, DbError> {
        read_chain_segments_desc(self.raw_connection(), chain_id)
    }
}

fn read_chain_segments_desc(
    conn: &rusqlite::Connection,
    chain_id: &str,
) -> Result<Vec<ChainSegmentRow>, DbError> {
    let mut statement = conn
        .prepare(
            "SELECT provider_name, session_id, started_at
             FROM session_chain_segments
             WHERE chain_id = ?1
             ORDER BY started_at DESC, id DESC",
        )
        .map_err(format_chain_segments_desc_prepare_error)?;
    let rows = statement
        .query_map(params![chain_id], map_chain_segment_row)
        .map_err(format_chain_segments_desc_query_error)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(format_chain_segments_desc_read_error)
}

fn map_chain_segment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChainSegmentRow> {
    Ok(ChainSegmentRow {
        provider_name: row.get(0)?,
        session_id: row.get(1)?,
        started_at: row.get(2)?,
    })
}

fn format_chain_segments_desc_prepare_error(error: rusqlite::Error) -> DbError {
    format!("Failed to prepare chain segment list: {error}")
}

fn format_chain_segments_desc_query_error(error: rusqlite::Error) -> DbError {
    format!("Failed to query chain segments: {error}")
}

fn format_chain_segments_desc_read_error(error: rusqlite::Error) -> DbError {
    format!("Failed to read chain segments: {error}")
}
