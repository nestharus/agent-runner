//! ## Declared roles
//!
//! `validator`

use super::errors::ExternalQuotaError;
use oulipoly_provider::generated::DescribeResult;

pub(crate) fn validate_quota_capability(
    describe: &DescribeResult,
) -> Result<(), ExternalQuotaError> {
    if describe.capabilities.quota {
        return Ok(());
    }
    Err(ExternalQuotaError::missing_quota_capability())
}
