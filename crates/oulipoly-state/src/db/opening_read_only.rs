//! ## Declared roles
//!
//! - validator
//! - mapper
//!
//! Role set: { validator, mapper }
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

#[cfg(unix)]
pub(super) fn path_is_unreadable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::metadata(path) {
        Ok(metadata) => metadata.permissions().mode() & 0o444 == 0,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[cfg(not(unix))]
pub(super) fn path_is_unreadable(path: &Path) -> bool {
    match std::fs::File::open(path) {
        Ok(_) => false,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

impl StateDb {
    pub(super) fn validate_read_only_paths(path: &Path) -> Result<(), ReadOnlyOpenError> {
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
        Self::validate_read_only_sidecars(path)
    }

    pub(super) fn validate_read_only_sidecars(path: &Path) -> Result<(), ReadOnlyOpenError> {
        for sidecar in [wal_path(path), shm_path(path)] {
            if sidecar.exists() && path_is_unreadable(&sidecar) {
                return Err(ReadOnlyOpenError::WalSidecarError {
                    path: path.to_path_buf(),
                    message: format!("SQLite sidecar is not readable: {}", sidecar.display()),
                });
            }
        }
        Ok(())
    }

    pub(super) fn open_read_only_connection(
        path: &Path,
    ) -> Result<sqlite::Connection, ReadOnlyOpenError> {
        sqlite::Connection::open_with_flags(path, sqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|err| classify_read_only_open_error(path, err))
    }

    pub(super) fn probe_read_only_schema(
        path: &Path,
        conn: &sqlite::Connection,
    ) -> Result<(), ReadOnlyOpenError> {
        conn.query_row("SELECT count(*) FROM sqlite_schema", [], |_row| Ok(()))
            .map_err(|err| classify_read_only_open_error(path, err))
    }
}
