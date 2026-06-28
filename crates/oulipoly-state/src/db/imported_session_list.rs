//! Imported and resumable session listing persistence.

use super::*;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ImportedSessionListRow {
    pub chain_id: String,
    pub active_provider: String,
    pub active_provider_session_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub last_used_or_updated_at: DateTime<Utc>,
    pub turn_count: usize,
    pub is_imported: bool,
}

struct ImportedSessionListRawRow {
    chain_id: String,
    active_provider: String,
    active_provider_session_id: String,
    title: Option<String>,
    cwd: Option<String>,
    last_used_or_updated_at_raw: String,
    turn_count: i64,
    is_imported: i64,
}

impl StateDb {
    pub fn imported_session_list(&self) -> Result<Vec<ImportedSessionListRow>, DbError> {
        let raw_rows = self.query_imported_session_list_rows()?;
        let mut rows = Self::parse_imported_session_list_rows(raw_rows)?;
        Self::sort_imported_session_list_rows(&mut rows);
        Ok(rows)
    }

    fn query_imported_session_list_rows(&self) -> Result<Vec<ImportedSessionListRawRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare(Self::imported_session_list_sql())
            .map_err(Self::format_imported_session_list_prepare_error)?;
        let rows = stmt
            .query_map([], Self::map_imported_session_list_raw_row)
            .map_err(Self::format_imported_session_list_query_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(Self::format_imported_session_list_read_error)
    }

    fn imported_session_list_sql() -> &'static str {
        "SELECT c.chain_id,
                s.provider_name,
                s.session_id,
                m.title,
                m.cwd,
                COALESCE(m.provider_updated_at, c.last_used_at, m.last_seen_at) AS last_used_or_updated_at,
                (
                    SELECT COUNT(*)
                    FROM session_turns st
                    WHERE st.provider_name = s.provider_name
                      AND st.session_id = s.session_id
                ) AS turn_count,
                CASE WHEN m.provider_session_id IS NULL THEN 0 ELSE 1 END AS is_imported
         FROM session_chains c
         JOIN session_chain_segments s ON s.id = (
             SELECT s2.id
             FROM session_chain_segments s2
             WHERE s2.chain_id = c.chain_id
               AND s2.ended_at IS NULL
             ORDER BY s2.started_at DESC, s2.id DESC
             LIMIT 1
         )
         LEFT JOIN imported_session_display_metadata m
           ON m.provider_name = s.provider_name
          AND m.provider_session_id = s.session_id"
    }

    fn map_imported_session_list_raw_row(
        row: &sqlite::Row<'_>,
    ) -> sqlite::Result<ImportedSessionListRawRow> {
        Ok(ImportedSessionListRawRow {
            chain_id: row.get(0)?,
            active_provider: row.get(1)?,
            active_provider_session_id: row.get(2)?,
            title: row.get(3)?,
            cwd: row.get(4)?,
            last_used_or_updated_at_raw: row.get(5)?,
            turn_count: row.get(6)?,
            is_imported: row.get(7)?,
        })
    }

    fn parse_imported_session_list_rows(
        rows: Vec<ImportedSessionListRawRow>,
    ) -> Result<Vec<ImportedSessionListRow>, DbError> {
        rows.into_iter()
            .map(Self::parse_imported_session_list_row)
            .collect()
    }

    fn parse_imported_session_list_row(
        row: ImportedSessionListRawRow,
    ) -> Result<ImportedSessionListRow, DbError> {
        Ok(ImportedSessionListRow {
            chain_id: row.chain_id,
            active_provider: row.active_provider,
            active_provider_session_id: row.active_provider_session_id,
            title: row.title,
            cwd: row.cwd,
            last_used_or_updated_at: Self::strict_rfc3339_message(
                &row.last_used_or_updated_at_raw,
                "session list last_used_or_updated_at",
            )?,
            turn_count: row.turn_count.max(0) as usize,
            is_imported: row.is_imported != 0,
        })
    }

    fn sort_imported_session_list_rows(rows: &mut [ImportedSessionListRow]) {
        rows.sort_by(|left, right| {
            right
                .last_used_or_updated_at
                .cmp(&left.last_used_or_updated_at)
                .then_with(|| left.active_provider.cmp(&right.active_provider))
                .then_with(|| {
                    left.active_provider_session_id
                        .cmp(&right.active_provider_session_id)
                })
        });
    }

    fn format_imported_session_list_prepare_error(error: sqlite::Error) -> DbError {
        format!("Failed to prepare imported session list query: {error}")
    }

    fn format_imported_session_list_query_error(error: sqlite::Error) -> DbError {
        format!("Failed to query imported session list: {error}")
    }

    fn format_imported_session_list_read_error(error: sqlite::Error) -> DbError {
        format!("Failed to read imported session list row: {error}")
    }
}
