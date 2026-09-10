//! Frequency control for indefinitely eligible automatic-wake retries.
//!
//! The attempt count is chronology/backoff input, never an exhaustion budget.
//! The delay ceiling bounds retry frequency, not retry lifetime; settlement or
//! ordinary loss of eligibility ends the sequence.
//!
//! ## Declared roles
//!
//! `mapper`, `orchestration`

use std::time::Duration;

use super::auto_wake_env::AutoWakeEnv;
use super::constants::AUTO_WAKE_RETRY_MAX_MS;

pub(super) fn sleep_before_failed_auto_wake_retry(auto_wake: &AutoWakeEnv) {
    std::thread::sleep(auto_wake_retry_delay(auto_wake));
}

fn auto_wake_retry_delay(auto_wake: &AutoWakeEnv) -> Duration {
    Duration::from_millis(bounded_auto_wake_retry_delay_ms(
        auto_wake.retry_base_milliseconds,
        auto_wake.chronological_attempt_count,
    ))
}

fn bounded_auto_wake_retry_delay_ms(base_ms: u64, auto_wake_count: i64) -> u64 {
    let exponent = auto_wake_count.saturating_sub(1).clamp(0, 10) as u32;
    base_ms
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(AUTO_WAKE_RETRY_MAX_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retry_env(chronological_attempt_count: i64) -> AutoWakeEnv {
        AutoWakeEnv {
            token: String::new(),
            chronological_attempt_count,
            retry_base_milliseconds: 1_000,
        }
    }

    #[test]
    fn long_failed_wake_sequence_keeps_bounded_exponential_retry_cadence() {
        let delays = (1..=20)
            .map(|count| auto_wake_retry_delay(&retry_env(count)))
            .collect::<Vec<_>>();

        assert_eq!(
            &delays[..6],
            &[
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
            ]
        );
        assert!(
            delays[6..]
                .iter()
                .all(|delay| *delay == Duration::from_secs(30))
        );
    }

    #[test]
    fn maximum_chronology_keeps_retry_delay_at_ceiling() {
        assert_eq!(
            auto_wake_retry_delay(&retry_env(i64::MAX - 1)),
            Duration::from_millis(AUTO_WAKE_RETRY_MAX_MS)
        );
        assert_eq!(
            auto_wake_retry_delay(&retry_env(i64::MAX)),
            Duration::from_millis(AUTO_WAKE_RETRY_MAX_MS)
        );
    }
}
