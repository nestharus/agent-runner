//! ## Declared roles
//!
//! Roles: parser, predicate, accessor, validator, mapper.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/session_capture/parse_forced_flag.rs
//!     role: adapter
//!     Translates:
//!       - forced-flag-verified-jsonl-contract
//!       - runtime-session-id-contract
//! ```

use super::messages::{forced_flag_missing_event_message, system_init_missing_session_id_message};
use serde_json::Value;

pub(in crate::executor::cli) fn parse_forced_flag_verified_session_id(
    stdout: &[u8],
) -> Result<String, String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(parse_stdout_json_line)
        .find_map(|value| forced_flag_value_result(&value))
        .unwrap_or_else(|| Err(forced_flag_missing_event_message()))
}

pub(super) fn parse_stdout_json_line(line: &str) -> Option<Value> {
    serde_json::from_str::<Value>(line).ok()
}

fn value_is_result_event(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("result")
}

fn json_session_id(value: &Value) -> Option<&str> {
    value.get("session_id").and_then(Value::as_str)
}

fn value_is_system_init_event(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("system")
        && value.get("subtype").and_then(Value::as_str) == Some("init")
}

fn forced_flag_value_result(value: &Value) -> Option<Result<String, String>> {
    if value_is_result_event(value)
        && let Some(session_id) = json_session_id(value)
    {
        return Some(Ok(session_id.to_string()));
    }
    if !value_is_system_init_event(value) {
        return None;
    }
    Some(required_system_init_session_id(value))
}

fn required_system_init_session_id(value: &Value) -> Result<String, String> {
    value
        .get("session_id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(system_init_missing_session_id_message)
}
