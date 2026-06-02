//! Role: mapper.

use super::reason_format;
use super::status_projection;
use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
use crate::services::{TerminalClassification, TerminalClassifyServiceRequest};
use oulipoly_provider::generated::{
    TerminalClassifyResult, TerminalSignalKind as ProviderTerminalSignalKind,
};
use std::time::{Duration, UNIX_EPOCH};

pub(crate) fn map_terminal_classify_result(
    request: &TerminalClassifyServiceRequest,
    result: TerminalClassifyResult,
) -> Result<TerminalClassification, super::errors::TerminalClassifyError> {
    let status = status_projection::terminal_status_evidence(&request.status);
    let terminal_reason =
        reason_format::classify_result_reason(&result.terminal_signal.kind, &status);
    Ok(TerminalClassification {
        exit_code: status_projection::exit_code(&request.status),
        terminal_reason,
        terminal_signal: TerminalSignal {
            kind: runtime_kind(&result.terminal_signal.kind),
            provider_name: request.provider_name.clone(),
            evidence: signal_evidence(&result.terminal_signal),
            observed_at: UNIX_EPOCH
                + Duration::from_millis(result.terminal_signal.observed_at_unix_ms),
        },
    })
}

fn runtime_kind(kind: &ProviderTerminalSignalKind) -> TerminalSignalKind {
    match kind {
        ProviderTerminalSignalKind::CleanExit => TerminalSignalKind::CleanExit,
        ProviderTerminalSignalKind::NonzeroExit => TerminalSignalKind::NonzeroExit,
        ProviderTerminalSignalKind::SignalExit => TerminalSignalKind::SignalExit,
        ProviderTerminalSignalKind::SpawnError => TerminalSignalKind::SpawnError,
        ProviderTerminalSignalKind::QuotaExhaustedInband => {
            TerminalSignalKind::QuotaExhaustedInband
        }
        ProviderTerminalSignalKind::MaybeQuotaExhausted => TerminalSignalKind::MaybeQuotaExhausted,
        ProviderTerminalSignalKind::RateLimited => TerminalSignalKind::RateLimited,
        ProviderTerminalSignalKind::ProlongedSilence => TerminalSignalKind::ProlongedSilence,
        ProviderTerminalSignalKind::Cancelled | ProviderTerminalSignalKind::Unknown => {
            TerminalSignalKind::Unknown
        }
    }
}

fn signal_evidence(signal: &oulipoly_provider::generated::TerminalSignal) -> String {
    signal
        .evidence
        .clone()
        .unwrap_or_else(|| signal.kind.as_str().to_string())
}
