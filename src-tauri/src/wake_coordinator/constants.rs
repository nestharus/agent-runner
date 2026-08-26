//! ## Declared roles
//!
//! `accessor`

pub(super) use oulipoly_core::{
    AUTO_WAKE_COUNT_ENV, AUTO_WAKE_ENV, AUTO_WAKE_RETRY_BASE_MS_ENV, AUTO_WAKE_SESSION_ID_ENV,
    AUTO_WAKE_TOKEN_ENV,
};
pub(super) const PARENT_INVOCATION_ENV: &str = "OULIPOLY_PARENT_INVOCATION";
pub(super) const DEFAULT_AUTO_WAKE_RETRY_BASE_MS: u64 = 1_000;
pub(super) const AUTO_WAKE_RETRY_MAX_MS: u64 = 30_000;
pub(super) const WAKE_CLAIM_STALE_AFTER_SECONDS: i64 = 10 * 60;
pub(super) const WAKE_RECLAIM_SWEEP_SCAN_LIMIT: usize = 8 * 32;
pub(super) const WAKE_RECLAIM_SWEEP_INTERVAL_SECONDS: u64 = 60;
pub(super) const WAKE_RECLAIM_STATE_SNAPSHOT_TIMEOUT_SECONDS: u64 = 5;
pub(super) const LIVE_PTY_RETRY_INTERVAL_SECONDS: u64 = 3;
pub(super) const WAKE_RECLAIM_HANDOFF_OWNER_ENV: &str = "OULIPOLY_WAKE_RECLAIM_HANDOFF_OWNER";
pub(super) const WAKE_RECLAIM_HANDOFF_TOKEN_ENV: &str = "OULIPOLY_WAKE_RECLAIM_HANDOFF_TOKEN";
pub(super) const CONSUMED_NOTIFICATION_MARKER: &str = "[OULIPOLY NOTIFICATIONS]";

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_core::RUNNER_PRIVATE_AUTO_WAKE_ENV_NAMES;

    #[test]
    fn wake_producer_variables_match_shared_private_catalog() {
        assert_eq!(
            RUNNER_PRIVATE_AUTO_WAKE_ENV_NAMES,
            [
                AUTO_WAKE_ENV,
                AUTO_WAKE_SESSION_ID_ENV,
                AUTO_WAKE_TOKEN_ENV,
                AUTO_WAKE_COUNT_ENV,
                AUTO_WAKE_RETRY_BASE_MS_ENV,
            ]
        );
    }
}
