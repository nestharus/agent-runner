//! ## Declared roles
//!
//! Roles: parser, predicate, validator.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/parse_stdout_event.rs
//!     role: adapter
//!     Translates:
//!       - stdout-json-event-session-contract
//!       - runtime-session-id-contract
//! ```

use super::json_path::lookup_json_path;
use super::messages::{configured_event_missing_id_path_message, stdout_event_missing_message};
use super::parse_forced_flag::parse_stdout_json_line;
use serde_json::Value;

pub(in crate::executor::cli) fn parse_stdout_json_event_session_id(
    stdout: &[u8],
    event_type: &str,
    event_id_path: &str,
) -> Result<String, String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(parse_stdout_json_line)
        .find(|value| value_matches_event_type(value, event_type))
        .map(|value| required_configured_event_session_id(&value, event_type, event_id_path))
        .unwrap_or_else(|| Err(stdout_event_missing_message(event_type)))
}

fn value_matches_event_type(value: &Value, event_type: &str) -> bool {
    value.get("type").and_then(Value::as_str) == Some(event_type)
}

fn required_configured_event_session_id(
    value: &Value,
    event_type: &str,
    event_id_path: &str,
) -> Result<String, String> {
    lookup_json_path(value, event_id_path)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| configured_event_missing_id_path_message(event_type, event_id_path))
}
