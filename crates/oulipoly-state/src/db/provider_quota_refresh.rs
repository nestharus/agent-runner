//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, orchestration }
//!
//! Provider quota refresh aggregate orchestration and empty-refresh writes.

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
            .map_err(Self::format_refresh_begin_error)?;

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
        tx.commit().map_err(Self::format_refresh_commit_error)?;
        Ok(())
    }

    fn format_refresh_begin_error(e: sqlite::Error) -> String {
        format!("Failed to begin tx: {e}")
    }

    fn format_refresh_commit_error(e: sqlite::Error) -> String {
        format!("Failed to commit refresh: {e}")
    }

    pub(super) fn record_empty_quota_refresh(
        tx: sqlite::Transaction<'_>,
        provider_name: &str,
        now: &str,
        prior_windows: &[QuotaWindow],
    ) -> Result<(), String> {
        Self::write_empty_quota_refresh(&tx, provider_name, now, prior_windows)?;
        tx.commit().map_err(Self::format_refresh_commit_error)
    }

    pub(super) fn write_empty_quota_refresh(
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

    pub(super) fn write_initial_empty_quota_refresh(
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
        .map_err(Self::format_empty_quota_refresh_error)?;
        Ok(())
    }

    pub(super) fn write_preserving_empty_quota_refresh(
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
        .map_err(Self::format_empty_quota_refresh_error)?;
        Ok(())
    }

    fn format_empty_quota_refresh_error(e: sqlite::Error) -> String {
        format!("Failed to record empty quota refresh: {e}")
    }

    pub(super) fn turns_between_quota_refreshes(
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

    pub(super) fn quota_windows_by_id(windows: &[QuotaWindow]) -> HashMap<u32, &QuotaWindow> {
        windows
            .iter()
            .map(|window| (window.window_id, window))
            .collect()
    }

    pub(super) fn quota_aggregate_projection(
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

    pub(super) fn legacy_quota_projection(windows: &[QuotaWindowInput]) -> (f64, Option<String>) {
        match windows.iter().max_by_key(|window| window.resets_at) {
            Some(window) => (window.used_percent, Some(window.resets_at.to_rfc3339())),
            None => (0.0, None),
        }
    }

    pub(super) fn quota_topology_peak(
        prior: Option<&QuotaRecord>,
        windows: &[QuotaWindowInput],
    ) -> i64 {
        prior
            .map(|quota| quota.topology_peak_live_window_count)
            .unwrap_or(0)
            .max(windows.len()) as i64
    }
}
