//! ## Declared roles
//!
//! `mapper`, `filter`, `predicate`, `formatter`, `orchestration`, `validator`.

mod trace;

use super::{
    EPS_HOURS, ERROR_THRESHOLD,
    invocation_fallback::round_robin_fallback,
    projection::{ProviderProjection, WindowProjection, compute_projections_from_records},
};
use chrono::Utc;
use oulipoly_config::ModelConfig;
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};
use trace::trace_fanout_selection;

pub const FANOUT_SCORE_BAND_RATIO: f64 = 2.0;

#[derive(Debug, Clone)]
pub(super) struct ProviderEval {
    pub(super) index: usize,
    pub(super) binding_score: Option<f64>,
    pub(super) unlearned: bool,
    pub(super) fanout_usage: Option<FanoutUsageKey>,
    pub(super) live_load: u64,
}

#[derive(Copy, Clone, Debug)]
pub(super) struct FanoutUsageKey {
    pub(super) worst_projected_used: Option<f64>,
    pub(super) soonest_reset_hours: Option<f64>,
}

pub(super) fn score_by_density(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
    live_loads: &[u64],
) -> usize {
    let projections =
        compute_projections_from_records(model, state, quotas, windows, candidates, Utc::now());
    let evals = provider_evals_from_projections(projections.as_slice(), live_loads);
    let eligible = density_eligible_evals(evals.as_slice());

    if eligible.is_empty() {
        return round_robin_fallback(model, state, candidates, live_loads);
    }

    select_binding_score_with_fanout(model, &eligible)
}

fn provider_evals_from_projections(
    projections: &[ProviderProjection],
    live_loads: &[u64],
) -> Vec<ProviderEval> {
    projections
        .iter()
        .map(|projection| provider_eval_from_projection(projection, live_loads))
        .collect()
}

fn provider_eval_from_projection(
    projection: &ProviderProjection,
    live_loads: &[u64],
) -> ProviderEval {
    ProviderEval {
        index: projection.provider_index,
        binding_score: projection_binding_score(projection),
        unlearned: projection_is_unlearned(projection),
        fanout_usage: Some(projection_fanout_usage(projection)),
        live_load: projection_live_load(projection, live_loads),
    }
}

fn projection_binding_score(projection: &ProviderProjection) -> Option<f64> {
    projection.binding_score
}

fn projection_is_unlearned(projection: &ProviderProjection) -> bool {
    projection.binding_score.is_none() && projection.recent_error_count < ERROR_THRESHOLD as u32
}

fn projection_fanout_usage(projection: &ProviderProjection) -> FanoutUsageKey {
    fanout_usage_key(projection)
}

fn projection_live_load(projection: &ProviderProjection, live_loads: &[u64]) -> u64 {
    super::live_load::live_load_at(live_loads, projection.provider_index)
}

fn density_eligible_evals(evals: &[ProviderEval]) -> Vec<ProviderEval> {
    evals
        .iter()
        .filter(|eval| !eval.unlearned && eval.binding_score.is_some())
        .cloned()
        .collect()
}

fn best_binding_score<'a>(evals: &[&'a ProviderEval]) -> &'a ProviderEval {
    assert_binding_score_candidates(evals);
    max_binding_score_eval(evals)
}

fn assert_binding_score_candidates(evals: &[&ProviderEval]) {
    debug_assert!(!evals.is_empty(), "best_binding_score: empty slice");
    debug_assert!(
        evals.iter().all(|e| e.binding_score.is_some()),
        "best_binding_score: caller must filter to providers with a learned binding_score"
    );
}

fn max_binding_score_eval<'a>(evals: &[&'a ProviderEval]) -> &'a ProviderEval {
    evals
        .iter()
        .copied()
        .max_by(|a, b| {
            a.binding_score
                .unwrap()
                .partial_cmp(&b.binding_score.unwrap())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap()
}

pub(super) fn approx_eq_usage(a: f64, b: f64) -> bool {
    (a - b).abs() <= f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

pub(super) fn fanout_usage_key(projection: &ProviderProjection) -> FanoutUsageKey {
    fanout_usage_key_from_parts(
        worst_projected_usage(projection),
        soonest_relevant_reset_hours(projection),
    )
}

fn fanout_usage_key_from_parts(
    worst_projected_used: Option<f64>,
    soonest_reset_hours: Option<f64>,
) -> FanoutUsageKey {
    FanoutUsageKey {
        worst_projected_used,
        soonest_reset_hours,
    }
}

fn worst_projected_usage(projection: &ProviderProjection) -> Option<f64> {
    max_finite_value(projected_usage_values(projection).as_slice())
}

fn projected_usage_values(projection: &ProviderProjection) -> Vec<f64> {
    projection
        .projections_per_window
        .iter()
        .map(projected_usage_value)
        .collect()
}

fn projected_usage_value(window: &WindowProjection) -> f64 {
    window.projected_used
}

fn max_finite_value(values: &[f64]) -> Option<f64> {
    max_value(finite_values(values).as_slice())
}

fn finite_values(values: &[f64]) -> Vec<f64> {
    values
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect()
}

fn max_value(values: &[f64]) -> Option<f64> {
    values.iter().copied().reduce(f64::max)
}

fn soonest_relevant_reset_hours(projection: &ProviderProjection) -> Option<f64> {
    if let Some(worst) = worst_projected_usage(projection) {
        return soonest_reset_hours_for_worst_usage(projection, worst);
    }
    soonest_finite_reset_hours(projection)
}

fn soonest_reset_hours_for_worst_usage(
    projection: &ProviderProjection,
    worst_projected_used: f64,
) -> Option<f64> {
    min_reset_hour(reset_hours_for_worst_usage(projection, worst_projected_used).as_slice())
}

fn reset_hours_for_worst_usage(
    projection: &ProviderProjection,
    worst_projected_used: f64,
) -> Vec<f64> {
    reset_hours(worst_usage_reset_windows(projection, worst_projected_used).as_slice())
}

fn worst_usage_reset_windows(
    projection: &ProviderProjection,
    worst_projected_used: f64,
) -> Vec<&WindowProjection> {
    projection
        .projections_per_window
        .iter()
        .filter(|window| reset_belongs_to_worst_usage(window, worst_projected_used))
        .collect()
}

fn reset_hours(windows: &[&WindowProjection]) -> Vec<f64> {
    windows
        .iter()
        .map(|window| window.hours_until_reset)
        .collect()
}

fn reset_belongs_to_worst_usage(window: &WindowProjection, worst_projected_used: f64) -> bool {
    window.projected_used.is_finite()
        && approx_eq_usage(window.projected_used, worst_projected_used)
        && window.hours_until_reset.is_finite()
}

fn soonest_finite_reset_hours(projection: &ProviderProjection) -> Option<f64> {
    min_reset_hour(finite_reset_hours(reset_hour_values(projection).as_slice()).as_slice())
}

fn reset_hour_values(projection: &ProviderProjection) -> Vec<f64> {
    projection
        .projections_per_window
        .iter()
        .map(|window| window.hours_until_reset)
        .collect()
}

fn finite_reset_hours(hours: &[f64]) -> Vec<f64> {
    hours
        .iter()
        .copied()
        .filter(|hours| hours.is_finite())
        .collect()
}

fn min_reset_hour(hours: &[f64]) -> Option<f64> {
    hours.iter().copied().reduce(f64::min)
}

pub(super) fn finite_fanout_usage(eval: &ProviderEval) -> Option<f64> {
    eval.fanout_usage
        .and_then(|usage| usage.worst_projected_used)
        .filter(|value| value.is_finite())
}

pub(super) fn finite_fanout_reset(eval: &ProviderEval) -> Option<f64> {
    eval.fanout_usage
        .and_then(|usage| usage.soonest_reset_hours)
        .filter(|value| value.is_finite())
}

fn fanout_candidate_order(a: &ProviderEval, b: &ProviderEval) -> std::cmp::Ordering {
    fanout_usage_order(a, b)
        .or_else(|| fanout_score_order(a, b))
        .unwrap_or_else(|| fanout_reset_or_index_order(a, b))
}

fn fanout_usage_order(a: &ProviderEval, b: &ProviderEval) -> Option<std::cmp::Ordering> {
    let (a_usage, b_usage) = usage_pair(a, b)?;
    distinct_usage_pair(a_usage, b_usage).then(|| usage_pair_order(a_usage, b_usage))
}

fn usage_pair(a: &ProviderEval, b: &ProviderEval) -> Option<(f64, f64)> {
    Some((finite_fanout_usage(a)?, finite_fanout_usage(b)?))
}

fn distinct_usage_pair(a_usage: f64, b_usage: f64) -> bool {
    !approx_eq_usage(a_usage, b_usage)
}

fn usage_pair_order(a_usage: f64, b_usage: f64) -> std::cmp::Ordering {
    a_usage
        .partial_cmp(&b_usage)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn fanout_score_order(a: &ProviderEval, b: &ProviderEval) -> Option<std::cmp::Ordering> {
    let (a_score, b_score) = score_pair(a, b)?;
    finite_distinct_fanout_scores(a_score, b_score).then(|| score_pair_order(a_score, b_score))
}

fn score_pair(a: &ProviderEval, b: &ProviderEval) -> Option<(f64, f64)> {
    Some((a.binding_score?, b.binding_score?))
}

fn score_pair_order(a_score: f64, b_score: f64) -> std::cmp::Ordering {
    b_score
        .partial_cmp(&a_score)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn finite_distinct_fanout_scores(a_score: f64, b_score: f64) -> bool {
    a_score.is_finite() && b_score.is_finite() && !approx_eq_usage(a_score, b_score)
}

fn fanout_reset_or_index_order(a: &ProviderEval, b: &ProviderEval) -> std::cmp::Ordering {
    reset_order(a, b).unwrap_or_else(|| live_load_index_order(a, b))
}

fn reset_order(a: &ProviderEval, b: &ProviderEval) -> Option<std::cmp::Ordering> {
    match (finite_fanout_reset(a), finite_fanout_reset(b)) {
        (Some(a_reset), Some(b_reset)) if distinct_reset_hours(a_reset, b_reset) => {
            Some(reset_pair_order(a_reset, b_reset))
        }
        (Some(_), None) => Some(std::cmp::Ordering::Less),
        (None, Some(_)) => Some(std::cmp::Ordering::Greater),
        _ => None,
    }
}

fn reset_pair_order(a_reset: f64, b_reset: f64) -> std::cmp::Ordering {
    a_reset
        .partial_cmp(&b_reset)
        .unwrap_or(std::cmp::Ordering::Equal)
}

fn live_load_index_order(a: &ProviderEval, b: &ProviderEval) -> std::cmp::Ordering {
    a.live_load
        .cmp(&b.live_load)
        .then_with(|| a.index.cmp(&b.index))
}

fn distinct_reset_hours(a_reset: f64, b_reset: f64) -> bool {
    (a_reset - b_reset).abs() > EPS_HOURS
}

pub(super) fn select_binding_score_with_fanout(
    model: &ModelConfig,
    eligible: &[ProviderEval],
) -> usize {
    let eligible_refs = eligible_refs(eligible);
    let argmax = best_binding_score(&eligible_refs);

    if fanout_should_use_argmax(eligible) {
        return argmax.index;
    }

    let best = best_positive_binding_score(eligible);
    if !binding_score_can_fanout(best) {
        return argmax.index;
    }

    let mut band = fanout_score_band(eligible, best);
    if fanout_band_is_singleton(band.as_slice()) {
        return argmax.index;
    }
    sort_fanout_band(band.as_mut_slice());

    let selected = selected_fanout_candidate(&band);

    trace_changed_fanout_selection(model, &band, selected, argmax);

    selected.index
}

fn eligible_refs(eligible: &[ProviderEval]) -> Vec<&ProviderEval> {
    eligible.iter().collect()
}

fn fanout_band_is_singleton(band: &[&ProviderEval]) -> bool {
    band.len() < 2
}

fn sort_fanout_band(band: &mut [&ProviderEval]) {
    band.sort_by_key(|eval| eval.index);
}

fn trace_changed_fanout_selection(
    model: &ModelConfig,
    band: &[&ProviderEval],
    selected: &ProviderEval,
    argmax: &ProviderEval,
) {
    if selected.index != argmax.index {
        trace_fanout_selection(model, band, selected);
    }
}

fn fanout_should_use_argmax(eligible: &[ProviderEval]) -> bool {
    eligible.len() < 2 || eligible.iter().any(nonfinite_binding_score)
}

fn nonfinite_binding_score(eval: &ProviderEval) -> bool {
    !eval.binding_score.unwrap().is_finite()
}

fn best_positive_binding_score(eligible: &[ProviderEval]) -> f64 {
    max_score(positive_binding_scores(eligible).as_slice())
}

fn positive_binding_scores(eligible: &[ProviderEval]) -> Vec<f64> {
    binding_score_values(positive_binding_score_evals(eligible).as_slice())
}

fn positive_binding_score_evals(eligible: &[ProviderEval]) -> Vec<&ProviderEval> {
    eligible
        .iter()
        .filter(|eval| has_positive_binding_score(eval))
        .collect()
}

fn has_positive_binding_score(eval: &ProviderEval) -> bool {
    eval.binding_score.is_some_and(|score| score > 0.0)
}

fn binding_score_values(evals: &[&ProviderEval]) -> Vec<f64> {
    evals
        .iter()
        .map(|eval| eval.binding_score.unwrap())
        .collect()
}

fn max_score(scores: &[f64]) -> f64 {
    scores.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

fn binding_score_can_fanout(best: f64) -> bool {
    best.is_finite() && best > 0.0
}

fn fanout_score_band(eligible: &[ProviderEval], best: f64) -> Vec<&ProviderEval> {
    eligible
        .iter()
        .filter(|eval| binding_score_in_fanout_band(eval, best))
        .collect()
}

fn binding_score_in_fanout_band(eval: &ProviderEval, best: f64) -> bool {
    eval.binding_score.unwrap() >= best / FANOUT_SCORE_BAND_RATIO
}

fn selected_fanout_candidate<'a>(band: &[&'a ProviderEval]) -> &'a ProviderEval {
    band.iter()
        .copied()
        .min_by(|a, b| fanout_candidate_order(a, b))
        .unwrap()
}
