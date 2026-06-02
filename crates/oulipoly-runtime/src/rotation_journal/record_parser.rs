//! ## Declared roles
//! parser

use super::error_formatter;
use super::journal_recovery_failure;
use super::types::RotationJournalRecord;
use crate::rotation_domain::ExternalRotationError;

pub(super) fn parse_rotation_journal_record(
    bytes: &[u8],
) -> Result<RotationJournalRecord, ExternalRotationError> {
    serde_json::from_slice(bytes)
        .map_err(|error| journal_recovery_failure(error_formatter::decode_journal(error)))
}
