//! ## Declared roles
//!
//! `accessor`, `mapper`, `filter`, `predicate`, `orchestration`.

use super::{ERROR_THRESHOLD, ERROR_WINDOW_MINUTES, live_load::live_load_then_index_order};
use oulipoly_config::ModelConfig;
use oulipoly_state::StateDb;

pub(super) fn score_by_invocation_count(
    model: &ModelConfig,
    state: &StateDb,
    candidates: &[usize],
    live_loads: &[u64],
) -> usize {
    let mut scores = invocation_count_scores(model, state, candidates);
    sort_invocation_count_scores(scores.as_mut_slice(), live_loads);

    if all_invocation_candidates_suppressed(scores.as_slice()) {
        return round_robin_fallback(model, state, candidates, live_loads);
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
    let signals = fallback_provider_signals(model, state, provider_index);
    if fallback_provider_is_suppressed(signals.recent_errors) {
        return (provider_index, f64::MAX);
    }

    fallback_score_tuple(provider_index, signals)
}

#[derive(Copy, Clone)]
struct FallbackProviderSignals {
    recent_errors: i64,
    invocation_count: i64,
}

fn fallback_provider_signals(
    model: &ModelConfig,
    state: &StateDb,
    provider_index: usize,
) -> FallbackProviderSignals {
    FallbackProviderSignals {
        recent_errors: fallback_recent_error_count(model, state, provider_index),
        invocation_count: fallback_invocation_count(model, state, provider_index),
    }
}

fn fallback_score_tuple(provider_index: usize, signals: FallbackProviderSignals) -> (usize, f64) {
    (
        provider_index,
        signals.invocation_count as f64 + fallback_error_penalty(signals.recent_errors),
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

fn sort_invocation_count_scores(scores: &mut [(usize, f64)], live_loads: &[u64]) {
    scores.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap()
            .then_with(|| live_load_then_index_order(a.0, b.0, live_loads))
    });
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
    live_loads: &[u64],
) -> usize {
    assert_round_robin_candidates(candidates);
    select_lowest_invocation_count(
        round_robin_invocation_counts(model, state, candidates).as_slice(),
        live_loads,
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

fn select_lowest_invocation_count(counts: &[(usize, i64)], live_loads: &[u64]) -> usize {
    let mut best = initial_invocation_candidate(counts);
    let mut min_count = initial_invocation_count();

    for &(provider_index, count) in counts {
        if invocation_candidate_is_better(provider_index, count, best, min_count, live_loads) {
            min_count = count;
            best = provider_index;
        }
    }

    best
}

fn initial_invocation_candidate(counts: &[(usize, i64)]) -> usize {
    counts
        .first()
        .map(|(provider_index, _)| *provider_index)
        .unwrap_or(0)
}

fn initial_invocation_count() -> i64 {
    i64::MAX
}

fn invocation_candidate_is_better(
    provider_index: usize,
    count: i64,
    best: usize,
    min_count: i64,
    live_loads: &[u64],
) -> bool {
    count < min_count
        || invocation_candidate_wins_tie(provider_index, count, best, min_count, live_loads)
}

fn invocation_candidate_wins_tie(
    provider_index: usize,
    count: i64,
    best: usize,
    min_count: i64,
    live_loads: &[u64],
) -> bool {
    count == min_count && live_load_then_index_order(provider_index, best, live_loads).is_lt()
}
