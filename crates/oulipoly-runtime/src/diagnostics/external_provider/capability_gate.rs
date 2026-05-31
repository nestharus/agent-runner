//! Role: validator.

use super::errors::TerminalClassifyError;
use oulipoly_provider::generated::DescribeResult;

pub(crate) fn validate_terminal_capability(
    describe: &DescribeResult,
) -> Result<(), TerminalClassifyError> {
    if describe.capabilities.terminal {
        Ok(())
    } else {
        Err(TerminalClassifyError::missing_capability())
    }
}
