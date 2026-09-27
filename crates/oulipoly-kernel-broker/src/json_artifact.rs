//! Private durable JSON publication and source-aware readback.
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(".json-pending-")
        {
            wait_for_live_publication(&entry.path())?;
            continue;
        }
        let kind = entry.file_type()?;
        if kind.is_dir() {
            require_no_pending(&entry.path())?;
        }
    }
    Ok(())
}

/// A surviving physical worker can still be publishing when its broker
/// restarts. Wait on that exact temporary inode; an abandoned inode remains
/// an explicit unknown and still closes the lane.
fn wait_for_live_publication(path: &Path) -> io::Result<()> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(named) => {
            let opened = file.metadata()?;
            if named.dev() != opened.dev() || named.ino() != opened.ino() {
                return Err(io::Error::other("pending JSON inode changed"));
            }
            Err(pending_error(path, "broker_startup"))
        }
        Err(error) => Err(error),
    }
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
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        let error = io::Error::last_os_error();
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    let result = (|| {
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
    fn restart_waits_for_surviving_publisher_but_refuses_abandoned_temp() {
        let dir = tempfile::tempdir().unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let publisher_dir = dir.path().to_path_buf();
        let publisher = std::thread::spawn(move || {
            create_new_with(
                &publisher_dir,
                "receipt.json",
                &serde_json::json!({"x": 1}),
                || {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                },
                || Ok(()),
            )
        });
        entered_rx.recv().unwrap();
        let scan_dir = dir.path().to_path_buf();
        let scanner = std::thread::spawn(move || require_no_pending(&scan_dir));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(!scanner.is_finished());
        release_tx.send(()).unwrap();
        publisher.join().unwrap().unwrap();
        scanner.join().unwrap().unwrap();
        fs::write(dir.path().join(".json-pending-abandoned"), b"{").unwrap();
        assert!(require_no_pending(dir.path()).is_err());
    }

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
