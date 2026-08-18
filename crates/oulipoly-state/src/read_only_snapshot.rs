//! Physical snapshot support for SQLite readers that must not mutate source storage.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
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

impl ReadOnlySnapshot {
    pub(crate) fn create_with_cancel(
        source: &Path,
        is_cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        Self::create_with_retry_policy(
            source,
            SNAPSHOT_TIMEOUT,
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
        let deadline = Instant::now() + timeout;
        loop {
            if copy_artifact_set(&source, &path, deadline, is_cancelled)? {
                after_copy()?;
                if artifact_sets_match(&source, &path, deadline, is_cancelled)? {
                    return Ok(Self { _dir: dir, path });
                }
            }
            ensure_snapshot_active(deadline, is_cancelled)?;
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

fn canonical_sqlite_source(source: &Path) -> io::Result<PathBuf> {
    let canonical = std::fs::canonicalize(source)?;
    let metadata = std::fs::metadata(&canonical)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "SQLite source is not a regular file",
        ));
    }
    if !sqlite_source_has_one_link(&metadata) {
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
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    for suffix in SQLITE_ARTIFACT_SUFFIXES {
        ensure_snapshot_active(deadline, is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        if source_artifact.exists() {
            match copy_before_deadline(
                &source_artifact,
                &destination_artifact,
                deadline,
                is_cancelled,
            ) {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound && !suffix.is_empty() => {
                    remove_if_present(&destination_artifact)?;
                    return Ok(false);
                }
                Err(err) => return Err(err),
            }
        } else if destination_artifact.exists() {
            std::fs::remove_file(destination_artifact)?;
        }
    }
    Ok(true)
}

fn artifact_sets_match(
    source: &Path,
    destination: &Path,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    for suffix in SQLITE_ARTIFACT_SUFFIXES {
        ensure_snapshot_active(deadline, is_cancelled)?;
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        if !files_match(
            &source_artifact,
            &destination_artifact,
            deadline,
            is_cancelled,
        )? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn copy_before_deadline(
    source: &Path,
    destination: &Path,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    let mut reader = File::open(source)?;
    let mut writer = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(destination)?;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_active(deadline, is_cancelled)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        writer.write_all(&buffer[..read])?;
    }
}

fn files_match(
    left: &Path,
    right: &Path,
    deadline: Instant,
    is_cancelled: &dyn Fn() -> bool,
) -> io::Result<bool> {
    let left = open_if_present(left)?;
    let right = open_if_present(right)?;
    let (mut left, mut right) = match (left, right) {
        (Some(left), Some(right)) => (left, right),
        (None, None) => return Ok(true),
        _ => return Ok(false),
    };
    let mut left_buffer = [0_u8; COPY_BUFFER_BYTES];
    let mut right_buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        ensure_snapshot_active(deadline, is_cancelled)?;
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

fn open_if_present(path: &Path) -> io::Result<Option<File>> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

fn ensure_snapshot_active(deadline: Instant, is_cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Read-only SQLite snapshot cancelled",
        ));
    }
    (Instant::now() < deadline)
        .then_some(())
        .ok_or_else(snapshot_changed_error)
}

fn snapshot_changed_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "SQLite source changed while creating a read-only snapshot",
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

#[cfg(unix)]
fn sqlite_source_has_one_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink() == 1
}

#[cfg(windows)]
fn sqlite_source_has_one_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.number_of_links() == Some(1)
}

#[cfg(not(any(unix, windows)))]
fn sqlite_source_has_one_link(_metadata: &std::fs::Metadata) -> bool {
    false
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
}
