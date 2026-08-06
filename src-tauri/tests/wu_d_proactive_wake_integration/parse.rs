//! ## Declared roles
//!
//! Roles: parser.
//!
//! TEST: JSON and process-record parsers for proactive wake integration
//! diagnostics.

use chrono::{DateTime, Utc};

pub(crate) fn runner_bin() -> &'static str {
    env!("CARGO_BIN_EXE_oulipoly-agent-runner")
}

pub(crate) fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
