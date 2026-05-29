//! ## Declared roles
//!
//! `filter`, `predicate`, `mapper`, `accessor`, `orchestration`.
//!
//! ## Component declared roles
//! component_declared_roles:
//!   component: crates/oulipoly-runtime/src/balancer/eligibility.rs
//!   roles:
//!     - filter
//!     - predicate
//!     - mapper
//!     - accessor
//!     - orchestration

use super::snapshot::QuotaSnapshot;
use chrono::{DateTime, Utc};
use oulipoly_config::{ModelConfig, ProviderConfig};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

const EXHAUSTED_USED_PERCENT: f64 = 1.0;

pub(super) fn eligible_provider_indices(
    model: &ModelConfig,
    state: &StateDb,
    snapshot: &QuotaSnapshot,
    now: DateTime<Utc>,
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
    now: DateTime<Utc>,
) -> Vec<usize> {
    provider_indices
        .into_iter()
        .filter(|&provider_index| {
            provider_is_route_eligible(provider_index, snapshot, reset_implied, now)
        })
        .collect()
}

pub(super) fn all_provider_indices(model: &ModelConfig) -> Vec<usize> {
    (0..model.providers.len()).collect()
}

fn reset_implied_flags(
    provider_indices: &[usize],
    snapshot: &QuotaSnapshot,
    now: DateTime<Utc>,
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
    now: DateTime<Utc>,
) -> bool {
    !provider_is_quota_exhausted(
        snapshot.quotas[provider_index].as_ref(),
        &snapshot.windows[provider_index],
        now,
    ) || reset_implied[provider_index]
}

pub(super) fn provider_is_quota_exhausted(
    quota: Option<&QuotaRecord>,
    windows: &[QuotaWindow],
    now: DateTime<Utc>,
) -> bool {
    quota
        .and_then(|quota| quota.exhausted_at.as_ref())
        .is_some()
        || quota
            .and_then(|quota| quota.next_available_at)
            .is_some_and(|ts| marker_blocks_routing(ts, now))
        || windows
            .iter()
            .any(|window| window.resets_at > now && window.used_percent >= EXHAUSTED_USED_PERCENT)
}

/// Honour `next_available_at` only while it's more than the release-slack
/// window in the future. Markers inside the slack window are treated as
/// expired so the next dispatch can probe the provider rather than wait
/// for the exact release timestamp (clock skew + cooldown bookkeeping).
/// Reads `OULIPOLY_MARKER_RELEASE_SLACK_SECS` via the shared helper so
/// cached-only routing (`ctx=None`) matches the verify path's slack.
fn marker_blocks_routing(next_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    next_at
        > now + chrono::Duration::seconds(crate::quota::marker_verification::release_slack_secs())
}

fn reset_implied(quota: Option<&QuotaRecord>, windows: &[QuotaWindow], now: DateTime<Utc>) -> bool {
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
