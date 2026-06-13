//! ## Declared roles
//!
//! - accessor
//! - mapper
//! - formatter
//!
//! Role set: { accessor, mapper, formatter }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/invocation_live_load.rs
//!     role: intrinsic-surface
//!     Domain: invocation-live-load-persistence
//!     Owns:
//!       - the StateDb live-load read surface: the count of `running`
//!         (started, not-yet-finalized) invocations per model+provider within a
//!         recency window, read from the invocations table this concern queries
//!       - Intrinsic StateDb/rusqlite carriers and time helpers referenced via
//!         `use super::*`, subordinate to this domain: StateDb, sqlite, DateTime,
//!         Utc, chrono::Duration
//! ```
//!
//! Live per-account session-load count used by anti-storm rotation.

use super::*;

impl StateDb {
    /// Count invocations still `running` (started, not yet finalized) for this
    /// model+provider within the last `window_minutes`. The recency bound treats
    /// a `running` row older than the window as a dead/crashed invocation rather
    /// than live load, so a stale row cannot permanently inflate an account's
    /// apparent load. Used as the live-load signal for anti-storm rotation.
    pub fn running_invocation_count(
        &self,
        model_name: &str,
        provider_name: &str,
        window_minutes: i64,
    ) -> Result<i64, String> {
        let cutoff = Self::running_invocation_cutoff(window_minutes);

        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM invocations
                 WHERE model_name = ?1 AND provider_name = ?2
                   AND status = 'running' AND created_at > ?3",
                sqlite::params![model_name, provider_name, &cutoff],
                Self::map_running_invocation_count_row,
            )
            .map_err(Self::format_running_invocation_count_error)?;

        Ok(count)
    }

    fn map_running_invocation_count_row(row: &sqlite::Row<'_>) -> sqlite::Result<i64> {
        row.get(0)
    }

    fn running_invocation_cutoff(window_minutes: i64) -> String {
        (Utc::now() - chrono::Duration::minutes(window_minutes)).to_rfc3339()
    }

    fn format_running_invocation_count_error(e: sqlite::Error) -> String {
        format!("Failed to count running invocations: {e}")
    }
}
