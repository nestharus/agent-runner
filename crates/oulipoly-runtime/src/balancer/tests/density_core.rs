//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn score_by_density_skips_past_reset_windows() {
    // Live-caught 2026-04-22: gamma had a 5h window whose resets_at
    // was hours in the past (anthropic-usage returning empty kept the
    // stale row alive via PR #6's preserve-on-empty path). The stored
    // used_percent is from the previous window instance, so it has no
    // bearing on current headroom. Previously the code clamped
    // hours_until_reset to EPS_HOURS = 1/60h, which torpedoed the
    // provider's binding score to near-zero and made a low-usage
    // account (64% weekly) lose to a heavily-used one (91% weekly).
    // Now past-reset windows are skipped during binding computation.
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Provider `a`: healthy 7d window (low usage) + stale past-reset
    // 5h window.
    use chrono::Duration;
    let a_windows = vec![
        oulipoly_state::QuotaWindowInput {
            used_percent: 0.10,
            resets_at: Utc::now() + Duration::hours(24 * 7),
        },
        oulipoly_state::QuotaWindowInput {
            used_percent: 0.90,
            resets_at: Utc::now() - Duration::hours(1), // RESET PASSED
        },
    ];
    db.upsert_quota_refresh("a", &a_windows).unwrap();
    db.set_window_delta_for_test("a", 0, 0.01, 22).unwrap();
    mark_provider_turn_count_caught_up(&db, "a");

    // Provider `b`: heavily-used 7d window, nothing past-reset.
    seed_windows_with_deltas(&db, "b", &[(0.85, 24 * 7, 0.01, 22)]);

    // With past-reset skipping, `a` is ranked only on its 7d window
    // (much more headroom than b's 7d). Without the skip, a's
    // near-zero 5h binding would lose to b.
    assert_eq!(selected_provider_index(&model, &db), 0);
}

#[test]
fn score_by_density_penalizes_provider_missing_window_siblings_have() {
    // Live-caught 2026-04-22: alpha in the alpha-opus pool had
    // only a 7d window reported (anthropic-usage returned 1 window
    // because Anthropic's API hides the 5h timer when the account
    // is near weekly cap), while beta and gamma both had 2
    // windows. Claude's 7d was at 91% used — the MOST pressed
    // account in the pool. But with only one window to min over
    // vs siblings' two, alpha's binding ((1-0.91)*41h ≈ 3.65)
    // beat gamma's min((1-0.64)*41h, (1-0.04)*3.6h) ≈ 3.46
    // simply because gamma's 5h tier pulled its binding down.
    // 10/10 live invocations routed to the near-exhausted account.
    //
    // Defensive pessimism: when a sibling reports more live windows
    // than this provider AND the provider's visible window is
    // itself near cap, assume the missing slots are fully consumed
    // (0 remaining headroom) and pull the provider's binding to
    // zero. The "hidden 5h window" + "visible 7d near cap"
    // combination is the Anthropic "near weekly cap" signal.
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Provider `a`: ONE 7d window at 91% used (mimics alpha's
    // "hidden 5h while near weekly cap" state).
    seed_windows_with_deltas(&db, "a", &[(0.91, 24 * 7, 0.01, 22)]);

    // Provider `b`: TWO windows (7d + 5h), less used than `a`.
    seed_windows_with_deltas(&db, "b", &[(0.64, 24 * 7, 0.01, 22), (0.04, 5, 0.01, 22)]);

    // Without the penalty, `a` would win on its single 7d binding
    // ((1-0.91)*168h ≈ 15.1) over `b`'s short-window-constrained
    // min((1-0.64)*168h, (1-0.04)*5h) ≈ 4.8. With the penalty,
    // `a`'s binding is forced to 0 because (i) it has fewer live
    // windows than `b` and (ii) its visible 7d is near cap, so
    // `b` wins.
    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn score_by_density_does_not_penalize_idle_provider_missing_short_window() {
    // Live-caught 2026-04-26: omega (98% remaining, near-zero
    // recent usage) had only a 7d window reported by chatgpt-usage
    // because ChatGPT's API only emits `primary_window` when an
    // account has an active 5h timer (i.e. recent activity). Codex2
    // (64% remaining, actively in use) had both windows. Under the
    // unconditional missing-window penalty, omega's binding was
    // forced to 0 and every invocation routed to the more-pressed
    // sigma — omega stayed idle, which kept it 5h-windowless,
    // which kept it penalized. Vicious cycle.
    //
    // ChatGPT's "hide 5h when idle" is the OPPOSITE signal from
    // Anthropic's "hide 5h when near cap". The visible-usage gate
    // distinguishes them: only penalize when a visible window is
    // itself near cap. An idle account's visible 7d is far from
    // cap, so no penalty applies, and the lower-usage provider
    // wins on its actual headroom.
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Provider `a`: ONE 7d window at 2% used (mimics omega's idle
    // "no primary_window emitted" state).
    seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 22)]);

    // Provider `b`: TWO windows (7d + 5h), actively in use.
    seed_windows_with_deltas(&db, "b", &[(0.36, 24 * 7, 0.01, 22), (0.20, 5, 0.01, 22)]);

    // No penalty for `a` (its visible 7d is nowhere near cap), so
    // `a` wins on raw headroom: (1-0.02)*168h ≈ 164.6 beats
    // min((1-0.36)*168h, (1-0.20)*5h) ≈ 4.0.
    assert_eq!(selected_provider_index(&model, &db), 0);
}

#[test]
fn exhausted_filter_does_not_prevent_refresh_loop_from_clearing() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22)]);
    db.mark_exhausted("a").unwrap();
    db.mark_exhausted("b").unwrap();

    // Simulate a successful non-empty refresh for b. The production
    // refresh loop must make this same state transition before filtering.
    db.upsert_quota_refresh("b", &[quota_window(0.60, 24 * 7)])
        .unwrap();

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn density_scoring_picks_lowest_used_when_windows_match() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = three_provider_model();

    // All three providers reset in the same window length (7d) so density
    // collapses to remaining-headroom comparison: a=0.50, b=0.10, c=0.30.
    // Highest remaining = b (0.90) → pick b.
    seed_windows_with_deltas(&db, "a", &[(0.50, 24 * 7, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.10, 24 * 7, 0.01, 22)]);
    seed_windows_with_deltas(&db, "c", &[(0.30, 24 * 7, 0.01, 22)]);

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn density_picks_account_with_more_time_when_used_equal() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    // Both providers have learned equivalent burn rates and equal usage.
    // The account with more time to reset has more projected turns left.
    seed_windows_with_deltas(&db, "a", &[(0.50, 1, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.50, 24 * 7, 0.01, 22)]);

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn binding_constraint_avoids_account_with_pressed_short_window() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22), (0.95, 5, 0.30, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.30, 24 * 7, 0.01, 22), (0.20, 5, 0.30, 22)]);

    assert_eq!(selected_provider_index(&model, &db), 1);
}

#[test]
fn falls_back_to_invocation_count_when_windows_missing() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.90, 24 * 7, 0.01, 22)]);
    record_invocation_for_test(&db, "test", "a", 0, true);

    assert_eq!(selected_provider_index(&model, &db), 1);
}
