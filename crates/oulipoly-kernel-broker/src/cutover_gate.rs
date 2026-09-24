//! Durable broker ingress latch. Closing this latch is only the first step of
//! an installed cutover: it neither joins existing writers nor proves SQLite
//! quiescence, and it never constructs `QuiescedCutoverProof`.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const OPEN: &[u8] = b"oulipoly-entry-gate-v1 open\n";
const CLOSED: &[u8] = b"oulipoly-entry-gate-v1 closed\n";
const FORWARD_ONLY: &[u8] = b"oulipoly-forward-only-publication-v1\n";
pub const FORWARD_ONLY_MARKER: &str = "forward-only-publication.v1";
pub const ADMISSION_LOCK: &str = "entry-admission.lock";
static PROCESS_INSTANCES: OnceLock<Mutex<std::collections::BTreeSet<(u64, u64)>>> = OnceLock::new();

fn process_instances() -> &'static Mutex<std::collections::BTreeSet<(u64, u64)>> {
    PROCESS_INSTANCES.get_or_init(|| Mutex::new(std::collections::BTreeSet::new()))
}

pub struct EntryGate {
    directory: PathBuf,
    // A second broker must not bind a fresh socket while the first one can
    // still admit requests. The lock is held for this process's lifetime.
    _instance: File,
    admission: File,
    directory_identity: (u64, u64),
    closed: bool,
    forward_only: bool,
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
        let gate_marker = read_marker(&directory.join("entry-gate.v1"), CLOSED.len() + 1)?;
        let closed = match gate_marker.as_deref() {
            Some(CLOSED) => true,
            Some(OPEN) | None => false,
            _ => return Err(io::Error::other("invalid durable entry gate")),
        };
        let forward_only =
            match read_marker(&directory.join(FORWARD_ONLY_MARKER), FORWARD_ONLY.len() + 1)?
                .as_deref()
            {
                None => false,
                Some(FORWARD_ONLY) => true,
                _ => return Err(io::Error::other("invalid forward-only publication marker")),
            };
        if forward_only && !closed {
            return Err(io::Error::other(
                "forward-only publication requires closed durable entry gate",
            ));
        }
        // A published sidecar is forward-only. Both markers must survive a
        // restart; a missing intent is an interrupted or uncoordinated install.
        match fs::symlink_metadata(directory.join("sidecar")) {
            Ok(_) if !closed || !forward_only => {
                return Err(io::Error::other(
                    "published sidecar requires closed gate and forward-only intent",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        // The lock inode is durable. Recreating it after an interrupted close
        // could detach leases held by an earlier Runner incarnation.
        let admission_path = directory.join(ADMISSION_LOCK);
        if gate_marker.is_some() && !admission_path.exists() {
            return Err(io::Error::other("durable gate lost admission lock inode"));
        }
        let admission = open_admission_lock(&admission_path, true)?;
        Ok(Self {
            directory: directory.to_owned(),
            _instance: instance,
            admission,
            directory_identity: identity,
            closed,
            forward_only,
        })
    }

    pub fn is_closed(&self) -> bool {
        self.closed
    }

    pub fn is_forward_only(&self) -> bool {
        self.forward_only
    }

    /// A stable negative result means a fixed-image Runner admitted before X
    /// still owns a process-lifetime lease. A positive result covers only
    /// those fixed-image processes; old unfenced binaries remain separate debt.
    pub fn fixed_image_writers_drained(&self) -> io::Result<bool> {
        if !self.closed {
            return Err(io::Error::other("fixed-image drain requires closed gate"));
        }
        let lock = record_lock(libc::F_WRLCK as _);
        if unsafe { libc::fcntl(self.admission.as_raw_fd(), libc::F_SETLK, &lock) } != 0 {
            let error = io::Error::last_os_error();
            if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EAGAIN)) {
                return Ok(false);
            }
            return Err(error);
        }
        let unlock = record_lock(libc::F_UNLCK as _);
        if unsafe { libc::fcntl(self.admission.as_raw_fd(), libc::F_SETLK, &unlock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
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

    /// Persist the irreversible boundary before a proven installer renames a
    /// staged sidecar to its fixed name. This does not prove old-writer
    /// quiescence and is deliberately not wired to a production publisher yet.
    /// A failed write stays closed in this process; an invalid marker refuses
    /// restart. Recovery after a valid marker can only proceed forward.
    pub fn begin_forward_only_publication(&mut self) -> io::Result<()> {
        if !self.closed {
            return Err(io::Error::other("publication requires closed entry gate"));
        }
        if self.forward_only {
            return Ok(());
        }
        match fs::symlink_metadata(self.directory.join("sidecar")) {
            Ok(_) => return Err(io::Error::other("sidecar already published without intent")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.forward_only = true;
        let path = self.directory.join(FORWARD_ONLY_MARKER);
        let temporary = self
            .directory
            .join(format!(".forward-only-{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(FORWARD_ONLY)?;
        file.sync_all()?;
        let source = std::ffi::CString::new(temporary.as_os_str().as_encoded_bytes())
            .map_err(io::Error::other)?;
        let target = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(io::Error::other)?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                target.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        File::open(&self.directory)?.sync_all()
    }

    /// Failed prerequisites can return to v29 only before the fixed broker
    /// sidecar name is published. A stage directory is inert and may remain.
    pub fn abort_before_publication(&mut self) -> io::Result<()> {
        if self.forward_only {
            return Err(io::Error::other(
                "forward-only publication forbids gate abort",
            ));
        }
        match fs::symlink_metadata(self.directory.join("sidecar")) {
            Ok(_) => return Err(io::Error::other("published sidecar forbids gate abort")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        if !self.closed {
            return Ok(());
        }
        if !self.fixed_image_writers_drained()? {
            return Err(io::Error::other(
                "fixed-image writers remain before gate abort",
            ));
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

/// Hold this guard for the entire Runner invocation. A caller must obtain it
/// before asking the broker for its route. X may close admission meanwhile,
/// but the broker cannot report a drained fixed-image generation until this
/// process exits. The guard never grants v30 State authority.
pub struct FixedImageAdmission {
    _file: File,
}

impl FixedImageAdmission {
    pub fn acquire(directory: &Path) -> io::Result<Self> {
        let file = open_admission_lock(&directory.join(ADMISSION_LOCK), false)?;
        let lock = record_lock(libc::F_RDLCK as _);
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLKW, &lock) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { _file: file })
    }
}

fn record_lock(kind: libc::c_short) -> libc::flock {
    libc::flock {
        l_type: kind,
        l_whence: libc::SEEK_SET as _,
        l_start: 1,
        l_len: 1,
        l_pid: 0,
    }
}

fn open_admission_lock(path: &Path, create: bool) -> io::Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing admission parent"))?;
    let directory = fs::symlink_metadata(parent)?;
    if !directory.is_dir() || directory.file_type().is_symlink() || directory.mode() & 0o022 != 0 {
        return Err(io::Error::other("unsafe admission directory"));
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    if create {
        options.write(true);
    }
    let file = if create && !path.exists() {
        let file = options.create_new(true).mode(0o640).open(path)?;
        // Broker startup precedes socket binding. Set exact read-only group
        // access even when the process umask is stricter than the unit mode.
        file.set_permissions(fs::Permissions::from_mode(0o640))?;
        file
    } else {
        options.open(path)?
    };
    let opened = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if !opened.is_file()
        || named.file_type().is_symlink()
        || opened.dev() != named.dev()
        || opened.ino() != named.ino()
        || opened.nlink() != 1
        || opened.uid() != directory.uid()
        || opened.mode() & 0o777 != 0o640
    {
        return Err(io::Error::other("unsafe fixed-image admission lock"));
    }
    Ok(file)
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

fn read_marker(path: &Path, max_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let mut file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            validate_file(&file, path)?;
            let mut bytes = Vec::new();
            std::io::Read::by_ref(&mut file)
                .take(max_bytes as u64)
                .read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{Duration, Instant};

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
        gate.begin_forward_only_publication().unwrap();
        fs::create_dir(directory.path().join("sidecar")).unwrap();
        assert!(gate.abort_before_publication().is_err());
        drop(gate);
        assert!(EntryGate::open(directory.path()).unwrap().is_closed());
    }

    #[test]
    fn published_sidecar_refuses_restart_without_closed_durable_gate() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("sidecar")).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());

        fs::remove_dir(directory.path().join("sidecar")).unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        drop(gate);
        fs::create_dir(directory.path().join("sidecar")).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        fs::remove_dir(directory.path().join("sidecar")).unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.begin_forward_only_publication().unwrap();
        drop(gate);
        fs::create_dir(directory.path().join("sidecar")).unwrap();
        assert!(EntryGate::open(directory.path()).unwrap().is_closed());

        fs::write(directory.path().join("entry-gate.v1"), OPEN).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
    }

    #[test]
    fn interrupted_publication_stays_forward_only_without_fixed_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        assert!(gate.begin_forward_only_publication().is_err());
        gate.close().unwrap();
        gate.begin_forward_only_publication().unwrap();
        gate.begin_forward_only_publication().unwrap();
        assert!(gate.is_forward_only());
        assert!(gate.abort_before_publication().is_err());
        drop(gate);
        let mut restarted = EntryGate::open(directory.path()).unwrap();
        assert!(restarted.is_closed());
        assert!(restarted.is_forward_only());
        assert!(crate::registry::RootRegistry::open(directory.path()).is_ok());
        assert!(restarted.abort_before_publication().is_err());
    }

    #[test]
    fn damaged_or_aliased_publication_intent_refuses_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        drop(gate);
        let marker = directory.path().join(FORWARD_ONLY_MARKER);
        fs::write(&marker, b"partial").unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        fs::write(&marker, FORWARD_ONLY).unwrap();
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600)).unwrap();
        fs::hard_link(&marker, directory.path().join("alias")).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        fs::remove_file(&marker).unwrap();
        std::os::unix::fs::symlink("alias", &marker).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
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
        let retired_state = rusqlite::Connection::open(directory.path().join("state.db")).unwrap();
        retired_state
            .execute_batch(
                "CREATE TABLE old_effects(v INTEGER); INSERT INTO old_effects VALUES(1);",
            )
            .unwrap();
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
        retired_state
            .execute("INSERT INTO old_effects VALUES(2)", [])
            .unwrap();
        assert!(
            gate.fixed_image_writers_drained().unwrap(),
            "an unleased old writer is invisible to the fixed-image drain"
        );
        let count: i64 = connection
            .query_row("SELECT count(*) FROM t", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2, "the ingress latch cannot prove writer quiescence");
        let state_count: i64 = retired_state
            .query_row("SELECT count(*) FROM old_effects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(state_count, 2, "X cannot stop an old direct State writer");
    }

    #[test]
    fn fixed_image_lease_remains_debt_across_close_and_broker_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cutover_gate::tests::fixed_image_lease_child"])
            .env("OULIPOLY_LEASE_TEST_DIRECTORY", directory.path())
            .spawn()
            .unwrap();
        let ready = directory.path().join("lease-ready");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "lease child failed to start");
            std::thread::sleep(Duration::from_millis(10));
        }
        gate.close().unwrap();
        assert!(!gate.fixed_image_writers_drained().unwrap());
        assert!(gate.abort_before_publication().is_err());
        drop(gate);
        let mut restarted = EntryGate::open(directory.path()).unwrap();
        assert!(restarted.is_closed());
        assert!(!restarted.fixed_image_writers_drained().unwrap());
        fs::write(directory.path().join("lease-release"), b"go").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(restarted.fixed_image_writers_drained().unwrap());
        restarted.abort_before_publication().unwrap();
        assert!(!restarted.is_closed());
        let _new_lease = FixedImageAdmission::acquire(directory.path()).unwrap();
    }

    #[test]
    fn fixed_image_lease_child() {
        let Some(directory) = std::env::var_os("OULIPOLY_LEASE_TEST_DIRECTORY") else {
            return;
        };
        let directory = Path::new(&directory);
        let _lease = FixedImageAdmission::acquire(directory).unwrap();
        fs::write(directory.join("lease-ready"), b"ready").unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !directory.join("lease-release").exists() {
            assert!(
                Instant::now() < deadline,
                "test parent did not release lease"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn existing_marker_with_missing_or_replaced_lease_refuses_restart() {
        let directory = tempfile::tempdir().unwrap();
        let mut gate = EntryGate::open(directory.path()).unwrap();
        gate.close().unwrap();
        drop(gate);
        let lock = directory.path().join(ADMISSION_LOCK);
        fs::remove_file(&lock).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
        std::os::unix::fs::symlink("replacement", &lock).unwrap();
        assert!(EntryGate::open(directory.path()).is_err());
    }
}
