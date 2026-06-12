//! ## Declared roles
//!
//! - formatter
//! - mapper
//! - orchestration
//! - predicate
//!
//! Role set: { formatter, mapper, orchestration, predicate }
//!
//! Session-turn DTOs and ingest writes.

use super::*;
use chrono::{DateTime, Utc};

/// One turn ingested from a CLI session log. The unified store across
/// every provider CLI we know how to parse.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SessionTurnRecord {
    pub provider_name: String,
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    /// "user" or "assistant" — only "assistant" turns count toward quota.
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub source_file: String,
}

/// One turn batched into `ingest_session_turns_batch`. Named struct
/// instead of a tuple so callers can't accidentally swap positional
/// fields (the role / parent_turn_id pair is otherwise easy to mix up).
#[derive(Debug, Clone)]
pub struct SessionTurnIngest {
    pub session_id: String,
    pub turn_id: String,
    pub timestamp: DateTime<Utc>,
    pub role: String,
    pub parent_turn_id: Option<String>,
    pub is_sidechain: bool,
    pub is_compaction_boundary: bool,
    pub body: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTurnCounts {
    pub total: u64,
    pub assistant: u64,
    pub sidechain: u64,
}

pub(super) struct SessionTurnBindValues<'a> {
    pub(super) session_id: &'a str,
    pub(super) turn_id: &'a str,
    pub(super) timestamp: String,
    pub(super) role: &'a str,
    pub(super) parent_turn_id: Option<&'a str>,
    pub(super) is_sidechain: i64,
    pub(super) is_compaction_boundary: i64,
    pub(super) body: Option<&'a str>,
}

pub(super) struct SessionTurnInsertRow<'a> {
    pub(super) provider_name: &'a str,
    pub(super) session_id: &'a str,
    pub(super) turn_id: &'a str,
    pub(super) timestamp: String,
    pub(super) role: &'a str,
    pub(super) source_file: &'a str,
    pub(super) ingested_at: String,
    pub(super) body: Option<&'a str>,
}

impl StateDb {
    // --- Session log ingestion ---

    /// Insert one parsed turn. Idempotent: re-running a scan against an
    /// unchanged log is a no-op for already-seen turns.
    pub fn ingest_session_turn(
        &self,
        provider_name: &str,
        session_id: &str,
        turn_id: &str,
        timestamp: &DateTime<Utc>,
        role: &str,
        source_file: &str,
    ) -> Result<bool, String> {
        let row = Self::session_turn_insert_row(
            provider_name,
            session_id,
            turn_id,
            timestamp,
            role,
            source_file,
        );
        let changed = self.insert_session_turn_row(&row)?;
        Ok(Self::session_turn_insert_changed(changed))
    }

    fn session_turn_insert_row<'a>(
        provider_name: &'a str,
        session_id: &'a str,
        turn_id: &'a str,
        timestamp: &DateTime<Utc>,
        role: &'a str,
        source_file: &'a str,
    ) -> SessionTurnInsertRow<'a> {
        SessionTurnInsertRow {
            provider_name,
            session_id,
            turn_id,
            timestamp: Self::format_session_turn_timestamp(timestamp),
            role,
            source_file,
            ingested_at: Self::current_rfc3339_timestamp(),
            body: None,
        }
    }

    fn insert_session_turn_row(&self, row: &SessionTurnInsertRow<'_>) -> Result<usize, String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO session_turns
                    (provider_name, session_id, turn_id, timestamp, role, is_compaction_boundary, source_file, ingested_at, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8)",
                sqlite::params![
                    row.provider_name,
                    row.session_id,
                    row.turn_id,
                    &row.timestamp,
                    row.role,
                    row.source_file,
                    &row.ingested_at,
                    row.body,
                ],
            )
            .map_err(Self::format_session_turn_ingest_error)
    }

    fn session_turn_insert_changed(changed: usize) -> bool {
        changed > 0
    }

    fn format_session_turn_ingest_error(err: sqlite::Error) -> String {
        format!("Failed to ingest session turn: {err}")
    }

    /// Bulk-insert turns inside a single transaction with a prepared
    /// statement. Hundreds of thousands of rows go from minutes to seconds
    /// vs the per-row method. Returns the count of newly-inserted rows
    /// (duplicates collapsed by the UNIQUE constraint don't count).
    pub fn ingest_session_turns_batch(
        &self,
        provider_name: &str,
        turns: &[SessionTurnIngest],
    ) -> Result<u64, String> {
        if Self::session_turn_batch_is_empty(turns) {
            return Ok(0);
        }
        let now = Self::current_rfc3339_timestamp();
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(Self::format_session_turn_batch_begin_error)?;
        let new_count = Self::insert_session_turn_batch_rows(&tx, provider_name, turns, &now)?;
        tx.commit()
            .map_err(Self::format_session_turn_batch_commit_error)?;
        Ok(new_count)
    }

    fn format_session_turn_batch_begin_error(err: sqlite::Error) -> String {
        format!("Failed to begin transaction: {err}")
    }

    fn format_session_turn_batch_commit_error(err: sqlite::Error) -> String {
        format!("Failed to commit batch: {err}")
    }

    pub(super) fn session_turn_batch_is_empty(turns: &[SessionTurnIngest]) -> bool {
        turns.is_empty()
    }

    pub(super) fn insert_session_turn_batch_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut stmt = Self::prepare_session_turn_batch_insert(conn)?;
        Self::execute_session_turn_writes(&mut stmt, provider_name, turns, ingested_at)
    }

    pub(super) fn prepare_session_turn_batch_insert(
        conn: &sqlite::Connection,
    ) -> Result<sqlite::Statement<'_>, String> {
        conn.prepare(Self::session_turn_batch_insert_sql())
            .map_err(Self::format_session_turn_prepare_error)
    }

    pub(super) fn execute_session_turn_writes(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        turns: &[SessionTurnIngest],
        ingested_at: &str,
    ) -> Result<u64, String> {
        let mut new_count: u64 = 0;
        for turn in turns {
            let binds = Self::bind_session_turn_row_params(turn);
            let n =
                Self::execute_session_turn_batch_insert(stmt, provider_name, &binds, ingested_at)?;
            new_count += n as u64;
        }
        Ok(new_count)
    }

    pub(super) fn format_session_turn_prepare_error(err: sqlite::Error) -> String {
        format!("Failed to prepare batch insert: {err}")
    }

    pub(super) fn session_turn_batch_insert_sql() -> &'static str {
        "INSERT OR IGNORE INTO session_turns
            (
                provider_name,
                session_id,
                turn_id,
                timestamp,
                role,
                parent_turn_id,
                is_sidechain,
                is_compaction_boundary,
                source_file,
                ingested_at,
                body
            )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '', ?9, ?10)"
    }

    pub(super) fn bind_session_turn_row_params(
        turn: &SessionTurnIngest,
    ) -> SessionTurnBindValues<'_> {
        SessionTurnBindValues {
            session_id: &turn.session_id,
            turn_id: &turn.turn_id,
            timestamp: Self::format_session_turn_timestamp(&turn.timestamp),
            role: &turn.role,
            parent_turn_id: turn.parent_turn_id.as_deref(),
            is_sidechain: Self::sqlite_bool(turn.is_sidechain),
            is_compaction_boundary: Self::sqlite_bool(turn.is_compaction_boundary),
            body: turn.body.as_deref(),
        }
    }

    fn format_session_turn_timestamp(timestamp: &DateTime<Utc>) -> String {
        timestamp.to_rfc3339()
    }

    pub(super) fn sqlite_bool(value: bool) -> i64 {
        if value { 1 } else { 0 }
    }

    pub(super) fn execute_session_turn_batch_insert(
        stmt: &mut sqlite::Statement<'_>,
        provider_name: &str,
        binds: &SessionTurnBindValues<'_>,
        ingested_at: &str,
    ) -> Result<usize, String> {
        stmt.execute(sqlite::params![
            provider_name,
            binds.session_id,
            binds.turn_id,
            &binds.timestamp,
            binds.role,
            binds.parent_turn_id,
            binds.is_sidechain,
            binds.is_compaction_boundary,
            ingested_at,
            binds.body,
        ])
        .map_err(Self::format_session_turn_batch_insert_error)
    }

    fn format_session_turn_batch_insert_error(err: sqlite::Error) -> String {
        format!("Batch insert row failed: {err}")
    }
}
