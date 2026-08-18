//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - validator
//! - mapper
//! - predicate
//!
//! Role set: { accessor, formatter, validator, mapper, predicate }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/opening_read_only.rs
//!     role: intrinsic-surface
//!     Domain: opening-read-only-persistence
//!     Owns:
//!       - StateDb opening-read-only persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, Path, PathBuf, ReadOnlyOpenError, StateDb, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: PermissionsExt
//! ```
//!
//! Read-only state database open error classification and path validation.

use super::*;

pub(super) fn classify_read_only_open_error(path: &Path, err: sqlite::Error) -> ReadOnlyOpenError {
    match sqlite::project_read_only_open_error(path, &err) {
        sqlite::ReadOnlyOpenFailure::WalSidecar { message }
        | sqlite::ReadOnlyOpenFailure::ShmSidecar { message } => {
            read_only_wal_sidecar_error(path.to_path_buf(), message)
        }
        sqlite::ReadOnlyOpenFailure::PlainDb { kind, message } => {
            read_only_plain_db_error(path, kind, message)
        }
        sqlite::ReadOnlyOpenFailure::Unknown { message } => {
            ReadOnlyOpenError::Operational { message }
        }
    }
}

pub(super) fn read_only_plain_db_error(
    path: &Path,
    kind: sqlite::PlainDbKind,
    message: String,
) -> ReadOnlyOpenError {
    match kind {
        sqlite::PlainDbKind::NotDatabase | sqlite::PlainDbKind::Corrupt => {
            read_only_not_database(path.to_path_buf(), message)
        }
        sqlite::PlainDbKind::PermissionDenied => read_only_permission_denied(path.to_path_buf()),
        sqlite::PlainDbKind::ReadOnly
        | sqlite::PlainDbKind::CannotOpen
        | sqlite::PlainDbKind::SystemIo => ReadOnlyOpenError::Operational { message },
    }
}

pub(super) fn read_only_not_database(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::NotADatabase { path, message }
}

pub(super) fn read_only_permission_denied(path: PathBuf) -> ReadOnlyOpenError {
    ReadOnlyOpenError::PermissionDenied { path }
}

pub(super) fn read_only_wal_sidecar_error(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::WalSidecarError { path, message }
}

pub(super) fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

pub(super) fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

fn sidecar_is_unreadable(sidecar: &Path) -> bool {
    sidecar.exists() && path_is_unreadable(sidecar)
}

fn unreadable_sidecar_error(path: &Path, sidecar: &Path) -> ReadOnlyOpenError {
    read_only_wal_sidecar_error(path.to_path_buf(), unreadable_sidecar_message(sidecar))
}

fn unreadable_sidecar_message(sidecar: &Path) -> String {
    format!("SQLite sidecar is not readable: {}", sidecar.display())
}

#[cfg(unix)]
pub(super) fn path_is_unreadable(path: &Path) -> bool {
    classify_unix_path_unreadable(read_path_metadata(path))
}

#[cfg(unix)]
fn read_path_metadata(path: &Path) -> std::io::Result<std::fs::Metadata> {
    std::fs::metadata(path)
}

#[cfg(unix)]
fn classify_unix_path_unreadable(result: std::io::Result<std::fs::Metadata>) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match result {
        Ok(metadata) => metadata.permissions().mode() & 0o444 == 0,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[cfg(not(unix))]
pub(super) fn path_is_unreadable(path: &Path) -> bool {
    classify_file_open_unreadable(open_file_for_read_probe(path))
}

#[cfg(not(unix))]
fn open_file_for_read_probe(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(not(unix))]
fn classify_file_open_unreadable(result: std::io::Result<std::fs::File>) -> bool {
    match result {
        Ok(_) => false,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

impl StateDb {
    pub(super) fn validate_read_only_paths(path: &Path) -> Result<PathBuf, ReadOnlyOpenError> {
        if !path.exists() {
            return Err(ReadOnlyOpenError::Missing {
                path: path.to_path_buf(),
            });
        }
        if path_is_unreadable(path) {
            return Err(ReadOnlyOpenError::PermissionDenied {
                path: path.to_path_buf(),
            });
        }
        let canonical =
            std::fs::canonicalize(path).map_err(|error| ReadOnlyOpenError::Operational {
                message: format!("Failed to resolve read-only SQLite database identity: {error}"),
            })?;
        Self::validate_read_only_sidecars(&canonical)?;
        Ok(canonical)
    }

    pub(super) fn validate_read_only_sidecars(path: &Path) -> Result<(), ReadOnlyOpenError> {
        for sidecar in [wal_path(path), shm_path(path)] {
            if sidecar_is_unreadable(&sidecar) {
                return Err(unreadable_sidecar_error(path, &sidecar));
            }
        }
        Ok(())
    }

    pub(super) fn open_read_only_connection(
        path: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<
        (
            sqlite::Connection,
            crate::read_only_snapshot::ReadOnlySnapshot,
        ),
        ReadOnlyOpenError,
    > {
        let snapshot =
            crate::read_only_snapshot::ReadOnlySnapshot::create_with_cancel(path, is_cancelled)
                .map_err(|err| ReadOnlyOpenError::Operational {
                    message: format!("Failed to snapshot read-only SQLite database: {err}"),
                })?;
        let conn = sqlite::Connection::open_with_flags(
            snapshot.path(),
            sqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|err| classify_read_only_open_error(path, err))?;
        Ok((conn, snapshot))
    }

    pub(super) fn probe_read_only_schema(
        path: &Path,
        conn: &sqlite::Connection,
    ) -> Result<(), ReadOnlyOpenError> {
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_row| Ok(()))
            .map_err(|err| classify_read_only_open_error(path, err))
    }
}
