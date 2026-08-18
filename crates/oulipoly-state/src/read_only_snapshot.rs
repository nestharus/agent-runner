//! Physical snapshot support for SQLite readers that must not mutate source storage.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SNAPSHOT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(5);
const SQLITE_ARTIFACT_SUFFIXES: [&str; 3] = ["", "-wal", "-journal"];

pub(crate) struct ReadOnlySnapshot {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl ReadOnlySnapshot {
    pub(crate) fn create(source: &Path) -> io::Result<Self> {
        Self::create_with_copy_hook(source, || Ok(()))
    }

    fn create_with_copy_hook(
        source: &Path,
        mut after_copy: impl FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let file_name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no file name")
        })?;
        let path = dir.path().join(file_name);
        let started = Instant::now();
        loop {
            if copy_artifact_set(source, &path)? {
                after_copy()?;
                if artifact_sets_match(source, &path)? {
                    return Ok(Self { _dir: dir, path });
                }
            }
            if started.elapsed() >= SNAPSHOT_TIMEOUT {
                break;
            }
            std::thread::sleep(SNAPSHOT_RETRY_INTERVAL);
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "SQLite source changed while creating a read-only snapshot",
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

fn copy_artifact_set(source: &Path, destination: &Path) -> io::Result<bool> {
    for suffix in SQLITE_ARTIFACT_SUFFIXES {
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        if source_artifact.exists() {
            match std::fs::copy(source_artifact, &destination_artifact) {
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

fn artifact_sets_match(source: &Path, destination: &Path) -> io::Result<bool> {
    for suffix in SQLITE_ARTIFACT_SUFFIXES {
        let source_artifact = path_with_suffix(source, suffix);
        let destination_artifact = path_with_suffix(destination, suffix);
        if read_if_present(&source_artifact)? != read_if_present(&destination_artifact)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn read_if_present(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
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
}
