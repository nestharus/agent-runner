//! Declared role: predicate

use std::path::Path;

pub(super) fn unused_path(path: &Path) -> bool {
    !path.exists()
}

pub(super) fn missing_state_db(db_path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(db_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
            "State DB rebuild does not accept a leaf symlink: {}",
            db_path.display()
        )),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(format!(
            "Failed to inspect state DB rebuild source {}: {error}",
            db_path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn missing_state_db_preserves_metadata_errors() {
        let directory = tempfile::tempdir().unwrap();
        let non_directory = directory.path().join("not-a-directory");
        std::fs::write(&non_directory, b"file").unwrap();

        let error = missing_state_db(&non_directory.join("state.db")).unwrap_err();

        assert!(error.contains("Failed to inspect"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn missing_state_db_rejects_a_dangling_leaf_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        symlink(directory.path().join("missing.db"), &state_path).unwrap();

        let error = missing_state_db(&state_path).unwrap_err();

        assert!(error.contains("leaf symlink"), "{error}");
    }
}
