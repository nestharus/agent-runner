//! ## Declared roles
//!
//! Roles: parser.
//!
//! - parser: parses integer and number input values while preserving current
//!   parse failure text through the validation boundary.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/parse.rs
//!     role: adapter
//!     Translates:
//!       - executor-extra-inputs-contract
//!       - scalar-input-parse-contract
//! ```

use super::messages::{invalid_integer_message, invalid_number_message};

pub(super) fn parse_integer_input(val: &str, name: &str) -> Result<i64, String> {
    val.parse().map_err(|_| invalid_integer_message(name, val))
}

pub(super) fn parse_number_input(val: &str, name: &str) -> Result<f64, String> {
    val.parse().map_err(|_| invalid_number_message(name, val))
}
