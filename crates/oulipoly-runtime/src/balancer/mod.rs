use crate::migration::MigrationError;
use crate::quota::{
    InFlight, RefreshOutcome, has_refresh_source, is_routing_stale, is_stale,
    is_topology_probe_due, refresh_provider, refresh_provider_for_routing,
};
use crate::sessions::scan_provider;
use chrono::Utc;
use oulipoly_config::{
    ModelConfig, ProviderConfig, ProvidersConfig, SessionStorage, SessionsConfig,
};
pub use oulipoly_core::TransitionReason;
use oulipoly_state::{QuotaRecord, QuotaWindow, ResolvedResume, StateDb};
use std::fmt;

const ERROR_WINDOW_MINUTES: i64 = 30;
const ERROR_THRESHOLD: u64 = 3;
const EPS_HOURS: f64 = 1.0 / 60.0;
const EXHAUSTED_USED_PERCENT: f64 = 1.0;
/// Visible-usage threshold for the missing-window penalty. Anthropic's
/// usage API hides the 5h window when an account is near weekly cap
/// (observed live at 91% weekly). ChatGPT's API hides the 5h window
/// when there's *no recent activity* — the opposite signal. We only
/// trust "missing window means exhausted" when at least one of the
/// visible windows is itself near cap; otherwise the gap is benign
/// (idle account, different upstream behavior) and the provider should
/// not be torpedoed for it.
const HIDDEN_WINDOW_PENALTY_THRESHOLD: f64 = 0.85;
pub const FANOUT_SCORE_BAND_RATIO: f64 = 2.0;

#[derive(Debug, Clone)]
struct ProviderEval {
    index: usize,
    binding_score: Option<f64>,
    unlearned: bool,
    fanout_usage: Option<FanoutUsageKey>,
}

#[derive(Copy, Clone, Debug)]
struct FanoutUsageKey {
    worst_projected_used: Option<f64>,
    soonest_reset_hours: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderProjection {
    pub provider_index: usize,
    pub projections_per_window: Vec<WindowProjection>,
    pub binding_score: Option<f64>,
    pub recent_error_count: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowProjection {
    pub window_id: i64,
    pub projected_used: f64,
    pub hours_until_reset: f64,
    pub remaining_headroom: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MigrationDecision {
    Stay,
    Migrate {
        target_provider_index: usize,
        reason: TransitionReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    AllProvidersQuotaExhausted {
        model_name: String,
        provider_names: Vec<String>,
    },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoutingError::AllProvidersQuotaExhausted {
                model_name,
                provider_names,
            } => {
                let providers = if provider_names.is_empty() {
                    "<empty>".to_string()
                } else {
                    provider_names.join(", ")
                };
                write!(
                    f,
                    "all providers in pool {model_name} are quota-exhausted: {providers}"
                )
            }
        }
    }
}

impl std::error::Error for RoutingError {}

/// Contextual dependencies for quota-aware balancing. When present,
/// `select_provider` will trigger a synchronous refresh for any provider
/// whose cached quota is stale (older than `REFRESH_TTL_HOURS`) AND scan
/// each provider's CLI session logs for new turns. Pass `None` to use
/// cached-only scoring (e.g. from inside an async handler where blocking
/// on a network call isn't desirable).
pub struct BalanceContext<'a> {
    pub providers_cfg: &'a ProvidersConfig,
    pub sessions_cfg: &'a SessionsConfig,
    pub in_flight: &'a InFlight,
}

pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Result<usize, RoutingError> {
    let n = model.providers.len();

    // 1) Opportunistic refresh of any stale provider whose quota we can fetch.
    //    Also scan CLI session logs so calls_since_refresh reflects ALL
    //    activity (agent-runner invocations + direct user UI prompts).
    if let Some(ctx) = ctx {
        for p in &model.providers {
            if is_routing_stale(state, &p.name) {
                // Swallow the result — a failed refresh just leaves stale
                // (or missing) data, which the fallback logic below handles.
                let _: RefreshOutcome = refresh_provider_for_routing(
                    &p.name,
                    ctx.providers_cfg,
                    ctx.sessions_cfg,
                    ctx.in_flight,
                    state,
                );
            }
            // Session scan errors don't abort the pick — we just project with
            // a stale turn count instead of an up-to-date one.
            let _ = scan_provider(&p.name, ctx.sessions_cfg, state);
        }
    }

    // 2) Gather quota records + windows for each provider (cached reads only).
    let mut quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|p| state.get_quota(&p.name).ok().flatten())
        .collect();
    let mut windows: Vec<Vec<QuotaWindow>> = model
        .providers
        .iter()
        .map(|p| state.get_windows(&p.name).unwrap_or_default())
        .collect();
    if let Some(ctx) = ctx {
        let now = Utc::now();
        let live_window_counts: Vec<usize> = windows
            .iter()
            .map(|provider_windows| {
                provider_windows
                    .iter()
                    .filter(|w| w.resets_at > now)
                    .count()
            })
            .collect();
        let topology_peak_counts: Vec<usize> = quotas
            .iter()
            .map(|quota| {
                quota
                    .as_ref()
                    .map(|quota| quota.topology_peak_live_window_count)
                    .unwrap_or(0)
            })
            .collect();
        let pool_expected_live_windows = live_window_counts
            .iter()
            .zip(topology_peak_counts.iter())
            .map(|(live, peak)| (*live).max(*peak))
            .max()
            .unwrap_or(0);

        for (i, provider) in model.providers.iter().enumerate() {
            if !has_refresh_source(&provider.name, ctx.providers_cfg, ctx.sessions_cfg) {
                continue;
            }

            let live_window_count = live_window_counts[i];
            if is_topology_probe_due(
                state,
                &provider.name,
                live_window_count,
                pool_expected_live_windows,
            ) {
                let _ = state.record_topology_probe(&provider.name);
                tracing::info!(
                    provider_name = provider.name.as_str(),
                    live_window_count = live_window_count,
                    pool_expected_live_window_count = pool_expected_live_windows,
                    topology_peak_live_window_count = topology_peak_counts[i],
                    "topology probe fired"
                );
                let _: RefreshOutcome = refresh_provider_for_routing(
                    &provider.name,
                    ctx.providers_cfg,
                    ctx.sessions_cfg,
                    ctx.in_flight,
                    state,
                );
                quotas[i] = state.get_quota(&provider.name).ok().flatten();
                windows[i] = state.get_windows(&provider.name).unwrap_or_default();
            }
        }
    }
    let all_indices: Vec<usize> = (0..n).collect();
    let now = Utc::now();
    let reset_implied: Vec<bool> = all_indices
        .iter()
        .map(|i| reset_implied(quotas[*i].as_ref(), &windows[*i], now))
        .collect();
    clear_reset_implied_flags(state, model, &reset_implied);
    let filtered_indices: Vec<usize> = all_indices
        .iter()
        .copied()
        .filter(|i| {
            !provider_is_quota_exhausted(quotas[*i].as_ref(), &windows[*i], now)
                || reset_implied[*i]
        })
        .collect();
    if filtered_indices.is_empty() {
        return Err(RoutingError::AllProvidersQuotaExhausted {
            model_name: model.name.clone(),
            provider_names: model
                .providers
                .iter()
                .map(|provider| provider.name.clone())
                .collect(),
        });
    }
    let candidates: &[usize] = filtered_indices.as_slice();

    // 3) If every provider has at least one window, use density scoring.
    let all_have_windows = candidates.iter().all(|i| !windows[*i].is_empty());
    if all_have_windows {
        return Ok(score_by_density(
            model, state, &quotas, &windows, candidates,
        ));
    }

    // 4) Otherwise, fall back to lifetime invocation-count scoring.
    Ok(score_by_invocation_count(model, state, candidates))
}

fn score_by_density(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
) -> usize {
    let projections =
        compute_projections_from_records(model, state, quotas, windows, candidates, Utc::now());
    let evals = projections
        .iter()
        .map(|projection| ProviderEval {
            index: projection.provider_index,
            binding_score: projection.binding_score,
            unlearned: projection.binding_score.is_none()
                && projection.recent_error_count < ERROR_THRESHOLD as u32,
            fanout_usage: Some(fanout_usage_key(projection)),
        })
        .collect::<Vec<_>>();

    let eligible: Vec<ProviderEval> = evals
        .iter()
        .filter(|eval| !eval.unlearned && eval.binding_score.is_some())
        .cloned()
        .collect();

    if eligible.is_empty() {
        return round_robin_fallback(model, state, candidates);
    }

    select_binding_score_with_fanout(model, &eligible)
}

pub fn compute_projections(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Vec<ProviderProjection> {
    if let Some(ctx) = ctx {
        for p in &model.providers {
            if is_stale(state, &p.name) {
                let _: RefreshOutcome =
                    refresh_provider(&p.name, ctx.providers_cfg, ctx.in_flight, state);
            }
            let _ = scan_provider(&p.name, ctx.sessions_cfg, state);
        }
    }

    let quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|p| state.get_quota(&p.name).ok().flatten())
        .collect();
    let windows: Vec<Vec<QuotaWindow>> = model
        .providers
        .iter()
        .map(|p| state.get_windows(&p.name).unwrap_or_default())
        .collect();
    let candidates: Vec<usize> = (0..model.providers.len()).collect();
    compute_projections_from_records(model, state, &quotas, &windows, &candidates, Utc::now())
}

fn provider_is_quota_exhausted(
    quota: Option<&QuotaRecord>,
    windows: &[QuotaWindow],
    now: chrono::DateTime<Utc>,
) -> bool {
    quota
        .and_then(|quota| quota.exhausted_at.as_ref())
        .is_some()
        || windows
            .iter()
            .any(|window| window.resets_at > now && window.used_percent >= EXHAUSTED_USED_PERCENT)
}

fn reset_implied(
    quota: Option<&QuotaRecord>,
    windows: &[QuotaWindow],
    now: chrono::DateTime<Utc>,
) -> bool {
    quota
        .and_then(|quota| quota.exhausted_at.as_ref())
        .is_some()
        && !windows.is_empty()
        && windows.iter().all(|window| window.resets_at <= now)
}

fn clear_reset_implied_flags(state: &StateDb, model: &ModelConfig, reset_implied: &[bool]) {
    for (provider, is_reset_implied) in model.providers.iter().zip(reset_implied.iter()) {
        if !*is_reset_implied {
            continue;
        }

        if let Err(error) = state.clear_exhausted(&provider.name) {
            tracing::warn!(
                provider_name = provider.name.as_str(),
                error = error.as_str(),
                "failed to clear reset-implied quota exhaustion flag"
            );
        }
    }
}

fn compute_projections_from_records(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
    now: chrono::DateTime<Utc>,
) -> Vec<ProviderProjection> {
    // Compute the max number of live (not past-reset) windows any candidate
    // reports. A provider whose upstream API returns fewer windows than
    // siblings (observed 2026-04-22: `anthropic-usage` returns only the 7d
    // window for heavily-used accounts because Anthropic's API hides the 5h
    // timer when the account is near weekly cap) would otherwise dodge the
    // constraining short-tier term in `min_w` and beat its siblings on the
    // 7d-tier-only score. Defensive pessimism: when this provider has fewer
    // live windows than the pool max AND at least one visible window is near
    // cap, penalize the binding as if the missing slots were at 1.0 used.
    // The visible-usage gate distinguishes Anthropic's "hide 5h near cap"
    // from ChatGPT's "hide 5h when idle".
    let pool_max_live_windows = candidates
        .iter()
        .map(|&i| windows[i].iter().filter(|w| w.resets_at > now).count())
        .max()
        .unwrap_or(0);

    candidates
        .iter()
        .copied()
        .map(|i| {
            let ws = &windows[i];
            let recent_errors = state
                .recent_error_count(&model.name, &model.providers[i].name, ERROR_WINDOW_MINUTES)
                .unwrap_or(0);
            if recent_errors >= ERROR_THRESHOLD as i64 {
                return ProviderProjection {
                    provider_index: i,
                    projections_per_window: Vec::new(),
                    binding_score: None,
                    recent_error_count: recent_errors as u32,
                };
            }

            let q = quotas[i].as_ref();
            let turns = q
                .and_then(|q| {
                    state
                        .count_assistant_turns_since(
                            &model.providers[i].name,
                            q.refreshed_at.as_ref(),
                        )
                        .ok()
                })
                .unwrap_or(0);
            let mut binding_score = f64::INFINITY;
            let mut unlearned = false;
            let mut scored_window = false;
            let mut projections = Vec::new();

            let live_window_count = ws.iter().filter(|w| w.resets_at > now).count();
            let any_visible_near_cap = ws
                .iter()
                .any(|w| w.resets_at > now && w.used_percent >= HIDDEN_WINDOW_PENALTY_THRESHOLD);
            if live_window_count < pool_max_live_windows && any_visible_near_cap {
                binding_score = binding_score.min(0.0);
                scored_window = true;
            }

            for window in ws {
                // Skip windows whose reset already happened: the stored
                // used_percent is from the prior window instance, so treating
                // it as current headroom poisons the binding score.
                if window.resets_at <= now {
                    continue;
                }
                let Some(burn_rate) = bootstrap_burn_rate(i, window, quotas, windows) else {
                    unlearned = true;
                    continue;
                };
                let projected = project_used_percent(window.used_percent, turns, burn_rate);
                let hours = ((window.resets_at - now).num_seconds() as f64 / 3600.0).max(EPS_HOURS);
                let remaining_headroom = (1.0 - projected).max(0.0);
                binding_score = binding_score.min(remaining_headroom * hours);
                scored_window = true;
                projections.push(WindowProjection {
                    window_id: window.window_id as i64,
                    projected_used: projected,
                    hours_until_reset: hours,
                    remaining_headroom,
                });
            }

            ProviderProjection {
                provider_index: i,
                projections_per_window: projections,
                binding_score: if unlearned || !scored_window {
                    None
                } else {
                    Some(binding_score)
                },
                recent_error_count: recent_errors as u32,
            }
        })
        .collect()
}

pub fn decide_migration(
    state: &StateDb,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    manual_target: Option<&str>,
) -> Result<MigrationDecision, MigrationError> {
    if model.providers.len() <= 1 {
        return Ok(MigrationDecision::Stay);
    }

    let Some(active_provider_index) = model
        .providers
        .iter()
        .position(|provider| provider.name == resolved.active_provider)
    else {
        return Ok(MigrationDecision::Stay);
    };
    let active = &model.providers[active_provider_index];

    if let Some(target) = manual_target {
        if let Some(target_provider_index) = model.providers.iter().position(|p| p.name == target) {
            let target_provider = &model.providers[target_provider_index];
            if is_resume_migratable_pair(active, target_provider) {
                return Ok(MigrationDecision::Migrate {
                    target_provider_index,
                    reason: TransitionReason::Manual,
                });
            }
        }
        return Ok(MigrationDecision::Stay);
    }

    if !is_resume_migratable_pair(active, active) {
        return Ok(MigrationDecision::Stay);
    }

    let active_exhausted = state
        .get_quota(&active.name)
        .map_err(|message| MigrationError::Db { message })?
        .and_then(|quota| quota.exhausted_at)
        .is_some();
    let projections = compute_projections(model, state, None);

    if active_exhausted {
        if let Some(target) =
            lowest_load_migration_target(model, &projections, active, Some(active_provider_index))
        {
            return Ok(MigrationDecision::Migrate {
                target_provider_index: target.provider_index,
                reason: TransitionReason::Exhausted,
            });
        }
        return Ok(MigrationDecision::Stay);
    }

    let Some(best) = lowest_load_migration_target(model, &projections, active, None) else {
        return Ok(MigrationDecision::Stay);
    };
    if best.provider_index == active_provider_index {
        return Ok(MigrationDecision::Stay);
    }

    Ok(MigrationDecision::Migrate {
        target_provider_index: best.provider_index,
        reason: TransitionReason::QuotaThreshold,
    })
}

fn is_resume_migratable_pair(source: &ProviderConfig, target: &ProviderConfig) -> bool {
    matches!(
        (&source.session_storage, &target.session_storage),
        (
            Some(SessionStorage::ClaudeCode { .. }),
            Some(SessionStorage::ClaudeCode { .. })
        )
    )
}

fn provider_load(projection: &ProviderProjection) -> f64 {
    let max_projected_used = projection
        .projections_per_window
        .iter()
        .map(|window| window.projected_used)
        .fold(f64::NEG_INFINITY, f64::max);
    if max_projected_used.is_finite() {
        max_projected_used
    } else {
        0.0
    }
}

fn lowest_load_migration_target<'a>(
    model: &ModelConfig,
    projections: &'a [ProviderProjection],
    source_provider: &ProviderConfig,
    exclude_provider_index: Option<usize>,
) -> Option<&'a ProviderProjection> {
    projections
        .iter()
        .filter(|projection| Some(projection.provider_index) != exclude_provider_index)
        .filter(|projection| {
            model
                .providers
                .get(projection.provider_index)
                .is_some_and(|candidate| is_resume_migratable_pair(source_provider, candidate))
        })
        .min_by(|a, b| {
            let load_order = provider_load(a)
                .partial_cmp(&provider_load(b))
                .unwrap_or(std::cmp::Ordering::Equal);
            load_order.then_with(|| a.provider_index.cmp(&b.provider_index))
        })
}

fn best_binding_score<'a>(evals: &[&'a ProviderEval]) -> &'a ProviderEval {
    debug_assert!(!evals.is_empty(), "best_binding_score: empty slice");
    debug_assert!(
        evals.iter().all(|e| e.binding_score.is_some()),
        "best_binding_score: caller must filter to providers with a learned binding_score"
    );
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

fn approx_eq_usage(a: f64, b: f64) -> bool {
    (a - b).abs() <= f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

fn fanout_usage_key(projection: &ProviderProjection) -> FanoutUsageKey {
    let worst_projected_used = projection
        .projections_per_window
        .iter()
        .map(|window| window.projected_used)
        .filter(|projected_used| projected_used.is_finite())
        .reduce(f64::max);

    let soonest_reset_hours = if let Some(worst) = worst_projected_used {
        projection
            .projections_per_window
            .iter()
            .filter(|window| {
                window.projected_used.is_finite()
                    && approx_eq_usage(window.projected_used, worst)
                    && window.hours_until_reset.is_finite()
            })
            .map(|window| window.hours_until_reset)
            .reduce(f64::min)
    } else {
        projection
            .projections_per_window
            .iter()
            .map(|window| window.hours_until_reset)
            .filter(|hours| hours.is_finite())
            .reduce(f64::min)
    };

    FanoutUsageKey {
        worst_projected_used,
        soonest_reset_hours,
    }
}

fn finite_fanout_usage(eval: &ProviderEval) -> Option<f64> {
    eval.fanout_usage
        .and_then(|usage| usage.worst_projected_used)
        .filter(|value| value.is_finite())
}

fn finite_fanout_reset(eval: &ProviderEval) -> Option<f64> {
    eval.fanout_usage
        .and_then(|usage| usage.soonest_reset_hours)
        .filter(|value| value.is_finite())
}

fn fanout_candidate_order(a: &ProviderEval, b: &ProviderEval) -> std::cmp::Ordering {
    if let (Some(a_usage), Some(b_usage)) = (finite_fanout_usage(a), finite_fanout_usage(b))
        && !approx_eq_usage(a_usage, b_usage)
    {
        return a_usage
            .partial_cmp(&b_usage)
            .unwrap_or(std::cmp::Ordering::Equal);
    }

    if let (Some(a_score), Some(b_score)) = (a.binding_score, b.binding_score)
        && a_score.is_finite()
        && b_score.is_finite()
        && !approx_eq_usage(a_score, b_score)
    {
        return b_score
            .partial_cmp(&a_score)
            .unwrap_or(std::cmp::Ordering::Equal);
    }

    match (finite_fanout_reset(a), finite_fanout_reset(b)) {
        (Some(a_reset), Some(b_reset)) if (a_reset - b_reset).abs() > EPS_HOURS => a_reset
            .partial_cmp(&b_reset)
            .unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        _ => a.index.cmp(&b.index),
    }
}

fn select_binding_score_with_fanout(model: &ModelConfig, eligible: &[ProviderEval]) -> usize {
    let eligible_refs: Vec<&ProviderEval> = eligible.iter().collect();
    let argmax = best_binding_score(&eligible_refs);

    if eligible.len() < 2
        || eligible
            .iter()
            .any(|eval| !eval.binding_score.unwrap().is_finite())
    {
        return argmax.index;
    }

    let best = eligible
        .iter()
        .filter_map(|eval| eval.binding_score.filter(|score| *score > 0.0))
        .fold(f64::NEG_INFINITY, f64::max);
    if !best.is_finite() || best <= 0.0 {
        return argmax.index;
    }

    let mut band: Vec<&ProviderEval> = eligible
        .iter()
        .filter(|eval| eval.binding_score.unwrap() >= best / FANOUT_SCORE_BAND_RATIO)
        .collect();
    if band.len() < 2 {
        return argmax.index;
    }
    band.sort_by_key(|eval| eval.index);

    let selected = band
        .iter()
        .copied()
        .min_by(|a, b| fanout_candidate_order(a, b))
        .unwrap();

    if selected.index != argmax.index {
        let selected_provider_name = &model.providers[selected.index].name;
        let band_member_names = band
            .iter()
            .map(|eval| model.providers[eval.index].name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        tracing::info!(
            selected_provider_name = selected_provider_name.as_str(),
            band_member_names = band_member_names.as_str(),
            selected_binding_score = selected.binding_score.unwrap(),
            "fanout selected"
        );
    }

    selected.index
}

fn project_used_percent(base_used_percent: f64, turns: u64, burn_rate: f64) -> f64 {
    (base_used_percent + (turns as f64) * burn_rate).max(0.0)
}

fn learned_rate(window: &QuotaWindow) -> Option<f64> {
    match (window.last_delta_percent, window.last_delta_calls) {
        (Some(delta_percent), Some(delta_calls)) if delta_percent > 0.0 && delta_calls > 0 => {
            Some(delta_percent / delta_calls as f64)
        }
        _ => None,
    }
}

fn bootstrap_burn_rate(
    provider_index: usize,
    window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
    learned_rate(window)
        .or_else(|| pool_window_avg_percent_per_call(window.window_id, windows))
        .or_else(|| {
            duration_ratio_fallback_percent_per_call(provider_index, window, quotas, windows)
        })
}

fn pool_window_avg_percent_per_call(window_id: u32, windows: &[Vec<QuotaWindow>]) -> Option<f64> {
    let mut total_percent = 0.0;
    let mut total_calls: u64 = 0;
    for window in windows.iter().flatten() {
        if window.window_id == window_id
            && let (Some(delta_percent), Some(delta_calls)) =
                (window.last_delta_percent, window.last_delta_calls)
            && delta_percent > 0.0
            && delta_calls > 0
        {
            total_percent += delta_percent;
            total_calls += delta_calls;
        }
    }
    (total_calls > 0).then_some(total_percent / total_calls as f64)
}

fn duration_ratio_fallback_percent_per_call(
    provider_index: usize,
    target_window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
    let target_refreshed_at = quotas
        .get(provider_index)
        .and_then(|quota| quota.as_ref())
        .and_then(|quota| quota.refreshed_at.as_ref())?;
    let target_hours = ((target_window.resets_at - *target_refreshed_at).num_seconds() as f64
        / 3600.0)
        .max(EPS_HOURS);

    let mut best: Option<(f64, f64)> = None;
    for (i, provider_windows) in windows.iter().enumerate() {
        let Some(refreshed_at) = quotas
            .get(i)
            .and_then(|quota| quota.as_ref())
            .and_then(|quota| quota.refreshed_at.as_ref())
        else {
            continue;
        };
        for window in provider_windows {
            let Some(rate) = learned_rate(window) else {
                continue;
            };
            let long_hours =
                ((window.resets_at - *refreshed_at).num_seconds() as f64 / 3600.0).max(EPS_HOURS);
            if long_hours <= target_hours {
                continue;
            }
            if best.is_none_or(|(_, best_hours)| long_hours > best_hours) {
                best = Some((rate, long_hours));
            }
        }
    }

    best.map(|(rate, long_hours)| duration_ratio_rate(rate, long_hours, target_hours))
}

fn duration_ratio_rate(long_rate: f64, long_hours: f64, target_hours: f64) -> f64 {
    long_rate * (long_hours / target_hours.max(EPS_HOURS))
}

#[cfg(test)]
pub(crate) fn project_used_percent_for_test(
    base_used_percent: f64,
    turns: u64,
    burn_rate: f64,
) -> f64 {
    project_used_percent(base_used_percent, turns, burn_rate)
}

#[cfg(test)]
pub(crate) fn bootstrap_burn_rate_for_test(
    model: &ModelConfig,
    state: &StateDb,
    provider_index: usize,
    window_id: u32,
) -> Option<f64> {
    let quotas: Vec<Option<QuotaRecord>> = model
        .providers
        .iter()
        .map(|provider| state.get_quota(&provider.name).ok().flatten())
        .collect();
    let windows: Vec<Vec<QuotaWindow>> = model
        .providers
        .iter()
        .map(|provider| state.get_windows(&provider.name).unwrap_or_default())
        .collect();
    let target = windows
        .get(provider_index)?
        .iter()
        .find(|window| window.window_id == window_id)?;
    bootstrap_burn_rate(provider_index, target, &quotas, &windows)
}

#[cfg(test)]
pub(crate) fn bootstrap_duration_ratio_for_test(
    long_rate: f64,
    long_hours: f64,
    target_hours: f64,
) -> f64 {
    duration_ratio_rate(long_rate, long_hours, target_hours)
}

fn score_by_invocation_count(model: &ModelConfig, state: &StateDb, candidates: &[usize]) -> usize {
    let mut scores: Vec<(usize, f64)> = Vec::with_capacity(candidates.len());

    for &i in candidates {
        let recent_errors = state
            .recent_error_count(&model.name, &model.providers[i].name, ERROR_WINDOW_MINUTES)
            .unwrap_or(0);

        if recent_errors >= ERROR_THRESHOLD as i64 {
            scores.push((i, f64::MAX));
            continue;
        }

        let invocation_count = state
            .get_provider(&model.name, &model.providers[i].name)
            .ok()
            .flatten()
            .map(|p| p.invocation_count)
            .unwrap_or(0);

        let error_penalty = recent_errors as f64 * 10.0;
        scores.push((i, invocation_count as f64 + error_penalty));
    }

    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    if scores.iter().all(|(_, s)| *s == f64::MAX) {
        return round_robin_fallback(model, state, candidates);
    }
    scores[0].0
}

fn round_robin_fallback(model: &ModelConfig, state: &StateDb, candidates: &[usize]) -> usize {
    debug_assert!(
        !candidates.is_empty(),
        "round_robin_fallback: caller must pass a non-empty candidates slice"
    );
    let mut min_count = i64::MAX;
    let mut best = candidates.first().copied().unwrap_or(0);

    for &i in candidates {
        let count = state
            .get_provider(&model.name, &model.providers[i].name)
            .ok()
            .flatten()
            .map(|p| p.invocation_count)
            .unwrap_or(0);
        if count < min_count {
            min_count = count;
            best = i;
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, SecondsFormat, Utc};
    use oulipoly_config::{
        ProviderConfig, ProviderEntry, ProvidersConfig, SessionsConfig, model::PromptMode,
    };
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    fn record_invocation_for_test(
        db: &StateDb,
        model_name: &str,
        provider_name: &str,
        provider_index: usize,
        success: bool,
    ) {
        let id = db
            .start_invocation(&oulipoly_state::InvocationStart {
                invocation_uuid: Uuid::new_v4().to_string(),
                model_name: model_name.to_string(),
                provider_name: provider_name.to_string(),
                provider_index,
                parent_invocation_id: None,
            })
            .unwrap();
        db.finalize_invocation(id, success, if success { 0 } else { 1 }, None, None)
            .unwrap();
    }

    fn provider_eval_with_fanout_usage(
        index: usize,
        binding_score: f64,
        worst_projected_used: Option<f64>,
        soonest_reset_hours: Option<f64>,
    ) -> ProviderEval {
        ProviderEval {
            index,
            binding_score: Some(binding_score),
            unlearned: false,
            fanout_usage: Some(FanoutUsageKey {
                worst_projected_used,
                soonest_reset_hours,
            }),
        }
    }

    fn providers_config_with_scripts(scripts: &[(&str, &str)]) -> ProvidersConfig {
        let entries = scripts
            .iter()
            .map(|(provider_name, script)| {
                (
                    (*provider_name).to_string(),
                    ProviderEntry {
                        quota_script: Some((*script).to_string()),
                        ..ProviderEntry::default()
                    },
                )
            })
            .collect();
        ProvidersConfig { entries }
    }

    fn two_provider_model() -> ModelConfig {
        ModelConfig {
            name: "test".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                ProviderConfig::new("a", vec![]),
                ProviderConfig::new("b", vec![]),
            ],
            inputs: vec![],
        }
    }

    fn three_provider_model() -> ModelConfig {
        ModelConfig {
            name: "test3".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                ProviderConfig::new("a", vec![]),
                ProviderConfig::new("b", vec![]),
                ProviderConfig::new("c", vec![]),
            ],
            inputs: vec![],
        }
    }

    #[test]
    fn single_provider_always_zero() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = ModelConfig {
            name: "single".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("x", vec![])],
            inputs: vec![],
        };
        assert_eq!(select_provider(&model, &db, None).unwrap(), 0);
    }

    #[test]
    fn round_robin_on_fresh_state() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        let first = select_provider(&model, &db, None).unwrap();
        assert_eq!(first, 0);

        record_invocation_for_test(&db, "test", "a", 0, true);

        let second = select_provider(&model, &db, None).unwrap();
        assert_eq!(second, 1);
    }

    #[test]
    fn avoids_errored_providers() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        for _ in 0..3 {
            record_invocation_for_test(&db, "test", "a", 0, false);
        }

        assert_eq!(select_provider(&model, &db, None).unwrap(), 1);
    }

    // Risk: Balancer recent-error call-site | level: unit
    // Source: ~/projects/agent-runner/planning/trunk/proposals/10-routing-claude-skipped.md §Test-intent track
    #[test]
    fn fallback_recent_error_scoring_uses_provider_name_not_reused_index() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        for _ in 0..3 {
            record_invocation_for_test(&db, "test", "old-a", 0, false);
        }

        let selected = select_provider(&model, &db, None).unwrap();
        assert_eq!(
            model.providers[selected].name, "a",
            "stale failures for old-a at index 0 must not suppress current provider a"
        );
    }

    fn quota_window(used: f64, hours_until_reset: i64) -> oulipoly_state::QuotaWindowInput {
        use chrono::Duration;
        oulipoly_state::QuotaWindowInput {
            used_percent: used,
            resets_at: Utc::now() + Duration::hours(hours_until_reset),
        }
    }

    fn one_window(used: f64, hours_until_reset: i64) -> Vec<oulipoly_state::QuotaWindowInput> {
        vec![quota_window(used, hours_until_reset)]
    }

    fn seed_windows_with_deltas(
        db: &StateDb,
        provider_name: &str,
        windows: &[(f64, i64, f64, u64)],
    ) {
        let inputs: Vec<_> = windows
            .iter()
            .map(|(used, hours, _, _)| quota_window(*used, *hours))
            .collect();
        db.upsert_quota_refresh(provider_name, &inputs).unwrap();
        for (window_id, (_, _, delta_percent, delta_calls)) in windows.iter().enumerate() {
            db.set_window_delta_for_test(
                provider_name,
                window_id as u32,
                *delta_percent,
                *delta_calls,
            )
            .unwrap();
        }
    }

    fn seed_assistant_turns_since_refresh(db: &StateDb, provider_name: &str, count: usize) {
        use chrono::Duration;

        let refreshed_at = Utc::now() - Duration::hours(1);
        db.set_refreshed_at_for_test(provider_name, &refreshed_at)
            .unwrap();
        let turns: Vec<_> = (0..count)
            .map(|i| oulipoly_state::SessionTurnIngest {
                session_id: format!("{provider_name}-session"),
                turn_id: format!("{provider_name}-turn-{i}"),
                timestamp: refreshed_at + Duration::seconds((i + 1) as i64),
                role: "assistant".to_string(),
                parent_turn_id: None,
                is_sidechain: false,
                is_compaction_boundary: false,
                body: None,
            })
            .collect();
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }

    fn selected_provider_index(model: &ModelConfig, db: &StateDb) -> usize {
        select_provider(model, db, None).unwrap()
    }

    fn single_provider_model() -> ModelConfig {
        ModelConfig {
            name: "single".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![ProviderConfig::new("a", vec![])],
            inputs: vec![],
        }
    }

    #[derive(Clone, Copy)]
    enum TestWindow {
        SevenDay,
        FiveHour,
    }

    fn seed_two_window_used(
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

    fn assert_approx(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {actual} to be within {tolerance} of {expected}"
        );
    }

    #[test]
    fn select_provider_filters_exhausted_accounts() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.10, 24 * 7, 0.01, 22)]);
        seed_windows_with_deltas(&db, "b", &[(0.60, 24 * 7, 0.01, 22)]);
        db.mark_exhausted("a").unwrap();

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    #[test]
    fn select_provider_readmits_exhausted_account_when_all_windows_elapsed() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22), (0.80, -2, 0.30, 22)]);
        db.mark_exhausted("a").unwrap();

        assert_eq!(selected_provider_index(&model, &db), 0);
        assert_eq!(db.get_quota("a").unwrap().unwrap().exhausted_at, None);
    }

    #[test]
    fn select_provider_keeps_exhausted_account_excluded_while_a_window_is_live() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.20, -1, 0.01, 22), (1.0, 5, 0.30, 22)]);
        db.mark_exhausted("a").unwrap();

        assert_eq!(selected_provider_index(&model, &db), 1);
        assert!(db.get_quota("a").unwrap().unwrap().exhausted_at.is_some());
    }

    #[test]
    fn select_provider_keeps_zero_window_exhausted_account_excluded() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        db.upsert_quota_refresh("a", &[]).unwrap();
        db.mark_exhausted("a").unwrap();

        assert_eq!(selected_provider_index(&model, &db), 1);
        let err = select_provider(&single_provider_model(), &db, None).unwrap_err();
        assert_eq!(
            err,
            RoutingError::AllProvidersQuotaExhausted {
                model_name: "single".to_string(),
                provider_names: vec!["a".to_string()],
            },
            "zero-window exhausted provider must be excluded from the eligible set"
        );
        assert!(db.get_quota("a").unwrap().unwrap().exhausted_at.is_some());
    }

    #[test]
    fn select_provider_hard_excludes_accounts_at_or_over_live_window_quota() {
        for target_window in [TestWindow::SevenDay, TestWindow::FiveHour] {
            for used in [0.0, 0.99] {
                let db = StateDb::open(Path::new(":memory:")).unwrap();
                let model = two_provider_model();
                let (seven_day_used, five_hour_used) = match target_window {
                    TestWindow::SevenDay => (used, 0.20),
                    TestWindow::FiveHour => (0.20, used),
                };
                seed_two_window_used(&db, "a", seven_day_used, five_hour_used);
                seed_two_window_used(&db, "b", 0.995, 0.995);

                assert_eq!(
                    selected_provider_index(&model, &db),
                    0,
                    "used={used} should stay eligible below 100%"
                );
            }

            for used in [1.0, 1.5] {
                let db = StateDb::open(Path::new(":memory:")).unwrap();
                let model = two_provider_model();
                let (seven_day_used, five_hour_used) = match target_window {
                    TestWindow::SevenDay => (used, 0.20),
                    TestWindow::FiveHour => (0.20, used),
                };
                seed_two_window_used(&db, "a", seven_day_used, five_hour_used);
                seed_two_window_used(&db, "b", 0.50, 0.50);

                assert_eq!(
                    selected_provider_index(&model, &db),
                    1,
                    "used={used} must be excluded at or over 100%"
                );
            }
        }
    }

    #[test]
    fn select_provider_past_reset_window_at_quota_does_not_hard_exclude() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = single_provider_model();

        seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22)]);

        assert_eq!(selected_provider_index(&model, &db), 0);
    }

    #[test]
    fn select_provider_errors_when_single_account_is_at_or_over_quota() {
        for target_window in [TestWindow::SevenDay, TestWindow::FiveHour] {
            for used in [1.0, 1.5] {
                let db = StateDb::open(Path::new(":memory:")).unwrap();
                let model = single_provider_model();
                let (seven_day_used, five_hour_used) = match target_window {
                    TestWindow::SevenDay => (used, 0.20),
                    TestWindow::FiveHour => (0.20, used),
                };
                seed_two_window_used(&db, "a", seven_day_used, five_hour_used);

                let err = select_provider(&model, &db, None).unwrap_err();
                assert_eq!(
                    err,
                    RoutingError::AllProvidersQuotaExhausted {
                        model_name: "single".to_string(),
                        provider_names: vec!["a".to_string()],
                    }
                );
                assert!(
                    err.to_string()
                        .contains("all providers in pool single are quota-exhausted"),
                    "{err}"
                );
            }
        }
    }

    #[test]
    fn all_providers_exhausted_returns_clean_error() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        db.upsert_quota_refresh("a", &[]).unwrap();
        db.upsert_quota_refresh("b", &[]).unwrap();

        db.mark_exhausted("b").unwrap();
        db.mark_exhausted("a").unwrap();

        let err = select_provider(&model, &db, None).unwrap_err();
        assert_eq!(
            err,
            RoutingError::AllProvidersQuotaExhausted {
                model_name: "test".to_string(),
                provider_names: vec!["a".to_string(), "b".to_string()],
            }
        );
    }

    #[test]
    fn all_provider_windows_at_or_over_quota_returns_clean_error() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_two_window_used(&db, "a", 1.0, 0.20);
        seed_two_window_used(&db, "b", 0.20, 1.5);

        let err = select_provider(&model, &db, None).unwrap_err();
        assert!(
            err.to_string()
                .contains("all providers in pool test are quota-exhausted"),
            "{err}"
        );
    }

    #[test]
    fn score_by_density_skips_past_reset_windows() {
        // Live-caught 2026-04-22: claude3 had a 5h window whose resets_at
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

        // Provider `b`: heavily-used 7d window, nothing past-reset.
        seed_windows_with_deltas(&db, "b", &[(0.85, 24 * 7, 0.01, 22)]);

        // With past-reset skipping, `a` is ranked only on its 7d window
        // (much more headroom than b's 7d). Without the skip, a's
        // near-zero 5h binding would lose to b.
        assert_eq!(selected_provider_index(&model, &db), 0);
    }

    #[test]
    fn score_by_density_penalizes_provider_missing_window_siblings_have() {
        // Live-caught 2026-04-22: claude in the claude-opus pool had
        // only a 7d window reported (anthropic-usage returned 1 window
        // because Anthropic's API hides the 5h timer when the account
        // is near weekly cap), while claude2 and claude3 both had 2
        // windows. Claude's 7d was at 91% used — the MOST pressed
        // account in the pool. But with only one window to min over
        // vs siblings' two, claude's binding ((1-0.91)*41h ≈ 3.65)
        // beat claude3's min((1-0.64)*41h, (1-0.04)*3.6h) ≈ 3.46
        // simply because claude3's 5h tier pulled its binding down.
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

        // Provider `a`: ONE 7d window at 91% used (mimics claude's
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
        // Live-caught 2026-04-26: codex (98% remaining, near-zero
        // recent usage) had only a 7d window reported by chatgpt-usage
        // because ChatGPT's API only emits `primary_window` when an
        // account has an active 5h timer (i.e. recent activity). Codex2
        // (64% remaining, actively in use) had both windows. Under the
        // unconditional missing-window penalty, codex's binding was
        // forced to 0 and every invocation routed to the more-pressed
        // codex2 — codex stayed idle, which kept it 5h-windowless,
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

        // Provider `a`: ONE 7d window at 2% used (mimics codex's idle
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

    /// Risk: Equal projected usage could regress the codex invariant by letting reset beat score.
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

    /// Risk: The user-visible claude/claude4 reporter case could still pick the higher-usage account.
    /// Level: unit.
    /// Source: AGE-25 proposal §7 item 11 / contract item 10; Assumptions A2, A4, A5.
    #[test]
    fn density_fanout_smoke_selects_claude_51_over_claude4_82() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = ModelConfig {
            name: "claude-opus".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![
                ProviderConfig::new("claude", vec![]),
                ProviderConfig::new("claude4", vec![]),
            ],
            inputs: vec![],
        };

        seed_windows_with_deltas(&db, "claude", &[(0.51, 48, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude4", &[(0.82, 96, 0.01, 22)]);
        for _ in 0..5 {
            record_invocation_for_test(&db, &model.name, "claude", 0, true);
        }

        assert_eq!(
            selected_provider_index(&model, &db),
            0,
            "claude has lower projected usage than claude4 and must remain selected despite higher local count"
        );
    }

    /// Risk: Topology probe might run too late, after density already chose the stale single-window provider.
    /// Level: component.
    /// Source: proposal §Test-intent track row 6; Assumptions A2, A6.
    #[test]
    fn topology_probe_refreshes_incomplete_cached_provider_before_density() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);

        let long_resets =
            (Utc::now() + Duration::hours(80)).to_rfc3339_opts(SecondsFormat::Secs, true);
        let short_resets =
            (Utc::now() + Duration::hours(5)).to_rfc3339_opts(SecondsFormat::Secs, true);
        let repaired_a_script = format!(
            r#"printf '%s' '{{"windows":[{{"used_percent":4,"resets_at":"{long_resets}"}},{{"used_percent":90,"resets_at":"{short_resets}"}}]}}'"#
        );
        let providers_cfg = providers_config_with_scripts(&[
            ("a", repaired_a_script.as_str()),
            ("b", "printf '%s' '{\"windows\":[]}'"),
        ]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(
            db.get_windows("a").unwrap().len(),
            2,
            "topology probe should refresh the incomplete cached provider before scoring"
        );
        assert_eq!(
            selected, 1,
            "after the repaired short-window constraint is visible, provider b should win"
        );
    }

    /// Risk: Persistently one-window providers could run quota scripts every invocation.
    /// Level: component.
    /// Source: proposal §Test-intent track row 7; Assumptions A2, A6.
    #[test]
    fn topology_probe_respects_cooldown_for_persistent_short_topology() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);
        db.record_topology_probe("a").unwrap();

        let would_repair_a_script = r#"printf '%s' '{"windows":[{"used_percent":4,"resets_at":"2036-05-09T14:00:00Z"},{"used_percent":90,"resets_at":"2036-05-03T03:50:00Z"}]}'"#;
        let providers_cfg = providers_config_with_scripts(&[
            ("a", would_repair_a_script),
            ("b", "printf '%s' '{\"windows\":[]}'"),
        ]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(
            db.get_windows("a").unwrap().len(),
            1,
            "recent topology probe timestamp should suppress a repeat probe"
        );
        assert_eq!(
            selected, 0,
            "cooldown preserves cached routing rather than repeatedly running quota scripts"
        );
    }

    #[test]
    fn routing_refreshes_stale_quota_after_thirty_seconds_before_scoring() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
        db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(31)))
            .unwrap();

        let resets = (Utc::now() + Duration::hours(48)).to_rfc3339_opts(SecondsFormat::Secs, true);
        let exhausted_a_script = format!(
            r#"printf '%s' '{{"windows":[{{"used_percent":100,"resets_at":"{resets}"}}]}}'"#
        );
        let providers_cfg = providers_config_with_scripts(&[("a", exhausted_a_script.as_str())]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(selected, 1);
        assert!(
            db.get_windows("a")
                .unwrap()
                .iter()
                .any(|window| window.used_percent >= 1.0),
            "stale provider a should be refreshed to exhausted before routing"
        );
    }

    #[test]
    fn routing_uses_cached_quota_inside_thirty_second_ttl() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
        db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(10)))
            .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("quota-ran");
        let script = dir.path().join("quota.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch {}\nprintf '%s' '{{\"windows\":[{{\"used_percent\":100,\"resets_at\":\"2099-01-01T00:00:00Z\"}}]}}'\n",
                marker.display()
            ),
        )
        .unwrap();
        let script_cmd = format!("sh {}", script.display());
        let providers_cfg = providers_config_with_scripts(&[("a", script_cmd.as_str())]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(selected, 0);
        assert!(
            !marker.exists(),
            "fresh cached quota should suppress the quota script inside the routing TTL"
        );
    }

    #[test]
    fn routing_refresh_failure_falls_back_to_cached_quota() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        seed_windows_with_deltas(&db, "a", &[(0.02, 48, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.40, 48, 0.01, 40)]);
        db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::seconds(31)))
            .unwrap();

        let providers_cfg = providers_config_with_scripts(&[("a", "exit 1")]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(
            selected, 0,
            "refresh failures should leave cached state usable instead of aborting routing"
        );
        assert!(
            db.get_windows("a")
                .unwrap()
                .iter()
                .all(|window| window.used_percent < 1.0),
            "failed refresh must preserve prior cached windows"
        );
    }

    #[test]
    fn high_weekly_account_stops_winning_after_cumulative_turns() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        let long_delta = 0.01;
        let long_calls = 22;

        seed_windows_with_deltas(
            &db,
            "a",
            &[
                (0.80, 24 * 7, long_delta, long_calls),
                (0.04, 5, 0.30, long_calls),
            ],
        );
        seed_windows_with_deltas(
            &db,
            "b",
            &[
                (0.10, 24 * 7, long_delta, long_calls),
                (0.85, 5, 0.30, long_calls),
            ],
        );
        seed_assistant_turns_since_refresh(&db, "a", 500);

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

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

    #[test]
    fn fresh_pool_falls_through_to_invocation_count_round_robin() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();

        record_invocation_for_test(&db, "test", "a", 0, true);

        assert_eq!(selected_provider_index(&model, &db), 1);
    }

    fn migratable_model(provider_names: &[(&str, &str)]) -> ModelConfig {
        let providers = provider_names
            .iter()
            .map(|(name, storage_kind)| {
                let session_storage = match *storage_kind {
                    "claude_code" => Some(oulipoly_config::SessionStorage::ClaudeCode {
                        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
                    }),
                    "codex" => Some(oulipoly_config::SessionStorage::Codex {
                        sessions_dir: PathBuf::from(format!("/tmp/{name}/sessions")),
                    }),
                    "none" => None,
                    other => panic!("unknown storage kind fixture {other}"),
                };
                oulipoly_config::ProviderConfig {
                    name: (*name).to_string(),
                    command: (*name).to_string(),
                    args: Vec::new(),
                    interactive_args: Some(vec!["launch".to_string()]),
                    resume: Some(oulipoly_config::ResumeStrategy {
                        kind: oulipoly_config::ResumeKind::Flag,
                        flag: Some("--resume".to_string()),
                        subcommand: None,
                    }),
                    session_capture: None,
                    resume_acceptance: None,
                    session_storage,
                    system_prompt_override: None,
                    tool_restrictions: None,
                    invocation_mode: Default::default(),
                }
            })
            .collect();
        ModelConfig {
            name: "migration-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers,
            inputs: Vec::new(),
        }
    }

    fn resolved_for(model: &ModelConfig, provider_index: usize) -> oulipoly_state::ResolvedResume {
        let provider = &model.providers[provider_index];
        oulipoly_state::ResolvedResume {
            chain_id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string(),
            model_name: Some(model.name.clone()),
            model: Some(model.clone()),
            active_provider: provider.name.clone(),
            active_session_id: "5169694d-de0f-40d1-890c-6e28e55bab27".to_string(),
        }
    }

    fn drop_quota_table(db: &StateDb) {
        db.drop_provider_quotas_for_test();
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_picks_best_scored_sibling_on_resume() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.83, 50, 0.01, 22)]);
        seed_windows_with_deltas(
            &db,
            "claude2",
            &[(0.19, 24 * 7, 0.01, 22), (0.09, 3, 0.01, 22)],
        );

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 1,
                reason: TransitionReason::QuotaThreshold
            }
        );
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_stays_when_active_is_least_loaded() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.30, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.80, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_ignores_short_window_pressure_on_siblings() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude3", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.83, 50, 0.01, 22)]);
        seed_windows_with_deltas(
            &db,
            "claude3",
            &[(0.19, 24 * 7, 0.01, 22), (0.09, 3, 0.01, 22)],
        );

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 1,
                reason: TransitionReason::QuotaThreshold
            }
        );
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_breaks_ties_by_provider_index() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[
            ("claude", "claude_code"),
            ("claude2", "claude_code"),
            ("claude3", "claude_code"),
        ]);
        seed_windows_with_deltas(&db, "claude", &[(0.30, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.30, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude3", &[(0.90, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 2), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 0,
                reason: TransitionReason::QuotaThreshold
            }
        );
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_migrates_when_exhausted_flag_set() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.20, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.30, 5, 0.01, 22)]);
        db.mark_exhausted("claude").unwrap();

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 1,
                reason: TransitionReason::Exhausted
            }
        );
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_stays_when_single_provider_pool() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.99, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_stays_when_no_sibling_has_session_storage() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "none")]);
        seed_windows_with_deltas(&db, "claude", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.30, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_manual_overrides_best_score() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.50, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.60, 5, 0.01, 22)]);

        let decision =
            decide_migration(&db, &model, &resolved_for(&model, 0), Some("claude2")).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 1,
                reason: TransitionReason::Manual
            }
        );
    }

    // risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #1 / proposal A1, A2, A3.
    #[test]
    fn decide_migration_stays_for_codex_source_in_codex_pool() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("codex", "codex"), ("codex2", "codex")]);
        seed_windows_with_deltas(&db, "codex", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "codex2", &[(0.30, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #2 / proposal A1, A2, A3.
    #[test]
    fn decide_migration_stays_for_codex_source_with_no_storage_pool() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("codex", "none"), ("codex2", "none")]);
        seed_windows_with_deltas(&db, "codex", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "codex2", &[(0.30, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Codex/non-Claude resume migration abort; level: particular-integration; source: AGE-48 contract §Test plan #3 / proposal A1, A2, A3.
    #[test]
    fn decide_migration_stays_for_codex_source_with_claude_target() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("codex", "codex"), ("claude", "claude_code")]);
        seed_windows_with_deltas(&db, "codex", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude", &[(0.30, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Manual migration target eligibility; level: particular-integration; source: AGE-48 contract §Test plan #4 / proposal A1, A2, A3.
    #[test]
    fn decide_migration_stays_for_manual_migrate_to_codex_target() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("codex", "codex")]);

        let decision =
            decide_migration(&db, &model, &resolved_for(&model, 0), Some("codex")).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Exhausted branch reaches non-migratable source; level: particular-integration; source: AGE-48 contract §Test plan #5 / proposal A1, A2, A3.
    #[test]
    fn decide_migration_stays_for_codex_source_when_exhausted() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("codex", "codex"), ("claude", "claude_code")]);
        seed_windows_with_deltas(&db, "codex", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude", &[(0.30, 5, 0.01, 22)]);
        db.mark_exhausted("codex").unwrap();

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: Target filtering preserves load ordering after Codex exclusion; level: particular-integration; source: AGE-48 contract §Test plan #6 / proposal A1, A2, A4.
    #[test]
    fn decide_migration_picks_eligible_target_skipping_codex_target() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[
            ("claude", "claude_code"),
            ("codex", "codex"),
            ("claude2", "claude_code"),
        ]);
        seed_windows_with_deltas(&db, "claude", &[(0.90, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "codex", &[(0.10, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.20, 5, 0.01, 22)]);

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 2,
                reason: TransitionReason::QuotaThreshold
            }
        );
    }

    // risk: Best-on-resume decision; level: particular-integration; source: proposal §11.1 Best-on-resume decision / A2, A4.
    #[test]
    fn decide_migration_reports_projection_state_errors() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        drop_quota_table(&db);

        let err = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap_err();

        assert!(matches!(err, MigrationError::Db { .. }));
    }

    #[test]
    fn decide_migration_stays_when_manual_target_unknown() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);

        // characterization: AGE-48 may edit the manual branch, and unknown manual targets were uncovered.
        let decision = decide_migration(
            &db,
            &model,
            &resolved_for(&model, 0),
            Some("claude-missing"),
        )
        .unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    #[test]
    fn decide_migration_stays_when_manual_target_has_no_session_storage() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "none")]);

        // characterization: AGE-48 may edit the manual branch, and no-storage manual targets were uncovered.
        let decision =
            decide_migration(&db, &model, &resolved_for(&model, 0), Some("claude2")).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    #[test]
    fn decide_migration_stays_when_exhausted_active_has_no_eligible_sibling() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "none")]);
        seed_windows_with_deltas(&db, "claude", &[(0.99, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.30, 5, 0.01, 22)]);
        db.mark_exhausted("claude").unwrap();

        // characterization: AGE-48 may change target eligibility, and exhausted no-target behavior was uncovered.
        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    #[test]
    fn decide_migration_stays_when_active_provider_missing_from_migration_model() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        let mut resolved = resolved_for(&model, 0);
        resolved.active_provider = "archived-claude".to_string();

        // characterization: AGE-48 will read active-provider resolution, and missing-active behavior was uncovered.
        let decision = decide_migration(&db, &model, &resolved, None).unwrap();

        assert_eq!(decision, MigrationDecision::Stay);
    }

    // risk: compute_projections refactor equivalence; level: particular-integration; source: proposal §11.1 compute_projections refactor equivalence / A4.
    #[test]
    fn compute_projections_exposes_window_projection_used_by_selection() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.40, 5, 0.01, 22)]);
        seed_assistant_turns_since_refresh(&db, "claude", 10);

        let projections = compute_projections(&model, &db, None);

        let active = projections
            .iter()
            .find(|projection| projection.provider_index == 0)
            .expect("active provider projection");
        assert_eq!(active.projections_per_window.len(), 1);
        assert!(active.projections_per_window[0].projected_used >= 0.40);
        assert!(active.binding_score.is_some());
    }
}
