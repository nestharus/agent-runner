use chrono::{DateTime, Utc};
use oulipoly_state::{QuotaRecord, QuotaWindow, StateDb};

/// Minimum refresh TTL. Below 5 minutes we burn API calls without useful
/// signal change; the density projection already catches short-term spikes.
const MIN_TTL_SECS: i64 = 5 * 60;
/// Maximum refresh TTL. We never go longer than 24h even if every window is
/// long; this covers the edge case where a script emits no resets_at.
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
    let Some(refreshed_at) = refreshed_at_for(state, provider_name) else {
        return true;
    };
    let windows = quota_windows_or_empty(state, provider_name);
    projection_refresh_due(refreshed_at, &windows)
}

/// True when the quota cache is too old for a routing decision. This is
/// intentionally shorter than `is_stale`'s projection/dashboard TTL.
pub fn is_routing_stale(state: &StateDb, provider_name: &str) -> bool {
    let Some(refreshed_at) = refreshed_at_for(state, provider_name) else {
        return true;
    };
    let windows = quota_windows_or_empty(state, provider_name);
    routing_refresh_due(refreshed_at, &windows, Utc::now())
}

pub fn is_topology_probe_due(
    state: &StateDb,
    provider_name: &str,
    live_window_count: usize,
    pool_expected_live_windows: usize,
) -> bool {
    if topology_probe_not_needed(live_window_count, pool_expected_live_windows) {
        return false;
    }

    let Some(last_probe_at) = topology_probe_at_for(state, provider_name) else {
        return true;
    };
    topology_probe_cooldown_elapsed(last_probe_at, Utc::now())
}

/// Compute the refresh TTL for a provider based on its reported windows.
/// If no windows are present (first-time fetch for the provider), fall back
/// to MAX_TTL; we want some signal before burning API calls.
pub fn dynamic_ttl_secs(windows: &[QuotaWindow]) -> i64 {
    let Some(min_reset_secs) = min_seconds_until_reset(windows, Utc::now()) else {
        return MAX_TTL_SECS;
    };
    ttl_from_reset_seconds(min_reset_secs)
}

fn quota_row(state: &StateDb, provider_name: &str) -> Option<QuotaRecord> {
    state.get_quota(provider_name).ok().flatten()
}

fn refreshed_at_for(state: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
    quota_row(state, provider_name).and_then(|quota| quota.refreshed_at)
}

fn topology_probe_at_for(state: &StateDb, provider_name: &str) -> Option<DateTime<Utc>> {
    quota_row(state, provider_name).and_then(|quota| quota.last_topology_probe_at)
}

fn quota_windows_or_empty(state: &StateDb, provider_name: &str) -> Vec<QuotaWindow> {
    state.get_windows(provider_name).unwrap_or_default()
}

fn projection_refresh_due(refreshed_at: DateTime<Utc>, windows: &[QuotaWindow]) -> bool {
    if windows.is_empty() {
        return true;
    }
    let ttl_secs = dynamic_ttl_secs(windows);
    seconds_since(refreshed_at, Utc::now()) >= ttl_secs
}

fn routing_refresh_due(
    refreshed_at: DateTime<Utc>,
    windows: &[QuotaWindow],
    now: DateTime<Utc>,
) -> bool {
    if windows.is_empty() {
        return true;
    }
    seconds_since(refreshed_at, now) >= ROUTING_REFRESH_TTL_SECS
}

fn topology_probe_not_needed(live_window_count: usize, pool_expected_live_windows: usize) -> bool {
    live_window_count == 0 || live_window_count >= pool_expected_live_windows
}

fn topology_probe_cooldown_elapsed(last_probe_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    seconds_since(last_probe_at, now) >= TOPOLOGY_PROBE_COOLDOWN_SECS as i64
}

fn seconds_since(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (now - timestamp).num_seconds()
}

fn min_seconds_until_reset(windows: &[QuotaWindow], now: DateTime<Utc>) -> Option<i64> {
    windows
        .iter()
        .map(|window| seconds_until_reset(window, now))
        .min()
}

fn seconds_until_reset(window: &QuotaWindow, now: DateTime<Utc>) -> i64 {
    (window.resets_at - now).num_seconds().max(0)
}

fn ttl_from_reset_seconds(reset_secs: i64) -> i64 {
    (reset_secs / REFRESH_WINDOW_DIVISOR).clamp(MIN_TTL_SECS, MAX_TTL_SECS)
}

// Keep for tests that want to model "short" vs "long" windows by constructing
// synthetic resets_at values.
#[cfg(test)]
fn hours_from_now(h: i64) -> DateTime<Utc> {
    Utc::now() + chrono::Duration::hours(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::QuotaWindowInput;

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
        let ttl = dynamic_ttl_secs(&[five_hour, seven_day]);
        assert!((3500..=3700).contains(&ttl), "expected ~1h, got {ttl}s");
    }

    #[test]
    fn ttl_clamps_to_min_for_nearly_expired_windows() {
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
    /// Source: proposal Test-intent track row 8; Assumptions A2, A6.
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
    /// Source: proposal Test-intent track row 9; Assumptions A2, A6.
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
