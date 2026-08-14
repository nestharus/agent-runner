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
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            validate_missing_state_db_ancestry(db_path)?;
            Ok(true)
        }
        Err(error) => Err(format!(
            "Failed to inspect state DB rebuild source {}: {error}",
            db_path.display()
        )),
    }
}

fn validate_missing_state_db_ancestry(db_path: &Path) -> Result<(), String> {
    let inspection_path = if db_path.is_absolute() {
        db_path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| format!("Failed to inspect state DB rebuild ancestry: {error}"))?
            .join(db_path)
    };
    for ancestor in inspection_path.ancestors().skip(1) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!(
                    "State DB rebuild does not accept symlink ancestry: {}",
                    ancestor.display()
                ));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(format!(
                    "Failed to inspect state DB rebuild source {}: ancestor {} is not a directory",
                    db_path.display(),
                    ancestor.display()
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Failed to inspect state DB rebuild source {} through ancestor {}: {error}",
                    db_path.display(),
                    ancestor.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_state_db_preserves_metadata_errors() {
        let directory = tempfile::tempdir().unwrap();
        let non_directory = directory.path().join("not-a-directory");
        std::fs::write(&non_directory, b"file").unwrap();

        let error = missing_state_db(&non_directory.join("state.db")).unwrap_err();

        assert!(error.contains("Failed to inspect"), "{error}");
    }

    #[test]
    fn missing_state_db_accepts_only_a_missing_leaf_below_valid_directories() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("nested").join("state.db");

        assert!(missing_state_db(&state_path).unwrap());
        std::fs::write(directory.path().join("state.db"), b"state").unwrap();
        assert!(!missing_state_db(&directory.path().join("state.db")).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn missing_state_db_rejects_a_symlink_ancestor() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&target).unwrap();
        symlink(&target, &alias).unwrap();

        let error = missing_state_db(&alias.join("state.db")).unwrap_err();

        assert!(error.contains("symlink ancestry"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn missing_state_db_preserves_inaccessible_ancestry_errors() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let inaccessible = directory.path().join("inaccessible");
        std::fs::create_dir(&inaccessible).unwrap();
        let original_permissions = std::fs::metadata(&inaccessible).unwrap().permissions();
        std::fs::set_permissions(&inaccessible, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = missing_state_db(&inaccessible.join("state.db"));

        std::fs::set_permissions(&inaccessible, original_permissions).unwrap();
        assert!(
            result.is_err(),
            "inaccessible ancestry was treated as absence"
        );
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
