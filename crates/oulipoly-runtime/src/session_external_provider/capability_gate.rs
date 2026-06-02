//! Role: validator.

use super::provider_error::{ExternalSessionProviderError, map_capability_missing_error};

pub(crate) fn require_session_capability(
    describe: &oulipoly_provider::generated::DescribeResult,
) -> Result<(), ExternalSessionProviderError> {
    if describe.capabilities.session {
        Ok(())
    } else {
        Err(map_capability_missing_error())
    }
}
