//! ## Declared roles
//! formatter

use std::path::Path;

pub(super) fn ambiguous_journal(marker: &str) -> String {
    format!("quarantine ambiguous rotation journal: {marker}")
}

pub(super) fn journal_error_reason(
    error: &crate::rotation_domain::ExternalRotationError,
) -> String {
    error.to_string()
}

pub(super) fn read_journal(error: std::io::Error) -> String {
    format!("failed to read rotation journal: {error}")
}

pub(super) fn decode_journal(error: serde_json::Error) -> String {
    format!("failed to decode rotation journal: {error}")
}

pub(super) fn create_directory(error: std::io::Error) -> String {
    format!("failed to create journal directory: {error}")
}

pub(super) fn encode_journal(error: serde_json::Error) -> String {
    format!("failed to encode rotation journal: {error}")
}

pub(super) fn write_journal(error: std::io::Error) -> String {
    format!("failed to write rotation journal: {error}")
}

pub(super) fn write_lock(path: &Path, error: std::io::Error) -> String {
    format!("failed to write rotation lock {}: {error}", path.display())
}

pub(super) fn publish_journal(error: std::io::Error) -> String {
    format!("failed to publish rotation journal: {error}")
}

pub(super) fn remove_file(path: &Path, error: std::io::Error) -> String {
    format!("failed to remove {}: {error}", path.display())
}
