//! Role: mapper.

use super::terminal_cancel_mapper::map_terminal_cancel_outcome;
use crate::executor::{
    ExecutionResult, SessionCaptureMethod, SessionCaptureResult, SubmittedUserTurn,
};
use crate::services::TerminalClassification;
use oulipoly_provider::stream::{DecodedLaunchEvent, LaunchResult};
use serde_json::Value;

const SUBMITTED_USER_TURN_MARKER: &str = "oulipoly.submitted_user_turn";

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
        submitted_user_turn: submitted_user_turn(&result),
        captured_child_invocations: Vec::new(),
        returned_artifacts: Vec::new(),
    }
}

fn submitted_user_turn(result: &LaunchResult) -> Option<SubmittedUserTurn> {
    result
        .events
        .iter()
        .find_map(submitted_user_turn_from_event)
}

fn submitted_user_turn_from_event(event: &DecodedLaunchEvent) -> Option<SubmittedUserTurn> {
    let DecodedLaunchEvent::Marker { name, value, .. } = event else {
        return None;
    };
    if name != SUBMITTED_USER_TURN_MARKER {
        return None;
    }
    submitted_user_turn_from_marker_value(value)
}

fn submitted_user_turn_from_marker_value(value: &Value) -> Option<SubmittedUserTurn> {
    let provider_session_id = nonempty_marker_string(value, "provider_session_id")
        .or_else(|| nonempty_marker_string(value, "session_id"))?;
    let prompt_sha256 = nonempty_marker_string(value, "prompt_sha256")?;
    Some(SubmittedUserTurn {
        provider_session_id,
        prompt_sha256,
        delivery_nonce: marker_string(value, "delivery_nonce"),
        source: marker_string(value, "source"),
        message_id: marker_string(value, "message_id"),
    })
}

fn nonempty_marker_string(value: &Value, key: &str) -> Option<String> {
    marker_string(value, key).filter(|value| !value.trim().is_empty())
}

fn marker_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
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

pub(crate) fn launch_provider_session_id(result: &LaunchResult) -> Option<String> {
    result
        .exit
        .session
        .as_ref()
        .and_then(provider_session_id_from_value)
}

fn provider_session_id_from_value(value: &Value) -> Option<String> {
    raw_provider_session_id(value)
        .and_then(accepted_provider_session_id)
        .map(owned_provider_session_id)
}

fn raw_provider_session_id(value: &Value) -> Option<&str> {
    value
        .get("provider_session_id")
        .and_then(Value::as_str)
        .or_else(|| value.get("session_id").and_then(Value::as_str))
}

fn accepted_provider_session_id(session_id: &str) -> Option<&str> {
    provider_session_id_is_present(session_id).then_some(session_id)
}

fn provider_session_id_is_present(session_id: &str) -> bool {
    !session_id.is_empty()
}

fn owned_provider_session_id(session_id: &str) -> String {
    session_id.to_string()
}
