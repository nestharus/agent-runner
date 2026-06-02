//! Marker verification health predicates.
//!
//! ## Declared roles
//! predicate

use crate::quota::RefreshOutcome;
use chrono::{DateTime, Duration, Utc};
use oulipoly_state::QuotaWindow;

/// Used-percent threshold (0..=1.0) above which a live window pins the
/// provider as exhausted. Mirrors balancer's `EXHAUSTED_USED_PERCENT`.
const EXHAUSTED_USED_PERCENT: f64 = 1.0;

pub(super) fn marker_within_release_slack(
    next_at: DateTime<Utc>,
    now: DateTime<Utc>,
    release_slack_secs: i64,
) -> bool {
    next_at <= now + Duration::seconds(release_slack_secs)
}

pub(super) fn cached_refresh_is_fresh(
    refreshed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    cooldown_secs: i64,
) -> bool {
    let Some(refreshed_at) = refreshed_at else {
        return false;
    };
    now - refreshed_at < Duration::seconds(cooldown_secs)
}

pub(super) fn cached_refresh_is_stale(
    refreshed_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    cooldown_secs: i64,
) -> bool {
    !cached_refresh_is_fresh(refreshed_at, now, cooldown_secs)
}

pub(super) fn windows_indicate_provider_healthy(
    windows: &[QuotaWindow],
    now: DateTime<Utc>,
) -> bool {
    !windows.is_empty() && !windows_have_live_exhaustion(windows, now)
}

fn windows_have_live_exhaustion(windows: &[QuotaWindow], now: DateTime<Utc>) -> bool {
    windows
        .iter()
        .any(|window| window_pins_exhausted(window, now))
}

fn window_pins_exhausted(window: &QuotaWindow, now: DateTime<Utc>) -> bool {
    window.resets_at > now && window.used_percent >= EXHAUSTED_USED_PERCENT
}

/// True when the refresh either wrote fresh windows (`Updated`) or a peer
/// in-process refresh is already running (`AlreadyInFlight`). `NoScript` and
/// `Failed` keep the marker because there is no new signal to overturn it.
pub(super) fn refresh_outcome_observes_fresh_or_inflight_data(outcome: &RefreshOutcome) -> bool {
    matches!(
        outcome,
        RefreshOutcome::Updated { .. } | RefreshOutcome::AlreadyInFlight
    )
}
