//! ## Declared roles
//!
//! Roles: mapper, accessor, predicate, filter, validator.
//!
//! - mapper: `map_launch_result_with_terminal_classification`,
//!   `launch_session_capture`, and `launch_provider_session_id` translate
//!   provider launch results into runtime execution, terminal, session-capture,
//!   submitted-turn, and assistant-productivity surfaces.
//! - accessor: `marker_string` and `raw_provider_session_id` read optional
//!   marker/session fields from launch JSON values.
//! - predicate: `provider_session_id_is_present` reports whether a session
//!   identifier is present before runtime capture.
//! - filter: `nonempty_marker_string` and `accepted_provider_session_id` select
//!   non-empty/accepted marker and session values, dropping empties.
//! - validator: `submitted_user_turn_from_marker_value` validates the
//!   submitted-user-turn marker payload before constructing the runtime DTO.
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
//!       - submitted-user-turn-marker-contract
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
//!     role: intrinsic-surface
//!     Domain: external-provider launch result mapping
//!     Owns:
//!       - LaunchResult stdout/stderr projection into ExecutionResult
//!       - terminal classification override and fallback mapping
//!       - launch session object to runtime session-capture mapping
//!       - submitted-user-turn marker extraction semantics
//!       - assistant-response productivity evidence projection
//!       - returned-artifact and child-invocation empty defaults for launch results
//! ```

use super::terminal_cancel_mapper::map_terminal_cancel_outcome;
use crate::executor::assistant_response::launch_result_produced_assistant_response;
use crate::executor::{
    ExecutionResult, SessionCaptureMethod, SessionCaptureResult, SubmittedUserTurn,
};
use crate::services::TerminalClassification;
use oulipoly_provider::generated::{PROMPT_ACCEPTANCE_V1, PROMPT_ACCEPTED_MARKER_V1};
use oulipoly_provider::stream::LaunchResult;
use serde_json::Value;

pub(crate) const PROVIDER_SESSION_MARKER: &str = "oulipoly.provider_session";

pub(crate) fn map_launch_result_with_terminal_classification(
    result: LaunchResult,
    provider_index: usize,
    provider_name: &str,
    classification: Option<TerminalClassification>,
    prompt_acceptance_v1: bool,
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
    let produced_assistant_response = launch_result_produced_assistant_response(&result);
    ExecutionResult {
        stdout,
        stderr,
        exit_code: terminal.exit_code,
        provider_index,
        session_capture: launch_session_capture(&result),
        resume_acceptance: None,
        terminal_reason: terminal.terminal_reason,
        terminal_signal: Some(terminal.terminal_signal),
        produced_assistant_response,
        submitted_user_turn: submitted_user_turn(&result, prompt_acceptance_v1),
        captured_child_invocations: Vec::new(),
        returned_artifacts: Vec::new(),
    }
}

fn submitted_user_turn(
    result: &LaunchResult,
    prompt_acceptance_v1: bool,
) -> Option<SubmittedUserTurn> {
    if !prompt_acceptance_v1 {
        return None;
    }
    result
        .retained_marker_value(PROMPT_ACCEPTED_MARKER_V1)
        .and_then(submitted_user_turn_from_marker_value)
}

fn submitted_user_turn_from_marker_value(value: &Value) -> Option<SubmittedUserTurn> {
    if nonempty_marker_string(value, "protocol").as_deref() != Some(PROMPT_ACCEPTANCE_V1) {
        return None;
    }
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
