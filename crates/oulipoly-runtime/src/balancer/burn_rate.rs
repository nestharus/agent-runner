//! ## Declared roles
//!
//! `predicate`, `mapper`, `accessor`.

use super::EPS_HOURS;
#[cfg(test)]
use super::snapshot::QuotaSnapshot;
use chrono::Utc;
#[cfg(test)]
use oulipoly_config::ModelConfig;
#[cfg(test)]
use oulipoly_state::StateDb;
use oulipoly_state::{QuotaRecord, QuotaWindow};

pub(super) fn project_used_percent(base_used_percent: f64, turns: u64, burn_rate: f64) -> f64 {
    (base_used_percent + (turns as f64) * burn_rate).max(0.0)
}

pub(super) fn learned_rate(window: &QuotaWindow) -> Option<f64> {
    match (window.last_delta_percent, window.last_delta_calls) {
        (Some(delta_percent), Some(delta_calls)) if delta_percent > 0.0 && delta_calls > 0 => {
            Some(delta_percent / delta_calls as f64)
        }
        _ => None,
    }
}

pub(super) fn bootstrap_burn_rate(
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

pub(super) fn pool_window_avg_percent_per_call(
    window_id: u32,
    windows: &[Vec<QuotaWindow>],
) -> Option<f64> {
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

pub(super) fn duration_ratio_fallback_percent_per_call(
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

pub(super) fn duration_ratio_rate(long_rate: f64, long_hours: f64, target_hours: f64) -> f64 {
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
