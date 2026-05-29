//! Verify-before-honor for `provider_quotas.next_available_at` markers.
//!
//! ## Declared roles
//! orchestration, accessor
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/quota/marker_verification/mod.rs
//!     role: adapter
//!     Translates:
//!       - oulipoly_state marker quota contract (`StateDb`, `QuotaRecord`, `get_quota`, `get_windows`, `clear_provider_unavailable`)
//!       - oulipoly_runtime quota refresh contract (`RefreshOutcome`, `refresh_provider_for_routing`, `InFlight`)
//!       - oulipoly_config provider/session config contract (`ProvidersConfig`, `SessionsConfig`)
//! ```
//!
//! Stale `next_available_at` (the AGE-163 working-set "provider is
//! unavailable until <ts>" marker) survives across a successful `--usage`
//! refresh because `write_quota_aggregate` only clears `exhausted_at`. The
//! routing layer then keeps treating the provider as unrouteable even when
//! every visible window is healthy.
//!
//! This module is the verification facade the balancer consults before
//! honoring such a marker. It keeps orchestration thin and delegates config,
//! health predicates, and file-lock behavior to focused submodules.

mod config;
mod health;
mod lock;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use crate::quota::{InFlight, refresh_provider_for_routing};
use chrono::{DateTime, Utc};
pub use config::release_slack_secs;
use oulipoly_config::{ProvidersConfig, SessionsConfig};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

/// Verify a possibly-stale `next_available_at` marker against the live
/// quota, clearing it when the marker is past its slack window or the
/// refreshed cache shows a healthy provider. No-op when the provider has
/// no marker. Refresh I/O is dedup'd across callers via the per-provider
/// file lock (cross-process) and the existing `InFlight` set (intra-process).
pub fn verify_or_clear_marker(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
    provider_name: &str,
    now: DateTime<Utc>,
) {
    let Some(quota) = read_quota(state, provider_name) else {
        return;
    };
    let Some(next_at) = quota.next_available_at else {
        return;
    };
    if health::marker_within_release_slack(next_at, now, release_slack_secs()) {
        clear_marker(state, provider_name);
        return;
    }
    if health::cached_refresh_is_fresh(quota.refreshed_at, now, config::cooldown_secs()) {
        verify_against_cached_windows(state, provider_name, now);
        return;
    }
    refresh_and_verify_under_lock(
        state,
        providers_cfg,
        sessions_cfg,
        in_flight,
        provider_name,
        now,
    );
}

fn read_quota(state: &StateDb, provider_name: &str) -> Option<QuotaRecord> {
    state.get_quota(provider_name).ok().flatten()
}

fn verify_against_cached_windows(state: &StateDb, provider_name: &str, now: DateTime<Utc>) {
    let windows = read_windows(state, provider_name);
    if health::windows_indicate_provider_healthy(&windows, now) {
        clear_marker(state, provider_name);
    }
}

fn read_windows(state: &StateDb, provider_name: &str) -> Vec<QuotaWindow> {
    state.get_windows(provider_name).unwrap_or_default()
}

fn refresh_and_verify_under_lock(
    state: &StateDb,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    in_flight: &InFlight,
    provider_name: &str,
    now: DateTime<Utc>,
) {
    let Ok(_guard) = lock::RefreshFileLock::acquire_blocking(provider_name) else {
        return;
    };
    if !still_needs_refresh_under_lock(state, provider_name, now) {
        verify_against_cached_windows(state, provider_name, now);
        return;
    }
    let outcome =
        refresh_provider_for_routing(provider_name, providers_cfg, sessions_cfg, in_flight, state);
    if health::refresh_outcome_observes_fresh_or_inflight_data(&outcome) {
        verify_against_cached_windows(state, provider_name, now);
    }
}

fn still_needs_refresh_under_lock(
    state: &StateDb,
    provider_name: &str,
    now: DateTime<Utc>,
) -> bool {
    let refreshed_at = read_quota(state, provider_name).and_then(|quota| quota.refreshed_at);
    health::cached_refresh_is_stale(refreshed_at, now, config::cooldown_secs())
}

fn clear_marker(state: &StateDb, provider_name: &str) {
    if let Err(err) = state.clear_provider_unavailable(provider_name) {
        tracing::warn!(
            provider_name = provider_name,
            error = err.as_str(),
            "failed to clear stale next_available_at marker"
        );
    }
}
