//! ## Declared roles
//! mapper

use super::types::{RotationJournalState, RotationRecoveryPlan};

pub(super) fn build_rotation_recovery_plan(state: RotationJournalState) -> RotationRecoveryPlan {
    match state {
        RotationJournalState::Absent => RotationRecoveryPlan::Noop,
        RotationJournalState::PendingAfterArtifact => RotationRecoveryPlan::RollBack,
        RotationJournalState::PendingDuringApply => RotationRecoveryPlan::RollForward,
        RotationJournalState::Quarantine { reason } => RotationRecoveryPlan::Quarantine { reason },
    }
}
