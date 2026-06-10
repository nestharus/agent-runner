//! ## Declared roles
//!
//! accessor, mapper, formatter
//!
//! Narrow live-load read surface for routing tie-breaks.

use crate::db::{InvocationStatus, StateDb};
use chrono::{DateTime, Utc};
use rusqlite::params_from_iter;
use std::collections::HashMap;

impl StateDb {
    pub fn running_invocation_counts_by_provider(
        &self,
        provider_names: &[&str],
        since: DateTime<Utc>,
    ) -> Result<HashMap<String, u64>, String> {
        read_running_invocation_counts_by_provider(self.connection(), provider_names, since)
    }
}

fn read_running_invocation_counts_by_provider(
    conn: &rusqlite::Connection,
    provider_names: &[&str],
    since: DateTime<Utc>,
) -> Result<HashMap<String, u64>, String> {
    if provider_names.is_empty() {
        return Ok(HashMap::new());
    }
    let sql = running_invocation_count_sql(provider_names.len());
    let since = since.to_rfc3339();
    let params = running_invocation_count_params(provider_names, &since);
    let mut statement = conn
        .prepare(&sql)
        .map_err(format_running_invocation_count_prepare_error)?;
    let rows = statement
        .query_map(params_from_iter(params), map_running_invocation_count_row)
        .map_err(format_running_invocation_count_query_error)?;
    rows.collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(format_running_invocation_count_map_error)
}

fn running_invocation_count_sql(provider_count: usize) -> String {
    let placeholders = std::iter::repeat_n("?", provider_count)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "SELECT provider_name, COUNT(*)
         FROM invocations
         WHERE status = ? AND created_at >= ? AND provider_name IN ({placeholders})
         GROUP BY provider_name"
    )
}

fn running_invocation_count_params<'a>(
    provider_names: &'a [&'a str],
    since: &'a str,
) -> Vec<&'a str> {
    std::iter::once(InvocationStatus::Running.as_str())
        .chain(std::iter::once(since))
        .chain(provider_names.iter().copied())
        .collect()
}

fn map_running_invocation_count_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u64)> {
    let provider_name = row.get(0)?;
    let count: i64 = row.get(1)?;
    Ok((provider_name, count.max(0) as u64))
}

fn format_running_invocation_count_prepare_error(error: rusqlite::Error) -> String {
    format!("Failed to prepare running invocation count query: {error}")
}

fn format_running_invocation_count_query_error(error: rusqlite::Error) -> String {
    format!("Failed to query running invocation counts: {error}")
}

fn format_running_invocation_count_map_error(error: rusqlite::Error) -> String {
    format!("Failed to map running invocation counts: {error}")
}
