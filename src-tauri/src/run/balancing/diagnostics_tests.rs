//! ## Declared roles
//!
//! `mapper`, `validator`

use super::diagnostics::balanced_result_error_category;
use crate::terminal_outcome_adapter::balanced_terminal_signal_for_outcome;
use oulipoly_runtime::executor::terminal_signal::TerminalSignalKind;
use oulipoly_runtime::executor::{
    CapturedChildInvocation, ExecutionResult, SessionCaptureMethod, SessionCaptureResult,
    TerminalSignal,
};
use std::collections::HashMap;
use std::time::SystemTime;

fn result_with_signal(kind: TerminalSignalKind, exit_code: i32) -> ExecutionResult {
    execution_result_with_signal(terminal_signal(kind), exit_code)
}

fn execution_result_with_signal(
    terminal_signal: TerminalSignal,
    exit_code: i32,
) -> ExecutionResult {
    ExecutionResult {
        stdout: Vec::new(),
        stderr: "ordinary provider failure".to_string(),
        exit_code,
        provider_index: 0,
        session_capture: SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
        resume_acceptance: None,
        terminal_reason: None,
        terminal_signal: Some(terminal_signal),
        produced_assistant_response: false,
        prompt_acceptance_attestation: None,
        captured_child_invocations: Vec::<CapturedChildInvocation>::new(),
        returned_artifacts: Vec::new(),
    }
}

fn terminal_signal(kind: TerminalSignalKind) -> TerminalSignal {
    TerminalSignal {
        kind,
        provider_name: "provider-a".to_string(),
        evidence: "typed evidence".to_string(),
        observed_at: SystemTime::UNIX_EPOCH,
    }
}

#[test]
fn resume_fallback_typed_signal_parity() {
    let services = crate::wiring::AgentRuntimeServices::cli_defaults().unwrap();
    let models = HashMap::new();

    let quota = balanced_result_error_category(
        &services,
        &result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1),
        &models,
        None,
    );
    let maybe = balanced_result_error_category(
        &services,
        &result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1),
        &models,
        None,
    );
    let clean = balanced_result_error_category(
        &services,
        &result_with_signal(TerminalSignalKind::CleanExit, 0),
        &models,
        None,
    );

    assert_eq!(quota.as_deref(), Some("quota_exhausted"));
    assert_eq!(maybe, None);
    assert_eq!(clean, None);
}

#[test]
fn balanced_terminal_signal_for_outcome_handles_new_kind() {
    let maybe = result_with_signal(TerminalSignalKind::MaybeQuotaExhausted, 1);
    let quota = result_with_signal(TerminalSignalKind::QuotaExhaustedInband, 1);

    assert_eq!(
        balanced_terminal_signal_for_outcome(&maybe, true).map(|signal| signal.kind),
        Some(TerminalSignalKind::MaybeQuotaExhausted)
    );
    assert_eq!(
        balanced_terminal_signal_for_outcome(&quota, true).map(|signal| signal.kind),
        Some(TerminalSignalKind::QuotaExhaustedInband)
    );
}
