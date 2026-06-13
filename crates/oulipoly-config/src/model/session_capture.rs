//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-config/src/model/session_capture.rs
//!     role: intrinsic-surface
//!     Domain: model-session-capture
//!     Owns:
//!       - SessionCapture session-id extraction configuration
//!       - SessionCaptureKind variant-specific required-field validation
//!       - stdout JSON event capture fields subordinate to model session capture
//! ```
//!
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCapture {
    pub kind: SessionCaptureKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readback_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_flag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionCaptureKind {
    None,
    ForcedFlagVerified,
    StdoutJsonEvent,
}

impl SessionCapture {
    /// Per-kind required fields per `tmp/01-pr-c-contract.md`. Catches
    /// malformed configs at load time instead of letting them surface
    /// as silent capture failures at execution time (V10 — observable).
    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            SessionCaptureKind::None => Ok(()),
            SessionCaptureKind::ForcedFlagVerified => {
                if self.flag.is_none() {
                    return Err(
                        "session_capture.kind = forced_flag_verified requires `flag`".into(),
                    );
                }
                Ok(())
            }
            SessionCaptureKind::StdoutJsonEvent => {
                self.validate_stdout_json_event_shape()?;
                if self.event_type.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `event_type`".into(),
                    );
                }
                if self.event_id_path.is_none() {
                    return Err(
                        "session_capture.kind = stdout_json_event requires `event_id_path`".into(),
                    );
                }
                Ok(())
            }
        }
    }

    fn validate_stdout_json_event_shape(&self) -> Result<(), String> {
        if let Some(json_args) = &self.json_args {
            if json_args.is_empty() {
                return Err(
                    "session_capture.kind = stdout_json_event requires non-empty `json_args`"
                        .into(),
                );
            }
            if self.json_flag.is_some() {
                return Err(
                    "session_capture.kind = stdout_json_event does not allow `json_flag` with `json_args`"
                        .into(),
                );
            }
            if self.last_message_flag.is_some() {
                return Err(
                    "session_capture.kind = stdout_json_event does not allow `last_message_flag` with `json_args`"
                        .into(),
                );
            }
            return Ok(());
        }

        if self.json_flag.is_some() {
            if self.last_message_flag.is_none() {
                return Err(
                    "session_capture.kind = stdout_json_event requires `last_message_flag` when `json_flag` is set"
                        .into(),
                );
            }
            return Ok(());
        }

        Err("session_capture.kind = stdout_json_event requires `json_flag` or `json_args`".into())
    }
}
