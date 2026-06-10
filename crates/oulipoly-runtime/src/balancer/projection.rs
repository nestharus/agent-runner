//! ## Declared roles
//!
//! `orchestration`, `mapper`, `accessor`, `predicate`, `filter`.

mod window;

use super::{
    BalanceContext, ERROR_THRESHOLD, ERROR_WINDOW_MINUTES, HIDDEN_WINDOW_PENALTY_THRESHOLD,
    burn_rate::{bootstrap_burn_rate, project_used_percent},
    eligibility::all_provider_indices,
    refresh_inputs::refresh_projection_inputs,
    snapshot::load_quota_snapshot,
};
use chrono::Utc;
use oulipoly_config::ModelConfig;
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};
pub(super) use window::live_window_count;
use window::{
    live_windows, pool_max_live_window_count, remaining_headroom, window_binding_score,
    window_hours_until_reset, window_is_live,
};

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

pub(super) fn compute_projections_from_records(
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
