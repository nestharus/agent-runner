//! Role: mapper.

use crate::executor::terminal_signal::TerminalStatusEvidence;
use oulipoly_provider::generated::ProcessStatus;

pub(crate) fn exit_code(status: &ProcessStatus) -> i32 {
    match status {
        ProcessStatus::Exited { code } => *code,
        ProcessStatus::SignalTerminated { signal } => 128 + *signal,
        ProcessStatus::Cancelled => 130,
        ProcessStatus::SpawnError { .. } => 1,
        ProcessStatus::ProlongedSilence { .. } => 124,
        ProcessStatus::Unknown => 1,
    }
}

pub(crate) fn terminal_status_evidence(status: &ProcessStatus) -> TerminalStatusEvidence {
    match status {
        ProcessStatus::Exited { code } => TerminalStatusEvidence::Exited { code: *code },
        ProcessStatus::SignalTerminated { signal } => {
            TerminalStatusEvidence::SignalTerminated { signal: *signal }
        }
        ProcessStatus::SpawnError { reason } => TerminalStatusEvidence::SpawnError {
            reason: reason.clone(),
        },
        ProcessStatus::ProlongedSilence { reason } => TerminalStatusEvidence::ProlongedSilence {
            reason: reason.clone(),
        },
        ProcessStatus::Cancelled | ProcessStatus::Unknown => TerminalStatusEvidence::Unknown,
    }
}
