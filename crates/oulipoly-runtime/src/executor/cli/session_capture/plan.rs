//! ## Declared roles
//!
//! Roles: mapper, orchestration, validator.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs
//!     role: adapter
//!     Translates:
//!       - provider-session-capture-config-contract
//!       - runtime-capture-plan-contract
//!       - runtime-last-message-sidecar-contract
//! ```

use super::args::{forced_flag_capture_args, stdout_json_event_capture_args};
use super::messages::required_capture_field_message;
use super::paths::last_message_capture_path;
use oulipoly_config::{SessionCapture, SessionCaptureKind};
use std::path::PathBuf;

pub(in crate::executor::cli) enum CapturePlan {
    None,
    ForcedFlagVerified {
        requested_session_id: String,
    },
    StdoutJsonEvent {
        event_type: String,
        event_id_path: String,
        last_message_path: PathBuf,
    },
}

pub(in crate::executor::cli) fn build_capture_plan(
    capture: Option<&SessionCapture>,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let Some(capture) = capture else {
        return Ok((CapturePlan::None, Vec::new(), Vec::new()));
    };

    match capture.kind {
        SessionCaptureKind::None => Ok(empty_capture_plan()),
        SessionCaptureKind::ForcedFlagVerified => {
            build_forced_flag_capture_plan(capture, start_known_provider_session_id)
        }
        SessionCaptureKind::StdoutJsonEvent => build_stdout_json_event_capture_plan(capture),
    }
}

fn empty_capture_plan() -> (CapturePlan, Vec<String>, Vec<PathBuf>) {
    (CapturePlan::None, Vec::new(), Vec::new())
}

fn build_forced_flag_capture_plan(
    capture: &SessionCapture,
    start_known_provider_session_id: Option<&str>,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let flag = required_capture_field(&capture.flag, "session_capture.flag")?;
    let requested_session_id = capture_requested_session_id(start_known_provider_session_id);
    let args = forced_flag_capture_args(flag, &requested_session_id, capture);
    Ok((
        CapturePlan::ForcedFlagVerified {
            requested_session_id,
        },
        args,
        Vec::new(),
    ))
}

fn build_stdout_json_event_capture_plan(
    capture: &SessionCapture,
) -> Result<(CapturePlan, Vec<String>, Vec<PathBuf>), String> {
    let json_flag = required_capture_field(&capture.json_flag, "session_capture.json_flag")?;
    let last_message_flag = required_capture_field(
        &capture.last_message_flag,
        "session_capture.last_message_flag",
    )?;
    let event_type = required_capture_field(&capture.event_type, "session_capture.event_type")?;
    let event_id_path =
        required_capture_field(&capture.event_id_path, "session_capture.event_id_path")?;
    let last_message_path = last_message_capture_path();
    Ok((
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            last_message_path: last_message_path.clone(),
        },
        stdout_json_event_capture_args(json_flag, last_message_flag, &last_message_path),
        vec![last_message_path],
    ))
}

fn required_capture_field(field: &Option<String>, name: &str) -> Result<String, String> {
    field
        .clone()
        .ok_or_else(|| required_capture_field_message(name))
}

fn capture_requested_session_id(start_known_provider_session_id: Option<&str>) -> String {
    start_known_provider_session_id
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
}
