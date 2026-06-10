//! ## Declared roles
//!
//! `accessor`, `predicate`, `filter`, `mapper`.

use super::WindowProjection;
use crate::balancer::EPS_HOURS;
use chrono::Utc;
use oulipoly_state::QuotaWindow;

pub(super) fn pool_max_live_window_count(
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

pub(in crate::balancer) fn live_window_count(
    windows: &[QuotaWindow],
    now: chrono::DateTime<Utc>,
) -> usize {
    windows
        .iter()
        .filter(|window| window_is_live(window, now))
        .count()
}

pub(super) fn window_is_live(window: &QuotaWindow, now: chrono::DateTime<Utc>) -> bool {
    window.resets_at > now
}

pub(super) fn live_windows(
    windows: &[QuotaWindow],
    now: chrono::DateTime<Utc>,
) -> impl Iterator<Item = &QuotaWindow> {
    windows
        .iter()
        .filter(move |window| window_is_live(window, now))
}

pub(super) fn window_hours_until_reset(window: &QuotaWindow, now: chrono::DateTime<Utc>) -> f64 {
    ((window.resets_at - now).num_seconds() as f64 / 3600.0).max(EPS_HOURS)
}

pub(super) fn remaining_headroom(projected_used: f64) -> f64 {
    (1.0 - projected_used).max(0.0)
}

pub(super) fn window_binding_score(window: &WindowProjection) -> f64 {
    window.remaining_headroom * window.hours_until_reset
}
