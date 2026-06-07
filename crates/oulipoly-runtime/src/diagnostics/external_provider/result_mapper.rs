//! Role: mapper.

use super::status_projection;
use crate::executor::cli::{terminal_exit_code_from_signal, terminal_reason_from_signal_status};
use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
use crate::services::{TerminalClassification, TerminalClassifyServiceRequest};
use oulipoly_provider::generated::{
    ProcessStatus, TerminalClassifyResult, TerminalSignalKind as ProviderTerminalSignalKind,
};
use std::time::{Duration, UNIX_EPOCH};

pub(crate) fn map_terminal_classify_result(
    request: &TerminalClassifyServiceRequest,
    result: TerminalClassifyResult,
) -> Result<TerminalClassification, super::errors::TerminalClassifyError> {
    let status = status_projection::terminal_status_evidence(&request.status);
    let terminal_signal = TerminalSignal {
        kind: runtime_kind(&result.terminal_signal.kind),
        provider_name: request.provider_name.clone(),
        evidence: signal_evidence(&result.terminal_signal),
        observed_at: UNIX_EPOCH + Duration::from_millis(result.terminal_signal.observed_at_unix_ms),
    };
    let terminal_reason = terminal_reason(&request.status, &terminal_signal, &status);
    Ok(TerminalClassification {
        exit_code: terminal_exit_code_from_signal(
            &terminal_signal,
            status_projection::exit_code(&request.status),
        ),
        terminal_reason,
        terminal_signal,
    })
}

fn terminal_reason(
    status: &ProcessStatus,
    signal: &TerminalSignal,
    status_evidence: &crate::executor::terminal_signal::TerminalStatusEvidence,
) -> Option<String> {
    match status {
        ProcessStatus::Cancelled => Some("cancelled".to_string()),
        ProcessStatus::Exited { .. }
        | ProcessStatus::SignalTerminated { .. }
        | ProcessStatus::SpawnError { .. }
        | ProcessStatus::ProlongedSilence { .. }
        | ProcessStatus::Unknown => {
            terminal_reason_from_signal_status(signal, Some(status_evidence))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_provider::generated::{
        ProcessStatus, TerminalSignal as ProviderTerminalSignal, TerminalSignalKind,
    };
    use std::time::SystemTime;

    const INCIDENT_PROVIDER_ERROR: &str =
        "provider error: opencode UnknownError: Failed to execute statement";

    fn request(status: ProcessStatus) -> TerminalClassifyServiceRequest {
        TerminalClassifyServiceRequest {
            model_name: "opencode-model".to_string(),
            provider_name: "opencode".to_string(),
            settings_id: "opencode".to_string(),
            stdout: Vec::new(),
            stderr: Vec::new(),
            status,
            observed_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn result(kind: TerminalSignalKind, evidence: Option<&str>) -> TerminalClassifyResult {
        TerminalClassifyResult {
            terminal_signal: ProviderTerminalSignal {
                kind,
                evidence: evidence.map(str::to_string),
                observed_at_unix_ms: 1_780_808_654_364,
            },
        }
    }

    #[test]
    fn unknown_provider_error_signal_with_exit_zero_maps_synthetic_failure_and_reason() {
        let classification = map_terminal_classify_result(
            &request(ProcessStatus::Exited { code: 0 }),
            result(TerminalSignalKind::Unknown, Some(INCIDENT_PROVIDER_ERROR)),
        )
        .expect("classification should map");

        assert_eq!(classification.exit_code, -1);
        let reason = classification
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
        let classification = map_terminal_classify_result(
            &request(ProcessStatus::Exited { code: 0 }),
            result(TerminalSignalKind::CleanExit, Some("clean exit")),
        )
        .expect("classification should map");

        assert_eq!(classification.exit_code, 0);
        assert_eq!(classification.terminal_reason, None);
    }

    #[test]
    fn unknown_provider_error_signal_with_real_nonzero_preserves_real_code() {
        let classification = map_terminal_classify_result(
            &request(ProcessStatus::Exited { code: 3 }),
            result(TerminalSignalKind::Unknown, Some(INCIDENT_PROVIDER_ERROR)),
        )
        .expect("classification should map");

        assert_eq!(classification.exit_code, 3);
        let reason = classification
            .terminal_reason
            .as_deref()
            .expect("provider error evidence should remain terminal_reason");
        assert!(reason.contains("Failed to execute statement"));
    }
}
