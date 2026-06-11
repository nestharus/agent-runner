//! ## Declared roles
//!
//! - mutator
//!
//! Role set: { mutator }
//!
//! Provider quota refresh aggregate and window row writes.

use super::*;

impl StateDb {
    /// Record a freshly-fetched set of quota windows. Computes per-window
    /// deltas for percent-per-turn learning. Resets `calls_since_refresh` to 0.
    ///
    /// Windows are replaced wholesale: anything not in `windows` is deleted,
    /// so a script that drops a window (e.g. CLI removed a rate limit) stops
    /// contributing to density scoring.
    pub fn upsert_quota_refresh(
        &self,
        provider_name: &str,
        windows: &[QuotaWindowInput],
    ) -> Result<(), String> {
        let now = Utc::now().to_rfc3339();

        let prior = self.get_quota(provider_name)?;
        let prior_windows = self.get_windows(provider_name)?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to begin tx: {e}"))?;

        if windows.is_empty() {
            return Self::record_empty_quota_refresh(tx, provider_name, &now, &prior_windows);
        }

        let turns_between_refreshes =
            self.turns_between_quota_refreshes(provider_name, prior.as_ref());
        let prior_windows_by_id = Self::quota_windows_by_id(&prior_windows);
        let projection = Self::quota_aggregate_projection(prior.as_ref(), windows);
        Self::write_quota_aggregate(&tx, provider_name, &now, projection)?;
        Self::replace_quota_window_rows(
            &tx,
            provider_name,
            windows,
            &prior_windows_by_id,
            turns_between_refreshes,
        )?;
        tx.commit()
            .map_err(|e| format!("Failed to commit refresh: {e}"))?;
        Ok(())
    }

    fn record_empty_quota_refresh(
        tx: sqlite::Transaction<'_>,
        provider_name: &str,
        now: &str,
        prior_windows: &[QuotaWindow],
    ) -> Result<(), String> {
        Self::write_empty_quota_refresh(&tx, provider_name, now, prior_windows)?;
        tx.commit()
            .map_err(|e| format!("Failed to commit refresh: {e}"))
    }

    fn write_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
        prior_windows: &[QuotaWindow],
    ) -> Result<(), String> {
        if prior_windows.is_empty() {
            Self::write_initial_empty_quota_refresh(conn, provider_name, now)
        } else {
            Self::write_preserving_empty_quota_refresh(conn, provider_name, now)
        }
    }

    fn write_initial_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, refreshed_at, last_empty_refresh_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT (provider_name) DO UPDATE SET
                refreshed_at = ?2,
                last_empty_refresh_at = ?2",
            sqlite::params![provider_name, now],
        )
        .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
        Ok(())
    }

    fn write_preserving_empty_quota_refresh(
        conn: &sqlite::Connection,
        provider_name: &str,
        now: &str,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quotas
                (provider_name, refreshed_at, last_empty_refresh_at)
             VALUES (?1, ?2, ?2)
             ON CONFLICT (provider_name) DO UPDATE SET
                last_empty_refresh_at = ?2",
            sqlite::params![provider_name, now],
        )
        .map_err(|e| format!("Failed to record empty quota refresh: {e}"))?;
        Ok(())
    }

    fn turns_between_quota_refreshes(
        &self,
        provider_name: &str,
        prior: Option<&QuotaRecord>,
    ) -> u64 {
        prior
            .map(|p| {
                self.count_assistant_turns_since(provider_name, p.refreshed_at.as_ref())
                    .unwrap_or(p.calls_since_refresh)
            })
            .unwrap_or(0)
    }

    fn quota_windows_by_id(windows: &[QuotaWindow]) -> HashMap<u32, &QuotaWindow> {
        windows
            .iter()
            .map(|window| (window.window_id, window))
            .collect()
    }

    fn quota_aggregate_projection(
        prior: Option<&QuotaRecord>,
        windows: &[QuotaWindowInput],
    ) -> QuotaAggregateProjection {
        let (legacy_used, legacy_resets) = Self::legacy_quota_projection(windows);
        QuotaAggregateProjection {
            legacy_used,
            legacy_resets,
            topology_peak_live_window_count: Self::quota_topology_peak(prior, windows),
        }
    }

    fn legacy_quota_projection(windows: &[QuotaWindowInput]) -> (f64, Option<String>) {
        match windows.iter().max_by_key(|window| window.resets_at) {
            Some(window) => (window.used_percent, Some(window.resets_at.to_rfc3339())),
            None => (0.0, None),
        }
    }

    fn quota_topology_peak(prior: Option<&QuotaRecord>, windows: &[QuotaWindowInput]) -> i64 {
        prior
            .map(|quota| quota.topology_peak_live_window_count)
            .unwrap_or(0)
            .max(windows.len()) as i64
    }

    fn write_quota_aggregate(
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
        .map_err(|e| format!("Failed to upsert quota: {e}"))?;
        Ok(())
    }

    fn replace_quota_window_rows(
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

    fn delete_quota_window_rows(
        conn: &sqlite::Connection,
        provider_name: &str,
    ) -> Result<(), String> {
        conn.execute(
            "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
            sqlite::params![provider_name],
        )
        .map_err(|e| format!("Failed to clear windows: {e}"))?;
        Ok(())
    }

    fn insert_quota_window_rows(
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

    fn quota_window_delta(
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

    fn classify_quota_window_delta(
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

    fn quota_delta_sample_is_learnable(
        delta_percent: f64,
        window: &QuotaWindowInput,
        turns_between_refreshes: u64,
    ) -> bool {
        delta_percent > 0.0
            && !Self::quota_delta_sample_is_small(turns_between_refreshes)
            && !Self::quota_window_is_near_rail(window)
            && !Self::quota_delta_rate_too_high(delta_percent, turns_between_refreshes)
    }

    fn quota_delta_sample_is_small(turns_between_refreshes: u64) -> bool {
        turns_between_refreshes < MIN_LEARN_SAMPLE_CALLS
    }

    fn quota_window_is_near_rail(window: &QuotaWindowInput) -> bool {
        window.used_percent >= NEAR_EXHAUSTED_USED_PERCENT
    }

    fn quota_delta_rate_too_high(delta_percent: f64, turns_between_refreshes: u64) -> bool {
        Self::quota_delta_rate(delta_percent, turns_between_refreshes) > MAX_LEARNABLE_BURN_RATE
    }

    fn quota_delta_rate(delta_percent: f64, turns_between_refreshes: u64) -> f64 {
        if turns_between_refreshes > 0 {
            delta_percent / (turns_between_refreshes as f64)
        } else {
            f64::INFINITY
        }
    }

    fn insert_quota_window_row(
        conn: &sqlite::Connection,
        provider_name: &str,
        index: usize,
        window: &QuotaWindowInput,
        delta: QuotaWindowDelta,
    ) -> Result<(), String> {
        conn.execute(
            "INSERT INTO provider_quota_windows
                (provider_name, window_id, used_percent, resets_at,
                 last_delta_percent, last_delta_calls)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            sqlite::params![
                provider_name,
                index as i64,
                window.used_percent,
                window.resets_at.to_rfc3339(),
                delta.last_delta_percent,
                delta.last_delta_calls.map(|value| value as i64),
            ],
        )
        .map_err(|e| format!("Failed to insert window: {e}"))?;
        Ok(())
    }
}
