//! Role: formatter.

use crate::executor::terminal_signal::TerminalSignalKind;

pub(crate) fn fixed_reason_for_kind(kind: TerminalSignalKind) -> Option<&'static str> {
    match kind {
        TerminalSignalKind::CleanExit => None,
        TerminalSignalKind::QuotaExhaustedInband => Some("quota_exhausted_inband"),
        TerminalSignalKind::MaybeQuotaExhausted => Some("maybe_quota_exhausted"),
        TerminalSignalKind::RateLimited => Some("rate_limited"),
        TerminalSignalKind::ProviderStorageContention => Some("provider_storage_contention"),
        TerminalSignalKind::ProviderUnavailable => Some("provider_unavailable"),
        TerminalSignalKind::ProlongedSilence => Some("bounded_silence"),
        TerminalSignalKind::NonzeroExit => Some("exit_nonzero"),
        TerminalSignalKind::SignalExit => Some("signal_exit"),
        TerminalSignalKind::SpawnError => Some("spawn_error"),
        TerminalSignalKind::Unknown => Some("unknown_exit"),
    }
}
