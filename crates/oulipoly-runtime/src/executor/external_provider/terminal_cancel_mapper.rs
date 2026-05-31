//! Role: mapper.

use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
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
    TerminalCancelOutcome {
        exit_code: exit_code(status),
        terminal_reason: terminal_reason(status),
        terminal_signal: TerminalSignal {
            kind: terminal_signal_kind(&signal.kind),
            provider_name: provider_name.to_string(),
            evidence: signal
                .evidence
                .clone()
                .unwrap_or_else(|| signal.kind.as_str().to_string()),
            observed_at: UNIX_EPOCH + Duration::from_millis(signal.observed_at_unix_ms),
        },
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

fn terminal_reason(status: &ProcessStatus) -> Option<String> {
    match status {
        ProcessStatus::Cancelled => Some("cancelled".to_string()),
        ProcessStatus::SpawnError { reason } | ProcessStatus::ProlongedSilence { reason } => {
            Some(reason.clone())
        }
        ProcessStatus::Unknown => Some("unknown".to_string()),
        ProcessStatus::Exited { .. } | ProcessStatus::SignalTerminated { .. } => None,
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
