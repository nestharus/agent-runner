//! ## Declared roles
//!
//! Roles: mapper, predicate, accessor, formatter, orchestration.
//!
//! - mapper: [`finalize_capture`] and the result constructors map capture
//!   plans plus parsed provider stdout onto [`SessionCaptureResult`].
//! - predicate: [`forced_flag_readback_matches`],
//!   [`restore_plain_stdout_should_warn`].
//! - accessor: [`restore_plain_stdout_from_file`] reads the runtime-created
//!   last-message sidecar.
//! - formatter: [`restore_plain_stdout_warning`].
//! - orchestration: [`maybe_restore_plain_stdout`] chooses sidecar restore vs.
//!   stdout fallback.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/capture_result.rs
//!     role: adapter
//!     Translates:
//!       - session-capture-result-dto-contract
//!       - forced-flag-verified-jsonl-contract
//!       - stdout-json-event-session-contract
//!       - runtime-last-message-sidecar-contract
//! ```

use super::super::{SessionCaptureMethod, SessionCaptureResult};
use super::session_capture::{
    CapturePlan, parse_forced_flag_verified_session_id, parse_stdout_json_event_session_id,
};
use std::path::Path;

pub(super) fn finalize_capture(
    plan: &CapturePlan,
    stdout: &[u8],
    streamed_session_id: Option<&str>,
) -> SessionCaptureResult {
    match plan {
        CapturePlan::None => no_capture_result(),
        CapturePlan::ForcedFlagVerified {
            requested_session_id,
        } => finalize_forced_flag_capture(requested_session_id, stdout),
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            ..
        } => finalize_stdout_json_event_capture(
            stdout,
            event_type,
            event_id_path,
            streamed_session_id,
        ),
    }
}

fn no_capture_result() -> SessionCaptureResult {
    SessionCaptureResult {
        session_id: None,
        method: SessionCaptureMethod::None,
    }
}

fn finalize_forced_flag_capture(requested_session_id: &str, stdout: &[u8]) -> SessionCaptureResult {
    forced_flag_capture_result(
        requested_session_id,
        parse_forced_flag_verified_session_id(stdout),
    )
}

fn forced_flag_capture_result(
    requested_session_id: &str,
    parsed: Result<String, String>,
) -> SessionCaptureResult {
    match parsed {
        Ok(observed) => forced_flag_observed_result(requested_session_id, &observed),
        Err(err) => failed_capture_result(err),
    }
}

fn forced_flag_observed_result(requested_session_id: &str, observed: &str) -> SessionCaptureResult {
    if forced_flag_readback_matches(requested_session_id, observed) {
        forced_flag_success_result(requested_session_id)
    } else {
        readback_mismatch_result(requested_session_id, observed)
    }
}

fn forced_flag_readback_matches(requested_session_id: &str, observed: &str) -> bool {
    observed == requested_session_id
}

fn forced_flag_success_result(requested_session_id: &str) -> SessionCaptureResult {
    SessionCaptureResult {
        session_id: Some(requested_session_id.to_string()),
        method: SessionCaptureMethod::ForcedFlagVerified,
    }
}

fn readback_mismatch_result(requested_session_id: &str, observed: &str) -> SessionCaptureResult {
    failed_capture_result(readback_mismatch_evidence(requested_session_id, observed))
}

fn readback_mismatch_evidence(requested_session_id: &str, observed: &str) -> String {
    format!("readback mismatch: requested {requested_session_id}, observed {observed}")
}

fn finalize_stdout_json_event_capture(
    stdout: &[u8],
    event_type: &str,
    event_id_path: &str,
    streamed_session_id: Option<&str>,
) -> SessionCaptureResult {
    if let Some(session_id) = streamed_session_id {
        return stdout_json_event_capture_result(Ok(session_id.to_string()));
    }
    stdout_json_event_capture_result(parse_stdout_json_event_session_id(
        stdout,
        event_type,
        event_id_path,
    ))
}

fn stdout_json_event_capture_result(parsed: Result<String, String>) -> SessionCaptureResult {
    match parsed {
        Ok(session_id) => SessionCaptureResult {
            session_id: Some(session_id),
            method: SessionCaptureMethod::StdoutJsonEvent,
        },
        Err(err) => failed_capture_result(err),
    }
}

fn failed_capture_result(err: String) -> SessionCaptureResult {
    SessionCaptureResult {
        session_id: None,
        method: SessionCaptureMethod::Failed(err),
    }
}

pub(super) fn maybe_restore_plain_stdout(
    plan: &CapturePlan,
    capture: &SessionCaptureResult,
    stdout: &[u8],
) -> Vec<u8> {
    match plan {
        CapturePlan::StdoutJsonEvent {
            last_message_path: Some(last_message_path),
            ..
        } => restore_plain_stdout_from_file(last_message_path, capture, stdout),
        _ => fallback_plain_stdout(stdout),
    }
}

fn restore_plain_stdout_from_file(
    last_message_path: &Path,
    capture: &SessionCaptureResult,
    stdout: &[u8],
) -> Vec<u8> {
    match read_plain_stdout_sidecar(last_message_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            warn_restore_plain_stdout(last_message_path, capture, &err);
            fallback_plain_stdout(stdout)
        }
    }
}

fn read_plain_stdout_sidecar(last_message_path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(last_message_path)
}

fn warn_restore_plain_stdout(
    last_message_path: &Path,
    capture: &SessionCaptureResult,
    err: &std::io::Error,
) {
    if restore_plain_stdout_should_warn(capture) {
        emit_restore_plain_stdout_warning(last_message_path, err);
    }
}

fn restore_plain_stdout_should_warn(capture: &SessionCaptureResult) -> bool {
    matches!(capture.method, SessionCaptureMethod::StdoutJsonEvent)
}

fn emit_restore_plain_stdout_warning(last_message_path: &Path, err: &std::io::Error) {
    eprintln!("{}", restore_plain_stdout_warning(last_message_path, err));
}

fn restore_plain_stdout_warning(last_message_path: &Path, err: &std::io::Error) -> String {
    format!(
        "Warning: failed to read reconstructed last-message output {}: {err}",
        last_message_path.display()
    )
}

fn fallback_plain_stdout(stdout: &[u8]) -> Vec<u8> {
    stdout.to_vec()
}
