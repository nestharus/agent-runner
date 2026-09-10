//! ## Declared roles
//!
//! `orchestration`.

use super::*;

#[test]
fn approx_eq_usage_uses_near_epsilon_relative_threshold() {
    assert!(approx_eq_usage(1.0, 1.0 + f64::EPSILON));
    assert!(!approx_eq_usage(1.0, 1.0 + f64::EPSILON * 4.0));
}

#[test]
fn fanout_usage_key_derives_reset_when_projected_usage_is_nonfinite() {
    let projection = ProviderProjection {
        provider_index: 0,
        projections_per_window: vec![
            WindowProjection {
                window_id: 0,
                projected_used: f64::NAN,
                hours_until_reset: 9.0,
                remaining_headroom: 0.0,
            },
            WindowProjection {
                window_id: 1,
                projected_used: f64::INFINITY,
                hours_until_reset: 3.0,
                remaining_headroom: 0.0,
            },
        ],
        binding_score: Some(1.0),
        turn_count_fresh: true,
        recent_error_count: 0,
    };

    let key = fanout_usage_key(&projection);

    assert_eq!(key.worst_projected_used, None);
    assert_eq!(key.soonest_reset_hours, Some(3.0));
}

#[test]
fn finite_fanout_fields_filter_nonfinite_values() {
    let eval = ProviderEval {
        index: 0,
        binding_score: Some(1.0),
        unlearned: false,
        fanout_usage: Some(FanoutUsageKey {
            worst_projected_used: Some(f64::NAN),
            soonest_reset_hours: Some(f64::INFINITY),
        }),
    };

    assert_eq!(finite_fanout_usage(&eval), None);
    assert_eq!(finite_fanout_reset(&eval), None);
}

#[test]
fn select_binding_score_with_fanout_uses_argmax_escape_branches() {
    let model = two_provider_model();
    let single = vec![provider_eval_with_fanout_usage(
        0,
        4.0,
        Some(0.90),
        Some(1.0),
    )];
    assert_eq!(select_binding_score_with_fanout(&model, &single), 0);

    let nonfinite_score = vec![
        ProviderEval {
            index: 0,
            binding_score: Some(f64::INFINITY),
            unlearned: false,
            fanout_usage: Some(FanoutUsageKey {
                worst_projected_used: Some(0.90),
                soonest_reset_hours: Some(12.0),
            }),
        },
        provider_eval_with_fanout_usage(1, 2.0, Some(0.10), Some(1.0)),
    ];
    assert_eq!(
        select_binding_score_with_fanout(&model, &nonfinite_score),
        0
    );

    let nonpositive_best = vec![
        provider_eval_with_fanout_usage(0, 0.0, Some(0.90), Some(12.0)),
        provider_eval_with_fanout_usage(1, -1.0, Some(0.10), Some(1.0)),
    ];
    assert_eq!(
        select_binding_score_with_fanout(&model, &nonpositive_best),
        0
    );
}

#[test]
fn project_used_percent_clamps_negative_projection_at_zero() {
    assert_eq!(project_used_percent_for_test(0.05, 10, -0.02), 0.0);
}

#[test]
fn learned_rate_rejects_nonpositive_delta_percent_and_zero_calls() {
    let resets_at = Utc::now() + Duration::hours(1);
    let zero_percent = quota_window_record("a", 0, 0.10, resets_at, Some(0.0), Some(10));
    let negative_percent = quota_window_record("a", 0, 0.10, resets_at, Some(-0.01), Some(10));
    let zero_calls = quota_window_record("a", 0, 0.10, resets_at, Some(0.01), Some(0));
    let valid = quota_window_record("a", 0, 0.10, resets_at, Some(0.02), Some(10));

    assert_eq!(learned_rate(&zero_percent), None);
    assert_eq!(learned_rate(&negative_percent), None);
    assert_eq!(learned_rate(&zero_calls), None);
    assert_eq!(learned_rate(&valid), Some(0.002));
}

#[test]
fn pool_window_avg_averages_matching_siblings_and_skips_invalid_deltas() {
    let resets_at = Utc::now() + Duration::hours(1);
    let windows = vec![
        vec![quota_window_record(
            "a",
            0,
            0.10,
            resets_at,
            Some(0.20),
            Some(10),
        )],
        vec![
            quota_window_record("b", 0, 0.10, resets_at, Some(-0.20), Some(10)),
            quota_window_record("b", 0, 0.10, resets_at, Some(0.50), Some(0)),
        ],
        vec![quota_window_record(
            "c",
            0,
            0.10,
            resets_at,
            Some(0.10),
            Some(10),
        )],
    ];

    assert_approx(
        pool_window_avg_percent_per_call(0, &windows).unwrap(),
        0.015,
        1e-12,
    );
}

#[test]
fn duration_ratio_fallback_requires_target_refresh_and_chooses_longest_learned_sibling() {
    let target_refreshed_at = Utc::now();
    let target = quota_window_record(
        "a",
        1,
        0.10,
        target_refreshed_at + Duration::hours(5),
        None,
        None,
    );
    let missing_target_quota = vec![None];
    let target_windows = vec![vec![target.clone()]];
    assert_eq!(
        duration_ratio_fallback_percent_per_call(
            0,
            &target,
            &missing_target_quota,
            &target_windows,
        ),
        None
    );

    let quotas = vec![
        Some(quota_record("a", Some(target_refreshed_at))),
        None,
        Some(quota_record("c", Some(target_refreshed_at))),
        Some(quota_record("d", Some(target_refreshed_at))),
        Some(quota_record("e", Some(target_refreshed_at))),
    ];
    let windows = vec![
        vec![target.clone()],
        vec![quota_window_record(
            "b",
            0,
            0.10,
            target_refreshed_at + Duration::hours(100),
            Some(0.90),
            Some(10),
        )],
        vec![quota_window_record(
            "c",
            0,
            0.10,
            target_refreshed_at + Duration::hours(5),
            Some(0.80),
            Some(10),
        )],
        vec![quota_window_record(
            "d",
            0,
            0.10,
            target_refreshed_at + Duration::hours(10),
            Some(0.20),
            Some(10),
        )],
        vec![quota_window_record(
            "e",
            0,
            0.10,
            target_refreshed_at + Duration::hours(20),
            Some(0.20),
            Some(20),
        )],
    ];

    let rate = duration_ratio_fallback_percent_per_call(0, &target, &quotas, &windows).unwrap();

    assert_approx(rate, 0.04, 1e-12);
}
