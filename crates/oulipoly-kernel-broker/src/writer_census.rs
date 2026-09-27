//! Broker-observed blockers for an offline State cutover. This scan can find
//! open handles, including unlinked WAL/SHM files. An empty result is NOT a
//! quiescence certificate: new processes and new FDs can race this scan until
//! an installed supervisor fences every supported launcher and respawn path.

const PROC_DIRENTS_BUFFER_BYTES: usize = 8192;
const FD_TARGET_BUFFER_BYTES: usize = 4096;

use crate::entry_registry::ProcessStamp;
use crate::identity::{PinnedProcess, has_detached_host_proc, host_proc_file};
use std::ffi::{CString, OsStr};
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Artifact {
    Main,
    Wal,
    Shm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenStateHandle {
    pub process: ProcessStamp,
    pub executable_dev: u64,
    pub executable_ino: u64,
    pub fd: i32,
    pub artifact: Artifact,
    pub deleted: bool,
}

struct SourceFile {
    kind: Artifact,
    path: PathBuf,
    identity: Option<(u64, u64)>,
}

fn file_identity(path: &Path, required: bool) -> io::Result<Option<(u64, u64)>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && meta.nlink() == 1 => Ok(Some((meta.dev(), meta.ino()))),
        Ok(_) => Err(io::Error::other("unsafe State artifact")),
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn source_files(source: &Path) -> io::Result<[SourceFile; 3]> {
    if !source.is_absolute() || source.file_name() != Some(OsStr::new("pid-identity.db")) {
        return Err(io::Error::other(
            "expected absolute State sidecar main path",
        ));
    }
    let parent = fs::canonicalize(
        source
            .parent()
            .ok_or_else(|| io::Error::other("no parent"))?,
    )?;
    let main = parent.join("pid-identity.db");
    let wal = parent.join("pid-identity.db-wal");
    let shm = parent.join("pid-identity.db-shm");
    Ok([
        SourceFile {
            kind: Artifact::Main,
            identity: file_identity(&main, true)?,
            path: main,
        },
        SourceFile {
            kind: Artifact::Wal,
            identity: file_identity(&wal, false)?,
            path: wal,
        },
        SourceFile {
            kind: Artifact::Shm,
            identity: file_identity(&shm, false)?,
            path: shm,
        },
    ])
}

// getdents is relative to the broker's detached, pinned host procfs. A
// process may disappear during enumeration; every surviving hit is separately
// pinned with pidfd, boot ID, starttime and PID namespace.
fn numeric_entries(directory: File) -> io::Result<Vec<i32>> {
    let mut result = Vec::new();
    let mut bytes = [0u8; PROC_DIRENTS_BUFFER_BYTES];
    loop {
        let count = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                directory.as_raw_fd(),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 0 {
            break;
        }
        let mut offset = 0;
        while offset < count as usize {
            if offset + 19 > count as usize {
                return Err(io::Error::other("short proc directory entry"));
            }
            let length = u16::from_ne_bytes([bytes[offset + 16], bytes[offset + 17]]) as usize;
            if length < 20 || offset + length > count as usize {
                return Err(io::Error::other("invalid proc directory entry"));
            }
            let name = &bytes[offset + 19..offset + length];
            let end = name
                .iter()
                .position(|byte| *byte == 0)
                .ok_or_else(|| io::Error::other("unterminated proc directory entry"))?;
            if let Ok(text) = std::str::from_utf8(&name[..end])
                && !text.is_empty()
                && text.bytes().all(|byte| byte.is_ascii_digit())
            {
                let number = text
                    .parse::<i32>()
                    .map_err(|_| io::Error::other("invalid numeric proc entry"))?;
                result.push(number);
            }
            offset += length;
        }
    }
    result.sort_unstable();
    Ok(result)
}

fn fd_target(directory: &File, fd: i32) -> io::Result<(Vec<u8>, (u64, u64))> {
    let name = CString::new(fd.to_string()).unwrap();
    let mut link = [0u8; FD_TARGET_BUFFER_BYTES];
    let length = unsafe {
        libc::readlinkat(
            directory.as_raw_fd(),
            name.as_ptr(),
            link.as_mut_ptr().cast(),
            link.len(),
        )
    };
    if length < 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize == link.len() {
        return Err(io::Error::other("truncated proc FD link"));
    }
    let target = link[..length as usize].to_vec();
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(directory.as_raw_fd(), name.as_ptr(), &mut stat, 0) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((target, (stat.st_dev, stat.st_ino)))
}

fn match_artifact(
    target: &[u8],
    identity: (u64, u64),
    files: &[SourceFile; 3],
) -> Option<(Artifact, bool)> {
    files.iter().find_map(|file| {
        let name = file.path.as_os_str().as_bytes();
        let deleted = target.strip_prefix(name) == Some(b" (deleted)".as_slice());
        (file.identity == Some(identity) || target == name || deleted)
            .then_some((file.kind, deleted))
    })
}

/// Report every observed main/WAL/SHM handle, regardless of its open mode or
/// owner. No caller may turn an empty vector into `QuiescedCutoverProof`.
pub fn observe_open_state_handles(source: &Path) -> io::Result<Vec<OpenStateHandle>> {
    if unsafe { libc::geteuid() } != 0 || !has_detached_host_proc() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "serving host-root broker procfs required",
        ));
    }
    for namespace in ["pid", "user"] {
        let self_ns = host_proc_file(&format!("self/ns/{namespace}"))?.metadata()?;
        let init_ns = host_proc_file(&format!("1/ns/{namespace}"))?.metadata()?;
        if (self_ns.dev(), self_ns.ino()) != (init_ns.dev(), init_ns.ino()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "broker is not in initial PID and user namespaces",
            ));
        }
    }
    let files = source_files(source)?;
    let mut handles = Vec::new();
    for pid in numeric_entries(host_proc_file(".")?)? {
        let process = match PinnedProcess::open(pid) {
            Ok(process) => process,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        match observe_process_handles(&process, &files, &mut handles) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound && process.exited()? => continue,
            Err(error) => return Err(error),
        }
    }
    for (index, file) in files.iter().enumerate() {
        if file_identity(&file.path, index == 0)? != file.identity {
            return Err(io::Error::other("State artifact changed during census"));
        }
    }
    Ok(handles)
}

fn observe_process_handles(
    process: &PinnedProcess,
    files: &[SourceFile; 3],
    handles: &mut Vec<OpenStateHandle>,
) -> io::Result<()> {
    let pid = process.host_pid;
    let fds = host_proc_file(&format!("{pid}/fd"))?;
    for fd in numeric_entries(fds.try_clone()?)? {
        let (target, identity) = match fd_target(&fds, fd) {
            Ok(value) => value,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        if let Some((artifact, deleted)) = match_artifact(&target, identity, &files) {
            process.verify()?;
            let image = host_proc_file(&format!("{pid}/exe"))?.metadata()?;
            handles.push(OpenStateHandle {
                process: ProcessStamp::from(process),
                executable_dev: image.dev(),
                executable_ino: image.ino(),
                fd,
                artifact,
                deleted,
            });
        }
    }
    process.verify()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn process_holding_main_wal_and_shm_is_observed_then_gone() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pid-identity.db");
        let mut db = rusqlite::Connection::open(&source).unwrap();
        db.pragma_update(None, "journal_mode", "WAL").unwrap();
        db.execute_batch("CREATE TABLE items (n INTEGER); INSERT INTO items VALUES (1);")
            .unwrap();
        let transaction = db.transaction().unwrap();
        transaction
            .execute("INSERT INTO items VALUES (2)", [])
            .unwrap();
        // The child holds the actual SQLite main/WAL/SHM descriptors open.
        let ready = dir.path().join("ready");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "writer_census::tests::sqlite_holder_probe"])
            .env("OULIPOLY_CENSUS_SOURCE", &source)
            .env("OULIPOLY_CENSUS_READY", &ready)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(ready.exists(), "SQLite holder did not start");
        let files = source_files(&source).unwrap();
        let process = PinnedProcess::open(child.id() as i32).unwrap();
        let mut observed = Vec::new();
        observe_process_handles(&process, &files, &mut observed).unwrap();
        let child_handles: Vec<_> = observed
            .iter()
            .filter(|item| item.process.host_pid == child.id() as i32)
            .collect();
        assert!(
            child_handles
                .iter()
                .any(|item| item.artifact == Artifact::Main)
        );
        assert!(
            child_handles
                .iter()
                .any(|item| item.artifact == Artifact::Wal)
        );
        assert!(
            child_handles
                .iter()
                .any(|item| item.artifact == Artifact::Shm)
        );
        let stamp = child_handles[0].process.clone();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(
            crate::identity::observed_incarnation_gone(
                stamp.host_pid,
                &stamp.boot_id,
                stamp.starttime_ticks,
                (stamp.pidns_dev, stamp.pidns_ino)
            )
            .unwrap()
        );
        assert!(process.exited().unwrap());
        drop(transaction);
    }

    #[test]
    fn sqlite_holder_probe() {
        let Some(source) = std::env::var_os("OULIPOLY_CENSUS_SOURCE") else {
            return;
        };
        let db = rusqlite::Connection::open(source).unwrap();
        db.execute_batch("BEGIN; SELECT * FROM items;").unwrap();
        let ready = std::env::var_os("OULIPOLY_CENSUS_READY").unwrap();
        File::create(ready).unwrap().write_all(b"ready").unwrap();
        std::thread::sleep(Duration::from_secs(10));
        drop(db);
    }

    #[test]
    fn wrong_source_name_and_symlink_refuse() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pid-identity.db");
        File::create(&source).unwrap();
        assert!(observe_open_state_handles(&dir.path().join("other.db")).is_err());
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        assert!(file_identity(&alias, true).is_err());
    }

    #[test]
    fn ordinary_library_process_cannot_claim_global_census() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pid-identity.db");
        File::create(&source).unwrap();
        assert!(!has_detached_host_proc());
        assert_eq!(
            observe_open_state_handles(&source).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn deleted_wal_handle_and_stale_pid_stamp_are_blockers() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pid-identity.db");
        File::create(&source).unwrap();
        let wal = dir.path().join("pid-identity.db-wal");
        let held = File::create(&wal).unwrap();
        fs::remove_file(&wal).unwrap();
        let files = source_files(&source).unwrap();
        let process = PinnedProcess::open(std::process::id() as i32).unwrap();
        let mut handles = Vec::new();
        observe_process_handles(&process, &files, &mut handles).unwrap();
        assert!(
            handles
                .iter()
                .any(|item| item.artifact == Artifact::Wal && item.deleted)
        );
        let stamp = ProcessStamp::from(&process);
        assert!(
            crate::identity::observed_incarnation_gone(
                stamp.host_pid,
                &stamp.boot_id,
                stamp.starttime_ticks + 1,
                (stamp.pidns_dev, stamp.pidns_ino),
            )
            .unwrap()
        );
        drop(held);
    }
}
