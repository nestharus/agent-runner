//! Private durable JSON publication and source-aware readback.
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

fn diagnostic_message(
    stage: &str,
    path: &Path,
    bytes: Option<&[u8]>,
    error: &dyn std::fmt::Display,
) -> String {
    let (size, hash) = bytes.map_or((0, "unavailable".to_owned()), |bytes| {
        (bytes.len(), format!("{:x}", Sha256::digest(bytes)))
    });
    format!(
        "oulipoly JSON artifact: stage={stage} path={} bytes={size} sha256={hash} cause={error}",
        path.display()
    )
}

fn diagnostic(stage: &str, path: &Path, bytes: Option<&[u8]>, error: &dyn std::fmt::Display) {
    eprintln!("{}", diagnostic_message(stage, path, bytes, error));
}

pub fn read<T: DeserializeOwned>(path: &Path, stage: &str) -> io::Result<T> {
    let file = File::open(path).map_err(|error| {
        diagnostic(stage, path, None, &error);
        error
    })?;
    read_open(file, path, stage)
}

pub fn read_open<T: DeserializeOwned>(mut file: File, path: &Path, stage: &str) -> io::Result<T> {
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        diagnostic(stage, path, Some(&bytes), &error);
        error
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        diagnostic(stage, path, Some(&bytes), &error);
        io::Error::new(io::ErrorKind::InvalidData, "broker JSON read failed")
    })
}

/// The final name appears only after all bytes and the private inode are synced.
/// A hard link gives create-new collision semantics without replacing a rival.
pub fn create_new<T: Serialize>(directory: &Path, name: &str, value: &T) -> io::Result<()> {
    create_new_with(directory, name, value, || Ok(()), || Ok(()))
}

/// A crash may strand a private temporary inode after admission intent.
/// It is never interpreted as absent authority on a fresh broker start.
pub fn require_no_pending(directory: &Path) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".json-pending-")
        {
            return Err(pending_error(&entry.path(), "broker_startup"));
        }
        if kind.is_dir() {
            require_no_pending(&entry.path())?;
        }
    }
    Ok(())
}

pub fn pending_error(path: &Path, stage: &str) -> io::Error {
    match fs::read(path) {
        Ok(bytes) => diagnostic(stage, path, Some(&bytes), &"unresolved private publication"),
        Err(error) => diagnostic(stage, path, None, &error),
    }
    io::Error::other("unresolved broker JSON publication")
}

fn create_new_with<T: Serialize>(
    directory: &Path,
    name: &str,
    value: &T,
    before_publish: impl FnOnce() -> io::Result<()>,
    after_publish: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let temp = directory.join(format!(".json-pending-{}", uuid::Uuid::new_v4()));
    let target = directory.join(name);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        before_publish()?;
        fs::hard_link(&temp, &target)?;
        File::open(directory)?.sync_all()?;
        after_publish()
    })();
    let _ = fs::remove_file(&temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_private_write_is_invisible_and_collision_preserves_winner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.json");
        assert!(
            create_new_with(
                dir.path(),
                "receipt.json",
                &serde_json::json!({"x": 1}),
                || {
                    assert!(!path.exists());
                    Err(io::Error::other("crash before publication"))
                },
                || Ok(())
            )
            .is_err()
        );
        assert!(!path.exists());
        create_new(dir.path(), "receipt.json", &serde_json::json!({"x": 1})).unwrap();
        assert_eq!(
            create_new(dir.path(), "receipt.json", &serde_json::json!({"x": 2}))
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(read::<serde_json::Value>(&path, "test").unwrap()["x"], 1);
        let after = dir.path().join("after.json");
        assert!(
            create_new_with(
                dir.path(),
                "after.json",
                &serde_json::json!({"x": 3}),
                || Ok(()),
                || Err(io::Error::other("crash after durable publication"))
            )
            .is_err()
        );
        assert_eq!(read::<serde_json::Value>(&after, "test").unwrap()["x"], 3);
        fs::write(dir.path().join(".json-pending-crash"), b"{").unwrap();
        assert!(require_no_pending(dir.path()).is_err());
    }

    #[test]
    fn malformed_readback_names_source_without_exposing_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("child-work-selection.json");
        fs::write(&path, b"\"secret").unwrap();
        let error = read::<serde_json::Value>(&path, "child_selection").unwrap_err();
        assert_eq!(error.to_string(), "broker JSON read failed");
        let line = diagnostic_message(
            "child_selection",
            &path,
            Some(b"\"secret"),
            &"EOF while parsing a string",
        );
        assert!(line.contains("stage=child_selection"));
        assert!(line.contains("child-work-selection.json bytes=7 sha256="));
        assert!(!line.contains("secret"));
    }
}
