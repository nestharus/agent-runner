//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::common::*;
#[test]
fn upsert_quota_refresh_learns_sample_at_cap_boundary() {
    // A plausible-high rate just below the cap DOES get learned,
    // confirming the cap doesn't accidentally reject real workloads.
    // dp=0.90 over 25 turns → 0.036/turn. Below MAX_LEARNABLE_BURN_RATE
    // (0.1), above MIN_LEARN_SAMPLE_CALLS (20), below
    // NEAR_EXHAUSTED_USED_PERCENT (0.99). All three gates pass.
    let db = test_db();
    let provider = "p";
    db.upsert_quota_refresh(provider, &[quota_input(0.0, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 25, "boundary");
    db.upsert_quota_refresh(provider, &[quota_input(0.90, "2026-04-22T00:00:00Z")])
        .unwrap();

    let w = db.get_windows(provider).unwrap();
    assert!((w[0].last_delta_percent.unwrap() - 0.90).abs() < 1e-9);
    assert_eq!(w[0].last_delta_calls, Some(25));
}

#[test]
fn upsert_quota_refresh_rejects_learn_when_new_sample_near_rail() {
    // Regression: live observation 2026-04-21 had provider-b2's 7-day window
    // briefly read used_percent=1.0 from an upstream ChatGPT API spike,
    // paired with 34 turns since prior refresh. The learner computed
    // rate ≈ 0.029/turn on WEEKLY (real weekly rates are ~6e-5/turn;
    // the 100% sample was a cap-hit trajectory, not a natural fill),
    // which then projected every future invocation near the ceiling.
    // User framing: "turns barely budge weekly" —
    // so a weekly sample that moves 100 points in one interval is
    // distrusted. The marker we key on is "new used_percent at the
    // rail (>= 0.99)"; this test pins that gate.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior weekly rate: 0.02 over 300 turns → 6.7e-5/turn.
    db.upsert_quota_refresh(provider, &[quota_input(0.50, "2026-04-28T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 300, "prior-weekly");
    db.upsert_quota_refresh(provider, &[quota_input(0.52, "2026-04-28T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(300));

    // Upstream spike: new sample arrives at used_percent = 1.0 after
    // 34 turns. MIN_LEARN_SAMPLE_CALLS and MAX_LEARNABLE_BURN_RATE
    // alone would have let this through (34 > 20, 0.48/34 = 0.014/turn
    // < 0.1). The NEAR_EXHAUSTED_USED_PERCENT gate catches it.
    let t1 = ts("2026-04-21T12:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 34, "spike");
    db.upsert_quota_refresh(provider, &[quota_input(1.0, "2026-04-28T00:00:00Z")])
        .unwrap();

    let after = db.get_windows(provider).unwrap();
    assert!(
        (after[0].last_delta_percent.unwrap() - 0.02).abs() < 1e-9,
        "near-rail sample must not overwrite prior weekly learn"
    );
    assert_eq!(after[0].last_delta_calls, Some(300));
    // used_percent still reflects the spike — we only distrust the rate.
    assert!((after[0].used_percent - 1.0).abs() < 1e-9);
}

#[test]
fn upsert_quota_refresh_rejects_small_sample_delta_as_noise() {
    // Regression: live observation 2026-04-21 had provider-a2 with a learned
    // delta of 0.01/6 (rate 0.00167/turn). Paired with 193 turns since
    // refresh at scoring time, that projected 0.65 → 0.97, hard-blocking
    // the whole provider-a-opus pool. Sample-size floor of MIN_LEARN_SAMPLE_CALLS
    // rejects any delta learn below 20 turns and carries the prior
    // learn forward. At provider-a2 scale, this would have kept the pool
    // usable for the next invocation.
    let db = test_db();
    let provider = "p";

    // Seed a plausible prior learn (0.01 over 50 calls = 2e-4/turn).
    db.upsert_quota_refresh(provider, &[quota_input(0.10, "2026-04-22T00:00:00Z")])
        .unwrap();
    let t0 = ts("2026-04-21T00:00:00Z");
    db.set_refreshed_at_for_test(provider, &t0).unwrap();
    insert_assistant_turns_after(&db, provider, t0, 50, "prior-learn");
    db.upsert_quota_refresh(provider, &[quota_input(0.11, "2026-04-22T00:00:00Z")])
        .unwrap();

    let prior = db.get_windows(provider).unwrap();
    assert!((prior[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9);
    assert_eq!(prior[0].last_delta_calls, Some(50));

    // Now a small-sample observation: dp=0.01 over just 6 turns. Well
    // below the MAX_LEARNABLE_BURN_RATE cap (rate ≈ 0.00167), but
    // the sample size is too small to trust.
    let t1 = ts("2026-04-21T06:00:00Z");
    db.set_refreshed_at_for_test(provider, &t1).unwrap();
    insert_assistant_turns_after(&db, provider, t1, 6, "small-sample");
    db.upsert_quota_refresh(provider, &[quota_input(0.12, "2026-04-22T00:00:00Z")])
        .unwrap();

    let after = db.get_windows(provider).unwrap();
    // Small-sample rejected: prior 0.01/50 carried forward.
    assert!(
        (after[0].last_delta_percent.unwrap() - 0.01).abs() < 1e-9,
        "small-sample delta should not overwrite prior learn"
    );
    assert_eq!(after[0].last_delta_calls, Some(50));
}
