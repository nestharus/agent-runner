//! ## Declared roles
//!
//! `orchestration`.

use super::*;

/// Risk: Fanout selector might use local invocation count instead of projected upstream usage.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 1; Assumptions A2, A4, A5.
#[test]
fn density_fanout_uses_invocation_counts_within_score_band() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 10.0, Some(0.40), Some(48.0)),
        provider_eval_with_fanout_usage(1, 7.0, Some(0.70), Some(6.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 0,
        "in-band fanout must pick the lower projected-usage provider, not the lower local invocation count"
    );
}

/// Risk: Public density selection could still let local invocation counts override projected usage.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 2; Assumptions A2, A4, A5.
#[test]
fn density_fanout_prefers_lower_projected_usage_over_local_invocation_count() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = two_provider_model();

    seed_windows_with_deltas(&db, "a", &[(0.40, 24, 0.01, 22)]);
    seed_windows_with_deltas(&db, "b", &[(0.55, 24, 0.01, 22)]);
    for _ in 0..4 {
        record_invocation_for_test(&db, &model.name, "a", 0, true);
    }

    assert_eq!(
        selected_provider_index(&model, &db),
        0,
        "provider a has higher score and lower projected usage; provider b's lower local count must not win"
    );
}

/// Risk: Tied usage and tied score might skip the soonest-reset layer.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 3; Assumptions A2, A4, A9.
#[test]
fn density_fanout_ties_score_and_usage_falls_to_soonest_reset() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 8.0, Some(0.50), Some(12.0)),
        provider_eval_with_fanout_usage(1, 8.0, Some(0.50), Some(6.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 1,
        "equal usage and score should fall to sooner reset"
    );
}

/// Risk: Unknown projected usage could make reset unavailable as the AC2 fallback.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 4; Assumptions A3, A4.
#[test]
fn density_fanout_falls_to_soonest_reset_when_usage_unknown_and_scores_tied() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 8.0, None, Some(12.0)),
        provider_eval_with_fanout_usage(1, 8.0, None, Some(6.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 1,
        "when usage and score cannot distinguish candidates, the sooner reset should win"
    );
}

/// Risk: One-sided unknown usage could incorrectly lose to known usage or reset timing.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 5; Assumption A3.
#[test]
fn density_fanout_higher_score_wins_when_one_usage_unknown() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 10.0, None, Some(12.0)),
        provider_eval_with_fanout_usage(1, 7.0, Some(0.50), Some(6.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 0,
        "one-sided unknown usage should fall through to score before reset"
    );
}

/// Risk: Equal projected usage could regress the omega invariant by letting reset beat score.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 6; Assumptions A3, A4.
#[test]
fn density_fanout_higher_score_wins_when_lower_score_has_equal_usage() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 10.0, Some(0.50), Some(12.0)),
        provider_eval_with_fanout_usage(1, 7.0, Some(0.50), Some(6.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 0,
        "equal projected usage must fall through to higher score before sooner reset"
    );
}

/// Risk: Deterministic fanout might become order-unstable or random.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 7; Assumptions A3, A4, A9.
#[test]
fn density_fanout_tiebreaks_by_score_then_index() {
    let model = two_provider_model();

    let score_tie_break = vec![
        provider_eval_with_fanout_usage(0, 9.0, Some(0.50), Some(1.0)),
        provider_eval_with_fanout_usage(1, 10.0, Some(0.50), Some(24.0)),
    ];
    assert_eq!(
        select_binding_score_with_fanout(&model, &score_tie_break),
        1,
        "tied projected usage should choose the higher binding score before reset"
    );

    let reset_tie_break = vec![
        provider_eval_with_fanout_usage(0, 10.0, Some(0.50), Some(12.0)),
        provider_eval_with_fanout_usage(1, 10.0, Some(0.50), Some(6.0)),
    ];
    assert_eq!(
        select_binding_score_with_fanout(&model, &reset_tie_break),
        1,
        "tied projected usage and score should choose the sooner reset"
    );

    let index_tie_break = vec![
        provider_eval_with_fanout_usage(0, 10.0, Some(0.50), Some(6.0)),
        provider_eval_with_fanout_usage(1, 10.0, Some(0.50), Some(6.0)),
    ];
    assert_eq!(
        select_binding_score_with_fanout(&model, &index_tie_break),
        0,
        "tied projected usage, score, and reset should choose the lower provider index"
    );
}

/// Risk: Fanout might send traffic to much lower-capacity providers.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 8; Assumption A8.
#[test]
fn density_hard_pins_when_score_gap_exceeds_band() {
    let model = two_provider_model();
    let eligible = vec![
        provider_eval_with_fanout_usage(0, 10.0, Some(0.80), Some(96.0)),
        provider_eval_with_fanout_usage(1, 4.99, Some(0.10), Some(1.0)),
    ];

    let selected = select_binding_score_with_fanout(&model, &eligible);

    assert_eq!(
        selected, 0,
        "providers outside the 2x score band cannot win through lower usage or sooner reset"
    );
}

/// Risk: The user-visible alpha/delta reporter case could still pick the higher-usage account.
/// Level: unit.
/// Source: AGE-25 proposal §7 item 11 / contract item 10; Assumptions A2, A4, A5.
#[test]
fn density_fanout_smoke_selects_alpha_51_over_delta_82() {
    let db = StateDb::open(Path::new(":memory:")).unwrap();
    let model = ModelConfig {
        name: "alpha-opus".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![
            ProviderConfig::new("alpha", vec![]),
            ProviderConfig::new("delta", vec![]),
        ],
        inputs: vec![],
        provider: None,
    };

    seed_windows_with_deltas(&db, "alpha", &[(0.51, 48, 0.01, 22)]);
    seed_windows_with_deltas(&db, "delta", &[(0.82, 96, 0.01, 22)]);
    for _ in 0..5 {
        record_invocation_for_test(&db, &model.name, "alpha", 0, true);
    }

    assert_eq!(
        selected_provider_index(&model, &db),
        0,
        "alpha has lower projected usage than delta and must remain selected despite higher local count"
    );
}
