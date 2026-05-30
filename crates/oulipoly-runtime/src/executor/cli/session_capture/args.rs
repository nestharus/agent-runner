//! ## Declared roles
//!
//! Roles: formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs
//!     role: adapter
//!     Translates:
//!       - session-capture-argv-contract
//! ```

use oulipoly_config::SessionCapture;
use std::path::Path;

pub(super) fn forced_flag_capture_args(
    flag: String,
    requested_session_id: &str,
    capture: &SessionCapture,
) -> Vec<String> {
    let mut args = vec![flag, requested_session_id.to_string()];
    if let Some(readback_args) = &capture.readback_args {
        args.extend(readback_args.clone());
    }
    args
}

pub(super) fn stdout_json_event_capture_args(
    json_flag: String,
    last_message_flag: String,
    last_message_path: &Path,
) -> Vec<String> {
    vec![
        json_flag,
        last_message_flag,
        last_message_path.to_string_lossy().into_owned(),
    ]
}
