//! predicate

use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;

use crate::zero_turn_orchestration::ZeroTurnAction;

pub(super) fn confirmed_zero_turn_maybe_quota(
    zero_turn_action: ZeroTurnAction,
    terminal_signal: &Option<oulipoly_runtime::executor::TerminalSignal>,
) -> bool {
    matches!(zero_turn_action, ZeroTurnAction::ConfirmedExhaustion)
        && terminal_signal
            .as_ref()
            .is_some_and(|signal| signal.kind == TerminalSignalKind::MaybeQuotaExhausted)
}

pub(super) fn completed_attempt_success(
    result: &oulipoly_runtime::executor::ExecutionResult,
    terminal_completion_confirmed: bool,
) -> bool {
    result.exit_code == 0 && terminal_completion_confirmed
}

pub(super) fn completed_attempt_quota_exhausted(error_category: Option<&str>) -> bool {
    crate::quota_zero_turn::error_category_is_quota_exhausted(error_category)
}

pub(super) fn retry_available(attempts: usize, max_attempts: usize) -> bool {
    attempts < max_attempts
}
