//! ## Declared roles
//!
//! - orchestration
//! - formatter
//!
//! Role set: { orchestration, formatter }
//!
//! Test-support writes for seeding provider quota edge cases.

use super::*;

impl StateDb {
    #[cfg(any(test, feature = "test-support"))]
    pub fn drop_provider_quotas_for_test(&self) {
        self.conn
            .execute_batch("DROP TABLE provider_quotas")
            .unwrap();
    }

    /// Test-only: backdate a provider's `refreshed_at` so tests can seed
    /// turns whose timestamps are "after" the refresh.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_refreshed_at_for_test(
        &self,
        provider_name: &str,
        refreshed_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas SET refreshed_at = ?1 WHERE provider_name = ?2",
                sqlite::params![refreshed_at.to_rfc3339(), provider_name],
            )
            .map_err(Self::format_set_refreshed_at_error)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_set_refreshed_at_error(e: sqlite::Error) -> String {
        format!("Failed to set refreshed_at: {e}")
    }

    /// Test-only: seed the PR 3 per-window burn-rate learning columns without
    /// adding a migration here. This intentionally fails at runtime until the
    /// production schema owns these columns.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_window_delta_for_test(
        &self,
        provider_name: &str,
        window_id: u32,
        last_delta_percent: f64,
        last_delta_calls: u64,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quota_windows
                 SET last_delta_percent = ?3,
                     last_delta_calls = ?4
                 WHERE provider_name = ?1 AND window_id = ?2",
                sqlite::params![
                    provider_name,
                    window_id as i64,
                    last_delta_percent,
                    last_delta_calls as i64
                ],
            )
            .map_err(Self::format_set_window_delta_error)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_set_window_delta_error(e: sqlite::Error) -> String {
        format!("Failed to set window delta: {e}")
    }

    /// Test-only: seed a provider quota row without any window rows.
    #[cfg(any(test, feature = "test-support"))]
    pub fn insert_quota_row_without_windows_for_test(
        &self,
        provider_name: &str,
        refreshed_at: &DateTime<Utc>,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO provider_quotas
                    (provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at)
                 VALUES (?1, 0, NULL, 0, ?2)
                 ON CONFLICT (provider_name) DO UPDATE SET
                    used_percent = 0,
                    resets_at = NULL,
                    calls_since_refresh = 0,
                    refreshed_at = ?2",
                sqlite::params![provider_name, refreshed_at.to_rfc3339()],
            )
            .map_err(Self::format_insert_quota_row_error)?;
        self.conn
            .execute(
                "DELETE FROM provider_quota_windows WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(Self::format_clear_quota_windows_error)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_insert_quota_row_error(e: sqlite::Error) -> String {
        format!("Failed to insert quota row: {e}")
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_clear_quota_windows_error(e: sqlite::Error) -> String {
        format!("Failed to clear quota windows: {e}")
    }

    /// Test-only: make a cached quota row unreadable through the public
    /// `get_quota` API by writing a storage value that production parsing
    /// rejects.
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_unreadable_cached_quota_for_test(
        &self,
        provider_name: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quotas
                 SET topology_peak_live_window_count = -1
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(Self::format_force_unreadable_cached_quota_error)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_force_unreadable_cached_quota_error(e: sqlite::Error) -> String {
        format!("Failed to force unreadable cached quota: {e}")
    }

    /// Test-only: make cached window rows unreadable through the public
    /// `get_windows` API by writing a timestamp value that strict window
    /// parsing rejects.
    #[cfg(any(test, feature = "test-support"))]
    pub fn force_unreadable_cached_windows_for_test(
        &self,
        provider_name: &str,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE provider_quota_windows
                 SET resets_at = 'not-rfc3339'
                 WHERE provider_name = ?1",
                sqlite::params![provider_name],
            )
            .map_err(Self::format_force_unreadable_cached_windows_error)?;
        Ok(())
    }

    #[cfg(any(test, feature = "test-support"))]
    fn format_force_unreadable_cached_windows_error(e: sqlite::Error) -> String {
        format!("Failed to force unreadable cached windows: {e}")
    }
}
