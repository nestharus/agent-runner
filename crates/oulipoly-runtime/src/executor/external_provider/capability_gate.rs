//! Role: validator.

use super::errors::ExternalProviderDispatchError;
use oulipoly_provider::generated::DescribeResult;

pub(crate) fn gate_required_capabilities(
    describe: &DescribeResult,
) -> Result<(), ExternalProviderDispatchError> {
    if !describe.capabilities.policy {
        return Err(ExternalProviderDispatchError::missing_required_capability(
            "policy.evaluate",
        ));
    }
    if !describe.capabilities.launch {
        return Err(ExternalProviderDispatchError::missing_required_capability(
            "launch",
        ));
    }
    Ok(())
}
