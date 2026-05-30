//! ## Declared roles
//!
//! Roles: formatter.
//!
//! - formatter: formats canonical input validation error messages without
//!   deciding validation outcomes.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-runtime/src/executor/cli/input_flags/messages.rs
//!     role: adapter
//!     Translates:
//!       - executor-extra-inputs-contract
//!       - input-validation-error-contract
//! ```

use std::fmt::Display;

pub(super) fn required_input_message(name: &str) -> String {
    format!("Required input '{name}' not provided")
}

pub(super) fn invalid_integer_message(name: &str, value: &str) -> String {
    format!("Input '{name}': '{value}' is not a valid integer")
}

pub(super) fn invalid_number_message(name: &str, value: &str) -> String {
    format!("Input '{name}': '{value}' is not a valid number")
}

pub(super) fn invalid_enum_option_message(name: &str, value: &str, options: &[String]) -> String {
    format!("Input '{name}': '{value}' is not a valid option. Valid: {options:?}")
}

pub(super) fn below_minimum_message<T: Display>(name: &str, value: T, minimum: T) -> String {
    format!("Input '{name}': {value} is below minimum {minimum}")
}

pub(super) fn exceeds_maximum_message<T: Display>(name: &str, value: T, maximum: T) -> String {
    format!("Input '{name}': {value} exceeds maximum {maximum}")
}

pub(super) fn array_min_items_message(name: &str, minimum: usize, actual: usize) -> String {
    format!("Input '{name}': need at least {minimum} items, got {actual}")
}

pub(super) fn array_max_items_message(name: &str, maximum: usize, actual: usize) -> String {
    format!("Input '{name}': maximum {maximum} items, got {actual}")
}
