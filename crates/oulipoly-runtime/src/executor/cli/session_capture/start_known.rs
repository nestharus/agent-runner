//! ## Declared roles
//!
//! Roles: orchestration, validator, mapper.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/start_known.rs
//!     role: adapter
//!     Translates:
//!       - provider-session-capture-config-contract
//!       - runtime-start-known-session-contract
//! ```

use super::messages::required_capture_field_message;
use oulipoly_config::{ProviderConfig, SessionCapture, SessionCaptureKind};

pub fn start_known_provider_session_id(
    provider: &ProviderConfig,
) -> Result<Option<String>, String> {
    let Some(capture) = provider.session_capture.as_ref() else {
        return Ok(None);
    };
    validate_start_known_capture(capture)?;
    Ok(start_known_session_id_for_capture(capture))
}

fn validate_start_known_capture(capture: &SessionCapture) -> Result<(), String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => validate_forced_flag_capture(capture),
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => Ok(()),
    }
}

fn validate_forced_flag_capture(capture: &SessionCapture) -> Result<(), String> {
    if capture.flag.is_none() {
        return Err(required_capture_field_message("session_capture.flag"));
    }
    Ok(())
}

fn start_known_session_id_for_capture(capture: &SessionCapture) -> Option<String> {
    match capture.kind {
        SessionCaptureKind::ForcedFlagVerified => Some(uuid::Uuid::new_v4().to_string()),
        SessionCaptureKind::None | SessionCaptureKind::StdoutJsonEvent => None,
    }
}
