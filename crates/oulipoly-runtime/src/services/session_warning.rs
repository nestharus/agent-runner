//! ## Declared roles
//! formatter
//!
//! Session lifecycle stderr warning formatter. This module preserves the
//! existing warning text prefix used by command-line tests.

use super::error::ServiceError;
use std::io::Write;

pub(super) fn write_session_ingest_warning(
    stderr: &mut dyn Write,
    message: &str,
) -> Result<(), ServiceError> {
    writeln!(stderr, "Warning: {message}").map_err(|err| ServiceError::Dependency {
        message: format!("Failed to write session ingest warning: {err}"),
    })
}
