//! ## Declared roles
//! orchestration

use super::record_access::{read_rotation_journal_bytes, write_rotation_journal_bytes};
use super::record_formatter::encode_rotation_journal_record;
use super::record_parser::parse_rotation_journal_record;
use super::types::RotationJournalRecord;
use crate::rotation_domain::ExternalRotationError;
use std::path::Path;

pub(super) fn read_rotation_journal_record(
    path: &Path,
) -> Result<RotationJournalRecord, ExternalRotationError> {
    let bytes = read_rotation_journal_bytes(path)?;
    parse_rotation_journal_record(&bytes)
}

pub(super) fn write_rotation_journal_record_to_path(
    path: &Path,
    record: &RotationJournalRecord,
) -> Result<(), ExternalRotationError> {
    let bytes = encode_rotation_journal_record(record)?;
    write_rotation_journal_bytes(path, &bytes)
}
