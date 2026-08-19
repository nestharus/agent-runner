//! Physical snapshot support for SQLite readers that must not mutate source storage.

#[cfg(unix)]
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SNAPSHOT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
// One observability scan opens up to three stores serially. Keep transient
// source churn from turning best-effort observation into multi-second delay.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_millis(250);
const SQLITE_ARTIFACT_SUFFIXES: [&str; 3] = ["", "-wal", "-journal"];
const COPY_BUFFER_BYTES: usize = 64 * 1024;

pub(crate) struct ReadOnlySnapshot {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SqliteArtifactIdentity {
    storage: u64,
    file: u64,
}

#[derive(Debug)]
struct OpenedSqliteArtifact {
    file: ManagedSourceFile,
    identity: SqliteArtifactIdentity,
}

#[derive(Debug)]
struct OpenedSqliteArtifactSet {
    artifacts: [Option<OpenedSqliteArtifact>; SQLITE_ARTIFACT_SUFFIXES.len()],
}

enum ValidatedSourceArtifact {
    Absent,
    Changed,
    Present {
        file: ManagedSourceFile,
        identity: SqliteArtifactIdentity,
    },
}

#[derive(Debug)]
struct ManagedSourceFile {
    file: Option<File>,
    source: PathBuf,
}

pub(crate) struct SourceConnectionGuard {
    source: PathBuf,
}

#[cfg(unix)]
#[derive(Default)]
struct SourceDescriptorState {
    connection_count: usize,
    deferred_files: Vec<File>,
}

#[cfg(unix)]
fn source_descriptor_registry() -> &'static Mutex<HashMap<PathBuf, SourceDescriptorState>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, SourceDescriptorState>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

impl ManagedSourceFile {
    fn new(source: &Path, file: File) -> Self {
        Self {
            file: Some(file),
            source: source.to_path_buf(),
        }
    }

    fn file(&self) -> &File {
        self.file.as_ref().expect("managed source file is present")
    }

    fn file_mut(&mut self) -> &mut File {
        self.file.as_mut().expect("managed source file is present")
    }
}

impl Drop for ManagedSourceFile {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            retire_source_file(&self.source, file);
        }
    }
}

impl SourceConnectionGuard {
    pub(crate) fn acquire(source: &Path) -> io::Result<Self> {
        let source = source_registry_key(source)?;
        register_source_connection(&source);
        Ok(Self { source })
    }
}

impl Drop for SourceConnectionGuard {
    fn drop(&mut self) {
        unregister_source_connection(&self.source);
    }
}

impl ReadOnlySnapshot {
    pub(crate) fn create_with_cancel(
        source: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        Self::create_with_retry_timeout(source, SNAPSHOT_TIMEOUT, is_cancelled)
    }

    pub(crate) fn create_with_retry_timeout(
        source: &Path,
        timeout: Duration,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        Self::create_with_retry_policy(
            source,
            timeout,
            SNAPSHOT_RETRY_INTERVAL,
            is_cancelled,
            || Ok(()),
        )
    }

    #[cfg(test)]
    fn create_with_copy_hook(
        source: &Path,
        after_copy: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::create_with_retry_policy(
            source,
            SNAPSHOT_TIMEOUT,
            SNAPSHOT_RETRY_INTERVAL,
            &|| false,
            after_copy,
        )
    }

    fn create_with_retry_policy(
        source: &Path,
        timeout: Duration,
        retry_interval: Duration,
        is_cancelled: &dyn Fn() -> bool,
        mut after_copy: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        let source = canonical_sqlite_source(source)?;
        let dir = tempfile::tempdir()?;
        let file_name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no file name")
        })?;
        let path = dir.path().join(file_name);
        let mut retry_deadline = None;
        loop {
            ensure_snapshot_not_cancelled(is_cancelled)?;
            if let Some(deadline) = retry_deadline {
                ensure_retry_active(deadline, is_cancelled)?;
            }
            if let Some(mut copied) = copy_artifact_set(&source, &path, is_cancelled)? {
                after_copy()?;
                if artifact_sets_match(&source, &path, &mut copied, is_cancelled)? {
                    return Ok(Self { _dir: dir, path });
                }
            }
            let deadline = *retry_deadline.get_or_insert_with(|| Instant::now() + timeout);
            ensure_retry_active(deadline, is_cancelled)?;
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(snapshot_changed_error());
            };
            if remaining.is_zero() {
                return Err(snapshot_changed_error());
            }
            std::thread::sleep(retry_interval.min(remaining));
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn source_registry_key(source: &Path) -> io::Result<PathBuf> {
    match std::fs::canonicalize(source) {
        Ok(source) => Ok(source),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = source.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "SQLite source has no parent")
            })?;
            let file_name = source.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "SQLite source has no file name",
                )
            })?;
            Ok(std::fs::canonicalize(parent)?.join(file_name))
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn register_source_connection(source: &Path) {
    let mut registry = source_descriptor_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    registry
        .entry(source.to_path_buf())
        .or_default()
        .connection_count += 1;
}

#[cfg(not(unix))]
fn register_source_connection(_source: &Path) {}

#[cfg(unix)]
fn unregister_source_connection(source: &Path) {
    let mut registry = source_descriptor_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let should_remove = if let Some(state) = registry.get_mut(source) {
        state.connection_count = state.connection_count.saturating_sub(1);
        state.connection_count == 0
    } else {
        false
    };
    if should_remove {
        // Close deferred descriptors while registration is excluded. A new
        // managed SQLite connection cannot appear between this close and its
        // own registry admission.
        drop(registry.remove(source));
    }
}

#[cfg(not(unix))]
fn unregister_source_connection(_source: &Path) {}

#[cfg(unix)]
fn retire_source_file(source: &Path, file: File) {
    let mut registry = source_descriptor_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(state) = registry
        .get_mut(source)
        .filter(|state| state.connection_count > 0)
    {
        state.deferred_files.push(file);
    } else {
        // Closing under the registry lock prevents a managed SQLite open from
        // racing the process-scoped fcntl lock side effect of close(2).
        drop(file);
    }
}

#[cfg(not(unix))]
fn retire_source_file(_source: &Path, file: File) {
    drop(file);
}

#[cfg(unix)]
fn take_deferred_source_file(source: &Path, expected: SqliteArtifactIdentity) -> Option<File> {
    let mut registry = source_descriptor_registry()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let state = registry.get_mut(source)?;
    let index = state.deferred_files.iter().position(|file| {
        crate::filesystem_identity::open_file_identity(file)
            .map(|identity| {
                identity.storage == expected.storage
                    && identity.file == expected.file
                    && identity.links > 0
            })
            .unwrap_or(false)
    })?;
    Some(state.deferred_files.swap_remove(index))
}

#[cfg(not(unix))]
fn take_deferred_source_file(_source: &Path, _expected: SqliteArtifactIdentity) -> Option<File> {
    None
}

fn canonical_sqlite_source(source: &Path) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(source)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite source is not a regular file",
        ));
    }
    if crate::filesystem_identity::path_file_identity(&canonical, &metadata)?.links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite source has multiple filesystem links",
        ));
    }
    Ok(canonical)
}

fn copy_artifact_set(
    source: &Path,
    destination: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<Option<OpenedSqliteArtifactSet>> {
    let mut artifacts = std::array::from_fn(|_| None);
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        match open_validated_source_artifact(source, &source_artifact)? {
            ValidatedSourceArtifact::Absent if suffix.is_empty() => return Ok(None),
            ValidatedSourceArtifact::Absent => remove_if_present(&destination_artifact)?,
            ValidatedSourceArtifact::Changed => return Ok(None),
            ValidatedSourceArtifact::Present { mut file, identity } => {
                copy_artifact(file.file_mut(), &destination_artifact, is_cancelled)?;
                if validated_artifact_identity(&source_artifact)? != Some(identity) {
                    return Ok(None);
                }
                artifacts[index] = Some(OpenedSqliteArtifact { file, identity });
            }
        }
    }
    Ok(Some(OpenedSqliteArtifactSet { artifacts }))
}

fn artifact_sets_match(
    source: &Path,
    destination: &Path,
    copied: &mut OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    artifact_sets_match_with_hook(source, destination, copied, is_cancelled, |_| Ok(()))
}

fn artifact_sets_match_with_hook(
    source: &Path,
    destination: &Path,
    copied: &mut OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
    mut after_artifact: impl FnMut(usize) -> io::Result<()>,
) -> io::Result<bool> {
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        match copied.artifacts[index].as_mut() {
            None if validated_artifact_identity(&source_artifact)?.is_none() => {}
            Some(opened)
                if validated_artifact_identity(&source_artifact)? == Some(opened.identity) =>
            {
                if !files_match(opened.file.file_mut(), &destination_artifact, is_cancelled)?
                    || validated_artifact_identity(&source_artifact)? != Some(opened.identity)
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        after_artifact(index)?;
    }
    artifact_identity_set_matches(source, copied, is_cancelled)
}

fn artifact_identity_set_matches(
    source: &Path,
    copied: &OpenedSqliteArtifactSet,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    for (index, suffix) in SQLITE_ARTIFACT_SUFFIXES.into_iter().enumerate() {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let expected = copied.artifacts[index]
            .as_ref()
            .map(|artifact| artifact.identity);
        if validated_artifact_identity(&source_artifact)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_artifact(
    reader: &mut File,
    destination: &Path,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    reader.seek(SeekFrom::Start(0))?;
    let mut writer = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read])?;
    }
}

fn files_match(left: &mut File, right: &Path, is_cancelled: &dyn Fn() -> bool) -> io::Result<bool> {
    left.seek(SeekFrom::Start(0))?;
    let mut right = match File::open(right) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let mut left_buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut right_buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_not_cancelled(is_cancelled)?;
        let left_read = left.read(&mut left_buffer)?;
        let right_read = right.read(&mut right_buffer)?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn open_validated_source_artifact(
    source: &Path,
    path: &Path,
) -> io::Result<ValidatedSourceArtifact> {
    open_validated_source_artifact_with_hook(source, path, || Ok(()))
}

fn open_validated_source_artifact_with_hook(
    source: &Path,
    path: &Path,
    after_open: impl FnOnce() -> io::Result<()>,
) -> io::Result<ValidatedSourceArtifact> {
    let Some(identity) = validated_artifact_identity(path)? else {
        return Ok(ValidatedSourceArtifact::Absent);
    };
    let file = match take_deferred_source_file(source, identity) {
        Some(file) => file,
        None => match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(ValidatedSourceArtifact::Changed);
            }
            Err(err) => return Err(err),
        },
    };
    let file = ManagedSourceFile::new(source, file);
    after_open()?;
    let Some(opened_identity) =
        validated_artifact_metadata(path, file.file(), &file.file().metadata()?)?
    else {
        return Ok(ValidatedSourceArtifact::Changed);
    };
    if opened_identity != identity || validated_artifact_identity(path)? != Some(identity) {
        return Ok(ValidatedSourceArtifact::Changed);
    }
    Ok(ValidatedSourceArtifact::Present { file, identity })
}

fn validated_artifact_identity(path: &Path) -> io::Result<Option<SqliteArtifactIdentity>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let identity = match crate::filesystem_identity::path_file_identity(path, &metadata) {
        Ok(identity) => identity,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    if identity.links == 0 {
        return Ok(None);
    }
    validated_artifact_identity_parts(path, &metadata, identity).map(Some)
}

fn validated_artifact_metadata(
    path: &Path,
    file: &File,
    metadata: &std::fs::Metadata,
) -> io::Result<Option<SqliteArtifactIdentity>> {
    let identity = crate::filesystem_identity::open_file_identity(file)?;
    if identity.links == 0 {
        return Ok(None);
    }
    validated_artifact_identity_parts(path, metadata, identity).map(Some)
}

fn validated_artifact_identity_parts(
    path: &Path,
    metadata: &std::fs::Metadata,
    identity: crate::filesystem_identity::OpenFileIdentity,
) -> io::Result<SqliteArtifactIdentity> {
    if !metadata.file_type().is_file() {
        return Err(unsupported_artifact_error(path, "is not a regular file"));
    }
    if identity.links != 1 {
        return Err(unsupported_artifact_error(
            path,
            "has multiple filesystem links",
        ));
    }
    Ok(SqliteArtifactIdentity {
        storage: identity.storage,
        file: identity.file,
    })
}

fn unsupported_artifact_error(path: &Path, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("SQLite source artifact {} {reason}", path.display()),
    )
}

fn ensure_snapshot_not_cancelled(is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Read-only SQLite snapshot cancelled",
        ));
    }
    Ok(())
}

fn ensure_retry_active(deadline: Instant, is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    ensure_snapshot_not_cancelled(is_cancelled)?;
    (Instant::now() < deadline)
        .then_some(())
        .ok_or_else(snapshot_changed_error)
}

fn snapshot_changed_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "SQLite source continued changing while retrying a read-only snapshot",
    )
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

fn path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut path = path.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_retries_after_four_consecutive_source_changes() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let changes = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let change = changes.get();
            if change < 4 {
                std::fs::write(&source, format!("change-{change}"))?;
                changes.set(change + 1);
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(changes.get(), 4);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"change-3");
    }

    #[test]
    fn snapshot_wal_churn_exhaustion_is_bounded_and_preserves_source_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        std::fs::write(&source, "initial").unwrap();
        std::fs::write(&wal, "initial-wal").unwrap();
        let changes = std::cell::Cell::new(0);
        let expected_wal = std::cell::RefCell::new(b"initial-wal".to_vec());
        let started = Instant::now();

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(35),
            Duration::from_millis(1),
            &|| false,
            || {
                let bytes = format!("change-{}", changes.get()).into_bytes();
                std::fs::write(&wal, &bytes)?;
                *expected_wal.borrow_mut() = bytes;
                changes.set(changes.get() + 1);
                Ok(())
            },
        );

        let error = match result {
            Ok(_) => panic!("continuous source changes must exhaust the retry budget"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(changes.get() > 1);
        assert_eq!(std::fs::read(&source).unwrap(), b"initial");
        assert_eq!(std::fs::read(&wal).unwrap(), *expected_wal.borrow());
        assert!(!path_with_suffix(&source, "-journal").exists());
    }

    #[test]
    fn snapshot_cancellation_interrupts_retry_without_mutating_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let cancelled = std::cell::Cell::new(false);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_secs(1),
            Duration::from_millis(10),
            &|| cancelled.get(),
            || {
                cancelled.set(true);
                Ok(())
            },
        );

        let error = match result {
            Ok(_) => panic!("cancelled snapshot must not be published"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read(&source).unwrap(), b"initial");
    }

    #[test]
    fn cancellation_during_multi_chunk_copy_stops_before_the_source_is_consumed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let destination = directory.path().join("snapshot.db");
        let source_bytes = vec![b'x'; COPY_BUFFER_BYTES * 4];
        std::fs::write(&source, &source_bytes).unwrap();
        let mut reader = File::open(&source).unwrap();
        let checks = std::cell::Cell::new(0);

        let result = copy_artifact(&mut reader, &destination, &|| {
            let check = checks.get();
            checks.set(check + 1);
            check >= 2
        });

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        let copied_bytes = std::fs::metadata(&destination).unwrap().len();
        assert_eq!(copied_bytes, (COPY_BUFFER_BYTES * 2) as u64);
        assert!(copied_bytes < source_bytes.len() as u64);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
    }

    #[test]
    fn stable_first_snapshot_attempt_completes_without_spending_retry_budget() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, vec![b'x'; COPY_BUFFER_BYTES * 2]).unwrap();

        let snapshot = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::ZERO,
            Duration::ZERO,
            &|| false,
            || Ok(()),
        )
        .expect("a stable first attempt must not be rejected by the retry deadline");

        assert_eq!(
            std::fs::read(snapshot.path()).unwrap(),
            std::fs::read(source).unwrap()
        );
    }

    #[test]
    fn retry_does_not_start_after_the_deadline_expires() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(30),
            Duration::from_secs(1),
            &|| false,
            || {
                attempts.set(attempts.get() + 1);
                std::fs::write(&source, format!("change-{}", attempts.get()))
            },
        );

        let error = match result {
            Ok(_) => panic!("a retry must not start after deadline exhaustion"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn stable_retry_that_started_before_the_deadline_may_finish_after_it() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(100),
            Duration::from_millis(1),
            &|| false,
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt == 0 {
                    std::fs::write(&source, "stable retry")?;
                } else {
                    std::thread::sleep(Duration::from_millis(150));
                }
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable retry");
    }

    #[test]
    fn changed_retry_that_finishes_after_the_deadline_does_not_start_a_third_pass() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        std::fs::write(&source, "initial").unwrap();
        let attempts = std::cell::Cell::new(0);

        let result = ReadOnlySnapshot::create_with_retry_policy(
            &source,
            Duration::from_millis(100),
            Duration::from_millis(1),
            &|| false,
            || {
                let attempt = attempts.get();
                attempts.set(attempt + 1);
                if attempt > 0 {
                    std::thread::sleep(Duration::from_millis(150));
                }
                std::fs::write(&source, format!("change-{}", attempt + 1))
            },
        );

        let error = match result {
            Ok(_) => panic!("a changed late retry must not publish or start another pass"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn same_bytes_replacement_after_copy_forces_a_fresh_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let replaced = directory.path().join("state-replaced.db");
        std::fs::write(&source, "stable bytes").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                std::fs::rename(&source, &replaced)?;
                std::fs::write(&source, "stable bytes")?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable bytes");
    }

    #[test]
    fn copied_artifact_set_retains_the_open_source_after_its_path_is_removed() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let destination = directory.path().join("snapshot.db");
        std::fs::write(&source, "retained source").unwrap();
        let mut copied = copy_artifact_set(&source, &destination, &|| false)
            .unwrap()
            .unwrap();

        std::fs::remove_file(&source).unwrap();
        let retained = copied.artifacts[0].as_mut().unwrap();
        retained.file.file_mut().seek(SeekFrom::Start(0)).unwrap();
        let mut retained_bytes = Vec::new();
        retained
            .file
            .file_mut()
            .read_to_end(&mut retained_bytes)
            .unwrap();

        assert_eq!(retained_bytes, b"retained source");
        assert!(!artifact_sets_match(&source, &destination, &mut copied, &|| false).unwrap());
    }

    #[test]
    fn same_bytes_wal_replacement_after_copy_forces_a_fresh_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        let replaced = directory.path().join("state-replaced.db-wal");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "stable wal").unwrap();
        let attempts = std::cell::Cell::new(0);

        let snapshot = ReadOnlySnapshot::create_with_copy_hook(&source, || {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            if attempt == 0 {
                std::fs::rename(&wal, &replaced)?;
                std::fs::write(&wal, "stable wal")?;
            }
            Ok(())
        })
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert_eq!(
            std::fs::read(path_with_suffix(snapshot.path(), "-wal")).unwrap(),
            b"stable wal"
        );
    }

    #[test]
    fn unlinked_wal_after_open_is_a_retryable_source_change() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "transient wal").unwrap();

        let opened =
            open_validated_source_artifact_with_hook(&source, &wal, || std::fs::remove_file(&wal))
                .unwrap();

        assert!(matches!(opened, ValidatedSourceArtifact::Changed));
        let snapshot = ReadOnlySnapshot::create_with_cancel(&source, &|| false).unwrap();
        assert_eq!(std::fs::read(snapshot.path()).unwrap(), b"stable main");
        assert!(!path_with_suffix(snapshot.path(), "-wal").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn snapshot_descriptor_retirement_preserves_same_process_record_lock() {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::MetadataExt;

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let state = crate::StateDb::open(&source).unwrap();
        let lock_file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source)
            .unwrap();
        let mut lock = libc::flock {
            l_type: libc::F_WRLCK as libc::c_short,
            l_whence: libc::SEEK_SET as libc::c_short,
            l_start: 0,
            l_len: 1,
            l_pid: 0,
        };
        assert_eq!(
            unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_SETLK, &lock) },
            0
        );
        let inode = lock_file.metadata().unwrap().ino();
        assert!(process_holds_record_lock(inode));

        let snapshot = ReadOnlySnapshot::create_with_cancel(&source, &|| false).unwrap();

        assert!(process_holds_record_lock(inode));
        drop(snapshot);
        for _ in 0..8 {
            drop(ReadOnlySnapshot::create_with_cancel(&source, &|| false).unwrap());
        }
        let retained = deferred_source_file_count(&source);
        assert!((1..=SQLITE_ARTIFACT_SUFFIXES.len()).contains(&retained));
        lock.l_type = libc::F_UNLCK as libc::c_short;
        assert_eq!(
            unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_SETLK, &lock) },
            0
        );
        drop(state);
    }

    #[cfg(target_os = "linux")]
    fn process_holds_record_lock(inode: u64) -> bool {
        let pid = std::process::id().to_string();
        let inode_suffix = format!(":{inode}");
        std::fs::read_to_string("/proc/locks")
            .unwrap()
            .lines()
            .any(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                fields.get(4) == Some(&pid.as_str())
                    && fields
                        .get(5)
                        .is_some_and(|identity| identity.ends_with(&inode_suffix))
            })
    }

    #[cfg(target_os = "linux")]
    fn deferred_source_file_count(source: &Path) -> usize {
        let source = source_registry_key(source).unwrap();
        source_descriptor_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&source)
            .map(|state| state.deferred_files.len())
            .unwrap_or(0)
    }

    #[test]
    fn main_replacement_after_its_check_is_rejected_by_final_artifact_set_join() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("state.db");
        let wal = path_with_suffix(&source, "-wal");
        let destination = directory.path().join("snapshot.db");
        let replaced = directory.path().join("state-replaced.db");
        std::fs::write(&source, "stable main").unwrap();
        std::fs::write(&wal, "stable wal").unwrap();
        let mut copied = copy_artifact_set(&source, &destination, &|| false)
            .unwrap()
            .unwrap();

        let matches =
            artifact_sets_match_with_hook(&source, &destination, &mut copied, &|| false, |index| {
                if index == 0 {
                    std::fs::rename(&source, &replaced)?;
                    std::fs::write(&source, "replacement main")?;
                }
                Ok(())
            })
            .unwrap();

        assert!(!matches);
    }

    #[cfg(unix)]
    #[test]
    fn aliased_wal_and_journal_artifacts_fail_closed() {
        use std::os::unix::fs::symlink;

        for suffix in ["-wal", "-journal"] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("state.db");
            let sidecar = path_with_suffix(&source, suffix);
            let target = directory.path().join(format!("aliased{suffix}"));
            std::fs::write(&source, "stable main").unwrap();
            std::fs::write(&target, "sidecar bytes").unwrap();
            symlink(&target, &sidecar).unwrap();

            let error = match ReadOnlySnapshot::create_with_cancel(&source, &|| false) {
                Ok(_) => panic!("an aliased {suffix} artifact must fail closed"),
                Err(error) => error,
            };

            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
    }
}
