//! ## Declared roles
//!
//! - predicate
//! - validator

use crate::MessengerError;

pub(crate) fn require_positive_version(version: u64) -> Result<u64, MessengerError> {
    if version == 0 {
        return Err(MessengerError::InvalidInput(
            "version_id version must be greater than zero".to_string(),
        ));
    }
    Ok(version)
}

pub(crate) fn require_parent_invocation_env(value: &str) -> Result<(), MessengerError> {
    if value.trim().is_empty() {
        return Err(MessengerError::MissingInvocationScope);
    }
    Ok(())
}

pub(crate) fn require_return_channel_env(value: &str) -> Result<(), MessengerError> {
    if value.trim().is_empty() {
        return Err(MessengerError::MissingReturnChannel);
    }
    Ok(())
}

pub(crate) fn require_parent_invocation_id(value: Option<&str>) -> Result<String, MessengerError> {
    value.map(str::to_string).ok_or_else(|| {
        MessengerError::InvalidInvocationScope(
            "OULIPOLY_PARENT_INVOCATION is missing id".to_string(),
        )
    })
}

pub(crate) fn is_unreserved_percent_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}
