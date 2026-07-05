//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-agent-messenger/src/error.rs
//!     role: adapter
//!     Translates:
//!       - messenger error contract
//!       - oulipoly-agent-store error contract
//!       - oulipoly-agent-scratchpad error contract
//!       - std and rusqlite error source contract
//!       - serde_json error contract
//! ```

use crate::formatter::{scratchpad_schema_message, store_schema_version_message};
use std::error::Error;
use std::fmt;
use std::io;

#[derive(Debug)]
pub enum MessengerError {
    InvalidInput(String),
    MissingInvocationScope,
    InvalidInvocationScope(String),
    MissingReturnChannel,
    InvalidReturnChannel(String),
    NotFound,
    Collision,
    Io(io::Error),
    Database(rusqlite::Error),
    MigrationRequired,
    IncompatibleSchema(String),
    Serialization(serde_json::Error),
    MetadataDecode(String),
    Scratchpad(oulipoly_agent_scratchpad::ScratchpadError),
    Store(oulipoly_agent_store::StoreError),
}

impl fmt::Display for MessengerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid input: {message}"),
            Self::MissingInvocationScope => write!(
                f,
                "missing invocation scope: pass --invocation-uuid or set OULIPOLY_PARENT_INVOCATION"
            ),
            Self::InvalidInvocationScope(message) => {
                write!(f, "invalid invocation scope: {message}")
            }
            Self::MissingReturnChannel => write!(
                f,
                "missing return channel: pass --return-channel or set OULIPOLY_RETURN_CHANNEL"
            ),
            Self::InvalidReturnChannel(message) => write!(f, "invalid return channel: {message}"),
            Self::NotFound => write!(f, "returned artifact not found"),
            Self::Collision => write!(f, "backing store collision"),
            Self::Io(err) => write!(f, "io error: {err}"),
            Self::Database(err) => write!(f, "database error: {err}"),
            Self::MigrationRequired => write!(f, "database schema migration required"),
            Self::IncompatibleSchema(message) => {
                write!(f, "incompatible database schema: {message}")
            }
            Self::Serialization(err) => write!(f, "json serialization error: {err}"),
            Self::MetadataDecode(message) => write!(f, "metadata decode error: {message}"),
            Self::Scratchpad(err) => write!(f, "{err}"),
            Self::Store(err) => write!(f, "{err}"),
        }
    }
}

impl Error for MessengerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Database(err) => Some(err),
            Self::Serialization(err) => Some(err),
            Self::Scratchpad(err) => Some(err),
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for MessengerError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<rusqlite::Error> for MessengerError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Database(value)
    }
}

impl From<serde_json::Error> for MessengerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serialization(value)
    }
}

impl From<oulipoly_agent_store::StoreError> for MessengerError {
    fn from(value: oulipoly_agent_store::StoreError) -> Self {
        match value {
            oulipoly_agent_store::StoreError::InvalidInput(message) => Self::InvalidInput(message),
            oulipoly_agent_store::StoreError::NotFound => Self::NotFound,
            oulipoly_agent_store::StoreError::Collision => Self::Collision,
            oulipoly_agent_store::StoreError::Io(err) => Self::Io(err),
            oulipoly_agent_store::StoreError::Database(err) => Self::Database(err),
            oulipoly_agent_store::StoreError::MigrationRequired => Self::MigrationRequired,
            oulipoly_agent_store::StoreError::IncompatibleSchema(version) => {
                Self::IncompatibleSchema(store_schema_version_message(version))
            }
        }
    }
}

impl From<oulipoly_agent_scratchpad::ScratchpadError> for MessengerError {
    fn from(value: oulipoly_agent_scratchpad::ScratchpadError) -> Self {
        match value {
            oulipoly_agent_scratchpad::ScratchpadError::InvalidInput(message) => {
                Self::InvalidInput(message)
            }
            oulipoly_agent_scratchpad::ScratchpadError::MissingInvocationScope => {
                Self::MissingInvocationScope
            }
            oulipoly_agent_scratchpad::ScratchpadError::InvalidInvocationScope(message) => {
                Self::InvalidInvocationScope(message)
            }
            oulipoly_agent_scratchpad::ScratchpadError::NotFound
            | oulipoly_agent_scratchpad::ScratchpadError::NotFoundNamed(_) => Self::NotFound,
            oulipoly_agent_scratchpad::ScratchpadError::Collision => Self::Collision,
            oulipoly_agent_scratchpad::ScratchpadError::Io(err) => Self::Io(err),
            oulipoly_agent_scratchpad::ScratchpadError::Database(err) => Self::Database(err),
            oulipoly_agent_scratchpad::ScratchpadError::MigrationRequired => {
                Self::MigrationRequired
            }
            oulipoly_agent_scratchpad::ScratchpadError::IncompatibleSchema => {
                Self::IncompatibleSchema(scratchpad_schema_message())
            }
            oulipoly_agent_scratchpad::ScratchpadError::Serialization(err) => {
                Self::Serialization(err)
            }
            oulipoly_agent_scratchpad::ScratchpadError::MetadataDecode(message) => {
                Self::MetadataDecode(message)
            }
        }
    }
}
