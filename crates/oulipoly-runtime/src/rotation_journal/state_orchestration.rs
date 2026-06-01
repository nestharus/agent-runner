//! ## Declared roles
//! orchestration, predicate, accessor, parser, mapper, formatter

use super::classifier::classify_rotation_journal_state;
use super::journal_predicates::journal_path_exists;
use super::phase_mapper::phase_marker;
use super::record_orchestration::read_rotation_journal_record;
use super::types::RotationJournalState;
use std::path::Path;

pub(super) fn read_rotation_journal_state(path: &Path) -> RotationJournalState {
    if !journal_path_exists(path) {
        return RotationJournalState::Absent;
    }
    let record = match read_rotation_journal_record(path) {
        Ok(record) => record,
        Err(error) => {
            return RotationJournalState::Quarantine {
                reason: super::error_formatter::journal_error_reason(&error),
            };
        }
    };
    classify_rotation_journal_state(Some(phase_marker(record.phase)))
}
