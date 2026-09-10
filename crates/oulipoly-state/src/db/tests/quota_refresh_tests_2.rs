//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/quota_refresh_tests_2.rs
//!     role: intrinsic-surface
//!     Domain: quota-refresh-tests-2-persistence
//!     Owns:
//!       - StateDb quota-refresh-tests-2 persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: calls_since_refresh, forward, insert_assistant_turns_after, last_empty_refresh_at, last_topology_probe_at_raw, quota_input, quota_window_detail_rows, quota_window_rows, test_db, ts
//! ```

use super::common::*;
use super::*;
#[test]
fn record_topology_probe_sets_timestamp_without_changing_windows() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.10, "2026-04-22T00:00:00Z"),
            quota_input(0.20, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();
    db.set_window_delta_for_test(provider, 0, 0.01, 40).unwrap();
    db.set_window_delta_for_test(provider, 1, 0.02, 40).unwrap();
    let before_windows = quota_window_detail_rows(&db, provider);
    let before = Utc::now();

    db.record_topology_probe(provider).unwrap();

    let after = Utc::now();
    let probe_at_raw =
        last_topology_probe_at_raw(&db, provider).expect("probe timestamp should be set");
    let probe_at = ts(&probe_at_raw);
    assert!(
        probe_at >= before - chrono::Duration::seconds(1)
            && probe_at <= after + chrono::Duration::seconds(1),
        "last_topology_probe_at {probe_at} should be near record_topology_probe call"
    );
    assert_eq!(
        quota_window_detail_rows(&db, provider),
        before_windows,
        "record_topology_probe must not mutate window rows or learning deltas"
    );
}

#[test]
fn upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();

    let replacement = [quota_input(0.30, "2026-04-23T12:00:00Z")];
    db.upsert_quota_refresh(provider, &replacement).unwrap();

    assert_eq!(
        quota_window_rows(&db, provider),
        vec![(0, 0.30, "2026-04-23T12:00:00+00:00".to_string())]
    );
}

#[test]
fn upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();

    let before = Utc::now();
    db.upsert_quota_refresh(provider, &[]).unwrap();
    let after = Utc::now();

    let last_empty = last_empty_refresh_at(&db, provider).unwrap();
    assert!(
        last_empty >= before - chrono::Duration::seconds(1)
            && last_empty <= after + chrono::Duration::seconds(1),
        "last_empty_refresh_at {last_empty} should be near empty refresh"
    );
}

#[test]
fn upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row() {
    let db = test_db();
    let provider = "p";

    db.upsert_quota_refresh(provider, &[]).unwrap();

    let quota = db.get_quota(provider).unwrap().unwrap();
    assert!(quota.refreshed_at.is_some());
    assert!(last_empty_refresh_at(&db, provider).is_some());
    assert!(db.get_windows(provider).unwrap().is_empty());
    assert!(quota.refreshed_at.is_some());
}

#[test]
fn upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist() {
    let db = test_db();
    let provider = "p";
    let windows = [
        quota_input(0.10, "2026-04-22T00:00:00Z"),
        quota_input(0.20, "2026-04-28T00:00:00Z"),
    ];
    db.upsert_quota_refresh(provider, &windows).unwrap();
    for _ in 0..5 {
        db.increment_calls_since_refresh(provider).unwrap();
    }
    assert_eq!(calls_since_refresh(&db, provider), 5);

    db.upsert_quota_refresh(provider, &[]).unwrap();

    assert_eq!(calls_since_refresh(&db, provider), 5);
}

#[test]
fn quota_refresh_preserves_prior_call_evidence_while_turn_ingest_is_untracked() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-22T00:00:00Z")])
        .unwrap();
    for _ in 0..7 {
        db.increment_calls_since_refresh(provider).unwrap();
    }

    let prior = db.get_quota(provider).unwrap().unwrap();

    assert_eq!(db.turns_between_quota_refreshes(provider, Some(&prior)), 7);
}

#[test]
fn upsert_quota_refresh_writes_per_window_delta_for_matching_window_id() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.20, "2026-04-22T00:00:00Z"),
            quota_input(0.30, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let refreshed_at = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, refreshed_at, 50, "delta-n1");

    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.38, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let windows = db.get_windows(provider).unwrap();
    assert_eq!(windows.len(), 2);
    assert!((windows[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(windows[0].last_delta_calls, Some(50));
    assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
    assert_eq!(windows[1].last_delta_calls, Some(50));
}

#[test]
fn upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change() {
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.20, "2026-04-22T00:00:00Z"),
            quota_input(0.30, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let first_refreshed_at = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &first_refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, first_refreshed_at, 50, "delta-n1");
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.38, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let second_refreshed_at = ts("2026-04-21T12:00:00Z");
    db.set_refreshed_at_for_test(provider, &second_refreshed_at)
        .unwrap();
    insert_assistant_turns_after(&db, provider, second_refreshed_at, 20, "delta-n2");
    db.upsert_quota_refresh(
        provider,
        &[
            quota_input(0.25, "2026-04-22T00:00:00Z"),
            quota_input(0.05, "2026-04-28T00:00:00Z"),
        ],
    )
    .unwrap();

    let windows = db.get_windows(provider).unwrap();
    assert_eq!(windows.len(), 2);
    assert!((windows[1].last_delta_percent.unwrap() - 0.08).abs() < 1e-9);
    assert_eq!(windows[1].last_delta_calls, Some(50));
}

#[test]
fn upsert_quota_refresh_rejects_pathological_burn_rate_sample() {
    // Regression: an upstream API spike (used_percent briefly reported as
    // 1.0) paired with a small turn count would previously learn a
    // pathological per-turn rate (~0.05/turn), carry it forward across
    // every subsequent no-change refresh, and permanently project every
    // provider near the ceiling. The sanity cap at
    // MAX_LEARNABLE_BURN_RATE = 0.1/turn rejects this sample and carries
    // the prior learn forward instead, so the pool stays usable.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior learn (0.05 / 100 calls = 5e-4 per turn).
    db.upsert_quota_refresh(provider, &[quota_input(0.20, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 100, "prior-learn");
    db.upsert_quota_refresh(provider, &[quota_input(0.25, "2026-04-22T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(100));

    // Now feed a pathological sample: used_percent jumps from 0.25 to
    // 0.95 over just 5 turns. dp = 0.70, dc = 5, so new_rate = 0.14/turn,
    // which exceeds MAX_LEARNABLE_BURN_RATE (0.1/turn).
    let t1 = ts("2026-04-21T06:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 5, "spike");
    db.upsert_quota_refresh(provider, &[quota_input(0.95, "2026-04-22T00:00:00Z")])
        .unwrap();

    let after_spike = db.get_windows(provider).unwrap();
    // Pathological sample rejected: delta is still the prior 0.05/100.
    assert!(
        (after_spike[0].last_delta_percent.unwrap() - 0.05).abs() < 1e-9,
        "spike sample should not overwrite prior learn; got {:?}",
        after_spike[0].last_delta_percent
    );
    assert_eq!(after_spike[0].last_delta_calls, Some(100));
    // used_percent still reflects the incoming sample — we only reject
    // the delta learn, not the quota observation itself.
    assert!((after_spike[0].used_percent - 0.95).abs() < 1e-9);
}
