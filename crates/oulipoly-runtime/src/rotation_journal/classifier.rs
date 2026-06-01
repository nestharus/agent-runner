//! ## Declared roles
//! predicate, mapper, formatter

use super::error_formatter;
use super::types::RotationJournalState;

pub(super) fn classify_rotation_journal_state(marker: Option<&str>) -> RotationJournalState {
    match marker {
        None => RotationJournalState::Absent,
        Some("crash_after_artifact") => RotationJournalState::PendingAfterArtifact,
        Some("crash_during_apply") => RotationJournalState::PendingDuringApply,
        Some(other) => RotationJournalState::Quarantine {
            reason: error_formatter::ambiguous_journal(other),
        },
    }
}
