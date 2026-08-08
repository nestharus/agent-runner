//! Physical snapshot support for SQLite readers that must not mutate source storage.

use std::io;
use std::path::{Path, PathBuf};

const SNAPSHOT_ATTEMPTS: usize = 4;
const SQLITE_ARTIFACT_SUFFIXES: [&str; 3] = ["", "-wal", "-journal"];

pub(crate) struct ReadOnlySnapshot {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl ReadOnlySnapshot {
    pub(crate) fn create(source: &Path) -> io::Result<Self> {
        let dir = tempfile::tempdir()?;
        let file_name = source.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "SQLite path has no file name")
        })?;
        let path = dir.path().join(file_name);
        for _ in 0..SNAPSHOT_ATTEMPTS {
            if !copy_artifact_set(source, &path)? {
                continue;
            }
            if artifact_sets_match(source, &path)? {
                return Ok(Self { _dir: dir, path });
            }
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
