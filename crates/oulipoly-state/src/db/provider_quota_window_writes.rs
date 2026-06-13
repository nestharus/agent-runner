//! ## Declared roles
//!
//! - formatter
//! - orchestration
//! - mapper
//! - predicate
//!
//! Role set: { formatter, orchestration, mapper, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/provider_quota_window_writes.rs
//!     role: intrinsic-surface
//!     Domain: provider-quota-window-writes-persistence
//!     Owns:
//!       - StateDb provider-quota-window-writes persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, HashMap, MAX_LEARNABLE_BURN_RATE, MIN_LEARN_SAMPLE_CALLS, NEAR_EXHAUSTED_USED_PERCENT, QuotaAggregateProjection, QuotaWindow, QuotaWindowDelta, QuotaWindowInput, StateDb, params, sqlite
//! ```
//!
//! ## Common-interface declarations
//!
//! ```yaml
//! common_interface_declarations:
//!   - component: crates/oulipoly-state/src/db/provider_quota_window_writes.rs
//!     role: common-interface
//!     Contract: provider-quota-window-positional-identity
//!     Declares:
//!       - The `&[QuotaWindowInput]` slice IS the StateDb-owned stable per-provider
//!         window-identity contract: a window's `window_id` is its position in the
//!         slice. The runtime quota producer (`QuotaScriptWindow`, validated by
//!         `parse_output`) emits windows in stable per-provider `window_id`/positional
//!         order before lowering to `QuotaWindowInput`, so `insert_quota_window_rows`
//!         and `prior_windows_by_id` match cross-refresh rows through this declared
//!         common interface (slice position), not incidental generated-output ordering.
//!       - This positional window identity is the pre-existing agreed producer↔StateDb
//!         contract; the WU #65 decomposition preserves it byte-for-byte and does not
//!         change quota-window matching semantics.
//! ```
//!
//! Provider quota window replacement and burn-rate delta classification.

use super::*;

impl StateDb {
    pub(super) fn write_quota_aggregate(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
        projection: QuotaAggregateProjection,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at,
                 topology_peak_live_window_count)
             VALUES (?1, ?2, ?3, 0, ?4, ?5)
             ON CONFLICT (provider_name) DO UPDATE SET
                used_percent = ?2,
                resets_at = ?3,
                calls_since_refresh = 0,
                refreshed_at = ?4,
                exhausted_at = NULL,
                topology_peak_live_window_count = MAX(topology_peak_live_window_count, ?5)",
            sqlite::params![
                provider_name,
                projection.legacy_used,
                projection.legacy_resets,
                now,
                projection.topology_peak_live_window_count
            ],
        )
        .map_err(Self::format_quota_upsert_error)?;
        Ok(())
    }

    fn format_quota_upsert_error(e: sqlite::Error) -> String {
        format!("Failed to upsert quota: {e}")
    }

    pub(super) fn replace_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        windows: &[QuotaWindowInput],
        prior_windows_by_id: &HashMap<u32, &QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> Result<(), String> {
        Self::delete_quota_window_rows(conn, provider_name)?;
        Self::insert_quota_window_rows(
            conn,
            provider_name,
            windows,
            prior_windows_by_id,
            turns_between_refreshes,
        )
    }

    pub(super) fn delete_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
    ) -> Result<(), String> {
        conn.execute(
            "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
            sqlite::params![provider_name],
        )
        .map_err(Self::format_clear_windows_error)?;
        Ok(())
    }

    fn format_clear_windows_error(e: sqlite::Error) -> String {
        format!("Failed to clear windows: {e}")
    }

    pub(super) fn insert_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
        windows: &[QuotaWindowInput],
        prior_windows_by_id: &HashMap<u32, &QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> Result<(), String> {
        for (index, window) in windows.iter().enumerate() {
            let delta = Self::quota_window_delta(
                window,
                prior_windows_by_id.get(&(index as u32)).copied(),
                turns_between_refreshes,
            );
            Self::insert_quota_window_row(conn, provider_name, index, window, delta)?;
        }
        Ok(())
    }

    pub(super) fn quota_window_delta(
        window: &QuotaWindowInput,
        prior_window: Option<&QuotaWindow>,
        turns_between_refreshes: u64,
    ) -> QuotaWindowDelta {
        match prior_window {
            Some(prior) => {
                Self::classify_quota_window_delta(window, prior, turns_between_refreshes)
            }
            None => QuotaWindowDelta {
                last_delta_percent: None,
                last_delta_calls: None,
            },
        }
    }

    pub(super) fn classify_quota_window_delta(
        window: &QuotaWindowInput,
        prior_window: &QuotaWindow,
        turns_between_refreshes: u64,
    ) -> QuotaWindowDelta {
        let delta_percent = (window.used_percent - prior_window.used_percent).max(0.0);
        if Self::quota_delta_sample_is_learnable(delta_percent, window, turns_between_refreshes) {
            QuotaWindowDelta {
                last_delta_percent: Some(delta_percent),
                last_delta_calls: Some(turns_between_refreshes),
            }
        } else {
            QuotaWindowDelta {
                last_delta_percent: prior_window.last_delta_percent,
                last_delta_calls: prior_window.last_delta_calls,
            }
        }
    }

    pub(super) fn quota_delta_sample_is_learnable(
        delta_percent: f64,
        window: &QuotaWindowInput,
        turns_between_refreshes: u64,
    ) -> bool {
        delta_percent > 0.0
            && !Self::quota_delta_sample_is_small(turns_between_refreshes)
            && !Self::quota_window_is_near_rail(window)
            && !Self::quota_delta_rate_too_high(delta_percent, turns_between_refreshes)
    }

    pub(super) fn quota_delta_sample_is_small(turns_between_refreshes: u64) -> bool {
        turns_between_refreshes < MIN_LEARN_SAMPLE_CALLS
    }

    pub(super) fn quota_window_is_near_rail(window: &QuotaWindowInput) -> bool {
        window.used_percent >= NEAR_EXHAUSTED_USED_PERCENT
    }

    pub(super) fn quota_delta_rate_too_high(
        delta_percent: f64,
        turns_between_refreshes: u64,
    ) -> bool {
        Self::quota_delta_rate(delta_percent, turns_between_refreshes) > MAX_LEARNABLE_BURN_RATE
    }

    pub(super) fn quota_delta_rate(delta_percent: f64, turns_between_refreshes: u64) -> f64 {
        if turns_between_refreshes > 0 {
            delta_percent / (turns_between_refreshes as f64)
        } else {
            f64::INFINITY
        }
    }

    pub(super) fn insert_quota_window_row(
        conn: &sqlite::Connection,
        provider_name: &str,
        index: usize,
        window: &QuotaWindowInput,
        delta: QuotaWindowDelta,
    ) -> Result<(), String> {
        let row = Self::quota_window_insert_row(index, window, delta);
        conn.execute(
            "INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at,
                 last_delta_percent, last_delta_calls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            sqlite::params![
                provider_name,
                row.window_id,
                row.used_percent,
                row.resets_at,
                row.last_delta_percent,
                row.last_delta_calls,
            ],
        )
        .map_err(Self::format_insert_window_error)?;
        Ok(())
    }

    fn format_insert_window_error(e: sqlite::Error) -> String {
        format!("Failed to insert window: {e}")
    }

    fn quota_window_insert_row(
        index: usize,
        window: &QuotaWindowInput,
        delta: QuotaWindowDelta,
    ) -> QuotaWindowInsertRow {
        QuotaWindowInsertRow {
            window_id: index as i64,
            used_percent: window.used_percent,
            resets_at: Self::quota_window_reset_timestamp(window),
            last_delta_percent: delta.last_delta_percent,
            last_delta_calls: Self::quota_window_delta_calls(delta.last_delta_calls),
        }
    }

    fn quota_window_reset_timestamp(window: &QuotaWindowInput) -> String {
        window.resets_at.to_rfc3339()
    }

    fn quota_window_delta_calls(value: Option<u64>) -> Option<i64> {
        value.map(|value| value as i64)
    }
}

struct QuotaWindowInsertRow {
    window_id: i64,
    used_percent: f64,
    resets_at: String,
    last_delta_percent: Option<f64>,
    last_delta_calls: Option<i64>,
}
