//! ## Declared roles
//!
//! - orchestration
//! - filter
//! - validator
//! - predicate
//! - mapper
//! - accessor
//!
//! Role set: { orchestration, filter, validator, predicate, mapper, accessor }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/age132_opening_tests.rs
//!     role: intrinsic-surface
//!     Domain: age132-opening-tests-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

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
        let sidecar_file = sqlite_sidecar_file(sidecar_dir.path(), &sidecar_path);
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

#[cfg(unix)]
fn sqlite_sidecar_file(dir: &Path, db_path: &Path) -> PathBuf {
    require_sqlite_sidecar_file(find_sqlite_sidecar_file(dir, db_path))
}

#[cfg(unix)]
fn find_sqlite_sidecar_file(dir: &Path, db_path: &Path) -> Option<PathBuf> {
    sqlite_sidecar_candidate_paths(dir)
        .into_iter()
        .find(|path| sqlite_sidecar_candidate(path, db_path))
}

fn sqlite_sidecar_candidate_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(sidecar_entry_path)
        .collect()
}

#[cfg(unix)]
fn require_sqlite_sidecar_file(path: Option<PathBuf>) -> PathBuf {
    path.expect("WAL mode should create at least one SQLite sidecar file")
}

#[cfg(unix)]
fn sidecar_entry_path(entry: std::io::Result<std::fs::DirEntry>) -> PathBuf {
    entry.unwrap().path()
}

#[cfg(unix)]
fn sqlite_sidecar_candidate(path: &Path, db_path: &Path) -> bool {
    path != db_path && path.is_file()
}
