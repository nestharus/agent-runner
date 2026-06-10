//! ## Declared roles
//!
//! `accessor`, `mapper`, `validator`.

use super::super::*;

pub(in crate::balancer::tests) fn selected_provider_index(
    model: &ModelConfig,
    db: &StateDb,
) -> usize {
    select_provider(model, db, None).unwrap()
}

pub(in crate::balancer::tests) fn single_provider_model() -> ModelConfig {
    ModelConfig {
        name: "single".to_string(),
        prompt_mode: PromptMode::Arg,
        providers: vec![ProviderConfig::new("a", vec![])],
        inputs: vec![],
        provider: None,
    }
}

#[derive(Clone, Copy)]
pub(in crate::balancer::tests) enum TestWindow {
    SevenDay,
    FiveHour,
}

pub(in crate::balancer::tests) fn seed_two_window_used(
    db: &StateDb,
    provider_name: &str,
    seven_day_used: f64,
    five_hour_used: f64,
) {
    seed_windows_with_deltas(
        db,
        provider_name,
        &[
            (seven_day_used, 24 * 7, 0.01, 22),
            (five_hour_used, 5, 0.30, 22),
        ],
    );
}

pub(in crate::balancer::tests) fn assert_approx(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "expected {actual} to be within {tolerance} of {expected}"
    );
}
