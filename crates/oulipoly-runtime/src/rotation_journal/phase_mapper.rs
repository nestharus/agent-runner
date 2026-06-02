//! ## Declared roles
//! mapper

use super::types::RotationJournalPhase;

pub(super) fn phase_marker(phase: RotationJournalPhase) -> &'static str {
    match phase {
        RotationJournalPhase::CrashAfterArtifact => "crash_after_artifact",
        RotationJournalPhase::CrashDuringApply => "crash_during_apply",
    }
}
