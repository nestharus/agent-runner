//! Per-provider quota refresh. Runs a user-supplied script (from
//! `providers.toml`) that hits the provider's usage API and prints JSON on
//! stdout. The parsed reading lands in `provider_quotas` + `provider_quota_windows`.

mod adapter_derived_source;
mod in_flight;
pub mod marker_verification;
mod outcome;
mod parse;
mod process;
mod refresh;
mod source;

pub use in_flight::{InFlight, InFlightGuard};
pub use marker_verification::verify_or_clear_marker;
pub use outcome::{QuotaScriptWindow, RefreshOutcome};
pub use parse::parse_output;
pub use process::{run_refresh_command, run_script};
pub use refresh::{RuntimeQuotaService, refresh_provider, refresh_provider_for_routing};
pub use source::has_refresh_source;

use chrono::Utc;
#[cfg(test)]
use oulipoly_state::QuotaWindowInput;
use oulipoly_state::StateDb;

/// Minimum refresh TTL. Below 5 minutes we burn API calls without useful
/// signal change; the density projection already catches short-term spikes.
const MIN_TTL_SECS: i64 = 5 * 60;
/// Maximum refresh TTL. We never go longer than 24h even if every window is
/// long — covers the edge case where a script emits no resets_at.
const MAX_TTL_SECS: i64 = 24 * 3600;
/// Denominator for dynamic TTL: refresh N times per window lifetime.
const REFRESH_WINDOW_DIVISOR: i64 = 5;
pub const TOPOLOGY_PROBE_COOLDOWN_SECS: u64 = 60 * 60;

/// Routing needs fresher data than the long-lived dashboard/projection cache.
/// This keeps a single burst of dispatches from hammering provider APIs while
/// still correcting stale account availability before each routing decision.
const ROUTING_REFRESH_TTL_SECS: i64 = 30;

/// True if the provider has no cached quota OR its oldest refresh is past
/// the dynamic TTL computed from its window lengths. TTL is
/// `min(hours_until_reset) / DIVISOR`, clamped to `[MIN_TTL, MAX_TTL]`.
/// A provider row with zero windows is inconsistent state; force stale.
pub fn is_stale(state: &StateDb, provider_name: &str) -> bool {
    let Ok(Some(q)) = state.get_quota(provider_name) else {
        return true;
    };
    let Some(refreshed_at) = q.refreshed_at else {
        return true;
    };
    let windows = state.get_windows(provider_name).unwrap_or_default();
    if windows.is_empty() {
        return true;
    }
    let ttl_secs = dynamic_ttl_secs(&windows);
    let age_secs = (Utc::now() - refreshed_at).num_seconds();
    age_secs >= ttl_secs
}

/// True when the quota cache is too old for a routing decision. This is
/// intentionally shorter than `is_stale`'s projection/dashboard TTL.
pub fn is_routing_stale(state: &StateDb, provider_name: &str) -> bool {
    let Ok(Some(q)) = state.get_quota(provider_name) else {
        return true;
    };
    let Some(refreshed_at) = q.refreshed_at else {
        return true;
    };
    if state
        .get_windows(provider_name)
        .map(|windows| windows.is_empty())
        .unwrap_or(true)
    {
        return true;
    }
    (Utc::now() - refreshed_at).num_seconds() >= ROUTING_REFRESH_TTL_SECS
}

pub fn is_topology_probe_due(
    state: &StateDb,
    provider_name: &str,
    live_window_count: usize,
    pool_expected_live_windows: usize,
) -> bool {
    if live_window_count == 0 || live_window_count >= pool_expected_live_windows {
        return false;
    }

    let Ok(Some(q)) = state.get_quota(provider_name) else {
        return true;
    };
    let Some(last_probe_at) = q.last_topology_probe_at else {
        return true;
    };

    (Utc::now() - last_probe_at).num_seconds() >= TOPOLOGY_PROBE_COOLDOWN_SECS as i64
}

/// Compute the refresh TTL for a provider based on its reported windows.
/// If no windows are present (first-time fetch for the provider), fall back
/// to MAX_TTL — we want some signal before burning API calls.
pub fn dynamic_ttl_secs(windows: &[oulipoly_state::QuotaWindow]) -> i64 {
    if windows.is_empty() {
        return MAX_TTL_SECS;
    }
    let now = Utc::now();
    let min_hours = windows
        .iter()
        .map(|w| (w.resets_at - now).num_seconds().max(0))
        .min()
        .unwrap_or(MAX_TTL_SECS);
    (min_hours / REFRESH_WINDOW_DIVISOR).clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

// Keep for tests that want to model "short" vs "long" windows by constructing
// synthetic resets_at values.
#[cfg(test)]
fn hours_from_now(h: i64) -> chrono::DateTime<Utc> {
    Utc::now() + chrono::Duration::hours(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stale_forces_refresh_when_windows_empty() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        state
            .insert_quota_row_without_windows_for_test("p", &Utc::now())
            .unwrap();

        assert!(is_stale(&state, "p"));
    }

    #[test]
    fn is_stale_honors_ttl_when_windows_present() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        let window = QuotaWindowInput {
            used_percent: 0.10,
            resets_at: hours_from_now(24),
        };
        state.upsert_quota_refresh("p", &[window]).unwrap();

        assert!(!is_stale(&state, "p"));
    }

    #[test]
    fn is_stale_treats_missing_quota_row_as_stale() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();

        assert!(is_stale(&state, "p"));
    }

    #[test]
    fn routing_stale_uses_short_thirty_second_ttl() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        state
            .upsert_quota_refresh(
                "p",
                &[QuotaWindowInput {
                    used_percent: 0.10,
                    resets_at: hours_from_now(48),
                }],
            )
            .unwrap();

        state
            .set_refreshed_at_for_test("p", &(Utc::now() - chrono::Duration::seconds(29)))
            .unwrap();
        assert!(!is_routing_stale(&state, "p"));

        state
            .set_refreshed_at_for_test("p", &(Utc::now() - chrono::Duration::seconds(31)))
            .unwrap();
        assert!(is_routing_stale(&state, "p"));
        assert!(
            !is_stale(&state, "p"),
            "dynamic projection TTL should remain longer than routing freshness TTL"
        );
    }

    #[test]
    fn routing_stale_forces_missing_or_empty_quota_refresh() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        assert!(is_routing_stale(&state, "missing"));

        state
            .insert_quota_row_without_windows_for_test("p", &Utc::now())
            .unwrap();
        assert!(is_routing_stale(&state, "p"));
    }

    #[test]
    fn ttl_shrinks_for_short_windows() {
        use oulipoly_state::QuotaWindow;
        let five_hour = QuotaWindow {
            provider_name: "x".into(),
            window_id: 0,
            used_percent: 0.0,
            resets_at: hours_from_now(5),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        let seven_day = QuotaWindow {
            provider_name: "x".into(),
            window_id: 1,
            used_percent: 0.0,
            resets_at: hours_from_now(24 * 7),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        // min(5h, 168h) / 5 ≈ 1h, clamped within [5min, 24h] → 1h
        let ttl = dynamic_ttl_secs(&[five_hour, seven_day]);
        assert!((3500..=3700).contains(&ttl), "expected ~1h, got {ttl}s");
    }

    #[test]
    fn ttl_clamps_to_min_for_nearly_expired_windows() {
        use oulipoly_state::QuotaWindow;
        let near_reset = QuotaWindow {
            provider_name: "x".into(),
            window_id: 0,
            used_percent: 0.0,
            resets_at: Utc::now() + chrono::Duration::seconds(10),
            last_delta_percent: None,
            last_delta_calls: None,
        };
        let ttl = dynamic_ttl_secs(&[near_reset]);
        assert_eq!(ttl, MIN_TTL_SECS);
    }

    #[test]
    fn ttl_empty_windows_falls_back_to_max() {
        assert_eq!(dynamic_ttl_secs(&[]), MAX_TTL_SECS);
    }

    /// Risk: Topology stale helper may fail to repair legacy one-window rows.
    /// Level: unit.
    /// Source: proposal §Test-intent track row 8; Assumptions A2, A6.
    #[test]
    fn topology_probe_due_when_below_expected_and_no_probe_timestamp() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        state
            .upsert_quota_refresh(
                "p",
                &[QuotaWindowInput {
                    used_percent: 0.10,
                    resets_at: hours_from_now(24),
                }],
            )
            .unwrap();

        assert!(is_topology_probe_due(&state, "p", 1, 2));
    }

    /// Risk: Topology helper could over-refresh complete or recently-probed providers.
    /// Level: unit.
    /// Source: proposal §Test-intent track row 9; Assumptions A2, A6.
    #[test]
    fn topology_probe_not_due_when_counts_match_or_cooldown_active() {
        let state = StateDb::open(std::path::Path::new(":memory:")).unwrap();
        state
            .upsert_quota_refresh(
                "p",
                &[QuotaWindowInput {
                    used_percent: 0.10,
                    resets_at: hours_from_now(24),
                }],
            )
            .unwrap();

        assert!(
            !is_topology_probe_due(&state, "p", 1, 1),
            "matching topology must not probe"
        );
        assert!(
            !is_topology_probe_due(&state, "p", 0, 2),
            "zero-window providers are handled by provider-local stale refresh"
        );

        state.record_topology_probe("p").unwrap();

        assert!(
            !is_topology_probe_due(&state, "p", 1, 2),
            "recent topology probe timestamp must activate cooldown"
        );
    }
}
