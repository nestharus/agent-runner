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
    if provider_names_are_empty(provider_names) {
        return Ok(empty_running_invocation_counts());
    }
    read_running_invocation_counts(
        conn,
        running_invocation_count_request(provider_names, since),
    )
}

struct RunningInvocationCountRequest {
    sql: String,
    params: Vec<String>,
}

struct RunningInvocationCountRow {
    provider_name: String,
    count: i64,
}

fn provider_names_are_empty(provider_names: &[&str]) -> bool {
    provider_names.is_empty()
}

fn empty_running_invocation_counts() -> HashMap<String, u64> {
    HashMap::new()
}

fn running_invocation_count_request(
    provider_names: &[&str],
    since: DateTime<Utc>,
) -> RunningInvocationCountRequest {
    let sql = running_invocation_count_sql(provider_names.len());
    let since = running_invocation_count_since_param(since);
    let params = running_invocation_count_params(provider_names, &since);
    RunningInvocationCountRequest { sql, params }
}

fn read_running_invocation_counts(
    conn: &rusqlite::Connection,
    request: RunningInvocationCountRequest,
) -> Result<HashMap<String, u64>, String> {
    let mut statement = conn
        .prepare(&request.sql)
        .map_err(format_running_invocation_count_prepare_error)?;
    let rows = statement
        .query_map(
            params_from_iter(request.params),
            map_running_invocation_count_row,
        )
        .map_err(format_running_invocation_count_query_error)?;
    rows.collect::<rusqlite::Result<HashMap<_, _>>>()
        .map_err(format_running_invocation_count_map_error)
}

fn running_invocation_count_since_param(since: DateTime<Utc>) -> String {
    since.to_rfc3339()
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

fn running_invocation_count_params(provider_names: &[&str], since: &str) -> Vec<String> {
    std::iter::once(InvocationStatus::Running.as_str().to_string())
        .chain(std::iter::once(since.to_string()))
        .chain(
            provider_names
                .iter()
                .map(|provider_name| provider_name.to_string()),
        )
        .collect()
}

fn map_running_invocation_count_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, u64)> {
    running_invocation_count_entry(parse_running_invocation_count_row(row)?)
}

fn parse_running_invocation_count_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RunningInvocationCountRow> {
    let provider_name = row.get(0)?;
    let count = row.get(1)?;
    Ok(RunningInvocationCountRow {
        provider_name,
        count,
    })
}

fn running_invocation_count_entry(
    row: RunningInvocationCountRow,
) -> rusqlite::Result<(String, u64)> {
    Ok((
        row.provider_name,
        normalized_running_invocation_count(row.count),
    ))
}

fn normalized_running_invocation_count(count: i64) -> u64 {
    count.max(0) as u64
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
