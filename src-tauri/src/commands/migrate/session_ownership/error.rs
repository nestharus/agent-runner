//! Declared role: mapper, formatter

use std::fmt;

#[derive(Debug, Clone)]
pub(crate) struct DryRunError {
    message: String,
}

impl DryRunError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for DryRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for DryRunError {}

impl From<std::io::Error> for DryRunError {
    fn from(err: std::io::Error) -> Self {
        Self::new(err.to_string())
    }
}

impl From<rusqlite::Error> for DryRunError {
    fn from(err: rusqlite::Error) -> Self {
        Self::new(err.to_string())
    }
}
