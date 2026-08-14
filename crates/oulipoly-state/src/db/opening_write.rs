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

pub struct StateReadConnection<'a> {
    conn: &'a sqlite::Connection,
}

impl StateReadConnection<'_> {
    pub fn prepare<'connection>(
        &'connection self,
        sql: &str,
    ) -> rusqlite::Result<rusqlite::Statement<'connection>> {
        let statement = self.conn.prepare(sql)?;
        if read_projection_allows(sql, statement.readonly()) {
            Ok(statement)
        } else {
            Err(rusqlite::Error::InvalidQuery)
        }
    }

    pub fn query_row<T, P, F>(&self, sql: &str, params: P, map: F) -> rusqlite::Result<T>
    where
        P: rusqlite::Params,
        F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    {
        let mut statement = self.prepare(sql)?;
        statement.query_row(params, map)
    }
}

fn read_projection_allows(sql: &str, sqlite_readonly: bool) -> bool {
    let Some(trimmed) = sql_without_leading_trivia(sql) else {
        return false;
    };
    let Some(directive) = trimmed
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("pragma"))
        .map(|_| trimmed[6..].trim_start())
    else {
        if !sqlite_readonly {
            return false;
        }
        let statement_class = trimmed
            .split(|character: char| character.is_whitespace() || character == '(')
            .next()
            .unwrap_or_default();
        return statement_class.eq_ignore_ascii_case("select")
            || statement_class.eq_ignore_ascii_case("with");
    };
    if directive.contains('=') {
        return false;
    }
    let name = directive
        .split(|character: char| character == '(' || character == ';' || character.is_whitespace())
        .next()
        .unwrap_or_default()
        .rsplit('.')
        .next()
        .unwrap_or_default();
    let has_argument = directive.contains('(');
    if has_argument
        && ![
            "foreign_key_list",
            "index_info",
            "index_list",
            "integrity_check",
            "quick_check",
            "table_info",
            "table_xinfo",
        ]
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
    {
        return false;
    }
    [
        "busy_timeout",
        "database_list",
        "foreign_key_list",
        "index_info",
        "index_list",
        "integrity_check",
        "journal_mode",
        "quick_check",
        "table_info",
        "table_xinfo",
        "user_version",
    ]
    .iter()
    .any(|allowed| name.eq_ignore_ascii_case(allowed))
}

fn sql_without_leading_trivia(mut sql: &str) -> Option<&str> {
    loop {
        sql = sql.trim_start();
        if let Some(comment) = sql.strip_prefix("--") {
            sql = comment.split_once('\n')?.1;
            continue;
        }
        if let Some(comment) = sql.strip_prefix("/*") {
            sql = comment.split_once("*/")?.1;
            continue;
        }
        return (!sql.is_empty()).then_some(sql);
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
        let nonlocal = Self::is_nonlocal_sqlite_path(path);
        if !nonlocal {
            Self::ensure_state_parent_dir(path)?;
        }
        let db_path = Self::normalized_state_open_path(path);
        let state_namespace_guard = if nonlocal {
            None
        } else {
            // Keep supported rebuild from replacing the inode behind this connection.
            Some(StateNamespaceGuard::acquire(&db_path, false)?)
        };
        Self::open_with_prepared_namespace(
            path,
            db_path,
            sink,
            provider_names,
            state_namespace_guard,
        )
    }

    fn open_with_prepared_namespace(
        source_path: &Path,
        db_path: PathBuf,
        sink: Box<dyn LifecycleEventSink + Send>,
        provider_names: &LegacyProviderNames,
        state_namespace_guard: Option<StateNamespaceGuard>,
    ) -> Result<Self, String> {
        let mut conn = Self::open_state_connection(&db_path)?;

        let ran_open_migrations = Self::run_open_migrations(&db_path, &mut conn)?;
        Self::apply_current_schema_repairs(&mut conn, ran_open_migrations, provider_names)?;
        let completion_authority_state =
            Self::durable_completion_authority_path(source_path, &db_path);
        let db = StateDb {
            conn,
            db_path,
            completion_authority_state,
            lifecycle_sink: Mutex::new(sink),
            _read_only_snapshot: None,
            _state_namespace_guard: state_namespace_guard,
        };
        db.complete_open_backfill()?;

        Ok(db)
    }

    fn open_state_connection(path: &Path) -> Result<sqlite::Connection, String> {
        let conn = sqlite::Connection::open(path).map_err(Self::format_state_db_open_error)?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(Self::format_state_db_foreign_keys_error)?;
        migrations::register_connection_primitives(&conn)
            .map_err(Self::format_state_db_primitive_error)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|err| format!("Failed to configure state DB busy timeout: {err}"))?;
        Ok(conn)
    }

    fn format_state_db_foreign_keys_error(err: sqlite::Error) -> String {
        format!("Failed to enable state DB foreign-key enforcement: {err}")
    }

    fn format_state_db_primitive_error(err: sqlite::Error) -> String {
        format!("Failed to register state DB connection primitives: {err}")
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

    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError> {
        Self::validate_read_only_paths(path)?;
        let (conn, snapshot) = Self::open_read_only_connection(path)?;
        Self::probe_read_only_schema(path, &conn)?;

        Ok(Self {
            conn,
            db_path: path.to_path_buf(),
            completion_authority_state: None,
            lifecycle_sink: Mutex::new(Box::new(NoopLifecycleEventSink)),
            _read_only_snapshot: Some(snapshot),
            _state_namespace_guard: None,
        })
    }

    pub fn open_default() -> Result<Self, String> {
        let db_path = Self::default_path()?;
        Self::open(&db_path)
    }

    pub fn open_for_memory(path: impl AsRef<Path>) -> Result<Self, String> {
        Self::open(path.as_ref())
    }

    pub fn acquire_rebuild_authority(path: &Path) -> Result<StateDbRebuildAuthority, String> {
        if Self::is_nonlocal_sqlite_path(path) {
            return Err("State DB rebuild authority requires a local file path".to_string());
        }
        Self::ensure_state_parent_dir(path)?;
        let db_path = Self::normalized_state_open_path(path);
        Ok(StateDbRebuildAuthority {
            _guard: StateNamespaceGuard::acquire(&db_path, true)?,
            db_path,
        })
    }

    pub fn initialize_after_rebuild(
        path: &Path,
        authority: &StateDbRebuildAuthority,
    ) -> Result<(), String> {
        let db_path = Self::normalized_state_open_path(path);
        if db_path != authority.db_path {
            return Err("State DB rebuild authority does not match the requested path".to_string());
        }
        let db = Self::open_with_prepared_namespace(
            path,
            db_path,
            Box::new(NoopLifecycleEventSink),
            &LegacyProviderNames::new(),
            None,
        )?;
        drop(db);
        Ok(())
    }

    pub fn default_path() -> Result<PathBuf, String> {
        Ok(crate::paths::data_dir()?.join("state.db"))
    }

    pub fn connection(&self) -> StateReadConnection<'_> {
        StateReadConnection { conn: &self.conn }
    }

    pub(crate) fn raw_connection(&self) -> &sqlite::Connection {
        &self.conn
    }

    pub fn path(&self) -> &Path {
        &self.db_path
    }

    pub(super) fn completion_authority_state_path(&self) -> Option<&Path> {
        let authority = self.completion_authority_state.as_ref()?;
        let canonical = std::fs::canonicalize(&authority.path).ok()?;
        let metadata = std::fs::metadata(&canonical).ok()?;
        if canonical != authority.path
            || !metadata.is_file()
            || !state_file_has_one_link(&metadata)
            || state_file_identity(&metadata)? != authority.file
        {
            return None;
        }
        Some(&authority.path)
    }

    fn normalized_state_open_path(path: &Path) -> PathBuf {
        if Self::is_nonlocal_sqlite_path(path) {
            return path.to_path_buf();
        }
        if let Ok(canonical) = std::fs::canonicalize(path) {
            return canonical;
        }
        let Some(file_name) = path.file_name() else {
            return path.to_path_buf();
        };
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        std::fs::canonicalize(parent)
            .map(|parent| parent.join(file_name))
            .unwrap_or_else(|_| path.to_path_buf())
    }

    fn durable_completion_authority_path(
        source_path: &Path,
        normalized_path: &Path,
    ) -> Option<CompletionAuthorityStateIdentity> {
        if !source_path.is_absolute()
            || Self::is_nonlocal_sqlite_path(source_path)
            || !source_path.ancestors().all(|component| {
                std::fs::symlink_metadata(component)
                    .is_ok_and(|metadata| !metadata.file_type().is_symlink())
            })
        {
            return None;
        }
        let canonical = std::fs::canonicalize(normalized_path).ok()?;
        let metadata = std::fs::metadata(&canonical).ok()?;
        if !metadata.is_file() || !state_file_has_one_link(&metadata) {
            return None;
        }
        Some(CompletionAuthorityStateIdentity {
            path: canonical,
            file: state_file_identity(&metadata)?,
        })
    }

    fn is_nonlocal_sqlite_path(path: &Path) -> bool {
        path == Path::new(":memory:") || path.as_os_str().as_encoded_bytes().starts_with(b"file:")
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

#[cfg(unix)]
fn state_file_has_one_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink() == 1
}

#[cfg(unix)]
fn state_file_identity(metadata: &std::fs::Metadata) -> Option<StateFileIdentity> {
    use std::os::unix::fs::MetadataExt;

    Some(StateFileIdentity {
        volume: metadata.dev(),
        file: metadata.ino(),
    })
}

#[cfg(windows)]
fn state_file_has_one_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.number_of_links() == Some(1)
}

#[cfg(windows)]
fn state_file_identity(metadata: &std::fs::Metadata) -> Option<StateFileIdentity> {
    use std::os::windows::fs::MetadataExt;

    Some(StateFileIdentity {
        volume: u64::from(metadata.volume_serial_number()?),
        file: metadata.file_index()?,
    })
}

#[cfg(not(any(unix, windows)))]
fn state_file_has_one_link(_metadata: &std::fs::Metadata) -> bool {
    false
}

#[cfg(not(any(unix, windows)))]
fn state_file_identity(_metadata: &std::fs::Metadata) -> Option<StateFileIdentity> {
    None
}

impl StateNamespaceGuard {
    fn acquire(state_path: &Path, exclusive: bool) -> Result<Self, String> {
        const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);
        #[cfg(not(test))]
        const ACQUISITION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
        #[cfg(test)]
        const ACQUISITION_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

        let authority_path = state_namespace_authority_path(state_path)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&authority_path)
            .map_err(|error| {
                format!(
                    "Failed to open State DB namespace authority fence {}: {error}",
                    authority_path.display()
                )
            })?;
        let deadline = std::time::Instant::now() + ACQUISITION_TIMEOUT;
        loop {
            let result = if exclusive {
                <std::fs::File as fs4::FileExt>::try_lock(&file)
            } else {
                <std::fs::File as fs4::FileExt>::try_lock_shared(&file)
            };
            match result {
                Ok(()) => return Ok(Self { file }),
                Err(fs4::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(fs4::TryLockError::WouldBlock) => {
                    return Err(format!(
                        "Timed out after {}ms acquiring State DB namespace authority fence {}",
                        ACQUISITION_TIMEOUT.as_millis(),
                        authority_path.display()
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "Failed to lock State DB namespace authority: {error}"
                    ));
                }
            }
        }
    }
}

impl Drop for StateNamespaceGuard {
    fn drop(&mut self) {
        let _ = <std::fs::File as fs4::FileExt>::unlock(&self.file);
    }
}

fn state_namespace_authority_path(state_path: &Path) -> Result<PathBuf, String> {
    let file_name = state_path
        .file_name()
        .ok_or_else(|| format!("State DB path has no file name: {}", state_path.display()))?;
    let mut authority_name = file_name.to_os_string();
    authority_name.push(".namespace.lock");
    Ok(state_path.with_file_name(authority_name))
}

#[cfg(test)]
mod state_namespace_tests {
    use super::*;

    #[test]
    fn writable_state_holds_shared_namespace_authority_for_its_lifetime() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let state = StateDb::open(&state_path).unwrap();

        let error = StateDb::acquire_rebuild_authority(&state_path)
            .err()
            .expect("live writable state must exclude rebuild");

        assert!(error.contains("Timed out"), "{error}");
        drop(state);
        drop(StateDb::acquire_rebuild_authority(&state_path).unwrap());
    }

    #[test]
    fn rebuild_authority_excludes_writable_open_and_covers_fresh_initialization() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let authority = StateDb::acquire_rebuild_authority(&state_path).unwrap();
        std::fs::remove_file(&state_path).unwrap();
        StateDb::initialize_after_rebuild(&state_path, &authority).unwrap();

        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let opener_path = state_path.clone();
        let opener = std::thread::spawn(move || sender.send(StateDb::open(&opener_path)).unwrap());
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "writable open escaped the rebuild namespace fence"
        );
        drop(authority);
        drop(receiver.recv().unwrap().unwrap());
        opener.join().unwrap();
    }
}
