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

struct StdoutJsonEventShape {
    json_args: Vec<String>,
    last_message_flag: Option<String>,
}

pub(in crate::executor::cli) enum CapturePlan {
    None,
    ForcedFlagVerified {
        requested_session_id: String,
    },
    StdoutJsonEvent {
        event_type: String,
        event_id_path: String,
        last_message_path: Option<PathBuf>,
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
    let shape = stdout_json_event_shape(capture)?;
    let event_type = required_capture_field(&capture.event_type, "session_capture.event_type")?;
    let event_id_path =
        required_capture_field(&capture.event_id_path, "session_capture.event_id_path")?;
    let last_message_path = shape
        .last_message_flag
        .as_ref()
        .map(|_| last_message_capture_path());
    let temp_files = last_message_path.iter().cloned().collect::<Vec<_>>();
    Ok((
        CapturePlan::StdoutJsonEvent {
            event_type,
            event_id_path,
            last_message_path: last_message_path.clone(),
        },
        stdout_json_event_capture_args(
            shape.json_args,
            shape.last_message_flag,
            last_message_path.as_deref(),
        ),
        temp_files,
    ))
}

fn stdout_json_event_shape(capture: &SessionCapture) -> Result<StdoutJsonEventShape, String> {
    if let Some(args) = &capture.json_args {
        if args.is_empty() {
            return Err("session_capture.json_args must be non-empty".to_string());
        }
        if capture.json_flag.is_some() {
            return Err(
                "session_capture.json_flag is not allowed when session_capture.json_args is set"
                    .to_string(),
            );
        }
        if capture.last_message_flag.is_some() {
            return Err(
                "session_capture.last_message_flag is not allowed when session_capture.json_args is set"
                    .to_string(),
            );
        }
        return Ok(StdoutJsonEventShape {
            json_args: args.clone(),
            last_message_flag: None,
        });
    }

    if let Some(flag) = &capture.json_flag {
        let last_message_flag = required_capture_field(
            &capture.last_message_flag,
            "session_capture.last_message_flag",
        )?;
        return Ok(StdoutJsonEventShape {
            json_args: vec![flag.clone()],
            last_message_flag: Some(last_message_flag),
        });
    }

    Err(required_capture_field_message(
        "session_capture.json_flag or session_capture.json_args",
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
