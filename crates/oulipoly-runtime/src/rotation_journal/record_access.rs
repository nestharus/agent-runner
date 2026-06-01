//! ## Declared roles
//! accessor, orchestration, formatter

use super::error_formatter;
use super::journal_recovery_failure;
use crate::rotation_domain::ExternalRotationError;
use std::path::Path;

pub(super) fn read_rotation_journal_bytes(path: &Path) -> Result<Vec<u8>, ExternalRotationError> {
    std::fs::read(path)
        .map_err(|error| journal_recovery_failure(error_formatter::read_journal(error)))
}

pub(super) fn write_rotation_journal_bytes(
    path: &Path,
    bytes: &[u8],
) -> Result<(), ExternalRotationError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| journal_recovery_failure(error_formatter::create_directory(error)))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)
        .map_err(|error| journal_recovery_failure(error_formatter::write_journal(error)))?;
    std::fs::rename(&tmp, path)
        .map_err(|error| journal_recovery_failure(error_formatter::publish_journal(error)))
}
