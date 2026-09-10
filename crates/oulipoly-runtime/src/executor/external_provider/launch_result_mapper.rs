//! ## Declared roles
//!
//! Roles: mapper, parser.
//!
//! - mapper: `map_launch_result_with_terminal_classification`,
//!   `launch_session_capture`, and `launch_provider_session_id` translate
//!   provider launch results into runtime execution, terminal, session-capture,
//!   prompt-acceptance-attestation, and assistant-productivity surfaces.
//! - parser: `parse_prompt_acceptance_attestation_marker` parses the negotiated
//!   provider marker while leaving exact host-correlation trust to the separate
//!   prompt-acceptance promotion boundary.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
//!     role: adapter
//!     Translates:
//!       - launch-jsonl-stream-contract
//!       - runtime-execution-result-contract
//!       - terminal-cancel-outcome-contract
//!       - session-capture-contract
//!       - prompt-acceptance-attestation-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
//!     role: intrinsic-surface
//!     Domain: external-provider launch result mapping
//!     Owns:
//!       - LaunchResult stdout/stderr projection into ExecutionResult
//!       - terminal classification override and fallback mapping
//!       - launch session object to runtime session-capture mapping
//!       - prompt-acceptance attestation extraction semantics
//!       - assistant-response productivity evidence projection
//!       - returned-artifact and child-invocation empty defaults for launch results
//! ```

use super::terminal_cancel_mapper::{map_terminal_cancel_outcome, process_exit_code};
use crate::executor::assistant_response::launch_result_produced_assistant_response;
use crate::executor::cli::{
    captured_child_invocations_from_stderr, terminal_exit_code_from_signal,
};
use crate::executor::terminal_signal::{TerminalSignal, TerminalSignalKind};
use crate::executor::{
    ExecutionOutputSpool, ExecutionResult, ExternalProviderSessionAuthority, ReturnedArtifactRef,
    SessionCaptureMethod, SessionCaptureResult,
};
use crate::services::TerminalClassification;
use crate::session_authority::VerifiedSessionAuthority;
use oulipoly_provider::error::ProviderClientError;
use oulipoly_provider::generated::{
    PROMPT_ACCEPTANCE_V1, PROMPT_ACCEPTED_MARKER_V1, ProcessStatus, PromptAcceptedMarkerValueV1,
};
use oulipoly_provider::stream::LaunchResult;
use serde_json::Value;
use std::time::SystemTime;

pub(crate) const PROVIDER_SESSION_MARKER: &str = "oulipoly.provider_session";

pub(crate) struct LaunchOutputArtifacts {
    pub(super) spool: ExecutionOutputSpool,
    pub(super) returned_artifacts: Vec<ReturnedArtifactRef>,
}

pub(crate) fn map_launch_result_with_terminal_classification(
    result: LaunchResult,
    provider_index: usize,
    classification: Option<TerminalClassification>,
    retain_prompt_acceptance_attestation_v1: bool,
    output: LaunchOutputArtifacts,
    authority: &ExternalProviderSessionAuthority,
) -> ExecutionResult {
    let LaunchOutputArtifacts {
        spool: output_spool,
        returned_artifacts,
    } = output;
    let stdout = result.stdout_bytes();
    let stderr = String::from_utf8_lossy(&result.stderr_bytes()).into_owned();
    let terminal = map_terminal_cancel_outcome(
        &result.exit.status,
        &result.exit.terminal_signal,
        &authority.account_name,
    );
    let terminal = classification.unwrap_or(TerminalClassification {
        exit_code: terminal.exit_code,
        terminal_reason: terminal.terminal_reason,
        terminal_signal: terminal.terminal_signal,
    });
    let produced_assistant_response = launch_result_produced_assistant_response(&result);
    let captured_child_invocations = captured_child_invocations_from_stderr(&stderr);
    ExecutionResult {
        stdout,
        stderr,
        output_spool: Some(output_spool),
        exit_code: terminal.exit_code,
        provider_index,
        session_capture: launch_session_capture(&result, authority),
        resume_acceptance: None,
        terminal_reason: terminal.terminal_reason,
        terminal_signal: Some(terminal.terminal_signal),
        produced_assistant_response,
        prompt_acceptance_attestation: prompt_acceptance_attestation(
            &result,
            retain_prompt_acceptance_attestation_v1,
        ),
        captured_child_invocations,
        returned_artifacts,
    }
}

fn prompt_acceptance_attestation(
    result: &LaunchResult,
    retain_prompt_acceptance_attestation_v1: bool,
) -> Option<PromptAcceptedMarkerValueV1> {
    if !retain_prompt_acceptance_attestation_v1 {
        return None;
    }
    result
        .retained_marker_value(PROMPT_ACCEPTED_MARKER_V1)
        .and_then(parse_prompt_acceptance_attestation_marker)
}

pub(crate) fn parse_prompt_acceptance_attestation_marker(
    value: &Value,
) -> Option<PromptAcceptedMarkerValueV1> {
    let attestation: PromptAcceptedMarkerValueV1 = serde_json::from_value(value.clone()).ok()?;
    (attestation.protocol == PROMPT_ACCEPTANCE_V1).then_some(attestation)
}

pub(crate) fn map_missing_final_exit_with_prompt_acceptance(
    error: &ProviderClientError,
    verified_session: Option<&VerifiedSessionAuthority>,
    provider_index: usize,
    provider_name: &str,
    retain_prompt_acceptance_attestation_v1: bool,
    returned_artifacts: Vec<ReturnedArtifactRef>,
    authority: &ExternalProviderSessionAuthority,
) -> Option<ExecutionResult> {
    if error.transport_kind() != "missing_final_exit" {
        return None;
    }
    let verified_session = verified_session?;
    let prompt_acceptance_attestation = retain_prompt_acceptance_attestation_v1
        .then(|| error.retained_launch_marker_value(PROMPT_ACCEPTED_MARKER_V1))
        .flatten()
        .and_then(parse_prompt_acceptance_attestation_marker)
        .filter(|attestation| {
            attestation.provider_session_id == verified_session.provider_session_id()
        });
    let provider_status = error.process_status();
    let signal = TerminalSignal {
        kind: TerminalSignalKind::Unknown,
        provider_name: provider_name.to_string(),
        evidence: missing_final_exit_evidence(provider_status),
        observed_at: SystemTime::now(),
    };
    let exit_code = terminal_exit_code_from_signal(
        &signal,
        provider_status.map(process_exit_code).unwrap_or(1),
    );
    let mut stderr = error.to_string();
    let provider_stderr = error.diagnostics().stderr_text();
    if !provider_stderr.is_empty() {
        stderr.push('\n');
        stderr.push_str(&provider_stderr);
    }
    let captured_child_invocations = captured_child_invocations_from_stderr(&stderr);
    Some(ExecutionResult {
        stdout: Vec::new(),
        stderr,
        output_spool: None,
        exit_code,
        provider_index,
        session_capture: SessionCaptureResult {
            session_id: Some(verified_session.provider_session_id().to_string()),
            method: SessionCaptureMethod::ExternalProviderLaunch(authority.clone()),
        },
        resume_acceptance: None,
        terminal_reason: Some("external_provider_missing_final_exit".to_string()),
        terminal_signal: Some(signal),
        produced_assistant_response: false,
        prompt_acceptance_attestation,
        captured_child_invocations,
        returned_artifacts,
    })
}

pub(crate) fn launch_failure_provider_session_id(error: &ProviderClientError) -> Option<String> {
    error
        .retained_launch_marker_value(PROVIDER_SESSION_MARKER)
        .and_then(marker_provider_session_id)
}

fn missing_final_exit_evidence(status: Option<&ProcessStatus>) -> String {
    let status = match status {
        Some(ProcessStatus::Exited { code }) => format!("provider_process=exited:{code}"),
        Some(ProcessStatus::SignalTerminated { signal }) => {
            format!("provider_process=signal_terminated:{signal}")
        }
        Some(ProcessStatus::SpawnError { reason }) => {
            format!("provider_process=spawn_error:{reason}")
        }
        Some(ProcessStatus::ProlongedSilence { reason }) => {
            format!("provider_process=prolonged_silence:{reason}")
        }
        Some(ProcessStatus::Cancelled) => "provider_process=cancelled".to_string(),
        Some(ProcessStatus::Unknown) => "provider_process=unknown".to_string(),
        None => "provider_process=unreported".to_string(),
    };
    format!("missing_final_exit;{status}")
}

fn launch_session_capture(
    result: &LaunchResult,
    authority: &ExternalProviderSessionAuthority,
) -> SessionCaptureResult {
    match launch_provider_session_id(result) {
        Some(session_id) => SessionCaptureResult {
            session_id: Some(session_id),
            method: SessionCaptureMethod::ExternalProviderLaunch(authority.clone()),
        },
        None => SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
    }
}

pub(crate) fn launch_provider_session_id(result: &LaunchResult) -> Option<String> {
    result
        .exit
        .session
        .as_ref()
        .and_then(provider_session_id_from_value)
        .or_else(|| {
            result
                .retained_marker_value(PROVIDER_SESSION_MARKER)
                .and_then(marker_provider_session_id)
        })
}

pub(crate) fn marker_provider_session_id(value: &Value) -> Option<String> {
    provider_session_id_from_value(value)
}

fn provider_session_id_from_value(value: &Value) -> Option<String> {
    value
        .get("provider_session_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("session_id").and_then(Value::as_str))
        .filter(|session_id| !session_id.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_authority::{
        AuthoritativeSessionObservation, SessionAuthorityExpectation, verify_session_authority,
    };
    use oulipoly_provider::stream::LaunchJsonlReader;

    #[test]
    fn age345_missing_final_exit_retains_the_supplied_launch_endpoint() {
        let marker = r#"{"contract":"oulipoly.provider/v1","request_id":"req-1","seq":1,"time_unix_ms":1,"kind":"marker","name":"oulipoly.provider_session","value":{"provider_session_id":"native-a"}}"#;
        let error = LaunchJsonlReader::new("req-1")
            .read(format!("{marker}\n").as_bytes())
            .unwrap_err();
        assert_eq!(error.transport_kind(), "missing_final_exit");
        let verified = verify_session_authority(
            SessionAuthorityExpectation {
                account_name: "account-a",
                provider_session_id: Some("native-a"),
            },
            Some(AuthoritativeSessionObservation {
                account_name: "account-a",
                provider_session_id: "native-a",
            }),
        )
        .unwrap()
        .unwrap();
        let authority = ExternalProviderSessionAuthority {
            account_name: "account-a".into(),
            provider_instance_id: "pinned-instance".into(),
            settings_id: "pinned-settings".into(),
        };
        let result = map_missing_final_exit_with_prompt_acceptance(
            &error,
            Some(&verified),
            0,
            "account-a",
            false,
            Vec::new(),
            &authority,
        )
        .unwrap();
        assert_eq!(
            result.session_capture.session_id.as_deref(),
            Some("native-a")
        );
        assert_eq!(
            result.session_capture.method,
            SessionCaptureMethod::ExternalProviderLaunch(authority.clone())
        );
        assert_ne!(result.exit_code, 0);
        assert!(
            map_missing_final_exit_with_prompt_acceptance(
                &error,
                None,
                0,
                "account-a",
                false,
                Vec::new(),
                &authority,
            )
            .is_none(),
            "unverified native marker must not mint endpoint authority"
        );
    }
}
