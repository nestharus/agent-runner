//! ## Declared roles
//!
//! Role set: { accessor, mapper, orchestration, formatter, predicate }
//!
//! Per ACR-249/ACR-250 sqlite_adapter.rs is a declared multi-role adapter over the
//! rusqlite/libsqlite3-sys SQLite contract and Oulipoly StateDb's read-only open
//! projection contract. Adapter `Translates:` contracts are listed in the existing
//! `## Adapter declarations` block below.
//!
//! ## Adapter declarations
//!
//! ```yaml
//! adapter_declarations:
//!   - component: crates/oulipoly-state/src/db/sqlite_adapter.rs
//!     role: adapter
//!     Translates:
//!       - rusqlite/libsqlite3-sys SQLite connection, row, transaction, statement, type, params, and OptionalExtension contract
//!       - Oulipoly StateDb SQLite persistence contract via re-exported `sqlite::*` namespace consumed from `db.rs`
//!       - rusqlite::Error / libsqlite3-sys extended-code error contract
//!       - Oulipoly read-only open/probe error projection contract (`ReadOnlyOpenFailure`, `SqliteFailureProjection`, `SidecarProbe`)
//! ```

pub(super) use rusqlite::OptionalExtension as RusqliteOptionalExtension;
pub(super) use rusqlite::ffi;
pub(super) use rusqlite::params;
pub(super) use rusqlite::params_from_iter;
pub(super) use rusqlite::{
    Connection, Error, OpenFlags, Result, Row, Statement, Transaction, types::Type,
};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub(super) trait OptionalExtension {}

impl<T> OptionalExtension for T {}

pub(super) struct SqliteFailureProjection {
    pub(super) code: ffi::ErrorCode,
    pub(super) extended_code: c_int,
    pub(super) display: String,
}

impl From<&Error> for SqliteFailureProjection {
    fn from(err: &Error) -> Self {
        let sqlite_error = err.sqlite_error().copied();
        let code = err.sqlite_error_code().unwrap_or(ffi::ErrorCode::Unknown);
        Self {
            code,
            extended_code: sqlite_error.map_or(code as c_int, |error| error.extended_code),
            display: err.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PlainDbKind {
    NotDatabase,
    Corrupt,
    PermissionDenied,
    ReadOnly,
    CannotOpen,
    SystemIo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ReadOnlyOpenFailure {
    WalSidecar { message: String },
    ShmSidecar { message: String },
    PlainDb { kind: PlainDbKind, message: String },
    Unknown { message: String },
}

pub(super) struct SidecarProbe {
    wal_exists: bool,
    shm_exists: bool,
}

impl SidecarProbe {
    pub(super) fn for_db(path: &Path) -> Self {
        Self {
            wal_exists: sidecar_path(path, "wal").exists(),
            shm_exists: sidecar_path(path, "shm").exists(),
        }
    }
}

impl ReadOnlyOpenFailure {
    pub(super) fn from_projection(
        _path: &Path,
        projection: SqliteFailureProjection,
        sidecar: SidecarProbe,
    ) -> Self {
        let message = projection.display;
        if extended_code_is_shm_sidecar(projection.extended_code) {
            return Self::ShmSidecar { message };
        }
        if extended_code_is_wal_sidecar(projection.extended_code) {
            return Self::WalSidecar { message };
        }
        if projection.code == ffi::ErrorCode::SystemIoFailure && sidecar.wal_exists {
            return Self::WalSidecar { message };
        }
        if projection.code == ffi::ErrorCode::SystemIoFailure && sidecar.shm_exists {
            return Self::ShmSidecar { message };
        }
        if let Some(kind) = plain_kind_for_code(projection.code) {
            return Self::PlainDb { kind, message };
        }
        Self::Unknown { message }
    }
}

pub(super) fn project_read_only_open_error(path: &Path, err: &Error) -> ReadOnlyOpenFailure {
    ReadOnlyOpenFailure::from_projection(
        path,
        SqliteFailureProjection::from(err),
        SidecarProbe::for_db(path),
    )
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}-{suffix}", path.display()))
}

fn extended_code_is_shm_sidecar(extended_code: c_int) -> bool {
    matches!(
        extended_code,
        ffi::SQLITE_IOERR_SHMOPEN
            | ffi::SQLITE_IOERR_SHMSIZE
            | ffi::SQLITE_IOERR_SHMLOCK
            | ffi::SQLITE_IOERR_SHMMAP
            | ffi::SQLITE_READONLY_CANTLOCK
    )
}

fn extended_code_is_wal_sidecar(extended_code: c_int) -> bool {
    matches!(
        extended_code,
        ffi::SQLITE_CANTOPEN_DIRTYWAL | ffi::SQLITE_READONLY_RECOVERY
    )
}

fn plain_kind_for_code(code: ffi::ErrorCode) -> Option<PlainDbKind> {
    match code {
        ffi::ErrorCode::NotADatabase => Some(PlainDbKind::NotDatabase),
        ffi::ErrorCode::DatabaseCorrupt => Some(PlainDbKind::Corrupt),
        ffi::ErrorCode::PermissionDenied => Some(PlainDbKind::PermissionDenied),
        ffi::ErrorCode::ReadOnly => Some(PlainDbKind::ReadOnly),
        ffi::ErrorCode::CannotOpen => Some(PlainDbKind::CannotOpen),
        ffi::ErrorCode::SystemIoFailure => Some(PlainDbKind::SystemIo),
        _ => None,
    }
}
