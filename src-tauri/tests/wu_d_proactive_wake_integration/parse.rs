//! ## Declared roles
//!
//! Roles: parser.
//!
//! TEST: JSON and process-record parsers for proactive wake integration
//! diagnostics.

use chrono::{DateTime, Utc};
use oulipoly_state::pid_identity::ProcessIdentity;
use serde_json::{Value, json};
use std::process::Output;

pub(crate) fn notify_wake(output: &Output) -> Value {
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    response.get("wake").cloned().unwrap_or(Value::Null)
}

pub(crate) fn json_value(input: &str) -> Value {
    serde_json::from_str(input).unwrap()
}

pub(crate) fn caller_chain(identity: &ProcessIdentity) -> Value {
    json!({
        "caller_chain": [{
            "pid": identity.os_pid,
            "boot_id": identity.os_boot_id,
            "starttime_ticks": identity.os_pid_starttime_ticks,
        }]
    })
}

pub(crate) fn identity(
    os_pid: i64,
    os_boot_id: &str,
    os_pid_starttime_ticks: i64,
) -> ProcessIdentity {
    ProcessIdentity {
        os_pid,
        os_boot_id: os_boot_id.to_string(),
        os_pid_starttime_ticks,
    }
}

pub(crate) fn runner_bin() -> &'static str {
    env!("CARGO_BIN_EXE_oulipoly-agent-runner")
}

pub(crate) fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
