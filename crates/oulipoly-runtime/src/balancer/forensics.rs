//! ## Declared roles
//! mapper, predicate, accessor, orchestration
//!
//! AGE-163 WU-A.4 post-failure-forensics surface. Translates typed terminal
//! signals to the four-class failure-class taxonomy, and writes the durable
//! working-set state (`provider_quotas.next_available_at` /
//! `failure_class` / `last_refresh_at`) inside a `BEGIN IMMEDIATE` lock.

use crate::executor::terminal_signal::TerminalSignalKind;
use crate::migration::MigrationError;
use chrono::{DateTime, Duration, Utc};
use oulipoly_state::StateDb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    RollingWindow5h,
    WeeklyOrLonger,
    UpstreamApiDown,
    TransientStderrNoise,
    /// The provider's backing session store contended under concurrent load.
    /// A short rotate-away cooldown sheds load from the contended account while
    /// it recovers, so concurrent siblings prefer a less-loaded account.
    ProviderStorageContention,
}

impl FailureClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureClass::RollingWindow5h => "RollingWindow5h",
            FailureClass::WeeklyOrLonger => "WeeklyOrLonger",
            FailureClass::UpstreamApiDown => "UpstreamApiDown",
            FailureClass::TransientStderrNoise => "TransientStderrNoise",
            FailureClass::ProviderStorageContention => "ProviderStorageContention",
        }
    }

    pub fn from_terminal_signal_kind(kind: TerminalSignalKind) -> Option<Self> {
        match kind {
            TerminalSignalKind::QuotaExhaustedInband => Some(FailureClass::RollingWindow5h),
            TerminalSignalKind::RateLimited => Some(FailureClass::TransientStderrNoise),
            TerminalSignalKind::ProviderStorageContention => {
                Some(FailureClass::ProviderStorageContention)
            }
            TerminalSignalKind::ProlongedSilence | TerminalSignalKind::SpawnError => {
                Some(FailureClass::UpstreamApiDown)
            }
            TerminalSignalKind::CleanExit
            | TerminalSignalKind::NonzeroExit
            | TerminalSignalKind::SignalExit
            | TerminalSignalKind::Unknown
            | TerminalSignalKind::ProviderUnavailable
            | TerminalSignalKind::MaybeQuotaExhausted => None,
        }
    }

    /// Policy-defined `next_available_at` offset from the failure timestamp.
    /// `None` for transient classes — those write only `last_refresh_at`.
    pub fn next_available_at_offset(self) -> Option<Duration> {
        match self {
            FailureClass::RollingWindow5h => Some(Duration::hours(5)),
            FailureClass::WeeklyOrLonger => Some(Duration::days(7)),
            FailureClass::UpstreamApiDown => Some(Duration::minutes(5)),
            FailureClass::ProviderStorageContention => Some(Duration::minutes(2)),
            FailureClass::TransientStderrNoise => None,
        }
    }
}

pub fn apply_post_failure_forensics(
    state: &StateDb,
    provider_name: &str,
    failure_class: FailureClass,
    now: DateTime<Utc>,
) -> Result<(), MigrationError> {
    touch_provider_refresh(state, provider_name, now)?;
    record_provider_unavailable_if_persistent(state, provider_name, failure_class, now)
}

fn touch_provider_refresh(
    state: &StateDb,
    provider_name: &str,
    now: DateTime<Utc>,
) -> Result<(), MigrationError> {
    state
        .touch_provider_refresh(provider_name, now)
        .map_err(|message| MigrationError::Db { message })
}

fn record_provider_unavailable_if_persistent(
    state: &StateDb,
    provider_name: &str,
    failure_class: FailureClass,
    now: DateTime<Utc>,
) -> Result<(), MigrationError> {
    if let Some(offset) = failure_class.next_available_at_offset() {
        state
            .record_provider_unavailable(provider_name, Some(now + offset), failure_class.as_str())
            .map_err(|message| MigrationError::Db { message })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn open_db() -> StateDb {
        StateDb::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn failure_class_from_quota_exhausted_inband_maps_to_rolling_window_5h() {
        assert_eq!(
            FailureClass::from_terminal_signal_kind(TerminalSignalKind::QuotaExhaustedInband),
            Some(FailureClass::RollingWindow5h)
        );
    }

    #[test]
    fn failure_class_from_rate_limited_maps_to_transient_stderr_noise() {
        assert_eq!(
            FailureClass::from_terminal_signal_kind(TerminalSignalKind::RateLimited),
            Some(FailureClass::TransientStderrNoise)
        );
    }

    #[test]
    fn failure_class_from_unknown_maps_to_none() {
        assert_eq!(
            FailureClass::from_terminal_signal_kind(TerminalSignalKind::Unknown),
            None
        );
    }

    #[test]
    fn failure_class_from_maybe_quota_exhausted_maps_to_none() {
        assert_eq!(
            FailureClass::from_terminal_signal_kind(TerminalSignalKind::MaybeQuotaExhausted),
            None
        );
    }

    #[test]
    fn apply_post_failure_forensics_writes_next_available_at_for_persistent_classes() {
        let db = open_db();
        let now = Utc::now();
        apply_post_failure_forensics(&db, "p", FailureClass::RollingWindow5h, now).unwrap();

        let quota = db.get_quota("p").unwrap().expect("row written");
        let na = quota.next_available_at.expect("next_available_at set");
        let expected = now + Duration::hours(5);
        assert!((na - expected).num_seconds().abs() <= 1);
        assert_eq!(quota.failure_class.as_deref(), Some("RollingWindow5h"));
        assert!(quota.last_refresh_at.is_some());
    }

    #[test]
    fn apply_post_failure_forensics_transient_class_does_not_write_next_available_at() {
        let db = open_db();
        let now = Utc::now();
        apply_post_failure_forensics(&db, "p", FailureClass::TransientStderrNoise, now).unwrap();

        let quota = db.get_quota("p").unwrap().expect("row written");
        assert_eq!(quota.next_available_at, None);
        assert_eq!(quota.failure_class, None);
        assert!(
            quota.last_refresh_at.is_some(),
            "transient class still records last_refresh_at"
        );
    }

    #[test]
    fn apply_post_failure_forensics_idempotent_repeat_call() {
        let db = open_db();
        let now = Utc::now();
        apply_post_failure_forensics(&db, "p", FailureClass::UpstreamApiDown, now).unwrap();
        apply_post_failure_forensics(&db, "p", FailureClass::UpstreamApiDown, now).unwrap();

        let quota = db.get_quota("p").unwrap().expect("row written");
        assert_eq!(quota.failure_class.as_deref(), Some("UpstreamApiDown"));
    }
}
