//! Role: mapper.

use super::terminal_cancel_mapper::map_terminal_cancel_outcome;
use crate::executor::{ExecutionResult, SessionCaptureMethod, SessionCaptureResult};
use crate::services::TerminalClassification;
use oulipoly_provider::stream::LaunchResult;
use serde_json::Value;

pub(crate) fn map_launch_result_with_terminal_classification(
    result: LaunchResult,
    provider_index: usize,
    provider_name: &str,
    classification: Option<TerminalClassification>,
) -> ExecutionResult {
    let stdout = result.stdout_bytes();
    let stderr = String::from_utf8_lossy(&result.stderr_bytes()).into_owned();
    let terminal = map_terminal_cancel_outcome(
        &result.exit.status,
        &result.exit.terminal_signal,
        provider_name,
    );
    let terminal = classification.unwrap_or(TerminalClassification {
        exit_code: terminal.exit_code,
        terminal_reason: terminal.terminal_reason,
        terminal_signal: terminal.terminal_signal,
    });
    ExecutionResult {
        stdout,
        stderr,
        exit_code: terminal.exit_code,
        provider_index,
        session_capture: launch_session_capture(&result),
        resume_acceptance: None,
        terminal_reason: terminal.terminal_reason,
        terminal_signal: Some(terminal.terminal_signal),
        captured_child_invocations: Vec::new(),
        returned_artifacts: Vec::new(),
    }
}

fn launch_session_capture(result: &LaunchResult) -> SessionCaptureResult {
    match launch_provider_session_id(result) {
        Some(session_id) => SessionCaptureResult {
            session_id: Some(session_id),
            method: SessionCaptureMethod::ExternalProviderLaunch,
        },
        None => SessionCaptureResult {
            session_id: None,
            method: SessionCaptureMethod::None,
        },
    }
}

fn launch_provider_session_id(result: &LaunchResult) -> Option<String> {
    result
        .exit
        .session
        .as_ref()
        .and_then(provider_session_id_from_value)
}

fn provider_session_id_from_value(value: &Value) -> Option<String> {
    value
        .get("provider_session_id")
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty())
        .map(str::to_string)
}
