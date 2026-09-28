//! Durable identity for accepted work namespaces. Only trusted broker code may
//! call `insert_prepared`, after the Runner guardian has positively accepted
//! the exact work. No socket operation exposes registration to a workload.
//! The launch module uses this registry before worker pre-exec release; a
//! work record alone does not certify execution or completion.
use crate::accepted_grant::GrantRecord;
use crate::entry_registry::ProcessStamp;
use crate::identity::{PeerIdentity, PinnedProcess, boot_id, observed_incarnation_gone};
use crate::json_artifact;
use crate::registry::{LiveRoot, RootRecord, RootRegistry};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkRecord {
    pub version: u32,
    pub boot_id: String,
    pub work_incarnation: String,
    pub root_id: String,
    pub root_init_host_pid: i32,
    pub root_init_starttime_ticks: u64,
    pub root_pidns_dev: u64,
    pub root_pidns_ino: u64,
    pub work_id: String,
    /// Binder for a broker launch's consumed positive grant. This field
    /// alone is not authority; private classifier fixtures may synthesize it.
    #[serde(default)]
    pub accepted_grant_id: Option<String>,
    pub parent_work_incarnation: Option<String>,
    pub init_host_pid: i32,
    pub init_starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
}

#[derive(Debug)]
pub struct LiveWork {
    pub record: WorkRecord,
    pub init: PinnedProcess,
}

#[derive(Debug)]
pub struct WorkRegistry {
    directory: PathBuf,
    live: Vec<LiveWork>,
    debt: Vec<WorkRecord>,
    poisoned: bool,
}

/// Exact physical Q readback. This proves the child PID1's own ECHILD receipt,
/// its original parent's wait, and absence of the registered PID incarnation.
/// It does not settle State, recipient ACKs, or retire the work record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WorkPhysicalQ {
    pub root_id: String,
    pub work_id: String,
    pub grant_id: String,
    pub work_incarnation: String,
    pub pid1_host_pid: i32,
    pub pid1_starttime_ticks: u64,
    pub pidns_dev: u64,
    pub pidns_ino: u64,
    pub work_record_sha256: String,
    pub terminal_receipt_sha256: String,
    pub pid1_wait_sha256: String,
    pub worker_wait_status: i32,
    pub pid1_wait_status: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkTerminalReceipt {
    version: u32,
    work_incarnation: String,
    init_host_pid: i32,
    worker_local_pid: i32,
    worker_wait_status: i32,
    physical_tree_drained: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkPid1WaitReceipt {
    version: u32,
    grant_id: String,
    work_id: String,
    pid1_parent_namespace_pid: i32,
    wait_status: i32,
    reaped: bool,
}

fn root_only_bytes(directory: &Path, name: &str, maximum: u64) -> io::Result<Vec<u8>> {
    let path = directory.join(name);
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    let before = file.metadata()?;
    let named = fs::symlink_metadata(&path)?;
    if !before.is_file()
        || before.uid() != 0
        || before.mode() & 0o077 != 0
        || before.len() > maximum
        || named.dev() != before.dev()
        || named.ino() != before.ino()
    {
        return Err(io::Error::other(
            "work physical witness is not exact root-only file",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let named = fs::symlink_metadata(&path)?;
    if bytes.len() as u64 != before.len()
        || after.len() != before.len()
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || named.dev() != before.dev()
        || named.ino() != before.ino()
    {
        return Err(io::Error::other(
            "work physical witness changed during read",
        ));
    }
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Scope {
    Work {
        root_id: String,
        work_id: String,
        work_incarnation: String,
    },
    Root(String),
    Outside,
    Uncertain,
}

fn ns_id(file: &File) -> io::Result<(u64, u64)> {
    let metadata = file.metadata()?;
    Ok((metadata.dev(), metadata.ino()))
}

fn parent_ns_id(file: &File) -> io::Result<(u64, u64)> {
    let fd = unsafe { libc::ioctl(file.as_raw_fd(), libc::NS_GET_PARENT) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    ns_id(&unsafe { File::from_raw_fd(fd) })
}

fn matching_root<'a>(record: &WorkRecord, roots: &'a RootRegistry) -> Option<&'a LiveRoot> {
    roots.live_roots().find(|root| {
        root.record.root_id == record.root_id
            && root.record.boot_id == record.boot_id
            && root.record.init_host_pid == record.root_init_host_pid
            && root.record.init_starttime_ticks == record.root_init_starttime_ticks
            && (root.record.pidns_dev, root.record.pidns_ino)
                == (record.root_pidns_dev, record.root_pidns_ino)
            && root.init.verify().is_ok()
    })
}

fn reattach(
    record: &WorkRecord,
    roots: &RootRegistry,
    parents: &[LiveWork],
) -> Option<PinnedProcess> {
    if record.version != 1
        || boot_id().ok().as_deref() != Some(record.boot_id.as_str())
        || matching_root(record, roots).is_none()
    {
        return None;
    }
    let parent_id = (match &record.parent_work_incarnation {
        Some(id) => parents
            .iter()
            .find(|parent| {
                parent.record.work_incarnation == *id && parent.record.root_id == record.root_id
            })
            .filter(|parent| parent.init.verify().is_ok())
            .map(|parent| (parent.record.pidns_dev, parent.record.pidns_ino)),
        None => {
            matching_root(record, roots).map(|root| (root.record.pidns_dev, root.record.pidns_ino))
        }
    })?;
    let init = PinnedProcess::open(record.init_host_pid).ok()?;
    if init.boot_id != record.boot_id
        || init.starttime_ticks != record.init_starttime_ticks
        || (init.pidns_dev, init.pidns_ino) != (record.pidns_dev, record.pidns_ino)
        || !matches!(init.is_namespace_init(), Ok(true))
        || parent_ns_id(init.namespace()).ok() != Some(parent_id)
    {
        return None;
    }
    Some(init)
}

impl WorkRegistry {
    /// Read one accepted child namespace's physical Q against its exact root
    /// fence and grant. A dead PID without both retained receipts is debt.
    pub fn physical_q(
        &self,
        expected: &RootRecord,
        roots: &RootRegistry,
        grant: &GrantRecord,
        terminal_dir: &Path,
    ) -> io::Result<WorkPhysicalQ> {
        roots.exact_record(expected)?;
        if roots.has_debt() || self.poisoned {
            return Err(io::Error::other("work root or registry uncertain"));
        }
        let stamp = ProcessStamp {
            host_pid: expected.init_host_pid,
            boot_id: expected.boot_id.clone(),
            starttime_ticks: expected.init_starttime_ticks,
            pidns_dev: expected.pidns_dev,
            pidns_ino: expected.pidns_ino,
        };
        if !grant.consumed || grant.root_id != expected.root_id || grant.root_init != stamp {
            return Err(io::Error::other("work accepted grant or root changed"));
        }
        let work = self
            .live
            .iter()
            .map(|w| &w.record)
            .chain(&self.debt)
            .find(|w| w.accepted_grant_id.as_deref() == Some(&grant.grant_id))
            .ok_or_else(|| io::Error::other("accepted child work record absent"))?;
        if work.version != 1
            || work.root_id != expected.root_id
            || work.work_id != grant.work_id
            || work.parent_work_incarnation != grant.parent_work_incarnation
            || work.boot_id != expected.boot_id
            || work.root_init_host_pid != expected.init_host_pid
            || work.root_init_starttime_ticks != expected.init_starttime_ticks
            || work.root_pidns_dev != expected.pidns_dev
            || work.root_pidns_ino != expected.pidns_ino
        {
            return Err(io::Error::other("accepted child work identity changed"));
        }
        if !observed_incarnation_gone(
            work.init_host_pid,
            &work.boot_id,
            work.init_starttime_ticks,
            (work.pidns_dev, work.pidns_ino),
        )? {
            return Err(io::Error::other("child work PID1 still live"));
        }
        if !roots.admission_fenced(&expected.root_id) {
            return Err(io::Error::other("work root admission fence absent"));
        }
        let record_bytes = root_only_bytes(
            &self.directory,
            &format!("{}.json", work.work_incarnation),
            16 * 1024,
        )?;
        let stored: WorkRecord = serde_json::from_slice(&record_bytes)?;
        if stored != *work {
            return Err(io::Error::other("child work record changed"));
        }
        let terminal_bytes = root_only_bytes(
            terminal_dir,
            &format!("{}.json", work.work_incarnation),
            4096,
        )?;
        let terminal: WorkTerminalReceipt = serde_json::from_slice(&terminal_bytes)?;
        if terminal.version != 1
            || terminal.work_incarnation != work.work_incarnation
            || terminal.init_host_pid != work.init_host_pid
            || terminal.worker_local_pid <= 1
            || !terminal.physical_tree_drained
        {
            return Err(io::Error::other("child work terminal Q changed or unknown"));
        }
        let wait_bytes = root_only_bytes(
            terminal_dir,
            &format!("{}.work-pid1-wait.json", grant.grant_id),
            4096,
        )?;
        let wait: WorkPid1WaitReceipt = serde_json::from_slice(&wait_bytes)?;
        if wait.version != 1
            || wait.grant_id != grant.grant_id
            || wait.work_id != grant.work_id
            || wait.pid1_parent_namespace_pid <= 0
            || !wait.reaped
            || !libc::WIFEXITED(wait.wait_status)
            || libc::WEXITSTATUS(wait.wait_status) != 0
        {
            return Err(io::Error::other("child work PID1 wait changed or unknown"));
        }
        Ok(WorkPhysicalQ {
            root_id: expected.root_id.clone(),
            work_id: work.work_id.clone(),
            grant_id: grant.grant_id.clone(),
            work_incarnation: work.work_incarnation.clone(),
            pid1_host_pid: work.init_host_pid,
            pid1_starttime_ticks: work.init_starttime_ticks,
            pidns_dev: work.pidns_dev,
            pidns_ino: work.pidns_ino,
            work_record_sha256: digest(&record_bytes),
            terminal_receipt_sha256: digest(&terminal_bytes),
            pid1_wait_sha256: digest(&wait_bytes),
            worker_wait_status: terminal.worker_wait_status,
            pid1_wait_status: wait.wait_status,
        })
    }
    pub fn open(directory: impl AsRef<Path>, roots: &RootRegistry) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let mut pending = Vec::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".json-pending-") {
                return Err(json_artifact::pending_error(
                    &entry.path(),
                    "work_registry_open",
                ));
            }
            if !name.ends_with(".json") || !entry.file_type()?.is_file() {
                return Err(io::Error::other("unrecognized work registry entry"));
            }
            let record: WorkRecord = json_artifact::read(&entry.path(), "work_registry_open")?;
            if format!("{}.json", record.work_incarnation) != name
                || uuid::Uuid::parse_str(&record.work_incarnation).is_err()
            {
                return Err(io::Error::other("work registry filename/ID mismatch"));
            }
            pending.push(record);
        }
        let mut registry = Self {
            directory,
            live: Vec::new(),
            debt: Vec::new(),
            poisoned: false,
        };
        // Parent records can be read in any filesystem order. Reattach in
        // layers, never treating a missing or ambiguous parent as a new root.
        while !pending.is_empty() {
            let mut deferred = Vec::new();
            let mut progress = false;
            for record in pending {
                match reattach(&record, roots, &registry.live) {
                    Some(init) => {
                        registry.live.push(LiveWork { record, init });
                        progress = true;
                    }
                    None => deferred.push(record),
                }
            }
            if !progress {
                registry.debt = deferred;
                break;
            }
            pending = deferred;
        }
        registry.check_unique()?;
        Ok(registry)
    }

    fn check_unique(&self) -> io::Result<()> {
        let mut incarnations = HashSet::new();
        let mut work_ids = HashSet::new();
        let mut grants = HashSet::new();
        let mut namespaces = HashSet::new();
        for record in self.live.iter().map(|work| &work.record).chain(&self.debt) {
            if !incarnations.insert(&record.work_incarnation)
                || !work_ids.insert((&record.root_id, &record.work_id))
                || record
                    .accepted_grant_id
                    .as_ref()
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_err() || !grants.insert(id))
            {
                return Err(io::Error::other(
                    "duplicate work incarnation or root/work ID",
                ));
            }
        }
        for work in &self.live {
            if !namespaces.insert((work.record.pidns_dev, work.record.pidns_ino)) {
                return Err(io::Error::other("duplicate live work PID namespace"));
            }
        }
        Ok(())
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned
            || !self.debt.is_empty()
            || self.live.iter().any(|work| work.init.verify().is_err())
    }

    pub fn debt_records(&self) -> &[WorkRecord] {
        &self.debt
    }

    pub fn live_works(&self) -> impl Iterator<Item = &LiveWork> {
        self.live.iter()
    }

    /// Records a namespace only after a separate positive accepted-work
    /// authority has been checked by trusted broker code. `work_id` is the
    /// Runner/Bash accepted ID; the broker creates an independent incarnation.
    /// The namespace PID1 must be held behind its pre-exec gate until this
    /// fsynced insertion succeeds. This method is not a launch or a grant.
    pub fn insert_prepared(
        &mut self,
        roots: &RootRegistry,
        root_id: &str,
        work_id: &str,
        parent_work_incarnation: Option<&str>,
        init_host_pid: i32,
    ) -> io::Result<WorkRecord> {
        self.insert_inner(
            roots,
            root_id,
            work_id,
            None,
            parent_work_incarnation,
            init_host_pid,
        )
    }

    /// The gated launch uses this form to couple PID1 to its fsynced,
    /// consumed accepted-work grant. This method itself does not authenticate
    /// the caller or release the pre-exec gate.
    pub fn insert_prepared_granted(
        &mut self,
        roots: &RootRegistry,
        root_id: &str,
        work_id: &str,
        accepted_grant_id: &str,
        parent_work_incarnation: Option<&str>,
        init_host_pid: i32,
    ) -> io::Result<WorkRecord> {
        uuid::Uuid::parse_str(accepted_grant_id)
            .map_err(|_| io::Error::other("invalid accepted grant ID"))?;
        self.insert_inner(
            roots,
            root_id,
            work_id,
            Some(accepted_grant_id),
            parent_work_incarnation,
            init_host_pid,
        )
    }

    fn insert_inner(
        &mut self,
        roots: &RootRegistry,
        root_id: &str,
        work_id: &str,
        accepted_grant_id: Option<&str>,
        parent_work_incarnation: Option<&str>,
        init_host_pid: i32,
    ) -> io::Result<WorkRecord> {
        if roots.has_debt() || self.has_debt() {
            return Err(io::Error::other("uncertain root or work debt"));
        }
        if work_id.is_empty() || work_id.len() > 256 || work_id.contains('\0') {
            return Err(io::Error::other("invalid accepted work ID"));
        }
        if self
            .live
            .iter()
            .any(|work| work.record.root_id == root_id && work.record.work_id == work_id)
            || accepted_grant_id.is_some_and(|grant| {
                self.live
                    .iter()
                    .any(|work| work.record.accepted_grant_id.as_deref() == Some(grant))
            })
        {
            return Err(io::Error::other(
                "accepted work or grant already registered",
            ));
        }
        let root = roots
            .live_roots()
            .find(|root| root.record.root_id == root_id && root.init.verify().is_ok())
            .ok_or_else(|| io::Error::other("exact live root missing"))?;
        let parent_id = if let Some(parent) = parent_work_incarnation {
            let parent = self
                .live
                .iter()
                .find(|work| {
                    work.record.work_incarnation == parent && work.record.root_id == root_id
                })
                .ok_or_else(|| io::Error::other("exact live parent work missing"))?;
            parent.init.verify()?;
            (parent.record.pidns_dev, parent.record.pidns_ino)
        } else {
            (root.record.pidns_dev, root.record.pidns_ino)
        };
        let init = PinnedProcess::open(init_host_pid)?;
        if !init.is_namespace_init()?
            || parent_ns_id(init.namespace())? != parent_id
            || (init.pidns_dev, init.pidns_ino) == parent_id
            || self.live.iter().any(|work| {
                (work.record.pidns_dev, work.record.pidns_ino) == (init.pidns_dev, init.pidns_ino)
            })
        {
            return Err(io::Error::other(
                "work PID1 is not a fresh direct child namespace",
            ));
        }
        let record = WorkRecord {
            version: 1,
            boot_id: root.record.boot_id.clone(),
            work_incarnation: uuid::Uuid::new_v4().to_string(),
            root_id: root_id.to_owned(),
            root_init_host_pid: root.record.init_host_pid,
            root_init_starttime_ticks: root.record.init_starttime_ticks,
            root_pidns_dev: root.record.pidns_dev,
            root_pidns_ino: root.record.pidns_ino,
            work_id: work_id.to_owned(),
            accepted_grant_id: accepted_grant_id.map(str::to_owned),
            parent_work_incarnation: parent_work_incarnation.map(str::to_owned),
            init_host_pid,
            init_starttime_ticks: init.starttime_ticks,
            pidns_dev: init.pidns_dev,
            pidns_ino: init.pidns_ino,
        };
        root.init.verify()?;
        init.verify()?;
        let persisted = json_artifact::create_new(
            &self.directory,
            &format!("{}.json", record.work_incarnation),
            &record,
        );
        if let Err(error) = persisted {
            self.poisoned = true;
            return Err(error);
        }
        self.live.push(LiveWork {
            record: record.clone(),
            init,
        });
        Ok(record)
    }
}

pub fn classify_scope(
    peer: &PeerIdentity,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
) -> Scope {
    classify_scope_inner(peer, host_namespace, roots, works, true)
}

/// Exact readback may outlive a child work PID1. It still refuses incomplete
/// registry writes and orphaned records; it grants no new work admission.
pub fn classify_scope_readback(
    peer: &PeerIdentity,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
) -> Scope {
    classify_scope_inner(peer, host_namespace, roots, works, false)
}

fn classify_scope_inner(
    peer: &PeerIdentity,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    require_all_live: bool,
) -> Scope {
    if peer.process.verify().is_err()
        || roots.has_debt()
        || works.poisoned
        || !works.debt.is_empty()
        || require_all_live && works.has_debt()
    {
        return Scope::Uncertain;
    }
    let Ok(mut namespace) = peer.process.namespace().try_clone() else {
        return Scope::Uncertain;
    };
    let Ok(host) = ns_id(host_namespace) else {
        return Scope::Uncertain;
    };
    let mut nearest_work: Option<&LiveWork> = None;
    for _ in 0..64 {
        let Ok(current) = ns_id(&namespace) else {
            return Scope::Uncertain;
        };
        let matches: Vec<_> = works
            .live_works()
            .filter(|work| (work.record.pidns_dev, work.record.pidns_ino) == current)
            .collect();
        if matches.len() > 1 {
            return Scope::Uncertain;
        }
        if nearest_work.is_none() {
            nearest_work = matches.first().copied();
        }
        let roots_here: Vec<_> = roots
            .live_roots()
            .filter(|root| (root.record.pidns_dev, root.record.pidns_ino) == current)
            .collect();
        if roots_here.len() > 1 {
            return Scope::Uncertain;
        }
        if let Some(root) = roots_here.first() {
            if root.init.verify().is_err() || peer.process.verify().is_err() {
                return Scope::Uncertain;
            }
            return match nearest_work {
                Some(work)
                    if work.record.root_id == root.record.root_id && work.init.verify().is_ok() =>
                {
                    Scope::Work {
                        root_id: root.record.root_id.clone(),
                        work_id: work.record.work_id.clone(),
                        work_incarnation: work.record.work_incarnation.clone(),
                    }
                }
                Some(_) => Scope::Uncertain,
                None => Scope::Root(root.record.root_id.clone()),
            };
        }
        if current == host {
            return if nearest_work.is_none() && peer.process.verify().is_ok() {
                Scope::Outside
            } else {
                Scope::Uncertain
            };
        }
        let fd = unsafe { libc::ioctl(namespace.as_raw_fd(), libc::NS_GET_PARENT) };
        if fd < 0 {
            return Scope::Uncertain;
        }
        namespace = unsafe { File::from_raw_fd(fd) };
    }
    Scope::Uncertain
}
