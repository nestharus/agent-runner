//! ## Declared roles
//!
//! `mapper`, `filter`, `predicate`, `formatter`, `orchestration`.

use super::{
    EPS_HOURS, ERROR_THRESHOLD,
    invocation_fallback::round_robin_fallback,
    projection::{ProviderProjection, WindowProjection, compute_projections_from_records},
};
use chrono::Utc;
use oulipoly_config::ModelConfig;
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

pub const FANOUT_SCORE_BAND_RATIO: f64 = 2.0;

#[derive(Debug, Clone)]
pub(super) struct ProviderEval {
    pub(super) index: usize,
    pub(super) binding_score: Option<f64>,
    pub(super) unlearned: bool,
    pub(super) fanout_usage: Option<FanoutUsageKey>,
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
) -> usize {
    let projections =
        compute_projections_from_records(model, state, quotas, windows, candidates, Utc::now());
    let evals = provider_evals_from_projections(projections.as_slice());
    let eligible = density_eligible_evals(evals.as_slice());

    if eligible.is_empty() {
        return round_robin_fallback(model, state, candidates);
    }

    select_binding_score_with_fanout(model, &eligible)
}

fn provider_evals_from_projections(projections: &[ProviderProjection]) -> Vec<ProviderEval> {
    projections
        .iter()
        .map(provider_eval_from_projection)
        .collect()
}

fn provider_eval_from_projection(projection: &ProviderProjection) -> ProviderEval {
    ProviderEval {
        index: projection.provider_index,
        binding_score: projection.binding_score,
        unlearned: projection.binding_score.is_none()
            && projection.recent_error_count < ERROR_THRESHOLD as u32,
        fanout_usage: Some(fanout_usage_key(projection)),
    }
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
    projection
        .projections_per_window
        .iter()
        .map(|window| window.projected_used)
        .filter(|projected_used| projected_used.is_finite())
        .reduce(f64::max)
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
    projection
        .projections_per_window
        .iter()
        .filter(|window| reset_belongs_to_worst_usage(window, worst_projected_used))
        .map(|window| window.hours_until_reset)
        .reduce(f64::min)
}

fn reset_belongs_to_worst_usage(window: &WindowProjection, worst_projected_used: f64) -> bool {
    window.projected_used.is_finite()
        && approx_eq_usage(window.projected_used, worst_projected_used)
        && window.hours_until_reset.is_finite()
}

fn soonest_finite_reset_hours(projection: &ProviderProjection) -> Option<f64> {
    projection
        .projections_per_window
        .iter()
        .map(|window| window.hours_until_reset)
        .filter(|hours| hours.is_finite())
        .reduce(f64::min)
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
    let (Some(a_usage), Some(b_usage)) = (finite_fanout_usage(a), finite_fanout_usage(b)) else {
        return None;
    };
    (!approx_eq_usage(a_usage, b_usage)).then(|| {
        a_usage
            .partial_cmp(&b_usage)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn fanout_score_order(a: &ProviderEval, b: &ProviderEval) -> Option<std::cmp::Ordering> {
    let (Some(a_score), Some(b_score)) = (a.binding_score, b.binding_score) else {
        return None;
    };
    (finite_distinct_fanout_scores(a_score, b_score)).then(|| {
        b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn finite_distinct_fanout_scores(a_score: f64, b_score: f64) -> bool {
    a_score.is_finite() && b_score.is_finite() && !approx_eq_usage(a_score, b_score)
}

fn fanout_reset_or_index_order(a: &ProviderEval, b: &ProviderEval) -> std::cmp::Ordering {
    match (finite_fanout_reset(a), finite_fanout_reset(b)) {
        (Some(a_reset), Some(b_reset)) if distinct_reset_hours(a_reset, b_reset) => a_reset
            .partial_cmp(&b_reset)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => a.index.cmp(&b.index),
    }
}

fn distinct_reset_hours(a_reset: f64, b_reset: f64) -> bool {
    (a_reset - b_reset).abs() > EPS_HOURS
}

pub(super) fn select_binding_score_with_fanout(
    model: &ModelConfig,
    eligible: &[ProviderEval],
) -> usize {
    let eligible_refs: Vec<&ProviderEval> = eligible.iter().collect();
    let argmax = best_binding_score(&eligible_refs);

    if fanout_should_use_argmax(eligible) {
        return argmax.index;
    }

    let best = best_positive_binding_score(eligible);
    if !binding_score_can_fanout(best) {
        return argmax.index;
    }

    let mut band = fanout_score_band(eligible, best);
    if band.len() < 2 {
        return argmax.index;
    }
    band.sort_by_key(|eval| eval.index);

    let selected = selected_fanout_candidate(&band);

    if selected.index != argmax.index {
        trace_fanout_selection(model, &band, selected);
    }

    selected.index
}

fn fanout_should_use_argmax(eligible: &[ProviderEval]) -> bool {
    eligible.len() < 2 || eligible.iter().any(nonfinite_binding_score)
}

fn nonfinite_binding_score(eval: &ProviderEval) -> bool {
    !eval.binding_score.unwrap().is_finite()
}

fn best_positive_binding_score(eligible: &[ProviderEval]) -> f64 {
    eligible
        .iter()
        .filter_map(|eval| eval.binding_score.filter(|score| *score > 0.0))
        .fold(f64::NEG_INFINITY, f64::max)
}

fn binding_score_can_fanout(best: f64) -> bool {
    best.is_finite() && best > 0.0
}

fn fanout_score_band(eligible: &[ProviderEval], best: f64) -> Vec<&ProviderEval> {
    eligible
        .iter()
        .filter(|eval| eval.binding_score.unwrap() >= best / FANOUT_SCORE_BAND_RATIO)
        .collect()
}

fn selected_fanout_candidate<'a>(band: &[&'a ProviderEval]) -> &'a ProviderEval {
    band.iter()
        .copied()
        .min_by(|a, b| fanout_candidate_order(a, b))
        .unwrap()
}

fn trace_fanout_selection(model: &ModelConfig, band: &[&ProviderEval], selected: &ProviderEval) {
    let selected_provider_name = &model.providers[selected.index].name;
    let band_member_names = fanout_band_member_names(model, band);
    tracing::info!(
        selected_provider_name = selected_provider_name.as_str(),
        band_member_names = band_member_names.as_str(),
        selected_binding_score = selected.binding_score.unwrap(),
        "fanout selected"
    );
}

fn fanout_band_member_names(model: &ModelConfig, band: &[&ProviderEval]) -> String {
    band.iter()
        .map(|eval| model.providers[eval.index].name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}
