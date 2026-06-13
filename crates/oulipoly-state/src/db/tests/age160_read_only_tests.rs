//! ## Declared roles
//!
//! - validator
//!
//! Role set: { validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/age160_read_only_tests.rs
//!     role: intrinsic-surface
//!     Domain: read-only-open-adapter-test-surface
//!     Owns:
//!       - crate::db::sqlite_adapter read-only open surface (Connection, OpenFlags, Statement, Transaction, Row, params, OptionalExtension)
//!       - ReadOnlyOpenFailure, SqliteFailureProjection, and SidecarProbe outcomes asserted by read-only open tests
//! ```

use super::common::*;
use super::*;
#[test]
fn age160_classify_read_only_open_error_via_typed_projection_not_database_permission_and_plain_unknown()
 {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_not_database(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::NotADatabase,
                sqlite::ffi::ErrorCode::NotADatabase as i32,
                "private diagnostic mentions wal but code is not-a-database",
            ),
        ),
        &path,
    );
    age160_assert_not_database(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::DatabaseCorrupt,
                sqlite::ffi::ErrorCode::DatabaseCorrupt as i32,
                "private diagnostic mentions shared memory but code is corrupt",
            ),
        ),
        &path,
    );
    age160_assert_permission_denied(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::PermissionDenied,
                sqlite::ffi::ErrorCode::PermissionDenied as i32,
                "permission denied",
            ),
        ),
        &path,
    );

    for (code, message) in [
        (
            sqlite::ffi::ErrorCode::SystemIoFailure,
            "plain SystemIoFailure must ignore wal/-shm diagnostic tokens",
        ),
        (
            sqlite::ffi::ErrorCode::ReadOnly,
            "read only database with wal-shaped private text",
        ),
        (
            sqlite::ffi::ErrorCode::CannotOpen,
            "cannot open database with shared memory-shaped private text",
        ),
    ] {
        age160_assert_operational(classify_read_only_open_error(
            &path,
            age160_sqlite_failure(code, code as i32, message),
        ));
    }
}

#[test]
fn age160_classify_read_only_open_error_via_typed_projection_wal_sidecar() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    std::fs::write(&path, b"placeholder").unwrap();
    std::fs::write(wal_path(&path), b"owned wal sidecar").unwrap();

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::SystemIoFailure,
                sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                "plain io failure text intentionally lacks sidecar tokens",
            ),
        ),
        &path,
    );

    let dirty_wal_path = temp.path().join("dirty-wal-state.db");
    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &dirty_wal_path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::CannotOpen,
                sqlite::ffi::SQLITE_CANTOPEN_DIRTYWAL,
                "dirty WAL extended code without diagnostic-token dependency",
            ),
        ),
        &dirty_wal_path,
    );
}

#[test]
fn age160_classify_read_only_open_error_via_typed_projection_readonly_cantlock() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::ReadOnly,
                sqlite::ffi::SQLITE_READONLY_CANTLOCK,
                "readonly cantlock extended code without diagnostic-token dependency",
            ),
        ),
        &path,
    );
}

#[test]
fn age160_classify_read_only_open_error_via_typed_projection_readonly_recovery() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::ReadOnly,
                sqlite::ffi::SQLITE_READONLY_RECOVERY,
                "readonly recovery extended code without diagnostic-token dependency",
            ),
        ),
        &path,
    );
}

#[test]
fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_probe_path_branch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    std::fs::write(&path, b"placeholder").unwrap();
    std::fs::write(shm_path(&path), b"owned shm sidecar").unwrap();

    assert!(
        !wal_path(&path).exists(),
        "fixture should exercise only the shm sidecar probe branch"
    );
    age160_assert_wal_sidecar(
        classify_read_only_open_error(
            &path,
            age160_sqlite_failure(
                sqlite::ffi::ErrorCode::SystemIoFailure,
                sqlite::ffi::ErrorCode::SystemIoFailure as i32,
                "plain io failure text intentionally lacks sidecar tokens",
            ),
        ),
        &path,
    );
}

#[test]
fn age160_classify_read_only_open_error_via_typed_projection_shm_sidecar_extended_codes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");

    for extended_code in [
        sqlite::ffi::SQLITE_IOERR_SHMOPEN,
        sqlite::ffi::SQLITE_IOERR_SHMSIZE,
        sqlite::ffi::SQLITE_IOERR_SHMLOCK,
        sqlite::ffi::SQLITE_IOERR_SHMMAP,
    ] {
        age160_assert_wal_sidecar(
            classify_read_only_open_error(
                &path,
                age160_sqlite_failure(
                    sqlite::ffi::ErrorCode::SystemIoFailure,
                    extended_code,
                    "typed SHM sidecar evidence; message intentionally generic",
                ),
            ),
            &path,
        );
    }
}

#[test]
fn age160_sqlite_adapter_read_only_projection_and_namespace_contract() {
    use crate::db::sqlite_adapter::{
        Connection as AdapterConnection, OpenFlags as AdapterOpenFlags,
        OptionalExtension as AdapterOptionalExtension, ReadOnlyOpenFailure, Row as AdapterRow,
        SidecarProbe, SqliteFailureProjection, Statement as AdapterStatement,
        Transaction as AdapterTransaction, params as adapter_params,
    };

    fn _accept_row(_: &AdapterRow<'_>) {}
    fn _accept_statement(_: &mut AdapterStatement<'_>) {}
    fn _accept_transaction(_: &AdapterTransaction<'_>) {}
    fn _accept_optional<T: AdapterOptionalExtension>(value: T) -> T {
        value
    }

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.db");
    let conn = AdapterConnection::open_with_flags(
        &path,
        AdapterOpenFlags::SQLITE_OPEN_READ_WRITE | AdapterOpenFlags::SQLITE_OPEN_CREATE,
    )
    .unwrap();
    conn.execute(
        "CREATE TABLE contract_probe (id INTEGER PRIMARY KEY)",
        adapter_params![],
    )
    .unwrap();

    let projection = SqliteFailureProjection::from(&age160_sqlite_failure(
        sqlite::ffi::ErrorCode::NotADatabase,
        sqlite::ffi::ErrorCode::NotADatabase as i32,
        "not db",
    ));
    assert!(matches!(
        ReadOnlyOpenFailure::from_projection(&path, projection, SidecarProbe::for_db(&path)),
        ReadOnlyOpenFailure::PlainDb { .. }
    ));
    let _ = _accept_optional(Ok::<Option<i64>, sqlite::Error>(Some(1)));
}
