//! ## Declared roles
//!
//! Roles: parser.
//!
//! TEST: JSON and process-record parsers for proactive wake integration
//! diagnostics.

use chrono::{DateTime, Utc};

pub(crate) fn runner_bin() -> &'static str {
    crate::bounded_runner_image::runner_bin()
}

pub(crate) fn ts(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}
