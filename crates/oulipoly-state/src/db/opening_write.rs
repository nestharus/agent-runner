//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - orchestration
//! - validator
//!
//! Role set: { accessor, formatter, orchestration, validator }
//!
//! ## Intrinsic-surface declarations
//!
//! ```yaml
//! intrinsic_surface_declarations:
//!   - component: crates/oulipoly-state/src/db/opening_write.rs
//!     role: intrinsic-surface
//!     Domain: opening-write-persistence
//!     Owns:
//!       - StateDb opening-write persistence surface: the StateDb methods, owned
//!         tables/rows, and SQL this concern extends, split out of the StateDb
//!         facade by the WU #65 decomposition with the public API preserved
//!       - Intrinsic StateDb/rusqlite carriers and concern-owned DTOs referenced
//!         via `use super::*`, subordinate to this domain: Connection, LifecycleEventSink, Mutex, NoopLifecycleEventSink, Path, PathBuf, ReadOnlyOpenError, StateDb, Transaction, sqlite
//!       - external contract symbols referenced by this concern via its `use`
//!         declarations, intrinsic and subordinate to this persistence domain: migrations
//! ```
//!
//! State database write/open entry points and current validator repairs.

use super::*;
use crate::migrations;

impl StateDb {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_sink(path, Box::new(NoopLifecycleEventSink))
    }

    pub fn open_with_sink(
        path: &Path,
        sink: Box<dyn LifecycleEventSink + Send>,
    ) -> Result<Self, String> {
        Self::open_with_sink_and_legacy_provider_names(path, sink, &LegacyProviderNames::new())
    }

    /// Open with a caller-pushed legacy provider-name lookup (PP-001 inversion).
    /// Used by the app's migrate path, which owns the models-config layout and
    /// resolves `(model_name, provider_index) -> provider_name` before pushing
    /// it in. The plain `open`/`open_with_sink` paths default to an empty map so
    /// StateDb never discovers the app config layout itself.
    pub fn open_with_legacy_provider_names(
        path: &Path,
        provider_names: &LegacyProviderNames,
    ) -> Result<Self, String> {
        Self::open_with_sink_and_legacy_provider_names(
            path,
            Box::new(NoopLifecycleEventSink),
            provider_names,
        )
    }

    fn open_with_sink_and_legacy_provider_names(
        path: &Path,
        sink: Box<dyn LifecycleEventSink + Send>,
        provider_names: &LegacyProviderNames,
    ) -> Result<Self, String> {
        Self::ensure_state_parent_dir(path)?;
        let mut conn = Self::open_state_connection(path)?;

        let ran_open_migrations = Self::run_open_migrations(path, &mut conn)?;
        Self::apply_current_schema_repairs(&mut conn, ran_open_migrations, provider_names)?;
        let db = StateDb {
            conn,
            db_path: path.to_path_buf(),
            lifecycle_sink: Mutex::new(sink),
        };
        db.complete_open_backfill()?;

        Ok(db)
    }

    fn open_state_connection(path: &Path) -> Result<sqlite::Connection, String> {
        sqlite::Connection::open(path).map_err(Self::format_state_db_open_error)
    }

    fn format_state_db_open_error(err: sqlite::Error) -> String {
        format!("Failed to open state DB: {err}")
    }

    fn complete_open_backfill(&self) -> Result<(), String> {
        self.backfill_session_chains()
            .map(|_| ())
            .map_err(Self::format_open_backfill_error)
    }

    fn format_open_backfill_error(err: String) -> String {
        format!("{err}; run `agents migrate-db` first")
    }

    pub(super) fn apply_current_schema_repairs(
        conn: &mut sqlite::Connection,
        ran_open_migrations: bool,
        provider_names: &LegacyProviderNames,
    ) -> Result<(), String> {
        Self::validate_providers_schema(conn)?;
        Self::ensure_invocations_schema(conn, provider_names)?;
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
        let mut tx = self.begin_state_db_transaction()?;
        match f(&mut tx) {
            Ok(value) => {
                Self::commit_state_db_transaction(tx)?;
                Ok(value)
            }
            Err(err) => Err(err),
        }
    }

    fn begin_state_db_transaction(&mut self) -> Result<Transaction<'_>, String> {
        self.conn
            .transaction()
            .map_err(Self::format_state_db_transaction_begin_error)
    }

    fn format_state_db_transaction_begin_error(err: sqlite::Error) -> String {
        format!("Failed to begin state DB transaction: {err}")
    }

    fn commit_state_db_transaction(tx: Transaction<'_>) -> Result<(), String> {
        tx.commit()
            .map_err(Self::format_state_db_transaction_commit_error)
    }

    fn format_state_db_transaction_commit_error(err: sqlite::Error) -> String {
        format!("Failed to commit state DB transaction: {err}")
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
            Self::create_state_parent_dir(parent)?;
        }
        Ok(())
    }

    fn create_state_parent_dir(parent: &Path) -> Result<(), String> {
        std::fs::create_dir_all(parent).map_err(Self::format_state_directory_create_error)
    }

    fn format_state_directory_create_error(err: std::io::Error) -> String {
        format!("Failed to create state directory: {err}")
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
}
