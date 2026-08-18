//! Shared writable-open fence for an interrupted State-plus-sidecar rebuild.

use std::path::{Path, PathBuf};

pub const STATE_SIDECAR_REBUILD_RECOVERY_MARKER: &str = ".state-sidecar-rebuild-in-progress";

pub fn marker_path(storage_path: &Path) -> PathBuf {
    storage_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(STATE_SIDECAR_REBUILD_RECOVERY_MARKER)
}

pub(crate) fn ensure_writable_open_allowed(storage_path: &Path) -> Result<(), String> {
    let marker = marker_path(storage_path);
    match std::fs::symlink_metadata(&marker) {
        Ok(_) => {
            return Err(format!(
                "process_integrity: state_sidecar_rebuild_recovery_in_progress: writable storage {} is unavailable while recovery marker {} exists; retry `agents migrate --rebuild`",
                storage_path.display(),
                marker.display(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "process_integrity: failed to inspect State-plus-sidecar rebuild recovery marker {}: {error}",
                marker.display(),
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn dangling_recovery_marker_still_blocks_writable_open() {
        let directory = tempfile::tempdir().unwrap();
        let storage_path = directory.path().join("state.db");
        let marker = marker_path(&storage_path);
        symlink(directory.path().join("missing-marker-target"), &marker).unwrap();

        let error = ensure_writable_open_allowed(&storage_path).unwrap_err();

        assert!(
            error.contains("state_sidecar_rebuild_recovery_in_progress"),
            "{error}"
        );
    }
}
