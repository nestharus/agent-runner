//! ## Declared roles
//! formatter

use super::error_formatter;
use super::journal_recovery_failure;
use super::types::RotationJournalRecord;
use crate::rotation_domain::ExternalRotationError;

pub(super) fn encode_rotation_journal_record(
    record: &RotationJournalRecord,
) -> Result<Vec<u8>, ExternalRotationError> {
    serde_json::to_vec_pretty(record)
        .map_err(|error| journal_recovery_failure(error_formatter::encode_journal(error)))
}
