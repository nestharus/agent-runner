//! Role: formatter.

use crate::executor::terminal_signal::{TerminalSignalKind, TerminalStatusEvidence};
use oulipoly_provider::generated::TerminalSignalKind as ProviderTerminalSignalKind;

pub(crate) fn classify_result_reason(
    provider_kind: &ProviderTerminalSignalKind,
    status: &TerminalStatusEvidence,
) -> Option<String> {
    match provider_kind {
        ProviderTerminalSignalKind::CleanExit => None,
        ProviderTerminalSignalKind::NonzeroExit => Some("exit_nonzero".to_string()),
        ProviderTerminalSignalKind::SignalExit => Some(signal_reason(status)),
        ProviderTerminalSignalKind::SpawnError => Some("spawn_error".to_string()),
        ProviderTerminalSignalKind::QuotaExhaustedInband => {
            Some("quota_exhausted_inband".to_string())
        }
        ProviderTerminalSignalKind::MaybeQuotaExhausted => {
            Some("maybe_quota_exhausted".to_string())
        }
        ProviderTerminalSignalKind::RateLimited => Some("rate_limited".to_string()),
        ProviderTerminalSignalKind::ProlongedSilence => Some("bounded_silence".to_string()),
        ProviderTerminalSignalKind::Cancelled => Some("cancelled".to_string()),
        ProviderTerminalSignalKind::Unknown => Some("unknown_exit".to_string()),
    }
}

pub(crate) fn fixed_reason_for_kind(kind: TerminalSignalKind) -> Option<&'static str> {
    match kind {
        TerminalSignalKind::CleanExit => None,
        TerminalSignalKind::QuotaExhaustedInband => Some("quota_exhausted_inband"),
        TerminalSignalKind::MaybeQuotaExhausted => Some("maybe_quota_exhausted"),
        TerminalSignalKind::RateLimited => Some("rate_limited"),
        TerminalSignalKind::ProlongedSilence => Some("bounded_silence"),
        TerminalSignalKind::NonzeroExit => Some("exit_nonzero"),
        TerminalSignalKind::SignalExit => Some("signal_exit"),
        TerminalSignalKind::SpawnError => Some("spawn_error"),
        TerminalSignalKind::Unknown => Some("unknown_exit"),
    }
}

fn signal_reason(status: &TerminalStatusEvidence) -> String {
    match status {
        TerminalStatusEvidence::SignalTerminated { signal } => {
            format!("signal:{}", signal_name(*signal))
        }
        _ => "signal_exit".to_string(),
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".to_string(),
        libc::SIGINT => "SIGINT".to_string(),
        libc::SIGQUIT => "SIGQUIT".to_string(),
        libc::SIGILL => "SIGILL".to_string(),
        libc::SIGTRAP => "SIGTRAP".to_string(),
        libc::SIGABRT => "SIGABRT".to_string(),
        libc::SIGBUS => "SIGBUS".to_string(),
        libc::SIGFPE => "SIGFPE".to_string(),
        libc::SIGKILL => "SIGKILL".to_string(),
        libc::SIGUSR1 => "SIGUSR1".to_string(),
        libc::SIGSEGV => "SIGSEGV".to_string(),
        libc::SIGUSR2 => "SIGUSR2".to_string(),
        libc::SIGPIPE => "SIGPIPE".to_string(),
        libc::SIGALRM => "SIGALRM".to_string(),
        libc::SIGTERM => "SIGTERM".to_string(),
        libc::SIGCHLD => "SIGCHLD".to_string(),
        libc::SIGCONT => "SIGCONT".to_string(),
        libc::SIGSTOP => "SIGSTOP".to_string(),
        libc::SIGTSTP => "SIGTSTP".to_string(),
        libc::SIGTTIN => "SIGTTIN".to_string(),
        libc::SIGTTOU => "SIGTTOU".to_string(),
        _ => signal.to_string(),
    }
}

#[cfg(not(unix))]
fn signal_name(signal: i32) -> String {
    signal.to_string()
}
