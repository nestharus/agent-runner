//! ## Declared roles
//! accessor, mapper, orchestration, predicate, formatter

use super::{error_formatter, journal_recovery_failure};
use crate::rotation_domain::ExternalRotationError;
use oulipoly_provider::generated::Artifact;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

const JOURNAL_FILE: &str = ".oulipoly-s7c-rotation-journal.json";
const JOURNAL_LOCK_FILE: &str = ".oulipoly-s7c-rotation.lock";

pub fn rotation_journal_path(root: &Path) -> PathBuf {
    root.join(JOURNAL_FILE)
}

pub fn rotation_journal_lock_path(root: &Path) -> PathBuf {
    root.join(JOURNAL_LOCK_FILE)
}

pub(super) fn cleanup_rotation_journal(root: &Path) -> Result<(), ExternalRotationError> {
    let path = rotation_journal_path(root);
    let lock = rotation_journal_lock_path(root);
    remove_journal_if_present(&path)?;
    release_rotation_lock(&lock)
}

pub(super) fn acquire_rotation_lock(path: &Path) -> Result<(), ExternalRotationError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(b"s7c-rotation-lock"))
        .map_err(|error| journal_recovery_failure(error_formatter::write_lock(path, error)))
}

pub(super) fn cleanup_journal_and_lock(path: &Path) -> Result<(), ExternalRotationError> {
    remove_journal_if_present(path)?;
    if let Some(root) = path.parent() {
        release_rotation_lock(&rotation_journal_lock_path(root))?;
    }
    Ok(())
}

pub(super) fn remove_record_artifacts(
    root: &Path,
    artifacts: &[Artifact],
) -> Result<(), ExternalRotationError> {
    for artifact in artifacts {
        if let Some(path) = artifact.path.as_deref() {
            let path = Path::new(path);
            if super::lock_cleanup_predicates::artifact_path_is_inside_rotation_root(root, path) {
                remove_file_if_present(path)?;
            }
        }
    }
    Ok(())
}

fn release_rotation_lock(path: &Path) -> Result<(), ExternalRotationError> {
    remove_file_if_present(path)
}

fn remove_journal_if_present(path: &Path) -> Result<(), ExternalRotationError> {
    remove_file_if_present(path)
}

fn remove_file_if_present(path: &Path) -> Result<(), ExternalRotationError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(journal_recovery_failure(error_formatter::remove_file(
            path, error,
        ))),
    }
}
