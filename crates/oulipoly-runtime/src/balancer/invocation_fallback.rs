//! ## Declared roles
//!
//! `accessor`, `mapper`, `filter`, `predicate`, `orchestration`.

use super::{ERROR_THRESHOLD, ERROR_WINDOW_MINUTES};
use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

pub(super) fn score_by_invocation_count(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> usize {
    let mut scores = invocation_count_scores(model, state, candidates);
    sort_invocation_count_scores(scores.as_mut_slice());

    if all_invocation_candidates_suppressed(scores.as_slice()) {
        return round_robin_fallback(model, state, candidates);
    }
    selected_invocation_count_score(scores.as_slice())
}

fn invocation_count_scores(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> Vec<(usize, f64)> {
    candidates
        .iter()
        .map(|&provider_index| invocation_count_score(model, state, provider_index))
        .collect()
}

fn invocation_count_score(
    model: &ModelConfig,
    state: &StateDb,
    provider_index: usize,
) -> (usize, f64) {
    let recent_errors = fallback_recent_error_count(model, state, provider_index);
    if fallback_provider_is_suppressed(recent_errors) {
        return (provider_index, f64::MAX);
    }

    (
        provider_index,
        fallback_invocation_count(model, state, provider_index) as f64
            + fallback_error_penalty(recent_errors),
    )
}

fn fallback_recent_error_count(model: &ModelConfig, state: &StateDb, provider_index: usize) -> i64 {
    state
        .recent_error_count(
            &model.name,
            &model.providers[provider_index].name,
            ERROR_WINDOW_MINUTES,
        )
        .unwrap_or(0)
}

fn fallback_provider_is_suppressed(recent_errors: i64) -> bool {
    recent_errors >= ERROR_THRESHOLD as i64
}

fn fallback_invocation_count(model: &ModelConfig, state: &StateDb, provider_index: usize) -> i64 {
    state
        .get_provider(&model.name, &model.providers[provider_index].name)
        .ok()
        .flatten()
        .map(|provider| provider.invocation_count)
        .unwrap_or(0)
}

fn fallback_error_penalty(recent_errors: i64) -> f64 {
    recent_errors as f64 * 10.0
}

fn sort_invocation_count_scores(scores: &mut [(usize, f64)]) {
    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
}

fn all_invocation_candidates_suppressed(scores: &[(usize, f64)]) -> bool {
    scores.iter().all(|(_, score)| *score == f64::MAX)
}

fn selected_invocation_count_score(scores: &[(usize, f64)]) -> usize {
    scores[0].0
}

pub(super) fn round_robin_fallback(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> usize {
    assert_round_robin_candidates(candidates);
    select_lowest_invocation_count(
        round_robin_invocation_counts(model, state, candidates).as_slice(),
    )
}

fn assert_round_robin_candidates(candidates: &[usize]) {
    debug_assert!(
        !candidates.is_empty(),
        "round_robin_fallback: caller must pass a non-empty candidates slice"
    );
}

fn round_robin_invocation_counts(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
) -> Vec<(usize, i64)> {
    candidates
        .iter()
        .map(|&provider_index| {
            (
                provider_index,
                fallback_invocation_count(model, state, provider_index),
            )
        })
        .collect()
}

fn select_lowest_invocation_count(counts: &[(usize, i64)]) -> usize {
    let mut min_count = i64::MAX;
    let mut best = counts
        .first()
        .map(|(provider_index, _)| *provider_index)
        .unwrap_or(0);

    for &(provider_index, count) in counts {
        if count < min_count {
            min_count = count;
            best = provider_index;
        }
    }

    best
}
