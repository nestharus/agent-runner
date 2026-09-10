//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn per_window_burn_rate_projects_short_window_faster_than_long() {
    let long_rate = 0.01 / 22.0;
    let short_rate = long_rate * 30.0;

    let long_projected = project_used_percent_for_test(0.10, 100, long_rate);
    let short_projected = project_used_percent_for_test(0.10, 100, short_rate);

    assert_approx(short_projected - 0.10, (long_projected - 0.10) * 30.0, 1e-9);
    assert!(short_projected >= 0.95);
    assert!(long_projected < 0.95);
}

#[test]
fn bootstrap_uses_sibling_pool_when_own_delta_absent() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let sibling_rate = 0.012 / 24.0;

    db.upsert_quota_refresh("a", &one_window(0.20, 24 * 7))
        .unwrap();
    mark_provider_turn_count_caught_up(&db, "a");
    seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.012, 24)]);

    let bootstrapped = bootstrap_burn_rate_for_test(&model, &db, 0, 0).unwrap();
    assert_approx(bootstrapped, sibling_rate, 1e-12);
    assert_eq!(selected_provider_index(&model, &db), 0);
}

#[test]
fn bootstrap_uses_duration_ratio_when_pool_has_only_long_delta() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();
    let long_rate = 0.01 / 22.0;
    let expected_short_rate = long_rate * (168.0 / 5.0);

    db.upsert_quota_refresh("a", &[quota_window(0.20, 24 * 7), quota_window(0.20, 5)])
        .unwrap();
    mark_provider_turn_count_caught_up(&db, "a");
    seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.01, 22)]);

    let bootstrapped = bootstrap_burn_rate_for_test(&model, &db, 0, 1).unwrap();
    assert_approx(
        bootstrapped,
        expected_short_rate,
        expected_short_rate * 0.10,
    );
}

#[test]
fn bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio() {
    let long_rate = 0.01 / 22.0;
    let derived = bootstrap_duration_ratio_for_test(long_rate, 168.0, 5.0);

    assert_approx(derived, long_rate * (168.0 / 5.0), long_rate * 0.05);
    assert!(derived > long_rate);
}

#[test]
fn duration_ratio_rate_uses_eps_guard_for_zero_or_negative_target_hours() {
    let long_rate = 0.01;
    let expected = long_rate * (2.0 / EPS_HOURS);

    assert_approx(duration_ratio_rate(long_rate, 2.0, 0.0), expected, 1e-12);
    assert_approx(duration_ratio_rate(long_rate, 2.0, -4.0), expected, 1e-12);
}

#[test]
fn bootstrap_returns_none_when_no_sibling_has_learned_rate() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    db.upsert_quota_refresh("a", &one_window(0.20, 24 * 7))
        .unwrap();
    db.upsert_quota_refresh("b", &one_window(0.30, 24 * 7))
        .unwrap();

    assert!(bootstrap_burn_rate_for_test(&model, &db, 0, 0).is_none());
}

// Intentionally no test for the "A unlearned while B learned" case.
//
// The §Q3 bootstrap cascade makes that state unreachable when
// siblings share a quota_script (the normal pool configuration):
// step 2 matches by window_id and rescues A from any same-slot
// sibling delta, and step 3 rescues short-window gaps from any
// longer-duration sibling rate. The only state where A is unlearned
// but some sibling is learned requires providers to emit mismatched
// window_id layouts, which is off-pattern and already covered by
// other tests (#11 sibling rescue, #12 duration-ratio rescue, #14
// no learning anywhere, #16 fresh pool round-robin). Do not
// resurrect this slot without first amending the cascade design.
