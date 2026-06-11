//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }

use super::*;
#[test]
fn age132_read_only_error_classifier_and_sidecar_paths_map_documented_variants() {
    let missing_dir = tempfile::tempdir().unwrap();
    let missing_path = missing_dir.path().join("missing-state.db");
    match StateDb::open_read_only(&missing_path) {
        Err(ReadOnlyOpenError::Missing { path }) => assert_eq!(path, missing_path),
        Ok(_) => panic!("expected Missing, got successful read-only open"),
        Err(other) => panic!("expected Missing, got {other:?}"),
    }

    let malformed_dir = tempfile::tempdir().unwrap();
    let malformed_path = malformed_dir.path().join("state.db");
    std::fs::write(&malformed_path, b"not sqlite").unwrap();
    match StateDb::open_read_only(&malformed_path) {
        Err(ReadOnlyOpenError::NotADatabase { path: p, .. }) => {
            assert_eq!(p, malformed_path);
        }
        Ok(_) => panic!("expected NotADatabase, got successful read-only open"),
        Err(other) => panic!("expected NotADatabase, got {other:?}"),
    }

    let valid_dir = tempfile::tempdir().unwrap();
    let valid_path = valid_dir.path().join("state.db");
    drop(StateDb::open(&valid_path).unwrap());
    drop(StateDb::open_read_only(&valid_path).unwrap());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let denied_dir = tempfile::tempdir().unwrap();
        let denied_path = denied_dir.path().join("state.db");
        drop(StateDb::open(&denied_path).unwrap());
        let mut denied_permissions = std::fs::metadata(&denied_path).unwrap().permissions();
        denied_permissions.set_mode(0o000);
        std::fs::set_permissions(&denied_path, denied_permissions).unwrap();
        match StateDb::open_read_only(&denied_path) {
            Err(ReadOnlyOpenError::PermissionDenied { path }) => assert_eq!(path, denied_path),
            Ok(_) => panic!("expected PermissionDenied, got successful read-only open"),
            Err(other) => panic!("expected PermissionDenied, got {other:?}"),
        }

        let sidecar_dir = tempfile::tempdir().unwrap();
        let sidecar_path = sidecar_dir.path().join("state.db");
        let sidecar_conn = sqlite::Connection::open(&sidecar_path).unwrap();
        sidecar_conn
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                     CREATE TABLE sidecar_probe (value TEXT);
                     INSERT INTO sidecar_probe (value) VALUES ('kept open');",
            )
            .unwrap();
        let sidecar_file = std::fs::read_dir(sidecar_dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path != &sidecar_path && path.is_file())
            .expect("WAL mode should create at least one SQLite sidecar file");
        let mut sidecar_permissions = std::fs::metadata(&sidecar_file).unwrap().permissions();
        sidecar_permissions.set_mode(0o000);
        std::fs::set_permissions(&sidecar_file, sidecar_permissions).unwrap();
        match StateDb::open_read_only(&sidecar_path) {
            Err(ReadOnlyOpenError::WalSidecarError { path, message }) => {
                assert_eq!(path, sidecar_path);
                assert!(message.contains("sidecar"), "{message}");
            }
            Ok(_) => panic!("expected WalSidecarError, got successful read-only open"),
            Err(other) => panic!("expected WalSidecarError, got {other:?}"),
        }
        drop(sidecar_conn);
    }

    match StateDb::open_read_only(valid_dir.path()) {
        Err(ReadOnlyOpenError::Operational { message }) => {
            assert!(!message.is_empty());
        }
        Ok(_) => panic!("expected Operational, got successful read-only open"),
        Err(other) => panic!("expected operational mapping, got {other:?}"),
    }
}
