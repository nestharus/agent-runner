//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - predicate
//! - validator
//!
//! Role set: { accessor, formatter, mapper, predicate, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/tests/common/read_only_error_helpers.rs
//!     role: intrinsic-surface
//!     Domain: read-only-error-helpers-test-fixture
//!     Owns:
//!       - the db test fixture surface this module owns: StateDb-owned temp databases,
//!       -   schema/rows, and concern DTOs it seeds and inspects via `use super::*`
//!       - all StateDb/rusqlite carriers referenced via `use super::*`, subordinate to
//!       -   this fixture domain: StateDb, sqlite, params, Connection, Transaction, Row,
//!       -   Statement, Uuid, and the concern-owned DTOs each test exercises
//! ```

use super::super::*;
pub(in crate::db::tests) fn age160_sqlite_failure(
    code: sqlite::ffi::ErrorCode,
    extended_code: i32,
    message: &str,
) -> sqlite::Error {
    sqlite::Error::SqliteFailure(
        sqlite::ffi::Error {
            code,
            extended_code,
        },
        Some(message.to_string()),
    )
}

pub(in crate::db::tests) fn age160_assert_not_database(
    error: ReadOnlyOpenError,
    expected_path: &Path,
) {
    match error {
        ReadOnlyOpenError::NotADatabase { path, .. } => assert_eq!(path, expected_path),
        other => panic!("expected NotADatabase, got {other:?}"),
    }
}

pub(in crate::db::tests) fn age160_assert_permission_denied(
    error: ReadOnlyOpenError,
    expected_path: &Path,
) {
    match error {
        ReadOnlyOpenError::PermissionDenied { path } => assert_eq!(path, expected_path),
        other => panic!("expected PermissionDenied, got {other:?}"),
    }
}

pub(in crate::db::tests) fn age160_assert_operational(error: ReadOnlyOpenError) {
    match error {
        ReadOnlyOpenError::Operational { message } => assert!(!message.is_empty()),
        other => panic!("expected Operational, got {other:?}"),
    }
}

pub(in crate::db::tests) fn age160_assert_wal_sidecar(
    error: ReadOnlyOpenError,
    expected_path: &Path,
) {
    match error {
        ReadOnlyOpenError::WalSidecarError { path, message } => {
            assert_eq!(path, expected_path);
            assert!(!message.is_empty());
        }
        other => panic!("expected WalSidecarError, got {other:?}"),
    }
}
