//! ## Declared roles
//!
//! - DTO
//! - mapper
//! - policy constant
//!
//! Role set: { DTO, mapper, policy constant }
//!
//! Provider quota persistence types and burn-rate guard constants.

use super::*;

/// Ceiling on the per-turn burn rate that `upsert_quota_refresh` is willing
/// to learn from a single refresh-to-refresh sample. A transient upstream
/// spike (observed on the ChatGPT usage endpoint: `used_percent` briefly
/// reported as 1.0 before the window reset) paired with a small turn count
/// produced a learned rate of ~0.05/turn that then got carried forward across
/// subsequent no-change refreshes, projecting every provider near the ceiling
/// and making the whole pool look unusable. The highest plausible real
/// rate observed in live data is ~5e-4/turn on a 5h Claude window; 0.1/turn
/// is a 200x safety margin that still filters the spike case.
pub(super) const MAX_LEARNABLE_BURN_RATE: f64 = 0.1;

/// Minimum assistant-turn sample size before a refresh-to-refresh delta is
/// accepted as a burn-rate learn. Below this, a 1%-on-6-turns observation
/// extrapolates to rates that are dominated by sample noise -- when the
/// rate is then multiplied by `turns_since_refresh` at scoring time, a
/// 65%-used window can project to 97% on nothing but measurement error,
/// making the provider look nearly exhausted. Live-caught 2026-04-21 on provider A with
/// `last_delta_percent=0.01 / last_delta_calls=6` -> projected
/// 0.65 + 193x0.00167 = 0.972, blocking the whole high-quota pool. 20
/// turns is the empirical floor where per-turn rates stabilize to within
/// ~2x of the long-run mean across observed Claude/Codex samples.
pub(super) const MIN_LEARN_SAMPLE_CALLS: u64 = 20;

/// Refuse to learn a burn rate from a sample where the window's
/// `used_percent` is already near its ceiling. A 100%-reading window did
/// not fill at a natural rate during the prior inter-refresh interval -- it
/// hit a wall at some unknown point during that window and stayed pinned.
/// The dp/dc ratio from that interval is an artifact of the cap, not a
/// physical rate. Live-caught 2026-04-21 on provider B after a transient
/// ChatGPT upstream spike reported `used_percent=1.0` on the 7-day
/// window: learned rate became 1.0/34 ~= 0.029/turn on WEEKLY (where real
/// rates live near 6e-5/turn), projecting every subsequent invocation
/// near the ceiling on nothing but a bad sample. User intuition:
/// "turns barely budge weekly" -- so any single sample imputing a weekly
/// move > 1 point is suspect, and the cleanest marker of "suspect" is
/// "the sample is at the rail." Matching ceiling from score_by_density.
pub(super) const NEAR_EXHAUSTED_USED_PERCENT: f64 = 0.99;

/// Per-provider (account) metadata. Keyed on provider name (e.g. `provider-a`,
/// `provider-b`), which spans every model routed through that account.
/// The actual quota numbers live in `provider_quota_windows` -- one row per
/// rolling window the CLI exposes (e.g. 5-hour + 7-day).
#[derive(Debug, Clone)]
pub struct QuotaRecord {
    pub provider_name: String,
    /// Calls recorded against this provider since the last refresh.
    pub calls_since_refresh: u64,
    pub refreshed_at: Option<DateTime<Utc>>,
    pub exhausted_at: Option<DateTime<Utc>>,
    pub topology_peak_live_window_count: usize,
    pub last_topology_probe_at: Option<DateTime<Utc>>,
    pub next_available_at: Option<DateTime<Utc>>,
    pub last_refresh_at: Option<DateTime<Utc>>,
    pub failure_class: Option<String>,
}

/// One rolling-quota window reported by a provider's quota script.
/// `window_id` is a stable per-provider position index (window 0, 1, ...)
/// so the same window survives across refreshes for delta-learning.
#[derive(Debug, Clone)]
pub struct QuotaWindow {
    pub provider_name: String,
    pub window_id: u32,
    /// 0..1 ratio. 0.23 = 23% of this window's budget consumed.
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
    pub last_delta_percent: Option<f64>,
    pub last_delta_calls: Option<u64>,
}

/// Input to `upsert_quota_refresh` -- one window's freshly-fetched values.
#[derive(Debug, Clone)]
pub struct QuotaWindowInput {
    pub used_percent: f64,
    pub resets_at: DateTime<Utc>,
}

pub(super) struct RawQuotaRecordRow {
    pub(super) calls_since_refresh: i64,
    pub(super) refreshed_at: Option<String>,
    pub(super) exhausted_at: Option<String>,
    pub(super) topology_peak_live_window_count: i64,
    pub(super) last_topology_probe_at: Option<String>,
    pub(super) next_available_at: Option<String>,
    pub(super) last_refresh_at: Option<String>,
    pub(super) failure_class: Option<String>,
}

pub(super) struct RawQuotaWindowRow {
    pub(super) window_id: i64,
    pub(super) used_percent: f64,
    pub(super) resets_at: String,
    pub(super) last_delta_percent: Option<f64>,
    pub(super) last_delta_calls: Option<i64>,
}

pub(super) struct QuotaAggregateProjection {
    pub(super) legacy_used: f64,
    pub(super) legacy_resets: Option<String>,
    pub(super) topology_peak_live_window_count: i64,
}

pub(super) struct QuotaWindowDelta {
    pub(super) last_delta_percent: Option<f64>,
    pub(super) last_delta_calls: Option<u64>,
}
