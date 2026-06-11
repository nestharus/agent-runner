//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - orchestration
//! - parser
//!
//! Role set: { accessor, mapper, orchestration, parser }

use super::super::*;
pub(in crate::db::tests) fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
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
    db.get_windows(provider_name)
        .unwrap()
        .into_iter()
        .map(|window| {
            (
                window.window_id,
                window.used_percent,
                window.resets_at.to_rfc3339(),
            )
        })
        .collect()
}

pub(in crate::db::tests) type QuotaWindowDetailRow = (u32, f64, String, Option<f64>, Option<u64>);

pub(in crate::db::tests) fn quota_window_detail_rows(
    db: &StateDb,
    provider_name: &str,
) -> Vec<QuotaWindowDetailRow> {
    db.get_windows(provider_name)
        .unwrap()
        .into_iter()
        .map(|window| {
            (
                window.window_id,
                window.used_percent,
                window.resets_at.to_rfc3339(),
                window.last_delta_percent,
                window.last_delta_calls,
            )
        })
        .collect()
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
        .map(|i| SessionTurnIngest {
            session_id: format!("{id_prefix}-session"),
            turn_id: format!("{id_prefix}-turn-{i}"),
            timestamp: since + chrono::Duration::seconds((i + 1) as i64),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        })
        .collect()
}

pub(in crate::db::tests) fn last_empty_refresh_at(
    db: &StateDb,
    provider_name: &str,
) -> Option<DateTime<Utc>> {
    last_empty_refresh_at_raw(db, provider_name).map(|value| {
        DateTime::parse_from_rfc3339(&value)
            .unwrap()
            .with_timezone(&Utc)
    })
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
    exhausted_at_raw(db, provider_name).map(|value| {
        DateTime::parse_from_rfc3339(&value)
            .unwrap()
            .with_timezone(&Utc)
    })
}
