//! ## Declared roles
//!
//! - mutator
//! - schema
//! - validator
//!
//! Role set: { mutator, schema, validator }
//!
//! StateDb open, migration dispatch, and read-only-open validation helpers.

use super::*;
use crate::migrations;
use crate::schema::{
    CURRENT_SCHEMA_VERSION, MINIMUM_SUPPORTED_SCHEMA_VERSION, SchemaCompatibility,
};

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

fn read_only_plain_db_error(
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

fn read_only_not_database(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::NotADatabase { path, message }
}

fn read_only_permission_denied(path: PathBuf) -> ReadOnlyOpenError {
    ReadOnlyOpenError::PermissionDenied { path }
}

fn read_only_wal_sidecar_error(path: PathBuf, message: String) -> ReadOnlyOpenError {
    ReadOnlyOpenError::WalSidecarError { path, message }
}

pub(super) fn wal_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", path.display()))
}

pub(super) fn shm_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", path.display()))
}

#[cfg(unix)]
fn path_is_unreadable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    match std::fs::metadata(path) {
        Ok(metadata) => metadata.permissions().mode() & 0o444 == 0,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[cfg(not(unix))]
fn path_is_unreadable(path: &Path) -> bool {
    match std::fs::File::open(path) {
        Ok(_) => false,
        Err(err) => err.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

impl StateDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_sink(path, Box::new(NoopLifecycleEventSink))
    }

    pub fn open_with_sink(
        path: &Path,
        sink: Box<dyn LifecycleEventSink + Send>,
    ) -> Result<Self, String> {
        Self::ensure_state_parent_dir(path)?;
        let mut conn =
            sqlite::Connection::open(path).map_err(|e| format!("Failed to open state DB: {e}"))?;

        let ran_open_migrations = Self::run_open_migrations(path, &mut conn)?;
        Self::apply_current_schema_repairs(&mut conn, ran_open_migrations)?;
        let db = StateDb {
            conn,
            db_path: path.to_path_buf(),
            lifecycle_sink: Mutex::new(sink),
        };
        db.backfill_session_chains()
            .map_err(|e| format!("{e}; run `agents migrate-db` first"))?;

        Ok(db)
    }

    fn apply_current_schema_repairs(
        conn: &mut sqlite::Connection,
        ran_open_migrations: bool,
    ) -> Result<(), String> {
        Self::validate_providers_schema(conn)?;
        Self::ensure_invocations_schema(conn)?;
        Self::ensure_providers_schema(conn)?;
        Self::ensure_provider_quotas_schema(conn)?;
        Self::ensure_provider_quotas_topology_schema(conn)?;
        Self::ensure_provider_quota_windows_schema(conn)?;
        Self::ensure_session_turns_schema(conn)?;
        if ran_open_migrations {
            Self::apply_returned_artifacts_schema(conn)?;
        }
        Ok(())
    }

    pub fn with_write_txn<R, F>(&mut self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut Transaction<'_>) -> Result<R, String>,
    {
        let mut tx = self
            .conn
            .transaction()
            .map_err(|e| format!("Failed to begin state DB transaction: {e}"))?;
        match f(&mut tx) {
            Ok(value) => {
                tx.commit()
                    .map_err(|e| format!("Failed to commit state DB transaction: {e}"))?;
                Ok(value)
            }
            Err(err) => Err(err),
        }
    }

    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError> {
        Self::validate_read_only_paths(path)?;
        let conn = Self::open_read_only_connection(path)?;
        Self::probe_read_only_schema(path, &conn)?;

        Ok(Self {
            conn,
            db_path: path.to_path_buf(),
            lifecycle_sink: Mutex::new(Box::new(NoopLifecycleEventSink)),
        })
    }

    pub fn open_default() -> Result<Self, String> {
        let db_path = Self::default_path()?;
        Self::open(&db_path)
    }

    pub fn open_for_memory(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::open(path.as_ref())
    }

    pub fn default_path() -> Result<PathBuf, String> {
        Ok(crate::paths::data_dir()?.join("state.db"))
    }

    pub fn connection(&self) -> &sqlite::Connection {
        &self.conn
    }

    pub(super) fn ensure_state_parent_dir(path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create state directory: {e}"))?;
        }
        Ok(())
    }

    pub(super) fn run_open_migrations(
        path: &Path,
        conn: &mut sqlite::Connection,
    ) -> Result<bool, String> {
        let compatibility = migrations::classify(conn)?;
        let ran_open_migrations = Self::compatibility_runs_open_migrations(&compatibility);
        Self::dispatch_open_migration_plan(path, conn, compatibility)?;
        Ok(ran_open_migrations)
    }

    fn compatibility_runs_open_migrations(compatibility: &SchemaCompatibility) -> bool {
        matches!(
            compatibility,
            SchemaCompatibility::Fresh
                | SchemaCompatibility::Migratable { .. }
                | SchemaCompatibility::LegacyVersionless
        )
    }

    fn dispatch_open_migration_plan(
        path: &Path,
        conn: &mut sqlite::Connection,
        compatibility: SchemaCompatibility,
    ) -> Result<(), String> {
        match compatibility {
            SchemaCompatibility::Fresh => {
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, 0)
            }
            SchemaCompatibility::Current { .. } => Self::set_wal_mode(conn),
            SchemaCompatibility::Migratable { stored } => {
                Self::set_wal_mode(conn)?;
                let stored = Self::promote_existing_dual_id_schema5_if_present(conn, stored)?;
                Self::run_current_plan_from(path, conn, stored)
            }
            SchemaCompatibility::LegacyVersionless => {
                Self::validate_versionless_shape(path, conn)?;
                Self::set_wal_mode(conn)?;
                Self::run_current_plan_from(path, conn, MINIMUM_SUPPORTED_SCHEMA_VERSION)
            }
            SchemaCompatibility::Future { stored } => Err(Self::future_schema_error(path, stored)),
            SchemaCompatibility::UnrecognizedVersionless => {
                Err(Self::unrecognized_versionless_error(path))
            }
            SchemaCompatibility::Corrupt { reason } => {
                Err(Self::corrupt_schema_error(path, reason))
            }
        }
    }

    fn run_current_plan_from(
        path: &Path,
        conn: &mut sqlite::Connection,
        stored: i32,
    ) -> Result<(), String> {
        let plan = migrations::current_plan_from(stored).map_err(|e| e.to_string())?;
        migrations::run_with_db_path(conn, &plan, path.to_path_buf()).map_err(|e| e.to_string())
    }

    fn validate_versionless_shape(path: &Path, conn: &sqlite::Connection) -> Result<(), String> {
        if migrations::classify_versionless(conn)?.is_some() {
            Ok(())
        } else {
            Err(Self::unrecognized_versionless_error(path))
        }
    }

    fn future_schema_error(path: &Path, stored: i32) -> String {
        migrations::MigrationError::Incompatible {
            db_path: path.to_path_buf(),
            stored,
            current: CURRENT_SCHEMA_VERSION,
        }
        .to_string()
    }

    fn unrecognized_versionless_error(path: &Path) -> String {
        migrations::MigrationError::UnrecognizedShape {
            db_path: path.to_path_buf(),
        }
        .to_string()
    }

    fn corrupt_schema_error(path: &Path, reason: String) -> String {
        format!(
            "Corrupt schema ({reason}); run `agents migrate --rebuild`. db={}",
            path.display()
        )
    }

    pub(super) fn apply_returned_artifacts_schema(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch(invocation_returned_artifacts_schema_sql!())
            .map_err(|e| format!("Failed to ensure returned-artifacts schema: {e}"))
    }

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

    fn validate_read_only_sidecars(path: &Path) -> Result<(), ReadOnlyOpenError> {
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

    fn set_wal_mode(conn: &sqlite::Connection) -> Result<(), String> {
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {e}; run `agents migrate --rebuild`"))
    }
}
