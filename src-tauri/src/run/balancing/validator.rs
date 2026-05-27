//! validator

use crate::terminal_outcome_adapter::TerminalSignalDisposition;
use oulipoly_runtime::executor;

pub(super) fn expect_maybe_quota_verify_disposition(disposition: TerminalSignalDisposition) {
    debug_assert!(matches!(
        disposition,
        TerminalSignalDisposition::MaybeQuotaVerify
    ));
}

pub(super) fn expect_quota_exhausted_retry_disposition(
    disposition: TerminalSignalDisposition,
    _expected: TerminalSignalDisposition,
) {
    debug_assert!(matches!(
        disposition,
        TerminalSignalDisposition::QuotaExhaustedRetry
    ));
}

pub(super) fn expect_prolonged_silence_fail_disposition(
    disposition: TerminalSignalDisposition,
    _expected: TerminalSignalDisposition,
) {
    debug_assert!(matches!(
        disposition,
        TerminalSignalDisposition::ProlongedSilenceFail
    ));
}

pub(super) fn expect_interactive_fail_disposition(
    disposition: TerminalSignalDisposition,
    _expected: TerminalSignalDisposition,
) {
    debug_assert!(matches!(
        disposition,
        TerminalSignalDisposition::InteractiveFail
    ));
}

pub(super) fn expect_completed_attempt_disposition(disposition: TerminalSignalDisposition) {
    debug_assert!(matches!(
        disposition,
        TerminalSignalDisposition::InteractiveClean | TerminalSignalDisposition::NotApplicable
    ));
}

pub(super) fn required_typed_terminal_reason<'a>(
    reason: Option<&'a str>,
    message: &str,
) -> &'a str {
    reason.expect(message)
}

pub(super) fn required_late_bind_provider_session_id(session_id: Option<&str>) -> &str {
    session_id.expect("late-bind predicate requires a session id")
}

pub(super) fn required_confirmed_zero_turn_signal(
    signal: &Option<executor::TerminalSignal>,
) -> &executor::TerminalSignal {
    signal
        .as_ref()
        .expect("confirmed zero-turn action requires a maybe signal")
}
