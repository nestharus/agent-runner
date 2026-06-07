//! Roles: mapper, validator.
//!
//! - mapper: provider ProcessStatus/TerminalSignal to host terminal outcome.
//! - validator: embedded behavior tests assert exit-code/reason mapping.

use crate::executor::cli::{terminal_exit_code_from_signal, terminal_reason_from_signal_status};
use crate::executor::terminal_signal::{
    TerminalSignal, TerminalSignalKind, TerminalStatusEvidence,
};
use oulipoly_provider::generated::{
    ProcessStatus, TerminalSignal as ProviderTerminalSignal,
    TerminalSignalKind as ProviderTerminalSignalKind,
};
use std::time::{Duration, UNIX_EPOCH};

pub(crate) struct TerminalCancelOutcome {
    pub(crate) exit_code: i32,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) terminal_signal: TerminalSignal,
}

pub(crate) fn map_terminal_cancel_outcome(
    status: &ProcessStatus,
    signal: &ProviderTerminalSignal,
    provider_name: &str,
) -> TerminalCancelOutcome {
    let terminal_signal = TerminalSignal {
        kind: terminal_signal_kind(&signal.kind),
        provider_name: provider_name.to_string(),
        evidence: signal
            .evidence
            .clone()
            .unwrap_or_else(|| signal.kind.as_str().to_string()),
        observed_at: UNIX_EPOCH + Duration::from_millis(signal.observed_at_unix_ms),
    };
    TerminalCancelOutcome {
        exit_code: terminal_exit_code_from_signal(&terminal_signal, exit_code(status)),
        terminal_reason: terminal_reason(status, &terminal_signal),
        terminal_signal,
    }
}

fn exit_code(status: &ProcessStatus) -> i32 {
    match status {
        ProcessStatus::Exited { code } => *code,
        ProcessStatus::SignalTerminated { signal } => 128 + *signal,
        ProcessStatus::Cancelled => 130,
        ProcessStatus::SpawnError { .. } => 1,
        ProcessStatus::ProlongedSilence { .. } => 124,
        ProcessStatus::Unknown => 1,
    }
}

fn terminal_reason(status: &ProcessStatus, signal: &TerminalSignal) -> Option<String> {
    match status {
        ProcessStatus::Cancelled => Some("cancelled".to_string()),
        ProcessStatus::SpawnError { reason } | ProcessStatus::ProlongedSilence { reason } => {
            Some(reason.clone())
        }
        ProcessStatus::Exited { .. }
        | ProcessStatus::SignalTerminated { .. }
        | ProcessStatus::Unknown => {
            terminal_reason_from_signal_status(signal, Some(&terminal_status_evidence(status)))
        }
    }
}

fn terminal_status_evidence(status: &ProcessStatus) -> TerminalStatusEvidence {
    match status {
        ProcessStatus::Exited { code } => TerminalStatusEvidence::Exited { code: *code },
        ProcessStatus::SignalTerminated { signal } => {
            TerminalStatusEvidence::SignalTerminated { signal: *signal }
        }
        ProcessStatus::Unknown
        | ProcessStatus::Cancelled
        | ProcessStatus::SpawnError { .. }
        | ProcessStatus::ProlongedSilence { .. } => TerminalStatusEvidence::Unknown,
    }
}

fn terminal_signal_kind(kind: &ProviderTerminalSignalKind) -> TerminalSignalKind {
    match kind {
        ProviderTerminalSignalKind::CleanExit => TerminalSignalKind::CleanExit,
        ProviderTerminalSignalKind::NonzeroExit => TerminalSignalKind::NonzeroExit,
        ProviderTerminalSignalKind::SignalExit => TerminalSignalKind::SignalExit,
        ProviderTerminalSignalKind::SpawnError => TerminalSignalKind::SpawnError,
        ProviderTerminalSignalKind::ProlongedSilence => TerminalSignalKind::ProlongedSilence,
        ProviderTerminalSignalKind::Cancelled | ProviderTerminalSignalKind::Unknown => {
            TerminalSignalKind::Unknown
        }
        ProviderTerminalSignalKind::QuotaExhaustedInband
        | ProviderTerminalSignalKind::MaybeQuotaExhausted
        | ProviderTerminalSignalKind::RateLimited => TerminalSignalKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INCIDENT_PROVIDER_ERROR: &str =
        "provider error: opencode UnknownError: Failed to execute statement";

    fn provider_signal(
        kind: ProviderTerminalSignalKind,
        evidence: Option<&str>,
    ) -> ProviderTerminalSignal {
        ProviderTerminalSignal {
            kind,
            evidence: evidence.map(str::to_string),
            observed_at_unix_ms: 1_780_808_654_364,
        }
    }

    #[test]
    fn unknown_provider_error_signal_with_exit_zero_maps_synthetic_failure_and_reason() {
        let outcome = map_terminal_cancel_outcome(
            &ProcessStatus::Exited { code: 0 },
            &provider_signal(
                ProviderTerminalSignalKind::Unknown,
                Some(INCIDENT_PROVIDER_ERROR),
            ),
            "opencode",
        );

        assert_eq!(outcome.exit_code, -1);
        let reason = outcome
            .terminal_reason
            .as_deref()
            .expect("provider error evidence should become terminal_reason");
        assert!(
            reason.contains("Failed to execute statement"),
            "terminal_reason should preserve provider evidence: {reason}"
        );
    }

    #[test]
    fn clean_exit_signal_with_exit_zero_stays_success_without_reason() {
        let outcome = map_terminal_cancel_outcome(
            &ProcessStatus::Exited { code: 0 },
            &provider_signal(ProviderTerminalSignalKind::CleanExit, Some("clean exit")),
            "opencode",
        );

        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.terminal_reason, None);
    }

    #[test]
    fn unknown_provider_error_signal_with_real_nonzero_preserves_real_code() {
        let outcome = map_terminal_cancel_outcome(
            &ProcessStatus::Exited { code: 3 },
            &provider_signal(
                ProviderTerminalSignalKind::Unknown,
                Some(INCIDENT_PROVIDER_ERROR),
            ),
            "opencode",
        );

        assert_eq!(outcome.exit_code, 3);
        let reason = outcome
            .terminal_reason
            .as_deref()
            .expect("provider error evidence should remain terminal_reason");
        assert!(reason.contains("Failed to execute statement"));
    }
}
