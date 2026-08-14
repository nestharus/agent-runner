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
        if Self::is_sqlite_uri_path(path) {
            return Err("State DB writable open does not accept SQLite URI paths".to_string());
        }
        let nonlocal = Self::is_nonlocal_sqlite_path(path);
        if !nonlocal {
            Self::validate_local_state_path(path)?;
            Self::ensure_state_parent_dir(path)?;
        }
        let db_path = Self::normalized_state_open_path(path);
        if !nonlocal {
            Self::validate_local_state_path(&db_path)?;
        }
        let state_namespace_guard = if nonlocal {
            None
        } else {
            // Keep supported rebuild from replacing the inode behind this connection.
            Some(StateNamespaceGuard::acquire(&db_path, false)?)
        };
        if !nonlocal {
            Self::validate_state_namespace_file(&db_path)?;
        }
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
        Self::validate_rebuild_path(path)?;
        Self::ensure_state_parent_dir(path)?;
        let db_path = Self::normalized_state_open_path(path);
        Self::validate_local_state_path(&db_path)?;
        let guard = StateNamespaceGuard::acquire(&db_path, true)?;
        Self::validate_rebuild_source(path, &db_path)?;
        Ok(StateDbRebuildAuthority {
            _guard: guard,
            db_path,
        })
    }

    pub fn acquire_writer_authority(path: &Path) -> Result<StateDbWriterAuthority, String> {
        if Self::is_nonlocal_sqlite_path(path) {
            return Err("State DB writer authority requires a local file path".to_string());
        }
        Self::validate_local_state_path(path)?;
        Self::ensure_state_parent_dir(path)?;
        let db_path = Self::normalized_state_open_path(path);
        Self::validate_local_state_path(&db_path)?;
        let guard = StateNamespaceGuard::acquire(&db_path, false)?;
        Self::validate_state_namespace_file(&db_path)?;
        Ok(StateDbWriterAuthority {
            _guard: guard,
            db_path,
        })
    }

    pub fn initialize_after_rebuild(
        path: &Path,
        authority: &StateDbRebuildAuthority,
    ) -> Result<(), String> {
        Self::validate_rebuild_path(path)?;
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

    pub fn validate_rebuild_path(path: &Path) -> Result<(), String> {
        if Self::is_nonlocal_sqlite_path(path) {
            return Err("State DB rebuild authority requires a local file path".to_string());
        }
        Self::validate_local_state_path(path)?;
        Self::validate_local_state_path(&Self::normalized_state_open_path(path))?;
        Self::reject_dangling_rebuild_ancestor_symlink(path)?;
        Self::reject_rebuild_leaf_symlink(path)
    }

    pub fn connection(&self) -> StateReadConnection<'_> {
        StateReadConnection { conn: &self.conn }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn disable_wal_autocheckpoint_for_test(&self) -> Result<(), String> {
        self.conn
            .pragma_update(None, "wal_autocheckpoint", 0)
            .map_err(|err| format!("Failed to disable state DB WAL auto-checkpointing: {err}"))
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
        path == Path::new(":memory:") || Self::is_sqlite_uri_path(path)
    }

    fn is_sqlite_uri_path(path: &Path) -> bool {
        path.as_os_str().as_encoded_bytes().starts_with(b"file:")
    }

    fn validate_local_state_path(path: &Path) -> Result<(), String> {
        if path.to_str().is_none() {
            return Err("State DB local file paths must be valid UTF-8".to_string());
        }
        if path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(reserved_state_storage_name)
        {
            return Err(format!(
                "State DB local file path uses a reserved storage role: {}",
                path.display()
            ));
        }
        Ok(())
    }

    fn reject_rebuild_leaf_symlink(path: &Path) -> Result<(), String> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(format!(
                "State DB rebuild does not accept a leaf symlink: {}",
                path.display()
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "Failed to inspect state DB rebuild source {}: {error}",
                path.display()
            )),
        }
    }

    fn reject_dangling_rebuild_ancestor_symlink(path: &Path) -> Result<(), String> {
        for ancestor in path.ancestors().skip(1) {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    std::fs::canonicalize(ancestor).map_err(|error| {
                        format!(
                            "State DB rebuild does not accept a dangling ancestor symlink {}: {error}",
                            ancestor.display()
                        )
                    })?;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "Failed to inspect state DB rebuild ancestor {}: {error}",
                        ancestor.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_rebuild_source(path: &Path, authority_path: &Path) -> Result<(), String> {
        Self::reject_rebuild_leaf_symlink(path)?;
        if Self::normalized_state_open_path(path) != authority_path {
            return Err(format!(
                "State DB rebuild source changed during authority acquisition: {}",
                path.display()
            ));
        }
        Self::validate_state_namespace_file(authority_path)
    }

    fn validate_state_namespace_file(path: &Path) -> Result<(), String> {
        inspect_state_storage_file(path, "namespace file")?;
        for artifact in state_sqlite_artifact_paths(path) {
            inspect_state_storage_file(&artifact, "SQLite artifact")?;
        }
        Ok(())
    }
}

#[cfg(any(unix, windows))]
fn inspect_state_storage_file(
    path: &Path,
    role: &str,
) -> Result<Option<StateFileIdentity>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Failed to inspect State DB {role} {}: {error}",
                path.display()
            ));
        }
    };
    if !metadata.is_file() || !state_file_has_one_link(&metadata) {
        return Err(format!(
            "State DB {role} requires a regular file with exactly one hard link: {}",
            path.display(),
        ));
    }
    state_file_identity(&metadata).map(Some).ok_or_else(|| {
        format!(
            "State DB {role} file identity is unavailable: {}",
            path.display()
        )
    })
}

#[cfg(not(any(unix, windows)))]
fn inspect_state_storage_file(
    path: &Path,
    role: &str,
) -> Result<Option<StateFileIdentity>, String> {
    Err(format!(
        "State DB {role} file identity is unsupported on this platform: {}",
        path.display()
    ))
}

impl StateDb {
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
        let initial_identity =
            inspect_state_storage_file(&authority_path, "namespace authority fence")?;
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
        let opened_identity = opened_state_storage_file_identity(
            &file,
            &authority_path,
            "namespace authority fence",
        )?;
        if initial_identity.is_some_and(|identity| identity != opened_identity) {
            return Err(format!(
                "State DB namespace authority fence changed during open: {}",
                authority_path.display()
            ));
        }
        let deadline = std::time::Instant::now() + ACQUISITION_TIMEOUT;
        loop {
            let result = if exclusive {
                <std::fs::File as fs4::FileExt>::try_lock(&file)
            } else {
                <std::fs::File as fs4::FileExt>::try_lock_shared(&file)
            };
            match result {
                Ok(()) => {
                    let retained_identity =
                        inspect_state_storage_file(&authority_path, "namespace authority fence")?
                            .ok_or_else(|| {
                            format!(
                                "State DB namespace authority fence is missing after lock: {}",
                                authority_path.display()
                            )
                        })?;
                    if retained_identity != opened_identity {
                        return Err(format!(
                            "State DB namespace authority fence changed during lock acquisition: {}",
                            authority_path.display()
                        ));
                    }
                    return Ok(Self { file });
                }
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

fn opened_state_storage_file_identity(
    file: &std::fs::File,
    path: &Path,
    role: &str,
) -> Result<StateFileIdentity, String> {
    let metadata = file.metadata().map_err(|error| {
        format!(
            "Failed to inspect opened State DB {role} {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() || !state_file_has_one_link(&metadata) {
        return Err(format!(
            "State DB {role} requires a regular file with exactly one hard link: {}",
            path.display()
        ));
    }
    state_file_identity(&metadata).ok_or_else(|| {
        format!(
            "State DB {role} file identity is unavailable: {}",
            path.display()
        )
    })
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

fn state_sqlite_artifact_paths(state_path: &Path) -> [PathBuf; 3] {
    [
        state_path_with_suffix(state_path, "-journal"),
        state_path_with_suffix(state_path, "-wal"),
        state_path_with_suffix(state_path, "-shm"),
    ]
}

fn state_path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn reserved_state_storage_name(file_name: &str) -> bool {
    let file_name = file_name.to_ascii_lowercase();
    file_name == "pid-identity.db"
        || file_name.ends_with(".pid-identity.db")
        || file_name.ends_with(".namespace.lock")
        || file_name.ends_with(".authority.lock")
        || file_name.ends_with("-journal")
        || file_name.ends_with("-wal")
        || file_name.ends_with("-shm")
}

impl StateDbWriterAuthority {
    pub fn path(&self) -> &Path {
        &self.db_path
    }
}

impl StateDbRebuildAuthority {
    pub fn path(&self) -> &Path {
        &self.db_path
    }
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

    #[test]
    fn legacy_writer_authority_participates_in_rebuild_exclusion() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&state_path).unwrap());
        let writer = StateDb::acquire_writer_authority(&state_path).unwrap();

        let error = StateDb::acquire_rebuild_authority(&state_path)
            .err()
            .expect("legacy writer must exclude rebuild");

        assert!(error.contains("Timed out"), "{error}");
        drop(writer);
        drop(StateDb::acquire_rebuild_authority(&state_path).unwrap());
    }

    #[test]
    fn preexisting_hard_links_are_rejected_by_every_state_namespace_authority() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let alias_path = directory.path().join("alternate.db");
        drop(StateDb::open(&state_path).unwrap());
        std::fs::hard_link(&state_path, &alias_path).unwrap();

        for error in [
            StateDb::open(&state_path).err().unwrap(),
            StateDb::open(&alias_path).err().unwrap(),
            StateDb::acquire_writer_authority(&alias_path)
                .err()
                .unwrap(),
            StateDb::acquire_rebuild_authority(&state_path)
                .err()
                .unwrap(),
        ] {
            assert!(error.contains("exactly one hard link"), "{error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn state_writers_reject_aliased_namespace_fences_and_sqlite_artifacts() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join("state.db");
        let other_state_path = directory.path().join("other.db");
        drop(StateDb::open(&state_path).unwrap());
        drop(StateDb::open(&other_state_path).unwrap());
        let state_before = std::fs::read(&state_path).unwrap();
        let other_before = std::fs::read(&other_state_path).unwrap();

        let authority_path = state_namespace_authority_path(&state_path).unwrap();
        std::fs::remove_file(&authority_path).unwrap();
        symlink(&other_state_path, &authority_path).unwrap();
        for error in [
            StateDb::open(&state_path).err().unwrap(),
            StateDb::acquire_writer_authority(&state_path)
                .err()
                .unwrap(),
            StateDb::acquire_rebuild_authority(&state_path)
                .err()
                .unwrap(),
        ] {
            assert!(
                error.contains("namespace authority fence")
                    && error.contains("exactly one hard link"),
                "{error}"
            );
        }
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before);
        assert_eq!(std::fs::read(&other_state_path).unwrap(), other_before);

        std::fs::remove_file(&authority_path).unwrap();
        drop(StateDb::open(&state_path).unwrap());
        let state_before_artifacts = std::fs::read(&state_path).unwrap();
        for suffix in ["-journal", "-wal", "-shm"] {
            let artifact = state_path_with_suffix(&state_path, suffix);
            if artifact.exists() {
                std::fs::remove_file(&artifact).unwrap();
            }
            std::fs::hard_link(&other_state_path, &artifact).unwrap();
            for error in [
                StateDb::open(&state_path).err().unwrap(),
                StateDb::acquire_writer_authority(&state_path)
                    .err()
                    .unwrap(),
                StateDb::acquire_rebuild_authority(&state_path)
                    .err()
                    .unwrap(),
            ] {
                assert!(
                    error.contains("SQLite artifact") && error.contains("exactly one hard link"),
                    "{error}"
                );
            }
            std::fs::remove_file(artifact).unwrap();
        }
        assert_eq!(std::fs::read(&state_path).unwrap(), state_before_artifacts);
        assert_eq!(std::fs::read(&other_state_path).unwrap(), other_before);
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_rejects_a_leaf_symlink_without_mutating_it_or_its_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_path = directory.path().join("target.db");
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&target_path).unwrap());
        symlink(&target_path, &state_path).unwrap();
        drop(StateDb::open(&state_path).unwrap());
        let target_before = std::fs::read(&target_path).unwrap();

        let error = StateDb::acquire_rebuild_authority(&state_path)
            .err()
            .expect("leaf-symlink rebuild authority must fail");

        assert!(error.contains("leaf symlink"), "{error}");
        assert!(
            std::fs::symlink_metadata(&state_path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read(&target_path).unwrap(), target_before);
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_source_rejoin_rejects_a_replaced_leaf() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_path = directory.path().join("target.db");
        let state_path = directory.path().join("state.db");
        drop(StateDb::open(&target_path).unwrap());
        symlink(&target_path, &state_path).unwrap();
        let authority_path = StateDb::normalized_state_open_path(&state_path);
        std::fs::remove_file(&state_path).unwrap();
        drop(StateDb::open(&state_path).unwrap());

        let error = StateDb::validate_rebuild_source(&state_path, &authority_path).unwrap_err();

        assert!(error.contains("source changed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_source_rejoin_rejects_a_retargeted_parent_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let first_parent = directory.path().join("first");
        let second_parent = directory.path().join("second");
        let alias_parent = directory.path().join("current");
        std::fs::create_dir_all(&first_parent).unwrap();
        std::fs::create_dir_all(&second_parent).unwrap();
        drop(StateDb::open(&first_parent.join("state.db")).unwrap());
        drop(StateDb::open(&second_parent.join("state.db")).unwrap());
        symlink(&first_parent, &alias_parent).unwrap();
        let state_path = alias_parent.join("state.db");
        let authority_path = StateDb::normalized_state_open_path(&state_path);
        std::fs::remove_file(&alias_parent).unwrap();
        symlink(&second_parent, &alias_parent).unwrap();

        let error = StateDb::validate_rebuild_source(&state_path, &authority_path).unwrap_err();

        assert!(error.contains("source changed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_path_rejects_a_dangling_ancestor_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let missing_parent = directory.path().join("missing-parent");
        let alias_parent = directory.path().join("data");
        symlink(&missing_parent, &alias_parent).unwrap();
        let state_path = alias_parent.join("state.db");

        let error = StateDb::validate_rebuild_path(&state_path).unwrap_err();

        assert!(error.contains("dangling ancestor symlink"), "{error}");
        assert!(!missing_parent.exists());
        assert!(
            !state_namespace_authority_path(&state_path)
                .unwrap()
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn writable_state_authorities_reject_non_utf8_paths_without_creating_artifacts() {
        use std::os::unix::ffi::OsStringExt;

        let directory = tempfile::tempdir().unwrap();
        let state_path = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"state-\xff.db".to_vec()));

        for error in [
            StateDb::open(&state_path).err().unwrap(),
            StateDb::acquire_writer_authority(&state_path)
                .err()
                .unwrap(),
            StateDb::acquire_rebuild_authority(&state_path)
                .err()
                .unwrap(),
        ] {
            assert!(error.contains("valid UTF-8"), "{error}");
        }
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn writable_state_authorities_reject_utf8_aliases_to_non_utf8_targets() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_parent = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"target-\xff".to_vec()));
        let target_path = target_parent.join("state.db");
        std::fs::create_dir(&target_parent).unwrap();
        std::fs::write(&target_path, b"unchanged").unwrap();
        let parent_alias = directory.path().join("parent-alias");
        symlink(&target_parent, &parent_alias).unwrap();
        let leaf_alias = directory.path().join("leaf-alias.db");
        symlink(&target_path, &leaf_alias).unwrap();

        for state_path in [parent_alias.join("state.db"), leaf_alias] {
            for error in [
                StateDb::open(&state_path).err().unwrap(),
                StateDb::acquire_writer_authority(&state_path)
                    .err()
                    .unwrap(),
                StateDb::acquire_rebuild_authority(&state_path)
                    .err()
                    .unwrap(),
            ] {
                assert!(error.contains("valid UTF-8"), "{error}");
            }
        }
        assert_eq!(std::fs::read(&target_path).unwrap(), b"unchanged");
        assert!(!target_parent.join("state.db.namespace.lock").exists());
    }

    #[test]
    fn writable_state_authorities_reject_reserved_storage_roles() {
        let directory = tempfile::tempdir().unwrap();

        for file_name in [
            "pid-identity.db",
            "alternate.db.pid-identity.db",
            "alternate.db.namespace.lock",
            "alternate.db.pid-identity.db.authority.lock",
            "alternate.db-journal",
            "alternate.db-wal",
            "alternate.db-shm",
            "ALTERNATE.DB.NAMESPACE.LOCK",
        ] {
            let path = directory.path().join(file_name);
            for error in [
                StateDb::open(&path).err().unwrap(),
                StateDb::acquire_writer_authority(&path).err().unwrap(),
                StateDb::acquire_rebuild_authority(&path).err().unwrap(),
            ] {
                assert!(error.contains("reserved storage role"), "{error}");
            }
            assert!(!path.exists());
            assert!(!state_namespace_authority_path(&path).unwrap().exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rebuild_initializer_rejects_a_non_utf8_requested_alias() {
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target_parent = directory.path().join("target");
        std::fs::create_dir(&target_parent).unwrap();
        let target_path = target_parent.join("state.db");
        let authority = StateDb::acquire_rebuild_authority(&target_path).unwrap();
        let alias_parent = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"alias-\xff".to_vec()));
        symlink(&target_parent, &alias_parent).unwrap();

        let error = StateDb::initialize_after_rebuild(&alias_parent.join("state.db"), &authority)
            .unwrap_err();

        assert!(error.contains("valid UTF-8"), "{error}");
        assert!(!target_path.exists());
    }
}
