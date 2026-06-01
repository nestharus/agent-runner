//! ## Declared roles
//! mapper

use crate::balancer::ManualMigrationRejection;
use crate::services::RotationFailedReason;

pub(super) fn rejection_to_rotation_failed(
    rejection: ManualMigrationRejection,
) -> RotationFailedReason {
    match rejection {
        ManualMigrationRejection::SingleProviderPool { provider } => {
            RotationFailedReason::ManualTargetIsSingleProviderPool { provider }
        }
        ManualMigrationRejection::ActiveProviderNotInPool { active } => {
            RotationFailedReason::ManualTargetActiveNotInPool { active }
        }
        ManualMigrationRejection::TargetNotInPool { target, pool } => {
            RotationFailedReason::ManualTargetNotInPool { target, pool }
        }
        ManualMigrationRejection::NotMigratablePair { source, target } => {
            RotationFailedReason::ManualTargetNotMigratable { source, target }
        }
    }
}
