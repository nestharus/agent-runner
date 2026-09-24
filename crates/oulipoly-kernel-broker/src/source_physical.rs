//! Broker-owned post-owner source custody. A consumed source grant may be
//! bound here only while its PID1 and worker are held behind the launch gate.
//! This module does not consume grants or launch recovery: those effects stay
//! closed until the State transition and PID1 producer use this same record.
use crate::entry_registry::{EntryRecord, ProcessStamp};
use crate::identity::{PinnedProcess, observed_incarnation_gone};
use crate::registry::RootRecord;
use oulipoly_state::mailbox::BrokerSourceEffectGrant;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static SOURCE_CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn request_source_cancel(_: libc::c_int) {
    SOURCE_CANCEL_REQUESTED.store(true, Ordering::Relaxed);
}

/// Install before the worker gate opens. PID1 handles cancellation without a
/// product lifetime cap; after cancellation it repeatedly signals its own
/// namespace while reaping adopted descendants.
pub fn install_source_pid1_cancel_handler() -> io::Result<()> {
    if unsafe { libc::getpid() } != 1 {
        return Err(io::Error::other("source reaper is not namespace PID1"));
    }
    SOURCE_CANCEL_REQUESTED.store(false, Ordering::Relaxed);
    if unsafe { libc::signal(libc::SIGUSR1, request_source_cancel as libc::sighandler_t) }
        == libc::SIG_ERR
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn signal_source_members(signal: libc::c_int) -> io::Result<()> {
    if unsafe { libc::kill(-1, signal) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutputStamp {
    pub device: u64,
    pub inode: u64,
}

impl OutputStamp {
    fn of(file: &File) -> io::Result<Self> {
        let meta = file.metadata()?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.uid() != 0 {
            return Err(io::Error::other(
                "source output is not a root-only regular file",
            ));
        }
        Ok(Self {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourcePhysicalRecord {
    pub version: u32,
    pub grant: BrokerSourceEffectGrant,
    pub root_init: ProcessStamp,
    pub guardian: ProcessStamp,
    pub driver: ProcessStamp,
    pub joined_child: ProcessStamp,
    pub pid1: ProcessStamp,
    pub worker: ProcessStamp,
    pub worker_local_pid: i32,
    pub stdout: OutputStamp,
    pub stderr: OutputStamp,
}

/// Written by the source PID1 only after the exact worker wait, ECHILD for
/// every adopted child, and sync of both output files. The later readback
/// requires the recorded PID1 incarnation to have ended as well.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTerminalReceipt {
    pub version: u32,
    pub grant_id: String,
    pub pid1: ProcessStamp,
    pub worker: ProcessStamp,
    pub worker_local_pid: i32,
    pub worker_wait_status: i32,
    pub adopted_tree_drained: bool,
    pub stdout_len: u64,
    pub stdout_sha256: String,
    pub stderr_len: u64,
    pub stderr_sha256: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SourceObservation {
    Live {
        cancel_requested: bool,
    },
    DrainPending {
        cancel_requested: bool,
    },
    Unknown {
        reason: &'static str,
        cancel_requested: bool,
    },
    Drained {
        worker_wait_status: i32,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        cancel_requested: bool,
    },
}

pub struct SourcePhysicalRegistry {
    directory: PathBuf,
    records: Vec<SourcePhysicalRecord>,
    orphaned: Vec<String>,
    poisoned: bool,
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn name(id: &str, suffix: &str) -> io::Result<String> {
    let parsed =
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid source grant ID"))?;
    if parsed.to_string() != id {
        return Err(io::Error::other("noncanonical source grant ID"));
    }
    Ok(format!("{id}.{suffix}"))
}

fn open_exact(directory: &Path, id: &str, suffix: &str) -> io::Result<File> {
    let path = directory.join(name(id, suffix)?);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let named = fs::symlink_metadata(&path)?;
    let opened = file.metadata()?;
    if !named.is_file() || named.dev() != opened.dev() || named.ino() != opened.ino() {
        return Err(io::Error::other("source witness name changed"));
    }
    Ok(file)
}

fn exact_bytes(directory: &Path, id: &str, suffix: &str, limit: u64) -> io::Result<Vec<u8>> {
    let mut file = open_exact(directory, id, suffix)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != 0 || meta.mode() & 0o077 != 0 || meta.len() > limit {
        return Err(io::Error::other("invalid root-only source witness file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let named = fs::symlink_metadata(directory.join(name(id, suffix)?))?;
    if bytes.len() as u64 != meta.len()
        || file.metadata()?.len() != meta.len()
        || named.dev() != meta.dev()
        || named.ino() != meta.ino()
    {
        return Err(io::Error::other("source witness file changed during read"));
    }
    Ok(bytes)
}

fn valid_record(record: &SourcePhysicalRecord) -> bool {
    record.version == 1
        && record.grant.phase == "consumed"
        && record.grant.revision >= 2
        && name(&record.grant.grant_id, "json").is_ok()
        && name(&record.grant.root_id, "json").is_ok()
        && name(&record.grant.source_generation, "json").is_ok()
        && name(&record.grant.owner_generation, "json").is_ok()
        && valid_digest(&record.grant.candidate.registration_digest)
        && record.grant.driver_identity.pid == i64::from(record.driver.host_pid)
        && record.grant.driver_identity.boot_id == record.driver.boot_id
        && u64::try_from(record.grant.driver_identity.starttime_ticks).ok()
            == Some(record.driver.starttime_ticks)
        && record.root_init.boot_id == record.pid1.boot_id
        && record.pid1.boot_id == record.worker.boot_id
        && record.worker_local_pid > 1
        && record.stdout.inode != 0
        && record.stderr.inode != 0
}

fn direct_child_namespace(pid1: &PinnedProcess, root_init: &PinnedProcess) -> io::Result<bool> {
    let parent = unsafe { libc::ioctl(pid1.namespace().as_raw_fd(), libc::NS_GET_PARENT) };
    if parent < 0 {
        return Err(io::Error::last_os_error());
    }
    let parent = unsafe { File::from_raw_fd(parent) };
    let parent = parent.metadata()?;
    Ok((parent.dev(), parent.ino()) == (root_init.pidns_dev, root_init.pidns_ino))
}

impl SourcePhysicalRegistry {
    /// The serving broker supplies its fixed root-only directory. A caller
    /// cannot provide an observation pathname or substitute a State row.
    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let meta = fs::symlink_metadata(&directory)?;
        if !meta.is_dir() || meta.uid() != 0 || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "source witness directory is not root-only",
            ));
        }
        let mut records = Vec::new();
        let mut ids = HashSet::new();
        let mut sources = HashSet::new();
        let mut auxiliary = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let filename = entry.file_name();
            let filename = filename.to_string_lossy();
            if !entry.file_type()?.is_file() {
                return Err(io::Error::other("unrecognized source witness entry"));
            }
            if let Some(id) = [".terminal.json", ".cancel.json", ".stdout", ".stderr"]
                .into_iter()
                .find_map(|suffix| filename.strip_suffix(suffix))
            {
                name(id, "json")?;
                auxiliary.push(id.to_owned());
                continue;
            }
            if !filename.ends_with(".json") {
                return Err(io::Error::other("unrecognized source witness file"));
            }
            let id = filename.trim_end_matches(".json");
            let bytes = exact_bytes(&directory, id, "json", 16 * 1024)?;
            let record: SourcePhysicalRecord = serde_json::from_slice(&bytes)?;
            if !valid_record(&record)
                || record.grant.grant_id != id
                || !ids.insert(record.grant.grant_id.clone())
                || !sources.insert((
                    record.grant.source_generation.clone(),
                    record.grant.candidate.registration_id.clone(),
                ))
            {
                return Err(io::Error::other("conflicting source physical record"));
            }
            records.push(record);
        }
        let mut orphaned: Vec<_> = auxiliary
            .into_iter()
            .filter(|id| !ids.contains(id))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        orphaned.sort();
        Ok(Self {
            directory,
            records,
            orphaned,
            poisoned: false,
        })
    }

    pub fn records(&self) -> &[SourcePhysicalRecord] {
        &self.records
    }

    pub fn orphaned_grants(&self) -> &[String] {
        &self.orphaned
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned || !self.orphaned.is_empty()
    }

    /// Creates the two root-only capture files while the child is still held.
    /// Their inodes are later included in the durable physical record.
    pub fn prepare_outputs(&mut self, grant_id: &str) -> io::Result<(File, File)> {
        if self.has_debt() || self.records.iter().any(|r| r.grant.grant_id == grant_id) {
            return Err(io::Error::other(
                "source grant already has physical custody",
            ));
        }
        let create = |suffix| {
            OpenOptions::new()
                .write(true)
                .read(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(self.directory.join(name(grant_id, suffix)?))
        };
        let result = (|| {
            let stdout = create("stdout")?;
            let stderr = create("stderr")?;
            File::open(&self.directory)?.sync_all()?;
            Ok((stdout, stderr))
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    /// Called only after a consumed grant, exact PID1 and held worker are
    /// available. A failure poisons this registry; the broker must not open
    /// the worker gate or retry with another child.
    #[expect(
        clippy::too_many_arguments,
        reason = "each physical actor is independently pinned"
    )]
    pub fn insert_held(
        &mut self,
        grant: BrokerSourceEffectGrant,
        root: &RootRecord,
        entry: &EntryRecord,
        root_init: &PinnedProcess,
        guardian: &PinnedProcess,
        driver: &PinnedProcess,
        joined_child: &PinnedProcess,
        pid1: &PinnedProcess,
        worker: &PinnedProcess,
        worker_local_pid: i32,
    ) -> io::Result<SourcePhysicalRecord> {
        if self.has_debt()
            || self.records.iter().any(|r| {
                r.grant.grant_id == grant.grant_id
                    || (r.grant.source_generation == grant.source_generation
                        && r.grant.candidate.registration_id == grant.candidate.registration_id)
            })
        {
            return Err(io::Error::other(
                "duplicate or poisoned source physical grant",
            ));
        }
        for process in [root_init, guardian, driver, joined_child, pid1, worker] {
            process.verify()?;
        }
        if root.root_id != grant.root_id
            || root.init_host_pid != root_init.host_pid
            || root.init_starttime_ticks != root_init.starttime_ticks
            || (root.pidns_dev, root.pidns_ino) != (root_init.pidns_dev, root_init.pidns_ino)
            || entry.root_id != grant.root_id
            || entry.guardian.as_ref() != Some(&ProcessStamp::from(guardian))
            || entry.prepared_driver.as_ref() != Some(&ProcessStamp::from(driver))
            || entry.joined_child.as_ref() != Some(&ProcessStamp::from(joined_child))
            || !entry.join_consumed
            || !pid1.is_namespace_init()?
            || !direct_child_namespace(pid1, root_init)?
            || !worker.direct_child_of(pid1)?
            || !worker.in_namespace(pid1.namespace())?
            || worker.namespace_pid()? != worker_local_pid
            || !driver.direct_child_of(guardian)?
            || !joined_child.in_namespace(root_init.namespace())?
        {
            return Err(io::Error::other("source physical actor lineage changed"));
        }
        let stdout_file = open_exact(&self.directory, &grant.grant_id, "stdout")?;
        let stderr_file = open_exact(&self.directory, &grant.grant_id, "stderr")?;
        if stdout_file.metadata()?.len() != 0 || stderr_file.metadata()?.len() != 0 {
            return Err(io::Error::other(
                "source output was written before held grant",
            ));
        }
        let stdout = OutputStamp::of(&stdout_file)?;
        let stderr = OutputStamp::of(&stderr_file)?;
        let record = SourcePhysicalRecord {
            version: 1,
            grant,
            root_init: ProcessStamp::from(root_init),
            guardian: ProcessStamp::from(guardian),
            driver: ProcessStamp::from(driver),
            joined_child: ProcessStamp::from(joined_child),
            pid1: ProcessStamp::from(pid1),
            worker: ProcessStamp::from(worker),
            worker_local_pid,
            stdout,
            stderr,
        };
        if !valid_record(&record) {
            return Err(io::Error::other("invalid consumed source physical binding"));
        }
        let path = self.directory.join(name(&record.grant.grant_id, "json")?);
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.records.push(record.clone());
        Ok(record)
    }

    /// This is the broker's post-owner reconciliation route. Its only
    /// selector is a grant already present in root-only physical custody.
    /// Inconclusive files, lost broker replies and PID reuse remain debt.
    pub fn observe(&self, grant_id: &str) -> io::Result<SourceObservation> {
        let record = self
            .records
            .iter()
            .find(|r| r.grant.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("source physical grant absent"))?;
        let cancel_requested = match exact_bytes(&self.directory, grant_id, "cancel.json", 4096) {
            Ok(bytes) => match serde_json::from_slice::<ProcessStamp>(&bytes) {
                Ok(stamp) if stamp == record.pid1 => true,
                _ => {
                    return Ok(SourceObservation::Unknown {
                        reason: "invalid-cancel-intent",
                        cancel_requested: false,
                    });
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(_) => {
                return Ok(SourceObservation::Unknown {
                    reason: "unreadable-cancel-intent",
                    cancel_requested: false,
                });
            }
        };
        let gone = observed_incarnation_gone(
            record.pid1.host_pid,
            &record.pid1.boot_id,
            record.pid1.starttime_ticks,
            (record.pid1.pidns_dev, record.pid1.pidns_ino),
        )?;
        let bytes = match exact_bytes(&self.directory, grant_id, "terminal.json", 16 * 1024) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(if gone {
                    SourceObservation::Unknown {
                        reason: "missing-terminal-receipt",
                        cancel_requested,
                    }
                } else {
                    SourceObservation::Live { cancel_requested }
                });
            }
            Err(_) => {
                return Ok(SourceObservation::Unknown {
                    reason: "unreadable-terminal-receipt",
                    cancel_requested,
                });
            }
        };
        let Ok(receipt) = serde_json::from_slice::<SourceTerminalReceipt>(&bytes) else {
            return Ok(SourceObservation::Unknown {
                reason: "invalid-terminal-receipt",
                cancel_requested,
            });
        };
        if receipt.version != 1
            || receipt.grant_id != grant_id
            || receipt.pid1 != record.pid1
            || receipt.worker != record.worker
            || receipt.worker_local_pid != record.worker_local_pid
            || !receipt.adopted_tree_drained
            || !valid_digest(&receipt.stdout_sha256)
            || !valid_digest(&receipt.stderr_sha256)
        {
            return Ok(SourceObservation::Unknown {
                reason: "conflicting-terminal-receipt",
                cancel_requested,
            });
        }
        if !gone {
            return Ok(SourceObservation::DrainPending { cancel_requested });
        }
        let output = |suffix, stamp: &OutputStamp, len, hash: &str| -> io::Result<Vec<u8>> {
            let mut file = open_exact(&self.directory, grant_id, suffix)?;
            if OutputStamp::of(&file)? != *stamp {
                return Err(io::Error::other("source output inode changed"));
            }
            if file.metadata()?.len() > 64 * 1024 * 1024 {
                return Err(io::Error::other("source output exceeds completeness limit"));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            let named = fs::symlink_metadata(self.directory.join(name(grant_id, suffix)?))?;
            if OutputStamp::of(&file)? != *stamp
                || named.dev() != stamp.device
                || named.ino() != stamp.inode
                || file.metadata()?.len() != bytes.len() as u64
                || bytes.len() as u64 != len
                || digest(&bytes) != hash
            {
                return Err(io::Error::other("source output completeness mismatch"));
            }
            Ok(bytes)
        };
        let (Ok(stdout), Ok(stderr)) = (
            output(
                "stdout",
                &record.stdout,
                receipt.stdout_len,
                &receipt.stdout_sha256,
            ),
            output(
                "stderr",
                &record.stderr,
                receipt.stderr_len,
                &receipt.stderr_sha256,
            ),
        ) else {
            return Ok(SourceObservation::Unknown {
                reason: "incomplete-or-changed-output",
                cancel_requested,
            });
        };
        Ok(SourceObservation::Drained {
            worker_wait_status: receipt.worker_wait_status,
            stdout,
            stderr,
            cancel_requested,
        })
    }

    /// Durable cancellation intent precedes an exact pidfd signal. The PID1
    /// owns TERM/KILL escalation and writes the same terminal/drain receipt.
    pub fn cancel(&mut self, grant_id: &str) -> io::Result<()> {
        let record = self
            .records
            .iter()
            .find(|r| r.grant.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("source physical grant absent"))?;
        let pid1 = PinnedProcess::open(record.pid1.host_pid)?;
        if ProcessStamp::from(&pid1) != record.pid1 {
            return Err(io::Error::other("source PID1 incarnation changed"));
        }
        let path = self.directory.join(name(grant_id, "cancel.json")?);
        match exact_bytes(&self.directory, grant_id, "cancel.json", 4096) {
            Ok(bytes) => {
                let stamp: ProcessStamp = serde_json::from_slice(&bytes)?;
                if stamp != record.pid1 {
                    return Err(io::Error::other("source cancellation intent changed"));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let result = (|| {
                    let mut file = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                        .open(path)?;
                    serde_json::to_writer(&mut file, &record.pid1)?;
                    file.write_all(b"\n")?;
                    file.sync_all()?;
                    File::open(&self.directory)?.sync_all()
                })();
                if let Err(error) = result {
                    self.poisoned = true;
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        pid1.signal(libc::SIGUSR1)
    }

    /// Reissue a durable cancellation after broker restart or lost signal
    /// reply. A changed PID incarnation is an error, never a numeric-PID kill.
    pub fn reconcile_cancellations(&mut self) -> Vec<(String, io::Result<()>)> {
        let ids: Vec<_> = self
            .records
            .iter()
            .map(|record| record.grant.grant_id.clone())
            .collect();
        ids.into_iter()
            .filter_map(
                |id| match exact_bytes(&self.directory, &id, "cancel.json", 4096) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Ok(_) => Some((id.clone(), self.cancel(&id))),
                    Err(error) => Some((id, Err(error))),
                },
            )
            .collect()
    }
}

/// Source PID1's terminal producer. The exact worker wait is distinct from
/// the ECHILD drain of its adopted descendants. An I/O, wait, or capture
/// failure leaves no positive terminal receipt and remains unknown debt.
pub fn reap_source_pid1_and_write_terminal(
    directory: &Path,
    record: &SourcePhysicalRecord,
) -> io::Result<()> {
    if unsafe { libc::getpid() } != 1 || !valid_record(record) {
        return Err(io::Error::other("invalid source PID1 terminal producer"));
    }
    let durable = SourcePhysicalRegistry::open(directory)?;
    if durable
        .records()
        .iter()
        .find(|r| r.grant.grant_id == record.grant.grant_id)
        != Some(record)
    {
        return Err(io::Error::other(
            "source PID1 record changed before terminal",
        ));
    }
    let mut worker_wait = None;
    let mut cancellation_started: Option<Instant> = None;
    loop {
        if SOURCE_CANCEL_REQUESTED.load(Ordering::Relaxed) {
            let started = *cancellation_started.get_or_insert_with(Instant::now);
            signal_source_members(if started.elapsed() >= Duration::from_secs(2) {
                libc::SIGKILL
            } else {
                libc::SIGTERM
            })?;
        }
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if reaped == record.worker_local_pid {
            worker_wait = Some(status);
        }
        if reaped > 0 {
            continue;
        }
        if reaped == 0 {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if error.raw_os_error() != Some(libc::ECHILD) {
            return Err(error);
        }
        break;
    }
    let worker_wait_status =
        worker_wait.ok_or_else(|| io::Error::other("exact source worker wait absent"))?;
    let capture = |suffix: &str, stamp: &OutputStamp| -> io::Result<(u64, String)> {
        let mut file = open_exact(directory, &record.grant.grant_id, suffix)?;
        if OutputStamp::of(&file)? != *stamp || file.metadata()?.len() > 64 * 1024 * 1024 {
            return Err(io::Error::other(
                "source output stamp or completeness limit changed",
            ));
        }
        file.sync_all()?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if file.metadata()?.len() != bytes.len() as u64 {
            return Err(io::Error::other(
                "source output changed after physical drain",
            ));
        }
        Ok((bytes.len() as u64, digest(&bytes)))
    };
    let (stdout_len, stdout_sha256) = capture("stdout", &record.stdout)?;
    let (stderr_len, stderr_sha256) = capture("stderr", &record.stderr)?;
    let receipt = SourceTerminalReceipt {
        version: 1,
        grant_id: record.grant.grant_id.clone(),
        pid1: record.pid1.clone(),
        worker: record.worker.clone(),
        worker_local_pid: record.worker_local_pid,
        worker_wait_status,
        adopted_tree_drained: true,
        stdout_len,
        stdout_sha256,
        stderr_len,
        stderr_sha256,
    };
    let path = directory.join(name(&record.grant.grant_id, "terminal.json")?);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    serde_json::to_writer(&mut file, &receipt)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    File::open(directory)?.sync_all()
}
