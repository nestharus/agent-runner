//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//! - parser
//! - validator
//!
//! Role set: { accessor, mapper, orchestration, parser, validator }

use super::super::*;
pub(in crate::db::tests) fn ts(value: &str) -> DateTime<Utc> {
    parse_test_timestamp_utc(value)
}

fn parse_test_timestamp_utc(value: &str) -> DateTime<Utc> {
    let parsed = parse_test_timestamp(value);
    let parsed = require_test_timestamp(parsed);
    map_test_timestamp(parsed)
}

fn parse_test_timestamp(value: &str) -> Result<DateTime<chrono::FixedOffset>, chrono::ParseError> {
    DateTime::parse_from_rfc3339(value)
}

fn require_test_timestamp(
    result: Result<DateTime<chrono::FixedOffset>, chrono::ParseError>,
) -> DateTime<chrono::FixedOffset> {
    result.unwrap()
}

fn map_test_timestamp(parsed: DateTime<chrono::FixedOffset>) -> DateTime<Utc> {
    parsed.with_timezone(&Utc)
}

pub(in crate::db::tests) fn quota_input(used_percent: f64, resets_at: &str) -> QuotaWindowInput {
    QuotaWindowInput {
        used_percent,
        resets_at: ts(resets_at),
    }
}

pub(in crate::db::tests) fn quota_window_rows(
    db: &StateDb,
    provider_name: &str,
) -> Vec<(u32, f64, String)> {
    map_quota_window_rows(read_quota_windows(db, provider_name))
}

fn read_quota_windows(db: &StateDb, provider_name: &str) -> Vec<QuotaWindow> {
    require_quota_windows(read_quota_windows_result(db, provider_name))
}

fn read_quota_windows_result(
    db: &StateDb,
    provider_name: &str,
) -> Result<Vec<QuotaWindow>, String> {
    db.get_windows(provider_name)
}

fn require_quota_windows(result: Result<Vec<QuotaWindow>, String>) -> Vec<QuotaWindow> {
    result.unwrap()
}

fn map_quota_window_rows(windows: Vec<QuotaWindow>) -> Vec<(u32, f64, String)> {
    windows.into_iter().map(quota_window_row).collect()
}

fn quota_window_row(window: QuotaWindow) -> (u32, f64, String) {
    (
        window.window_id,
        window.used_percent,
        quota_window_reset_timestamp(&window),
    )
}

pub(in crate::db::tests) type QuotaWindowDetailRow = (u32, f64, String, Option<f64>, Option<u64>);

pub(in crate::db::tests) fn quota_window_detail_rows(
    db: &StateDb,
    provider_name: &str,
) -> Vec<QuotaWindowDetailRow> {
    map_quota_window_detail_rows(read_quota_windows(db, provider_name))
}

fn map_quota_window_detail_rows(windows: Vec<QuotaWindow>) -> Vec<QuotaWindowDetailRow> {
    windows.into_iter().map(quota_window_detail_row).collect()
}

fn quota_window_detail_row(window: QuotaWindow) -> QuotaWindowDetailRow {
    (
        window.window_id,
        window.used_percent,
        quota_window_reset_timestamp(&window),
        window.last_delta_percent,
        window.last_delta_calls,
    )
}

fn quota_window_reset_timestamp(window: &QuotaWindow) -> String {
    window.resets_at.to_rfc3339()
}

pub(in crate::db::tests) fn insert_assistant_turns_after(
    db: &StateDb,
    provider_name: &str,
    since: DateTime<Utc>,
    count: usize,
    id_prefix: &str,
) {
    let turns = assistant_turns_after(since, count, id_prefix);
    db.ingest_session_turns_batch(provider_name, &turns)
        .unwrap();
}

fn assistant_turns_after(
    since: DateTime<Utc>,
    count: usize,
    id_prefix: &str,
) -> Vec<SessionTurnIngest> {
    (0..count)
        .map(|i| assistant_turn_after(since, id_prefix, i))
        .collect()
}

fn assistant_turn_after(since: DateTime<Utc>, id_prefix: &str, index: usize) -> SessionTurnIngest {
    SessionTurnIngest {
        session_id: assistant_session_id(id_prefix),
        turn_id: assistant_turn_id(id_prefix, index),
        timestamp: assistant_turn_timestamp(since, index),
        role: assistant_role(),
        parent_turn_id: None,
        is_sidechain: false,
        is_compaction_boundary: false,
        body: None,
    }
}

fn assistant_session_id(id_prefix: &str) -> String {
    format!("{id_prefix}-session")
}

fn assistant_turn_id(id_prefix: &str, index: usize) -> String {
    format!("{id_prefix}-turn-{index}")
}

fn assistant_turn_timestamp(since: DateTime<Utc>, index: usize) -> DateTime<Utc> {
    since + chrono::Duration::seconds((index + 1) as i64)
}

fn assistant_role() -> String {
    "assistant".to_string()
}

pub(in crate::db::tests) fn last_empty_refresh_at(
    db: &StateDb,
    provider_name: &str,
) -> Option<DateTime<Utc>> {
    parse_optional_test_timestamp(last_empty_refresh_at_raw(db, provider_name))
}

fn parse_optional_test_timestamp(raw: Option<String>) -> Option<DateTime<Utc>> {
    raw.as_deref().map(parse_test_timestamp_utc)
}

fn last_empty_refresh_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
    db.conn
        .query_row(
            "SELECT last_empty_refresh_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn last_topology_probe_at_raw(
    db: &StateDb,
    provider_name: &str,
) -> Option<String> {
    db.conn
        .query_row(
            "SELECT last_topology_probe_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn calls_since_refresh(db: &StateDb, provider_name: &str) -> u64 {
    db.conn
        .query_row(
            "SELECT calls_since_refresh
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, i64>(0),
        )
        .unwrap() as u64
}

pub(in crate::db::tests) fn exhausted_at_raw(db: &StateDb, provider_name: &str) -> Option<String> {
    db.conn
        .query_row(
            "SELECT exhausted_at
                 FROM provider_quotas
                 WHERE provider_name = ?1",
            sqlite::params![provider_name],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
}

pub(in crate::db::tests) fn exhausted_at(
    db: &StateDb,
    provider_name: &str,
) -> Option<DateTime<Utc>> {
    parse_optional_test_timestamp(exhausted_at_raw(db, provider_name))
}
