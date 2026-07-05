//! ## Declared roles
//!
//! - accessor
//! - filter
//! - formatter
//! - predicate
//! - validator

use crate::MessengerError;
use serde::{Deserialize, Serialize};

const RESERVED_RETURN_PREFIXES: [&str; 2] = ["scratchpad:", "return:"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnName(String);

impl ReturnName {
    pub fn new(value: impl Into<String>) -> Result<Self, MessengerError> {
        let value = value.into();
        validate_return_name(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_return_name(value: &str) -> Result<(), MessengerError> {
    require_non_empty_return_name(value)?;
    require_unreserved_return_name(value)
}

fn require_non_empty_return_name(value: &str) -> Result<(), MessengerError> {
    if value.is_empty() {
        return Err(MessengerError::InvalidInput(
            "return name must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn require_unreserved_return_name(value: &str) -> Result<(), MessengerError> {
    if let Some(prefix) = reserved_return_name_prefix(value) {
        return Err(MessengerError::InvalidInput(reserved_prefix_message(
            prefix,
        )));
    }
    Ok(())
}

fn reserved_return_name_prefix(value: &str) -> Option<&'static str> {
    RESERVED_RETURN_PREFIXES
        .into_iter()
        .find(|prefix| value.starts_with(prefix))
}

fn reserved_prefix_message(prefix: &str) -> String {
    format!("return name must not start with reserved prefix {prefix}")
}
