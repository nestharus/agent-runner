//! Terminal-signal outcome helpers without state side effects.
//!
//! ## Declared roles
//!
//! `mapper`, `predicate`, `formatter`

use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{ExecutionResult, TerminalSignal};

pub fn typed_terminal_reason_fallback(signal: &TerminalSignal) -> Option<&'static str> {
    oulipoly_runtime::diagnostics::canonical_terminal_reason_for_kind(signal.kind)
}

pub fn terminal_signal_reason<'a>(
    signal: &Option<TerminalSignal>,
    executor_terminal_reason: Option<&'a str>,
) -> Option<&'a str> {
    signal.as_ref()?;
    executor_terminal_reason.or_else(|| signal.as_ref().and_then(typed_terminal_reason_fallback))
}

pub fn terminal_signal_error_category<'a>(
    signal: &Option<TerminalSignal>,
    terminal_reason: &'a str,
) -> Option<&'a str> {
    match signal.as_ref().map(|signal| signal.kind) {
        Some(
            TerminalSignalKind::NonzeroExit
            | TerminalSignalKind::SignalExit
            | TerminalSignalKind::MaybeQuotaExhausted,
        ) => None,
        _ => Some(terminal_reason),
    }
}

pub fn resume_terminal_signal_for_outcome(
    signal: &Option<TerminalSignal>,
) -> Option<TerminalSignal> {
    signal
        .as_ref()
        .filter(|signal| {
            !matches!(
                signal.kind,
                TerminalSignalKind::NonzeroExit | TerminalSignalKind::SignalExit
            )
        })
        .cloned()
}

pub fn balanced_terminal_signal_for_outcome(
    result: &ExecutionResult,
    should_defer_generic_exit: bool,
) -> Option<TerminalSignal> {
    result
        .terminal_signal
        .as_ref()
        .filter(|signal| {
            !matches!(
                signal.kind,
                TerminalSignalKind::NonzeroExit | TerminalSignalKind::SignalExit
            ) || !should_defer_generic_exit
        })
        .cloned()
}

pub fn spawn_error_terminal_signal(
    provider_name: &str,
    evidence: impl Into<String>,
) -> TerminalSignal {
    TerminalSignal {
        kind: TerminalSignalKind::SpawnError,
        provider_name: provider_name.to_string(),
        evidence: evidence.into(),
        observed_at: std::time::SystemTime::now(),
    }
}
