//! ## Declared roles
//!
//! Roles: formatter.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/messages.rs
//!     role: adapter
//!     Translates:
//!       - session-capture-error-message-contract
//! ```

pub(super) fn required_capture_field_message(name: &str) -> String {
    format!("{name} is required")
}

pub(super) fn system_init_missing_session_id_message() -> String {
    "system.init event missing session_id".to_string()
}

pub(super) fn forced_flag_missing_event_message() -> String {
    "stdout did not contain a result or system.init session_id event".to_string()
}

pub(super) fn stdout_event_missing_message(event_type: &str) -> String {
    format!("stdout did not contain event '{event_type}'")
}

pub(super) fn configured_event_missing_id_path_message(
    event_type: &str,
    event_id_path: &str,
) -> String {
    format!("event '{event_type}' missing id path '{event_id_path}'")
}
