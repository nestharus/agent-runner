//! Durable broker ingress latch. Closing this latch is only the first step of
//! an installed cutover: it neither joins existing writers nor proves SQLite
//! quiescence, and it never constructs `QuiescedCutoverProof`.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const OPEN: &[u8] = b"oulipoly-entry-gate-v1 open\n";
const CLOSED: &[u8] = b"oulipoly-entry-gate-v1 closed\n";
static PROCESS_INSTANCES: OnceLock<Mutex<std::collections::BTreeSet<(u64, u64)>>> = OnceLock::new();

fn process_instances() -> &'static Mutex<std::collections::BTreeSet<(u64, u64)>> {
    PROCESS_INSTANCES.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}

pub struct EntryGate {
    directory: PathBuf,
    // A second broker must not bind a fresh socket while the first one can
    // still admit requests. The lock is held for this process's lifetime.
    _instance: File,
    directory_identity: (u64, u64),
    closed: bool,
}

impl EntryGate {
    pub fn open(directory: &Path) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("unsafe broker state directory"));
        }
        let identity = (metadata.dev(), metadata.ino());
        let mut instances = process_instances()
            .lock()
            .map_err(|_| io::Error::other("broker gate process registry poisoned"))?;
        if !instances.insert(identity) {
            return Err(io::Error::other(
                "broker entry gate already open in process",
            ));
        }
        drop(instances);
        let result = Self::open_locked(directory, identity);
        if result.is_err() {
            if let Ok(mut instances) = process_instances().lock() {
                instances.remove(&identity);
            }
        }
        result
    }

    fn open_locked(directory: &Path, identity: (u64, u64)) -> io::Result<Self> {
        let instance = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("entry-gate.lock"))?;
        validate_file(&instance, &directory.join("entry-gate.lock"))?;
        // POSIX record locks are held by this broker process and are not
        // inherited by forked Runner/worker children. An OFD/flock lock could
        // strand broker restart until an unrelated descendant exits.
        let lock = libc::flock {
            l_type: libc::F_WRLCK as _,
            l_whence: libc::SEEK_SET as _,
            l_start: 0,
            l_len: 0,
            l_pid: 0,
        };
        if unsafe { libc::fcntl(instance.as_raw_fd(), libc::F_SETLK, &lock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let closed = match fs::symlink_metadata(directory.join("entry-gate.v1")) {
            Ok(_) => {
                let path = directory.join("entry-gate.v1");
                let mut file = OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                    .open(&path)?;
                validate_file(&file, &path)?;
                let mut bytes = Vec::new();
                std::io::Read::by_ref(&mut file)
                    .take((CLOSED.len() + 1) as u64)
                    .read_to_end(&mut bytes)?;
                match bytes.as_slice() {
                    CLOSED => true,
                    OPEN => false,
                    _ => return Err(io::Error::other("invalid durable entry gate")),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        Ok(Self {
            directory: directory.to_owned(),
            _instance: instance,
            directory_identity: identity,
            closed,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Called only by an authenticated host-root administrative operation in
    /// the broker's serial dispatch loop.
    pub fn close(&mut self) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        // Failure after this point remains closed in memory; a partial marker
        // fails broker restart instead of silently reopening admission.
        self.closed = true;
        self.persist(CLOSED)
    }

    /// Failed prerequisites can return to v29 only before the fixed broker
    /// sidecar name is published. A stage directory is inert and may remain.
    pub fn abort_before_publication(&mut self) -> io::Result<()> {
        match fs::symlink_metadata(self.directory.join("sidecar")) {
            Ok(_) => return Err(io::Error::other("published sidecar forbids gate abort")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if !self.closed {
            return Ok(());
        }
        self.persist(OPEN)?;
        self.closed = false;
        Ok(())
    }

    fn persist(&self, state: &[u8]) -> io::Result<()> {
        let path = self.directory.join("entry-gate.v1");
        let temporary = self
            .directory
            .join(format!(".entry-gate-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(state)?;
        file.sync_all()?;
        fs::rename(temporary, path)?;
        File::open(&self.directory)?.sync_all()
    }
}

impl Drop for EntryGate {
    fn drop(&mut self) {
        if let Ok(mut instances) = process_instances().lock() {
            instances.remove(&self.directory_identity);
        }
    }
}

fn validate_file(file: &File, path: &Path) -> io::Result<()> {
    let opened = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if !opened.is_file()
        || named.file_type().is_symlink()
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
        || opened.nlink() != 1
        || opened.uid() != unsafe { libc::geteuid() }
        || opened.mode() & 0o777 != 0o600
    {
        return Err(io::Error::other("unsafe durable entry gate file"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    #[test]
    fn close_is_durable_singleton_and_never_reopens_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut first = EntryGate::open(directory.path()).unwrap();
        assert!(!first.is_closed());
        assert!(EntryGate::open(directory.path()).is_err());
        first.close().unwrap();
        first.close().unwrap();
        assert!(first.is_closed());
        drop(first);
        assert!(EntryGate::open(directory.path()).unwrap().is_closed());
    }

    #[test]
    fn another_process_cannot_serve_until_first_gate_owner_exits() {
        let directory = tempfile::tempdir().unwrap();
        let gate = EntryGate::open(directory.path()).unwrap();
        let probe = |blocked: bool| {
            let status = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "cutover_gate::tests::process_gate_probe"])
                .env("OULIPOLY_GATE_TEST_DIRECTORY", directory.path())
                .env(
                    "OULIPOLY_GATE_TEST_BLOCKED",
                    if blocked { "1" } else { "0" },
                )
                .status()
                .unwrap();
            assert!(status.success());
        };
        probe(true);
        drop(gate);
        probe(false);
    }

    #[test]
    fn process_gate_probe() {
        let Some(directory) = std::env::var_os("OULIPOLY_GATE_TEST_DIRECTORY") else {
            return;
        };
        let blocked = std::env::var_os("OULIPOLY_GATE_TEST_BLOCKED").as_deref()
            == Some(std::ffi::OsStr::new("1"));
        assert_eq!(EntryGate::open(Path::new(&directory)).is_err(), blocked);
    }

    #[test]
    fn failed_prepublication_cutover_can_resume_legacy_but_publication_cannot() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        gate.abort_before_publication().unwrap();
        assert!(!gate.is_closed());
        drop(gate);
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        fs::create_dir(directory.path().join("sidecar")).unwrap();
        assert!(gate.abort_before_publication().is_err());
        drop(gate);
        assert!(EntryGate::open(directory.path()).unwrap().is_closed());
    }

    #[test]
    fn interrupted_close_refuses_restart() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("entry-gate.v1"), b"partial").unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
    }

    #[test]
    fn linked_or_symlinked_marker_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("entry-gate.v1");
        fs::write(&marker, CLOSED).unwrap();
        // The test must create an unsafe marker even under a 0077 umask.
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&marker, directory.path().join("copy")).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink("copy", &marker).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
    }

    #[test]
    fn active_direct_writer_with_wal_and_shm_survives_ingress_closure() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pid-identity.db");
        let connection = rusqlite::Connection::open(&source).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL; CREATE TABLE t(v INTEGER); INSERT INTO t VALUES(1);",
            )
            .unwrap();
        let _main = File::open(&source).unwrap();
        let _wal = File::open(directory.path().join("pid-identity.db-wal")).unwrap();
        let _shm = File::open(directory.path().join("pid-identity.db-shm")).unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        connection.execute("INSERT INTO t VALUES(2)", []).unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2, "the ingress latch cannot prove writer quiescence");
    }
}
