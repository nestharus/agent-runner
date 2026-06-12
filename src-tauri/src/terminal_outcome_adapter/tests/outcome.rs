use super::signal;
use crate::terminal_outcome_adapter::typed_terminal_reason_fallback;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

#[test]
fn typed_terminal_reason_fallback_returns_maybe_quota_exhausted_for_new_kind() {
    let maybe = signal(TerminalSignalKind::MaybeQuotaExhausted);
    let quota = signal(TerminalSignalKind::QuotaExhaustedInband);

    assert_eq!(
        typed_terminal_reason_fallback(&maybe),
        Some("maybe_quota_exhausted")
    );
    assert_eq!(
        typed_terminal_reason_fallback(&quota),
        Some("quota_exhausted_inband")
    );
}
