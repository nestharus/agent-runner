//! ## Declared roles
//! predicate, formatter

use super::super::{ExternalRotationError, error_formatter};

pub(super) fn supports_rotation_or_migration(
    describe: &oulipoly_provider::generated::DescribeResult,
    operation: &'static str,
) -> Result<(), ExternalRotationError> {
    let supported = if operation.starts_with("rotation.") {
        describe.capabilities.rotation
    } else {
        describe.capabilities.migration
    };
    if supported {
        Ok(())
    } else {
        Err(error_formatter::capability_missing(operation))
    }
}
