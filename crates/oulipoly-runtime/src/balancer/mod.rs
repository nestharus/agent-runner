//! ## Declared roles
//!
//! `orchestration`, `filter`, `predicate`, `mapper`, `accessor`, `formatter`, `validator`.
//!
//! ## Intrinsic-surface declarations
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::quota_routing_surface
//!     role: intrinsic-surface
//!     Domain: quota-routing-refresh-and-topology
//!     Owns:
//!       - InFlight refresh deduplication carrier
//!       - RefreshOutcome ignored/degraded routing result
//!       - has_refresh_source topology-probe eligibility
//!       - is_routing_stale route-selection refresh TTL
//!       - is_stale projection refresh TTL
//!       - is_topology_probe_due topology repair predicate
//!       - refresh_provider projection refresh operation
//!       - refresh_provider_for_routing route-selection refresh operation
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::config_topology_session_contract
//!     role: intrinsic-surface
//!     Domain: balancer-config-topology-and-session-contract
//!     Owns:
//!       - ModelConfig provider pool and model name contract
//!       - ProviderConfig provider name/session storage contract
//!       - ProvidersConfig quota refresh-source contract
//!       - SessionsConfig session scan and adapter-derived refresh contract
//!       - SessionStorage resume migration eligibility contract
//!   - component: crates/oulipoly-runtime/src/balancer/mod.rs::quota_resume_state_surface
//!     role: intrinsic-surface
//!     Domain: quota-resume-state-carriers
//!     Owns:
//!       - StateDb quota/window read operations
//!       - StateDb recent-error and invocation-count read operations
//!       - StateDb reset-implied clear operation
//!       - StateDb topology-probe timestamp operation
//!       - QuotaRecord refreshed/exhausted/topology fields
//!       - QuotaWindow usage/reset/delta fields
//!       - ResolvedResume active-provider contract

use crate::migration::MigrationError;
use crate::quota::{
    InFlight, RefreshOutcome, has_refresh_source, is_routing_stale, is_stale,
    is_topology_probe_due, refresh_provider, refresh_provider_for_routing,
};
use crate::sessions::scan_provider;
use chrono::{DateTime, Utc};
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

struct QuotaSnapshot {
    quotas: Vec<Option<QuotaRecord>>,
    windows: Vec<Vec<QuotaWindow>>,
}

struct TopologyProbeRefresh {
    provider_index: usize,
    live_window_count: usize,
    pool_expected_live_windows: usize,
    topology_peak_live_window_count: usize,
}

pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Result<usize, RoutingError> {
    refresh_routing_inputs(model, state, ctx);
    let mut snapshot = load_quota_snapshot(model, state);
    if let Some(ctx) = ctx {
        repair_routing_topology(model, state, ctx, &mut snapshot);
    }

    let filtered_indices = eligible_provider_indices(model, state, &snapshot, Utc::now());
    if filtered_indices.is_empty() {
        return Err(all_providers_quota_exhausted_error(model));
    }

    Ok(score_routing_candidates(
        model,
        state,
        &snapshot,
        filtered_indices.as_slice(),
    ))
}

fn refresh_routing_inputs(model: &ModelConfig, state: &StateDb, ctx: Option<&BalanceContext<'_>>) {
    if let Some(ctx) = ctx {
        for provider in &model.providers {
            refresh_provider_for_stale_routing(provider, state, ctx);
            scan_provider_for_routing(provider, state, ctx);
        }
    }
}

fn refresh_provider_for_stale_routing(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    if is_routing_stale(state, &provider.name) {
        let _: RefreshOutcome = refresh_provider_for_routing(
            &provider.name,
            ctx.providers_cfg,
            ctx.sessions_cfg,
            ctx.in_flight,
            state,
        );
    }
}

fn scan_provider_for_routing(provider: &ProviderConfig, state: &StateDb, ctx: &BalanceContext<'_>) {
    let _ = scan_provider(&provider.name, ctx.sessions_cfg, state);
}

fn load_quota_snapshot(model: &ModelConfig, state: &StateDb) -> QuotaSnapshot {
    QuotaSnapshot {
        quotas: model
            .providers
            .iter()
            .map(|provider| cached_quota_record(state, &provider.name))
            .collect(),
        windows: model
            .providers
            .iter()
            .map(|provider| cached_quota_windows(state, &provider.name))
            .collect(),
    }
}

fn cached_quota_record(state: &StateDb, provider_name: &str) -> Option<QuotaRecord> {
    state.get_quota(provider_name).ok().flatten()
}

fn cached_quota_windows(state: &StateDb, provider_name: &str) -> Vec<QuotaWindow> {
    state.get_windows(provider_name).unwrap_or_default()
}

fn repair_routing_topology(
    model: &ModelConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
    snapshot: &mut QuotaSnapshot,
) {
    let now = Utc::now();
    let live_window_counts = live_window_counts(snapshot.windows.as_slice(), now);
    let topology_peak_counts = topology_peak_counts(snapshot.quotas.as_slice());
    let pool_expected_live_windows =
        expected_pool_live_window_count(&live_window_counts, &topology_peak_counts);

    for (provider_index, provider) in model.providers.iter().enumerate() {
        if topology_probe_should_refresh(
            state,
            provider,
            ctx,
            live_window_counts[provider_index],
            pool_expected_live_windows,
        ) {
            let probe = TopologyProbeRefresh {
                provider_index,
                live_window_count: live_window_counts[provider_index],
                pool_expected_live_windows,
                topology_peak_live_window_count: topology_peak_counts[provider_index],
            };
            record_topology_probe_and_refresh(model, state, ctx, snapshot, probe);
        }
    }
}

fn live_window_counts(windows: &[Vec<QuotaWindow>], now: chrono::DateTime<Utc>) -> Vec<usize> {
    windows
        .iter()
        .map(|provider_windows| live_window_count(provider_windows, now))
        .collect()
}

fn topology_peak_counts(quotas: &[Option<QuotaRecord>]) -> Vec<usize> {
    quotas
        .iter()
        .map(|quota| {
            quota
                .as_ref()
                .map(|quota| quota.topology_peak_live_window_count)
                .unwrap_or(0)
        })
        .collect()
}

fn expected_pool_live_window_count(
    live_window_counts: &[usize],
    topology_peak_counts: &[usize],
) -> usize {
    live_window_counts
        .iter()
        .zip(topology_peak_counts.iter())
        .map(|(live, peak)| (*live).max(*peak))
        .max()
        .unwrap_or(0)
}

fn topology_probe_should_refresh(
    state: &StateDb,
    provider: &ProviderConfig,
    ctx: &BalanceContext<'_>,
    live_window_count: usize,
    pool_expected_live_windows: usize,
) -> bool {
    has_refresh_source(&provider.name, ctx.providers_cfg, ctx.sessions_cfg)
        && is_topology_probe_due(
            state,
            &provider.name,
            live_window_count,
            pool_expected_live_windows,
        )
}

fn record_topology_probe_and_refresh(
    model: &ModelConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
    snapshot: &mut QuotaSnapshot,
    probe: TopologyProbeRefresh,
) {
    let provider = &model.providers[probe.provider_index];
    let _ = state.record_topology_probe(&provider.name);
    trace_topology_probe(
        provider,
        probe.live_window_count,
        probe.pool_expected_live_windows,
        probe.topology_peak_live_window_count,
    );
    let _: RefreshOutcome = refresh_provider_for_routing(
        &provider.name,
        ctx.providers_cfg,
        ctx.sessions_cfg,
        ctx.in_flight,
        state,
    );
    snapshot.quotas[probe.provider_index] = cached_quota_record(state, &provider.name);
    snapshot.windows[probe.provider_index] = cached_quota_windows(state, &provider.name);
}

fn trace_topology_probe(
    provider: &ProviderConfig,
    live_window_count: usize,
    pool_expected_live_windows: usize,
    topology_peak_live_window_count: usize,
) {
    tracing::info!(
        provider_name = provider.name.as_str(),
        live_window_count = live_window_count,
        pool_expected_live_window_count = pool_expected_live_windows,
        topology_peak_live_window_count = topology_peak_live_window_count,
        "topology probe fired"
    );
}

fn eligible_provider_indices(
    model: &ModelConfig,
    state: &StateDb,
    snapshot: &QuotaSnapshot,
    now: chrono::DateTime<Utc>,
) -> Vec<usize> {
    let all_indices = all_provider_indices(model);
    let reset_implied = reset_implied_flags(&all_indices, snapshot, now);
    clear_reset_implied_flags(state, model, &reset_implied);
    route_eligible_provider_indices(all_indices, snapshot, &reset_implied, now)
}

fn route_eligible_provider_indices(
    provider_indices: Vec<usize>,
    snapshot: &QuotaSnapshot,
    reset_implied: &[bool],
    now: chrono::DateTime<Utc>,
) -> Vec<usize> {
    provider_indices
        .into_iter()
        .filter(|&provider_index| {
            provider_is_route_eligible(provider_index, snapshot, reset_implied, now)
        })
        .collect()
}

fn all_provider_indices(model: &ModelConfig) -> Vec<usize> {
    (0..model.providers.len()).collect()
}

fn reset_implied_flags(
    provider_indices: &[usize],
    snapshot: &QuotaSnapshot,
    now: chrono::DateTime<Utc>,
) -> Vec<bool> {
    provider_indices
        .iter()
        .map(|provider_index| {
            reset_implied(
                snapshot.quotas[*provider_index].as_ref(),
                &snapshot.windows[*provider_index],
                now,
            )
        })
        .collect()
}

fn provider_is_route_eligible(
    provider_index: usize,
    snapshot: &QuotaSnapshot,
    reset_implied: &[bool],
    now: chrono::DateTime<Utc>,
) -> bool {
    !provider_is_quota_exhausted(
        snapshot.quotas[provider_index].as_ref(),
        &snapshot.windows[provider_index],
        now,
    ) || reset_implied[provider_index]
}

fn all_providers_quota_exhausted_error(model: &ModelConfig) -> RoutingError {
    RoutingError::AllProvidersQuotaExhausted {
        model_name: model.name.clone(),
        provider_names: model
            .providers
            .iter()
            .map(|provider| provider.name.clone())
            .collect(),
    }
}

fn score_routing_candidates(
    model: &ModelConfig,
    state: &StateDb,
    snapshot: &QuotaSnapshot,
    candidates: &[usize],
) -> usize {
    if candidates_have_windows(snapshot, candidates) {
        score_by_density(
            model,
            state,
            &snapshot.quotas,
            &snapshot.windows,
            candidates,
        )
    } else {
        score_by_invocation_count(model, state, candidates)
    }
}

fn candidates_have_windows(snapshot: &QuotaSnapshot, candidates: &[usize]) -> bool {
    candidates
        .iter()
        .all(|provider_index| !snapshot.windows[*provider_index].is_empty())
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

pub fn compute_projections(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) -> Vec<ProviderProjection> {
    refresh_projection_inputs(model, state, ctx);
    let snapshot = load_quota_snapshot(model, state);
    let candidates = all_provider_indices(model);
    compute_projections_from_records(
        model,
        state,
        &snapshot.quotas,
        &snapshot.windows,
        &candidates,
        Utc::now(),
    )
}

fn refresh_projection_inputs(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
) {
    if let Some(ctx) = ctx {
        for provider in &model.providers {
            refresh_provider_for_stale_projection(provider, state, ctx);
            scan_provider_for_projection(provider, state, ctx);
        }
    }
}

fn refresh_provider_for_stale_projection(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    if is_stale(state, &provider.name) {
        let _: RefreshOutcome =
            refresh_provider(&provider.name, ctx.providers_cfg, ctx.in_flight, state);
    }
}

fn scan_provider_for_projection(
    provider: &ProviderConfig,
    state: &StateDb,
    ctx: &BalanceContext<'_>,
) {
    let _ = scan_provider(&provider.name, ctx.sessions_cfg, state);
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
    for provider in reset_implied_providers(model, reset_implied) {
        clear_reset_implied_provider(state, provider);
    }
}

fn reset_implied_providers<'a>(
    model: &'a ModelConfig,
    reset_implied: &'a [bool],
) -> impl Iterator<Item = &'a ProviderConfig> {
    model
        .providers
        .iter()
        .zip(reset_implied.iter())
        .filter_map(|(provider, is_reset_implied)| is_reset_implied.then_some(provider))
}

fn clear_reset_implied_provider(state: &StateDb, provider: &ProviderConfig) {
    if let Err(error) = state.clear_exhausted(&provider.name) {
        warn_reset_implied_clear_failed(provider, error.as_str());
    }
}

fn warn_reset_implied_clear_failed(provider: &ProviderConfig, error: &str) {
    tracing::warn!(
        provider_name = provider.name.as_str(),
        error = error,
        "failed to clear reset-implied quota exhaustion flag"
    );
}

fn compute_projections_from_records(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
    now: chrono::DateTime<Utc>,
) -> Vec<ProviderProjection> {
    let pool_max_live_windows = pool_max_live_window_count(windows, candidates, now);
    candidates
        .iter()
        .copied()
        .map(|provider_index| {
            provider_projection_from_records(
                model,
                state,
                quotas,
                windows,
                provider_index,
                pool_max_live_windows,
                now,
            )
        })
        .collect()
}

struct ProjectionAssembly {
    binding_score: f64,
    unlearned: bool,
    scored_window: bool,
    projections: Vec<WindowProjection>,
}

impl ProjectionAssembly {
    fn new() -> Self {
        Self {
            binding_score: f64::INFINITY,
            unlearned: false,
            scored_window: false,
            projections: Vec::new(),
        }
    }
}

fn pool_max_live_window_count(
    windows: &[Vec<QuotaWindow>],
    candidates: &[usize],
    now: chrono::DateTime<Utc>,
) -> usize {
    candidates
        .iter()
        .map(|&provider_index| live_window_count(&windows[provider_index], now))
        .max()
        .unwrap_or(0)
}

fn live_window_count(windows: &[QuotaWindow], now: chrono::DateTime<Utc>) -> usize {
    windows
        .iter()
        .filter(|window| window_is_live(window, now))
        .count()
}

fn window_is_live(window: &QuotaWindow, now: chrono::DateTime<Utc>) -> bool {
    window.resets_at > now
}

fn provider_projection_from_records(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    provider_index: usize,
    pool_max_live_windows: usize,
    now: chrono::DateTime<Utc>,
) -> ProviderProjection {
    let recent_errors = projection_recent_error_count(model, state, provider_index);
    if projection_is_recent_error_suppressed(recent_errors) {
        return suppressed_provider_projection(provider_index, recent_errors);
    }

    let turns = assistant_turns_since_refresh(model, state, quotas, provider_index);
    let assembly = projection_assembly_for_provider(
        provider_index,
        turns,
        quotas,
        windows,
        pool_max_live_windows,
        now,
    );
    assemble_provider_projection(provider_index, recent_errors, assembly)
}

fn projection_recent_error_count(
    model: &ModelConfig,
    state: &StateDb,
    provider_index: usize,
) -> i64 {
    state
        .recent_error_count(
            &model.name,
            &model.providers[provider_index].name,
            ERROR_WINDOW_MINUTES,
        )
        .unwrap_or(0)
}

fn projection_is_recent_error_suppressed(recent_errors: i64) -> bool {
    recent_errors >= ERROR_THRESHOLD as i64
}

fn suppressed_provider_projection(provider_index: usize, recent_errors: i64) -> ProviderProjection {
    ProviderProjection {
        provider_index,
        projections_per_window: Vec::new(),
        binding_score: None,
        recent_error_count: recent_errors as u32,
    }
}

fn assistant_turns_since_refresh(
    model: &ModelConfig,
    state: &StateDb,
    quotas: &[Option<QuotaRecord>],
    provider_index: usize,
) -> u64 {
    quotas[provider_index]
        .as_ref()
        .and_then(|quota| {
            state
                .count_assistant_turns_since(
                    &model.providers[provider_index].name,
                    quota.refreshed_at.as_ref(),
                )
                .ok()
        })
        .unwrap_or(0)
}

fn projection_assembly_for_provider(
    provider_index: usize,
    turns: u64,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    pool_max_live_windows: usize,
    now: chrono::DateTime<Utc>,
) -> ProjectionAssembly {
    let mut assembly = ProjectionAssembly::new();
    apply_hidden_window_penalty(
        &mut assembly,
        &windows[provider_index],
        pool_max_live_windows,
        now,
    );

    for window in live_windows(&windows[provider_index], now) {
        apply_window_projection(
            &mut assembly,
            provider_index,
            window,
            turns,
            quotas,
            windows,
            now,
        );
    }

    assembly
}

fn apply_hidden_window_penalty(
    assembly: &mut ProjectionAssembly,
    windows: &[QuotaWindow],
    pool_max_live_windows: usize,
    now: chrono::DateTime<Utc>,
) {
    if hidden_window_penalty_applies(windows, pool_max_live_windows, now) {
        assembly.binding_score = assembly.binding_score.min(0.0);
        assembly.scored_window = true;
    }
}

fn hidden_window_penalty_applies(
    windows: &[QuotaWindow],
    pool_max_live_windows: usize,
    now: chrono::DateTime<Utc>,
) -> bool {
    live_window_count(windows, now) < pool_max_live_windows
        && any_live_window_near_cap(windows, now)
}

fn any_live_window_near_cap(windows: &[QuotaWindow], now: chrono::DateTime<Utc>) -> bool {
    windows.iter().any(|window| {
        window_is_live(window, now) && window.used_percent >= HIDDEN_WINDOW_PENALTY_THRESHOLD
    })
}

fn live_windows(
    windows: &[QuotaWindow],
    now: chrono::DateTime<Utc>,
) -> impl Iterator<Item = &QuotaWindow> {
    windows
        .iter()
        .filter(move |window| window_is_live(window, now))
}

fn apply_window_projection(
    assembly: &mut ProjectionAssembly,
    provider_index: usize,
    window: &QuotaWindow,
    turns: u64,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    now: chrono::DateTime<Utc>,
) {
    let Some(projected_window) =
        projected_live_window(provider_index, window, turns, quotas, windows, now)
    else {
        assembly.unlearned = true;
        return;
    };
    assembly.binding_score = assembly
        .binding_score
        .min(window_binding_score(&projected_window));
    assembly.scored_window = true;
    assembly.projections.push(projected_window);
}

fn projected_live_window(
    provider_index: usize,
    window: &QuotaWindow,
    turns: u64,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    now: chrono::DateTime<Utc>,
) -> Option<WindowProjection> {
    let burn_rate = bootstrap_burn_rate(provider_index, window, quotas, windows)?;
    let projected = project_used_percent(window.used_percent, turns, burn_rate);
    let hours = window_hours_until_reset(window, now);
    let remaining_headroom = remaining_headroom(projected);
    Some(WindowProjection {
        window_id: window.window_id as i64,
        projected_used: projected,
        hours_until_reset: hours,
        remaining_headroom,
    })
}

fn window_hours_until_reset(window: &QuotaWindow, now: chrono::DateTime<Utc>) -> f64 {
    ((window.resets_at - now).num_seconds() as f64 / 3600.0).max(EPS_HOURS)
}

fn remaining_headroom(projected_used: f64) -> f64 {
    (1.0 - projected_used).max(0.0)
}

fn window_binding_score(window: &WindowProjection) -> f64 {
    window.remaining_headroom * window.hours_until_reset
}

fn assemble_provider_projection(
    provider_index: usize,
    recent_errors: i64,
    assembly: ProjectionAssembly,
) -> ProviderProjection {
    let binding_score = projection_binding_score(&assembly);
    ProviderProjection {
        provider_index,
        projections_per_window: assembly.projections,
        binding_score,
        recent_error_count: recent_errors as u32,
    }
}

fn projection_binding_score(assembly: &ProjectionAssembly) -> Option<f64> {
    (!assembly.unlearned && assembly.scored_window).then_some(assembly.binding_score)
}

pub fn decide_migration(
    state: &StateDb,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    manual_target: Option<&str>,
) -> Result<MigrationDecision, MigrationError> {
    if !model_has_migration_alternative(model) {
        return Ok(MigrationDecision::Stay);
    }

    let Some(active_provider_index) = active_provider_index(model, resolved) else {
        return Ok(MigrationDecision::Stay);
    };
    let active = &model.providers[active_provider_index];

    if let Some(decision) = manual_migration_decision(model, active, manual_target) {
        return Ok(decision);
    }

    if !active_provider_supports_resume_migration(active) {
        return Ok(MigrationDecision::Stay);
    }

    let active_exhausted = active_provider_is_exhausted(state, active)?;
    let projections = compute_projections(model, state, None);

    if active_exhausted {
        return Ok(exhausted_migration_decision(
            model,
            &projections,
            active,
            active_provider_index,
        ));
    }

    Ok(quota_threshold_migration_decision(
        model,
        &projections,
        active,
        active_provider_index,
    ))
}

fn model_has_migration_alternative(model: &ModelConfig) -> bool {
    model.providers.len() > 1
}

fn active_provider_index(model: &ModelConfig, resolved: &ResolvedResume) -> Option<usize> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == resolved.active_provider)
}

fn manual_migration_decision(
    model: &ModelConfig,
    active: &ProviderConfig,
    manual_target: Option<&str>,
) -> Option<MigrationDecision> {
    manual_target.map(|target| {
        manual_target_provider_index(model, active, target)
            .map(manual_migration_to)
            .unwrap_or(MigrationDecision::Stay)
    })
}

fn manual_target_provider_index(
    model: &ModelConfig,
    active: &ProviderConfig,
    target: &str,
) -> Option<usize> {
    model
        .providers
        .iter()
        .position(|provider| provider.name == target)
        .filter(|target_provider_index| {
            is_resume_migratable_pair(active, &model.providers[*target_provider_index])
        })
}

fn manual_migration_to(target_provider_index: usize) -> MigrationDecision {
    MigrationDecision::Migrate {
        target_provider_index,
        reason: TransitionReason::Manual,
    }
}

fn active_provider_supports_resume_migration(active: &ProviderConfig) -> bool {
    is_resume_migratable_pair(active, active)
}

fn active_provider_is_exhausted(
    state: &StateDb,
    active: &ProviderConfig,
) -> Result<bool, MigrationError> {
    let quota = active_provider_quota(state, active)?;
    Ok(quota_is_exhausted(quota.as_ref()))
}

fn active_provider_quota(
    state: &StateDb,
    active: &ProviderConfig,
) -> Result<Option<QuotaRecord>, MigrationError> {
    state
        .get_quota(&active.name)
        .map_err(|message| MigrationError::Db { message })
}

fn quota_is_exhausted(quota: Option<&QuotaRecord>) -> bool {
    quota.and_then(|quota| quota.exhausted_at).is_some()
}

fn exhausted_migration_decision(
    model: &ModelConfig,
    projections: &[ProviderProjection],
    active: &ProviderConfig,
    active_provider_index: usize,
) -> MigrationDecision {
    lowest_load_migration_target(model, projections, active, Some(active_provider_index))
        .map(|target| migration_to(target.provider_index, TransitionReason::Exhausted))
        .unwrap_or(MigrationDecision::Stay)
}

fn quota_threshold_migration_decision(
    model: &ModelConfig,
    projections: &[ProviderProjection],
    active: &ProviderConfig,
    active_provider_index: usize,
) -> MigrationDecision {
    let Some(best) = lowest_load_migration_target(model, projections, active, None) else {
        return MigrationDecision::Stay;
    };
    if best.provider_index == active_provider_index {
        return MigrationDecision::Stay;
    }

    migration_to(best.provider_index, TransitionReason::QuotaThreshold)
}

fn migration_to(target_provider_index: usize, reason: TransitionReason) -> MigrationDecision {
    MigrationDecision::Migrate {
        target_provider_index,
        reason,
    }
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

/// AGE-163 WU-A.1 working-set membership predicate. A provider is in the
/// working set iff its `next_available_at` is null or has elapsed: the
/// post-failure forensics writer sets this column to push a provider out of
/// rotation for a typed cooldown window.
pub fn working_set_member(quota: Option<&QuotaRecord>, now: DateTime<Utc>) -> bool {
    quota
        .and_then(|q| q.next_available_at)
        .map_or(true, |ts| ts <= now)
}

/// AGE-163 WU-A.3 round-robin candidate selection. Walks the model's
/// provider pool starting after the persisted round-robin cursor, filters
/// through `working_set_member`, and returns the first eligible index.
/// `exclude_provider_index` skips a candidate (e.g. the one that just
/// failed). On success, advances the persisted cursor. Returns `Ok(None)`
/// when the working set is exhausted.
pub fn select_next_working_candidate(
    state: &StateDb,
    model: &ModelConfig,
    now: DateTime<Utc>,
    exclude_provider_index: Option<usize>,
) -> Result<Option<usize>, MigrationError> {
    let pool_len = model.providers.len();
    if pool_len == 0 {
        return Ok(None);
    }
    let cursor = state
        .next_round_robin_index_for_model(&model.name)
        .map_err(|message| MigrationError::Db { message })?
        .unwrap_or(usize::MAX);
    let start = if cursor == usize::MAX {
        0
    } else {
        (cursor + 1) % pool_len
    };
    for offset in 0..pool_len {
        let candidate_index = (start + offset) % pool_len;
        if Some(candidate_index) == exclude_provider_index {
            continue;
        }
        let provider = &model.providers[candidate_index];
        let quota = state
            .get_quota(&provider.name)
            .map_err(|message| MigrationError::Db { message })?;
        if working_set_member(quota.as_ref(), now) {
            state
                .advance_round_robin_index(&model.name, candidate_index, now)
                .map_err(|message| MigrationError::Db { message })?;
            return Ok(Some(candidate_index));
        }
    }
    Ok(None)
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
        .filter(|projection| {
            migration_projection_is_eligible(
                model,
                source_provider,
                exclude_provider_index,
                projection,
            )
        })
        .min_by(|a, b| migration_load_order(a, b))
}

fn migration_projection_is_eligible(
    model: &ModelConfig,
    source_provider: &ProviderConfig,
    exclude_provider_index: Option<usize>,
    projection: &ProviderProjection,
) -> bool {
    Some(projection.provider_index) != exclude_provider_index
        && model
            .providers
            .get(projection.provider_index)
            .is_some_and(|candidate| is_resume_migratable_pair(source_provider, candidate))
}

fn migration_load_order(a: &ProviderProjection, b: &ProviderProjection) -> std::cmp::Ordering {
    provider_load(a)
        .partial_cmp(&provider_load(b))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.provider_index.cmp(&b.provider_index))
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

fn approx_eq_usage(a: f64, b: f64) -> bool {
    (a - b).abs() <= f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

fn fanout_usage_key(projection: &ProviderProjection) -> FanoutUsageKey {
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

fn select_binding_score_with_fanout(model: &ModelConfig, eligible: &[ProviderEval]) -> usize {
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
    percent_per_call(pool_window_delta_totals(window_id, windows))
}

fn pool_window_delta_totals(window_id: u32, windows: &[Vec<QuotaWindow>]) -> (f64, u64) {
    let mut total_percent = 0.0;
    let mut total_calls: u64 = 0;
    for window in windows.iter().flatten() {
        if let Some((delta_percent, delta_calls)) = learned_window_delta(window_id, window) {
            total_percent += delta_percent;
            total_calls += delta_calls;
        }
    }
    (total_percent, total_calls)
}

fn learned_window_delta(window_id: u32, window: &QuotaWindow) -> Option<(f64, u64)> {
    if window.window_id != window_id {
        return None;
    }
    valid_delta_pair(window.last_delta_percent, window.last_delta_calls)
}

fn valid_delta_pair(delta_percent: Option<f64>, delta_calls: Option<u64>) -> Option<(f64, u64)> {
    match (delta_percent, delta_calls) {
        (Some(delta_percent), Some(delta_calls)) if delta_percent > 0.0 && delta_calls > 0 => {
            Some((delta_percent, delta_calls))
        }
        _ => None,
    }
}

fn percent_per_call((total_percent, total_calls): (f64, u64)) -> Option<f64> {
    (total_calls > 0).then_some(total_percent / total_calls as f64)
}

fn duration_ratio_fallback_percent_per_call(
    provider_index: usize,
    target_window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
    let target_refreshed_at = refreshed_at_for_provider(quotas, provider_index)?;
    let target_hours = window_duration_hours(target_window, target_refreshed_at);
    longest_learned_duration_candidate(quotas, windows, target_hours)
        .map(|candidate| duration_ratio_candidate_rate(candidate, target_hours))
}

#[derive(Clone, Copy)]
struct DurationRatioCandidate {
    rate: f64,
    hours: f64,
}

fn refreshed_at_for_provider(
    quotas: &[Option<QuotaRecord>],
    provider_index: usize,
) -> Option<&chrono::DateTime<Utc>> {
    quotas
        .get(provider_index)
        .and_then(|quota| quota.as_ref())
        .and_then(|quota| quota.refreshed_at.as_ref())
}

fn window_duration_hours(window: &QuotaWindow, refreshed_at: &chrono::DateTime<Utc>) -> f64 {
    ((window.resets_at - *refreshed_at).num_seconds() as f64 / 3600.0).max(EPS_HOURS)
}

fn longest_learned_duration_candidate(
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    target_hours: f64,
) -> Option<DurationRatioCandidate> {
    learned_duration_candidates(quotas, windows, target_hours)
        .into_iter()
        .fold(None, |best, candidate| {
            if best.is_none_or(|best: DurationRatioCandidate| candidate.hours > best.hours) {
                Some(candidate)
            } else {
                best
            }
        })
}

fn learned_duration_candidates(
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
    target_hours: f64,
) -> Vec<DurationRatioCandidate> {
    windows
        .iter()
        .enumerate()
        .flat_map(|(provider_index, provider_windows)| {
            learned_duration_candidates_for_provider(
                refreshed_at_for_provider(quotas, provider_index),
                provider_windows,
                target_hours,
            )
        })
        .collect()
}

fn learned_duration_candidates_for_provider(
    refreshed_at: Option<&chrono::DateTime<Utc>>,
    provider_windows: &[QuotaWindow],
    target_hours: f64,
) -> Vec<DurationRatioCandidate> {
    let Some(refreshed_at) = refreshed_at else {
        return Vec::new();
    };
    provider_windows
        .iter()
        .filter_map(|window| learned_duration_candidate(window, refreshed_at, target_hours))
        .collect()
}

fn learned_duration_candidate(
    window: &QuotaWindow,
    refreshed_at: &chrono::DateTime<Utc>,
    target_hours: f64,
) -> Option<DurationRatioCandidate> {
    let rate = learned_rate(window)?;
    let hours = window_duration_hours(window, refreshed_at);
    duration_candidate_is_longer_than_target(hours, target_hours)
        .then_some(DurationRatioCandidate { rate, hours })
}

fn duration_candidate_is_longer_than_target(candidate_hours: f64, target_hours: f64) -> bool {
    candidate_hours > target_hours
}

fn duration_ratio_candidate_rate(candidate: DurationRatioCandidate, target_hours: f64) -> f64 {
    duration_ratio_rate(candidate.rate, candidate.hours, target_hours)
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
    let snapshot = quota_snapshot_for_test(model, state);
    let target = test_target_window(&snapshot, provider_index, window_id)?;
    bootstrap_burn_rate(provider_index, target, &snapshot.quotas, &snapshot.windows)
}

#[cfg(test)]
fn quota_snapshot_for_test(model: &ModelConfig, state: &StateDb) -> QuotaSnapshot {
    QuotaSnapshot {
        quotas: test_cached_quotas(model, state),
        windows: test_cached_windows(model, state),
    }
}

#[cfg(test)]
fn test_cached_quotas(model: &ModelConfig, state: &StateDb) -> Vec<Option<QuotaRecord>> {
    model
        .providers
        .iter()
        .map(|provider| state.get_quota(&provider.name).ok().flatten())
        .collect()
}

#[cfg(test)]
fn test_cached_windows(model: &ModelConfig, state: &StateDb) -> Vec<Vec<QuotaWindow>> {
    model
        .providers
        .iter()
        .map(|provider| state.get_windows(&provider.name).unwrap_or_default())
        .collect()
}

#[cfg(test)]
fn test_target_window(
    snapshot: &QuotaSnapshot,
    provider_index: usize,
    window_id: u32,
) -> Option<&QuotaWindow> {
    snapshot
        .windows
        .get(provider_index)?
        .iter()
        .find(|window| window.window_id == window_id)
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

fn round_robin_fallback(model: &ModelConfig, state: &StateDb, candidates: &[usize]) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, SecondsFormat, Utc};
    use oulipoly_config::{
        ProviderConfig, ProviderEntry, ProvidersConfig, SessionSourceEntry, SessionsConfig,
        model::PromptMode,
    };
    use rusqlite::Connection;
    use std::path::{Path, PathBuf};
    use uuid::Uuid;

    fn record_invocation_for_test(
        db: &StateDb,
        model_name: &str,
        provider_name: &str,
        provider_index: usize,
        success: bool,
    ) {
        let start = invocation_start_for_test(model_name, provider_name, provider_index);
        let id = db.start_invocation(&start).unwrap();
        db.finalize_invocation(id, success, if success { 0 } else { 1 }, None, None)
            .unwrap();
    }

    fn quota_record_with_next_available_at(
        next_available_at: Option<DateTime<Utc>>,
    ) -> QuotaRecord {
        QuotaRecord {
            provider_name: "p".to_string(),
            calls_since_refresh: 0,
            refreshed_at: None,
            exhausted_at: None,
            topology_peak_live_window_count: 0,
            last_topology_probe_at: None,
            next_available_at,
            last_refresh_at: None,
            failure_class: None,
        }
    }

    #[test]
    fn working_set_member_true_when_next_available_at_null() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(None);
        assert!(working_set_member(Some(&q), now));
        assert!(working_set_member(None, now));
    }

    #[test]
    fn working_set_member_true_when_next_available_at_past() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(Some(now - Duration::hours(1)));
        assert!(working_set_member(Some(&q), now));
    }

    #[test]
    fn working_set_member_false_when_next_available_at_future() {
        let now = Utc::now();
        let q = quota_record_with_next_available_at(Some(now + Duration::hours(1)));
        assert!(!working_set_member(Some(&q), now));
    }

    fn working_set_model(provider_names: &[&str]) -> ModelConfig {
        ModelConfig {
            name: "working-set-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: provider_names
                .iter()
                .map(|name| ProviderConfig {
                    name: (*name).to_string(),
                    command: (*name).to_string(),
                    args: Vec::new(),
                    interactive_args: Some(vec!["launch".to_string()]),
                    resume: None,
                    session_capture: None,
                    resume_acceptance: None,
                    session_storage: Some(SessionStorage::ClaudeCode {
                        projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
                    }),
                    system_prompt_override: None,
                    tool_restrictions: None,
                    invocation_mode: Default::default(),
                })
                .collect(),
            inputs: Vec::new(),
        }
    }

    #[test]
    fn select_next_working_candidate_round_robins_through_working_set() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b", "c"]);
        let now = Utc::now();

        let first = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(first, Some(0));

        let second = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(second, Some(1));

        let third = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(third, Some(2));

        let fourth = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(fourth, Some(0), "cursor wraps around the pool");
    }

    #[test]
    fn select_next_working_candidate_skips_exclude_index() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b"]);
        let now = Utc::now();

        let picked = select_next_working_candidate(&db, &model, now, Some(0)).unwrap();
        assert_eq!(picked, Some(1));
    }

    #[test]
    fn select_next_working_candidate_returns_none_when_all_unavailable() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = working_set_model(&["a", "b"]);
        let now = Utc::now();
        let future = now + Duration::hours(1);
        db.record_provider_unavailable("a", Some(future), "RollingWindow5h")
            .unwrap();
        db.record_provider_unavailable("b", Some(future), "RollingWindow5h")
            .unwrap();

        let picked = select_next_working_candidate(&db, &model, now, None).unwrap();
        assert_eq!(picked, None);
    }

    fn invocation_start_for_test(
        model_name: &str,
        provider_name: &str,
        provider_index: usize,
    ) -> oulipoly_state::InvocationStart {
        oulipoly_state::InvocationStart {
            invocation_uuid: Uuid::new_v4().to_string(),
            model_name: model_name.to_string(),
            provider_name: provider_name.to_string(),
            provider_index,
            parent_invocation_id: None,
        }
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

    fn sessions_config_with_scripts(scripts: &[(&str, &str)]) -> SessionsConfig {
        let entries = scripts
            .iter()
            .map(|(provider_name, script)| {
                (
                    (*provider_name).to_string(),
                    SessionSourceEntry {
                        turn_script: (*script).to_string(),
                        transcript_locator: None,
                        state_dir: None,
                    },
                )
            })
            .collect();
        SessionsConfig { entries }
    }

    fn file_backed_state(label: &str) -> (tempfile::TempDir, PathBuf, StateDb) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{label}.db"));
        let db = StateDb::open(&path).unwrap();
        (dir, path, db)
    }

    fn drop_table(path: &Path, table: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(&drop_table_sql(table)).unwrap();
    }

    fn drop_table_sql(table: &str) -> String {
        format!("DROP TABLE {table};")
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
        let inputs = quota_window_inputs(windows);
        db.upsert_quota_refresh(provider_name, &inputs).unwrap();
        seed_window_deltas(db, provider_name, windows);
    }

    fn quota_window_inputs(
        windows: &[(f64, i64, f64, u64)],
    ) -> Vec<oulipoly_state::QuotaWindowInput> {
        windows
            .iter()
            .map(|(used, hours, _, _)| quota_window(*used, *hours))
            .collect()
    }

    fn seed_window_deltas(db: &StateDb, provider_name: &str, windows: &[(f64, i64, f64, u64)]) {
        for (window_id, window) in windows.iter().enumerate() {
            seed_window_delta(db, provider_name, window_id as u32, window);
        }
    }

    fn seed_window_delta(
        db: &StateDb,
        provider_name: &str,
        window_id: u32,
        window: &(f64, i64, f64, u64),
    ) {
        db.set_window_delta_for_test(provider_name, window_id, window.2, window.3)
            .unwrap();
    }

    fn quota_record(
        provider_name: &str,
        refreshed_at: Option<chrono::DateTime<Utc>>,
    ) -> QuotaRecord {
        QuotaRecord {
            provider_name: provider_name.to_string(),
            calls_since_refresh: 0,
            refreshed_at,
            exhausted_at: None,
            topology_peak_live_window_count: 0,
            last_topology_probe_at: None,
            next_available_at: None,
            last_refresh_at: None,
            failure_class: None,
        }
    }

    fn quota_window_record(
        provider_name: &str,
        window_id: u32,
        used_percent: f64,
        resets_at: chrono::DateTime<Utc>,
        last_delta_percent: Option<f64>,
        last_delta_calls: Option<u64>,
    ) -> QuotaWindow {
        QuotaWindow {
            provider_name: provider_name.to_string(),
            window_id,
            used_percent,
            resets_at,
            last_delta_percent,
            last_delta_calls,
        }
    }

    fn seed_assistant_turns_since_refresh(db: &StateDb, provider_name: &str, count: usize) {
        use chrono::Duration;

        let refreshed_at = Utc::now() - Duration::hours(1);
        db.set_refreshed_at_for_test(provider_name, &refreshed_at)
            .unwrap();
        let turns = assistant_turns_for_test(provider_name, count, refreshed_at);
        db.ingest_session_turns_batch(provider_name, &turns)
            .unwrap();
    }

    fn assistant_turns_for_test(
        provider_name: &str,
        count: usize,
        refreshed_at: chrono::DateTime<Utc>,
    ) -> Vec<oulipoly_state::SessionTurnIngest> {
        (0..count)
            .map(|i| assistant_turn_for_test(provider_name, i, refreshed_at))
            .collect()
    }

    fn assistant_turn_for_test(
        provider_name: &str,
        index: usize,
        refreshed_at: chrono::DateTime<Utc>,
    ) -> oulipoly_state::SessionTurnIngest {
        use chrono::Duration;

        oulipoly_state::SessionTurnIngest {
            session_id: format!("{provider_name}-session"),
            turn_id: format!("{provider_name}-turn-{index}"),
            timestamp: refreshed_at + Duration::seconds((index + 1) as i64),
            role: "assistant".to_string(),
            parent_turn_id: None,
            is_sidechain: false,
            is_compaction_boundary: false,
            body: None,
        }
    }

    fn production_source() -> &'static str {
        include_str!("mod.rs")
            .split("mod tests")
            .next()
            .expect("production source precedes tests")
    }

    #[test]
    fn age153_source_guard_balancer_has_no_terminal_signal_or_provider_output_authority() {
        let source = source_without_comments(production_source());
        for forbidden in ["TerminalSignal", "TerminalSignalKind", "terminal_signal"] {
            assert!(
                !contains_identifier_token(&source, forbidden),
                "balancer must not reference terminal-signal identifier token {forbidden:?}; AGE-153 routing authority is provider_quotas.exhausted_at"
            );
        }
        assert!(
            !contains_terminal_signal_use_import(&source),
            "balancer must not import terminal_signal modules or TerminalSignal types"
        );
        assert!(
            !contains_provider_output_parser_identifier(&source),
            "balancer must not call provider-output parser functions as routing authority"
        );
    }

    fn contains_identifier_token(source: &str, token: &str) -> bool {
        identifier_tokens(source).any(|identifier| identifier == token)
    }

    fn contains_terminal_signal_use_import(source: &str) -> bool {
        source.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("use ")
                && (trimmed.contains("terminal_signal") || trimmed.contains("TerminalSignal"))
        })
    }

    fn contains_provider_output_parser_identifier(source: &str) -> bool {
        identifier_tokens(source).any(is_provider_output_parser_identifier)
    }

    fn is_provider_output_parser_identifier(identifier: &str) -> bool {
        identifier == "parse_provider_output"
            || identifier.starts_with("parse_terminal_status_from_")
            || identifier.starts_with("provider_recognizer_for_")
            || ((identifier.starts_with("parse_") || identifier.starts_with("recognize_"))
                && ["stdout", "stderr", "stream", "output"]
                    .iter()
                    .any(|needle| identifier.contains(needle)))
    }

    fn identifier_tokens(source: &str) -> impl Iterator<Item = &str> {
        source
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .filter(|token| !token.is_empty())
    }

    fn source_without_comments(source: &str) -> String {
        let mut output = String::with_capacity(source.len());
        let mut chars = source.chars().peekable();
        let mut in_block_comment = false;
        while let Some(ch) = chars.next() {
            if in_block_comment {
                if ch == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block_comment = false;
                }
                continue;
            }
            if ch == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
                continue;
            }
            if ch == '/' && chars.peek() == Some(&'/') {
                for next in chars.by_ref() {
                    if next == '\n' {
                        output.push('\n');
                        break;
                    }
                }
                continue;
            }
            output.push(ch);
        }
        output
    }

    #[test]
    fn age153_decide_migration_observes_exhausted_at_without_terminal_signal_dependency() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[
            ("claude-age153-a", "claude_code"),
            ("claude-age153-b", "claude_code"),
        ]);
        seed_windows_with_deltas(&db, "claude-age153-a", &[(0.20, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude-age153-b", &[(0.30, 5, 0.01, 22)]);
        db.mark_exhausted("claude-age153-a").unwrap();

        let decision = decide_migration(&db, &model, &resolved_for(&model, 0), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 1,
                reason: TransitionReason::Exhausted,
            }
        );
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
    fn empty_model_reports_all_providers_exhausted_with_empty_display_list() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = ModelConfig {
            name: "empty".to_string(),
            prompt_mode: PromptMode::Arg,
            providers: vec![],
            inputs: vec![],
        };

        let err = select_provider(&model, &db, None).unwrap_err();

        assert_eq!(
            err,
            RoutingError::AllProvidersQuotaExhausted {
                model_name: "empty".to_string(),
                provider_names: vec![],
            }
        );
        assert_eq!(
            err.to_string(),
            "all providers in pool empty are quota-exhausted: <empty>"
        );
    }

    #[test]
    fn select_provider_ignores_session_scan_errors_and_uses_stale_turn_counts() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.50, 1)]);
        seed_windows_with_deltas(&db, "b", &[(0.40, 24, 0.01, 1)]);
        let providers_cfg = ProvidersConfig::default();
        let sessions_cfg = sessions_config_with_scripts(&[(
            "a",
            "printf '%s\n' '{\"session_id\":\"a-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2099-01-01T00:00:00Z\",\"role\":\"assistant\"}'; exit 1",
        )]);
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(
            selected, 0,
            "failed session scans should leave provider a's stale zero-turn projection in place"
        );
        assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 0);
    }

    #[test]
    fn topology_probe_skips_providers_without_refresh_source() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        seed_windows_with_deltas(&db, "a", &[(0.02, 24 * 7, 0.01, 40)]);
        seed_windows_with_deltas(&db, "b", &[(0.66, 80, 0.01, 40), (0.16, 3, 0.01, 40)]);
        let providers_cfg =
            providers_config_with_scripts(&[("b", "printf '%s' '{\"windows\":[]}'")]);
        let sessions_cfg = SessionsConfig::default();
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let selected = select_provider(&model, &db, Some(&ctx)).unwrap();

        assert_eq!(selected, 0);
        assert_eq!(
            db.get_windows("a").unwrap().len(),
            1,
            "provider a has incomplete topology but no refresh source, so the probe must not refresh it"
        );
        assert!(
            db.get_quota("a")
                .unwrap()
                .unwrap()
                .last_topology_probe_at
                .is_none(),
            "skipped topology probes must not stamp last_topology_probe_at"
        );
    }

    #[test]
    fn select_provider_treats_quota_and_window_read_errors_as_empty_cache() {
        let (_dir, path, db) = file_backed_state("select-read-errors");
        let model = two_provider_model();
        record_invocation_for_test(&db, &model.name, "a", 0, true);
        drop_table(&path, "provider_quotas");
        drop_table(&path, "provider_quota_windows");

        let selected = select_provider(&model, &db, None).unwrap();

        assert_eq!(
            selected, 1,
            "quota/window read failures should degrade to empty cache and use invocation-count fallback"
        );
    }

    #[test]
    fn clear_reset_implied_flags_does_not_abort_when_clear_fails() {
        let (_dir, path, db) = file_backed_state("clear-reset-fails");
        let model = single_provider_model();
        seed_windows_with_deltas(&db, "a", &[(1.0, -1, 0.01, 22)]);
        db.mark_exhausted("a").unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_clear_exhausted
             BEFORE UPDATE OF exhausted_at ON provider_quotas
             WHEN NEW.exhausted_at IS NULL
             BEGIN
                SELECT RAISE(ABORT, 'clear denied');
             END;",
        )
        .unwrap();

        let selected = select_provider(&model, &db, None).unwrap();

        assert_eq!(selected, 0);
        assert!(
            db.get_quota("a").unwrap().unwrap().exhausted_at.is_some(),
            "failed reset-implied clear is swallowed after warning and leaves the flag intact"
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
    fn compute_projections_with_context_refreshes_stale_quota_and_scans_sessions() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        let resets = (Utc::now() + Duration::hours(48)).to_rfc3339_opts(SecondsFormat::Secs, true);
        let quota_script = format!(
            "printf '%s' '{{\"windows\":[{{\"used_percent\":25,\"resets_at\":\"{resets}\"}}]}}'"
        );
        let providers_cfg = providers_config_with_scripts(&[("a", quota_script.as_str())]);
        let sessions_cfg = sessions_config_with_scripts(&[(
            "a",
            "printf '%s\n' '{\"session_id\":\"a-session\",\"turn_id\":\"turn-1\",\"timestamp\":\"2099-01-01T00:00:00Z\",\"role\":\"assistant\"}'",
        )]);
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let projections = compute_projections(&model, &db, Some(&ctx));

        assert_eq!(db.get_windows("a").unwrap().len(), 1);
        assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 1);
        assert_eq!(
            projections
                .iter()
                .find(|projection| projection.provider_index == 0)
                .unwrap()
                .projections_per_window
                .len(),
            0,
            "newly refreshed windows have no learned burn rate yet"
        );
    }

    #[test]
    fn compute_projections_with_context_swallows_refresh_and_scan_failures() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        seed_windows_with_deltas(&db, "a", &[(0.20, 24, 0.01, 10)]);
        db.set_refreshed_at_for_test("a", &(Utc::now() - Duration::days(30)))
            .unwrap();
        let providers_cfg = providers_config_with_scripts(&[("a", "exit 1")]);
        let sessions_cfg = sessions_config_with_scripts(&[("a", "exit 1")]);
        let in_flight = InFlight::new();
        let ctx = BalanceContext {
            providers_cfg: &providers_cfg,
            sessions_cfg: &sessions_cfg,
            in_flight: &in_flight,
        };

        let projections = compute_projections(&model, &db, Some(&ctx));

        let projection = projections
            .iter()
            .find(|projection| projection.provider_index == 0)
            .unwrap();
        assert_eq!(projection.projections_per_window.len(), 1);
        assert!(projection.binding_score.is_some());
        assert_eq!(db.count_assistant_turns_since("a", None).unwrap(), 0);
        assert_approx(db.get_windows("a").unwrap()[0].used_percent, 0.20, 1e-12);
    }

    #[test]
    fn compute_projections_suppresses_recent_error_provider_with_windows() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.01, 10)]);
        seed_windows_with_deltas(&db, "b", &[(0.20, 24, 0.01, 10)]);
        for _ in 0..3 {
            record_invocation_for_test(&db, &model.name, "a", 0, false);
        }

        let projections = compute_projections(&model, &db, None);
        let suppressed = projections
            .iter()
            .find(|projection| projection.provider_index == 0)
            .unwrap();
        let healthy = projections
            .iter()
            .find(|projection| projection.provider_index == 1)
            .unwrap();

        assert_eq!(suppressed.recent_error_count, 3);
        assert_eq!(suppressed.projections_per_window, Vec::new());
        assert_eq!(suppressed.binding_score, None);
        assert!(healthy.binding_score.is_some());
    }

    #[test]
    fn compute_projections_treats_turn_count_read_errors_as_zero_turns() {
        let (_dir, path, db) = file_backed_state("turn-count-errors");
        let model = two_provider_model();
        seed_windows_with_deltas(&db, "a", &[(0.10, 24, 0.01, 1)]);
        seed_assistant_turns_since_refresh(&db, "a", 10);
        drop_table(&path, "session_turns");

        let projections = compute_projections(&model, &db, None);

        let projection = projections
            .iter()
            .find(|projection| projection.provider_index == 0)
            .unwrap();
        assert_approx(
            projection.projections_per_window[0].projected_used,
            0.10,
            1e-12,
        );
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
    fn duration_ratio_rate_uses_eps_guard_for_zero_or_negative_target_hours() {
        let long_rate = 0.01;
        let expected = long_rate * (2.0 / EPS_HOURS);

        assert_approx(duration_ratio_rate(long_rate, 2.0, 0.0), expected, 1e-12);
        assert_approx(duration_ratio_rate(long_rate, 2.0, -4.0), expected, 1e-12);
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
    fn provider_load_falls_back_to_zero_without_finite_window_projection() {
        let projection = ProviderProjection {
            provider_index: 0,
            projections_per_window: vec![
                WindowProjection {
                    window_id: 0,
                    projected_used: f64::NAN,
                    hours_until_reset: 2.0,
                    remaining_headroom: 0.0,
                },
                WindowProjection {
                    window_id: 1,
                    projected_used: f64::INFINITY,
                    hours_until_reset: 4.0,
                    remaining_headroom: 0.0,
                },
            ],
            binding_score: Some(1.0),
            recent_error_count: 0,
        };

        assert_eq!(provider_load(&projection), 0.0);
    }

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

    #[test]
    fn score_by_invocation_count_all_error_suppressed_candidates_uses_round_robin() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = two_provider_model();
        for _ in 0..3 {
            record_invocation_for_test(&db, &model.name, "a", 0, false);
            record_invocation_for_test(&db, &model.name, "b", 1, false);
        }
        for _ in 0..2 {
            record_invocation_for_test(&db, &model.name, "a", 0, true);
        }

        let selected = select_provider(&model, &db, None).unwrap();

        assert_eq!(
            selected, 1,
            "when all fallback candidates are error-suppressed, round-robin falls back to lower invocation count"
        );
    }

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
            .map(|(name, storage_kind)| migratable_provider(name, storage_kind))
            .collect();
        ModelConfig {
            name: "migration-fixture".to_string(),
            prompt_mode: PromptMode::Arg,
            providers,
            inputs: Vec::new(),
        }
    }

    fn migratable_provider(name: &str, storage_kind: &str) -> oulipoly_config::ProviderConfig {
        oulipoly_config::ProviderConfig {
            name: name.to_string(),
            command: name.to_string(),
            args: Vec::new(),
            interactive_args: Some(vec!["launch".to_string()]),
            resume: Some(resume_strategy_for_test()),
            session_capture: None,
            resume_acceptance: None,
            session_storage: session_storage_for_test(name, storage_kind),
            system_prompt_override: None,
            tool_restrictions: None,
            invocation_mode: Default::default(),
        }
    }

    fn resume_strategy_for_test() -> oulipoly_config::ResumeStrategy {
        oulipoly_config::ResumeStrategy {
            kind: oulipoly_config::ResumeKind::Flag,
            flag: Some("--resume".to_string()),
            subcommand: None,
        }
    }

    fn session_storage_for_test(
        name: &str,
        storage_kind: &str,
    ) -> Option<oulipoly_config::SessionStorage> {
        match storage_kind {
            "claude_code" => Some(claude_code_storage_for_test(name)),
            "codex" => Some(codex_storage_for_test(name)),
            "none" => None,
            other => panic!("unknown storage kind fixture {other}"),
        }
    }

    fn claude_code_storage_for_test(name: &str) -> oulipoly_config::SessionStorage {
        oulipoly_config::SessionStorage::ClaudeCode {
            projects_dir: PathBuf::from(format!("/tmp/{name}/projects")),
        }
    }

    fn codex_storage_for_test(name: &str) -> oulipoly_config::SessionStorage {
        oulipoly_config::SessionStorage::Codex {
            sessions_dir: PathBuf::from(format!("/tmp/{name}/sessions")),
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

    #[test]
    fn decide_migration_stays_for_manual_codex_source_to_named_claude_target() {
        let db = StateDb::open(Path::new(":memory:")).unwrap();
        let model = migratable_model(&[("codex", "codex"), ("claude", "claude_code")]);

        let decision =
            decide_migration(&db, &model, &resolved_for(&model, 0), Some("claude")).unwrap();

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
    fn decide_migration_degrades_when_projection_window_reads_fail_after_active_quota_lookup() {
        let (_dir, path, db) = file_backed_state("migration-projection-degrades");
        let model = migratable_model(&[("claude", "claude_code"), ("claude2", "claude_code")]);
        seed_windows_with_deltas(&db, "claude", &[(0.80, 5, 0.01, 22)]);
        seed_windows_with_deltas(&db, "claude2", &[(0.20, 5, 0.01, 22)]);
        drop_table(&path, "provider_quota_windows");

        let decision = decide_migration(&db, &model, &resolved_for(&model, 1), None).unwrap();

        assert_eq!(
            decision,
            MigrationDecision::Migrate {
                target_provider_index: 0,
                reason: TransitionReason::QuotaThreshold,
            },
            "active quota lookup succeeds, then projection window reads degrade to zero-load tie-breaking"
        );
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
