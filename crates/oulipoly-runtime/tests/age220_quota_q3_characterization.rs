use chrono::{Duration, Utc};
use oulipoly_runtime::quota::{
    TOPOLOGY_PROBE_COOLDOWN_SECS, dynamic_ttl_secs, is_routing_stale, is_stale,
    is_topology_probe_due,
};
use oulipoly_state::{QuotaWindow, QuotaWindowInput, StateDb};
use std::path::{Path, PathBuf};

fn memory_state() -> StateDb {
    StateDb::open(Path::new(":memory:")).expect("state")
}

fn file_backed_state() -> (tempfile::TempDir, PathBuf, StateDb) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("state.db");
    let state = StateDb::open(&path).expect("state");
    (dir, path, state)
}

fn seed_quota_windows(state: &StateDb, provider: &str) {
    state
        .upsert_quota_refresh(
            provider,
            &[QuotaWindowInput {
                used_percent: 0.10,
                resets_at: Utc::now() + Duration::hours(24),
            }],
        )
        .expect("seed quota windows");
}

// These helpers intentionally patch nullable timestamp columns after normal
// StateDb seeding so the tests can characterize legacy rows without adding
// production-only hooks for malformed or partially migrated cache state.
fn set_refreshed_at_null(path: &Path, provider: &str) {
    rusqlite::Connection::open(path)
        .expect("open raw connection")
        .execute(
            "UPDATE provider_quotas SET refreshed_at = NULL WHERE provider_name = ?1",
            [provider],
        )
        .expect("set refreshed_at NULL");
}

fn set_last_topology_probe_at(path: &Path, provider: &str, timestamp: chrono::DateTime<Utc>) {
    rusqlite::Connection::open(path)
        .expect("open raw connection")
        .execute(
            "UPDATE provider_quotas
             SET last_topology_probe_at = ?2
             WHERE provider_name = ?1",
            rusqlite::params![provider, timestamp.to_rfc3339()],
        )
        .expect("set last_topology_probe_at");
}

#[test]
fn freshness_symbols_are_publicly_importable_from_quota_facade() {
    let _stale: fn(&StateDb, &str) -> bool = is_stale;
    let _routing_stale: fn(&StateDb, &str) -> bool = is_routing_stale;
    let _topology_due: fn(&StateDb, &str, usize, usize) -> bool = is_topology_probe_due;
    let _dynamic_ttl: fn(&[QuotaWindow]) -> i64 = dynamic_ttl_secs;
}

#[test]
fn stale_predicates_treat_missing_refreshed_at_as_stale_even_with_windows() {
    let (_dir, path, state) = file_backed_state();
    let provider = "legacy-null-refresh";
    seed_quota_windows(&state, provider);

    set_refreshed_at_null(&path, provider);

    let quota = state
        .get_quota(provider)
        .expect("quota query")
        .expect("quota row");
    assert!(
        quota.refreshed_at.is_none(),
        "fixture must model legacy quota rows with NULL refreshed_at"
    );
    assert!(
        !state
            .get_windows(provider)
            .expect("window query")
            .is_empty(),
        "fixture must keep non-empty windows so only refreshed_at drives staleness"
    );

    assert!(is_stale(&state, provider));
    assert!(is_routing_stale(&state, provider));
}

#[test]
fn topology_probe_due_for_incomplete_topology_when_quota_row_is_missing() {
    let state = memory_state();

    assert!(is_topology_probe_due(&state, "missing-quota-row", 1, 2));
}

#[test]
fn topology_probe_due_when_probe_timestamp_is_older_than_cooldown() {
    let (_dir, path, state) = file_backed_state();
    let provider = "old-topology-probe";
    seed_quota_windows(&state, provider);
    state
        .record_topology_probe(provider)
        .expect("record topology probe");
    let older_than_cooldown =
        Utc::now() - Duration::seconds(TOPOLOGY_PROBE_COOLDOWN_SECS as i64 + 5);
    set_last_topology_probe_at(&path, provider, older_than_cooldown);

    let quota = state
        .get_quota(provider)
        .expect("quota query")
        .expect("quota row");
    assert!(
        quota.last_topology_probe_at.is_some(),
        "fixture must include an existing probe timestamp"
    );

    assert!(is_topology_probe_due(&state, provider, 1, 2));
}

#[test]
fn dynamic_ttl_clamps_non_empty_far_future_window_to_max_ttl() {
    let far_future = QuotaWindow {
        provider_name: "far-future".to_string(),
        window_id: 0,
        used_percent: 0.0,
        resets_at: Utc::now() + Duration::days(60),
        last_delta_percent: None,
        last_delta_calls: None,
    };

    assert_eq!(dynamic_ttl_secs(&[far_future]), 24 * 3600);
}
