//! Durable identity for accepted work namespaces. Only trusted broker code may
//! call `insert_prepared`, after the Runner guardian has positively accepted
//! the exact work. No socket operation exposes registration to a workload.
//! The launch module uses this registry before worker pre-exec release; a
//! work record alone does not certify execution or completion.
use crate::accepted_grant::GrantRecord;
use crate::accepted_grant::GrantRegistry;
use crate::entry_registry::ProcessStamp;
use crate::identity::{PeerIdentity, PinnedProcess, boot_id, observed_incarnation_gone};
use crate::json_artifact;
use crate::registry::{LiveRoot, RootRecord, RootRegistry};
use crate::source_physical::SourcePhysicalRegistry;
use oulipoly_state::mailbox::{BrokerSidecar, FreshV30Lane, PreparedProcessStamp};
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
    closed_historical: HashSet<String>,
    poisoned: bool,
}

/// Exact physical Q readback. This proves the child PID1's own ECHILD receipt,
/// its original parent's wait, and absence of the registered PID incarnation.
/// It does not settle State, recipient ACKs, or retire the work record.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
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

/// A retained child-work retirement, distinct from the source and root seals.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkRetirement {
    pub version: u32,
    pub root: RootRecord,
    pub d_key: String,
    pub handoff_id: String,
    pub work_grant_id: String,
    pub work_grant_sha256: String,
    pub physical_q: WorkPhysicalQ,
    pub source_grant_id: String,
    pub source_retirement_sha256: String,
    pub root_terminal_execution_sha256: String,
    pub parent_output_sha256: String,
    pub child_event_sha256: String,
    pub original_native_grant_id: String,
    pub original_turn_id: String,
    pub original_turn_receipt_sha256: String,
    pub fresh_grant_id: String,
    pub fresh_turn_id: String,
    pub fresh_receipt_sha256: String,
    pub selected_k_sha256: String,
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

pub(crate) fn root_only_bytes(directory: &Path, name: &str, maximum: u64) -> io::Result<Vec<u8>> {
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
        || before.nlink() != 1
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
    fn seal_name(grant_id: &str) -> io::Result<String> {
        let id = uuid::Uuid::parse_str(grant_id)
            .map_err(|_| io::Error::other("invalid work grant ID"))?;
        if id.to_string() != grant_id {
            return Err(io::Error::other("noncanonical work grant ID"));
        }
        Ok(format!("{grant_id}.retired.json"))
    }

    /// Rebuild the proposed seal from fresh physical, State and both ACK
    /// readbacks. The original source must already have its separate seal.
    fn retirement_evidence(
        &self,
        expected: &RootRecord,
        roots: &RootRegistry,
        grants: &GrantRegistry,
        sources: &SourcePhysicalRegistry,
        sidecar: &BrokerSidecar,
        lane: &FreshV30Lane,
        grant: &GrantRecord,
        terminal_dir: &Path,
    ) -> io::Result<WorkRetirement> {
        let refuse = |reason| io::Error::other(reason);
        let grant_sha = grants.exact_consumed_sha256(grant)?;
        let q = self.physical_q(expected, roots, grant, terminal_dir)?;
        let (released, actor) = lane
            .released_handoff_for_root(&expected.root_id)
            .map_err(io::Error::other)?;
        let session = lane
            .read_session(&released.d_key)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("root D session absent"))?;
        let terminal = lane
            .read_private_root_terminal(&released, &actor, &session)
            .map_err(io::Error::other)?;
        let execution = terminal
            .execution
            .as_ref()
            .ok_or_else(|| refuse("root terminal execution absent"))?;
        let child = execution
            .child_event
            .as_ref()
            .ok_or_else(|| refuse("exact child W absent"))?;
        let mut bash_child = lane
            .read_bash_child(&child.request_id)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("registered Bash child absent"))?;
        bash_child.session = lane
            .read_session(&bash_child.d_key)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("Bash child D session absent"))?;
        let delegated = grant
            .delegated_root_h
            .as_ref()
            .ok_or_else(|| refuse("selected K absent from accepted grant"))?;
        let selected = delegated.selected_k.clone();
        lane.require_consumed_root_h_delegation(&released, &actor, &bash_child, &selected)
            .map_err(io::Error::other)?;
        let fresh_request = terminal
            .delivery_request_id
            .as_deref()
            .ok_or_else(|| refuse("fresh F request absent"))?;
        let fresh_ack = lane
            .read_headless_native_f_ack(fresh_request, &actor)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("fresh F ACK absent"))?;
        let attempt = lane
            .read_headless_native_f_attempt(fresh_request, &actor)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("fresh F attempt absent"))?;
        let source_id = &fresh_ack.proof.original_source_grant_id;
        let source = sources
            .retirement(source_id)?
            .ok_or_else(|| refuse("original source retirement absent"))?;
        let source_record = sources
            .records()
            .iter()
            .find(|record| record.grant.grant_id == *source_id)
            .ok_or_else(|| refuse("original source record absent"))?;
        let original = sidecar
            .read_native_recipient_grant(source_id)
            .map_err(io::Error::other)?
            .ok_or_else(|| refuse("original native ACK absent"))?;
        let selected_native: serde_json::Value = serde_json::from_str(&original.selected_k_json)?;
        let obligations = sidecar
            .read_root_source_effect_obligations(
                &expected.root_id,
                &PreparedProcessStamp {
                    host_pid: expected.init_host_pid,
                    boot_id: expected.boot_id.clone(),
                    starttime_ticks: expected.init_starttime_ticks,
                    pidns_dev: expected.pidns_dev,
                    pidns_ino: expected.pidns_ino,
                },
            )
            .map_err(io::Error::other)?;
        if source_record.joined_child != grant.joined_child
            || source_record.guardian != grant.guardian
        {
            return Err(refuse("source and accepted H actor differ"));
        }
        if delegated
            .proof
            .witness
            .delegated_root_h_request_id
            .as_deref()
            != Some(child.request_id.as_str())
            || delegated.proof.work_id != grant.work_id
            || child.parent_work_grant_id != selected.grant_id
            || child.parent_work_id != selected.work_id
            || bash_child.parent_work_grant_id != selected.grant_id
            || bash_child.parent_work_id != selected.work_id
        {
            return Err(refuse("accepted H, Bash child W and selected K differ"));
        }
        if sources.has_debt()
            || roots.has_unrelated_debt(expected)
            || !roots.admission_fenced(&expected.root_id)
            || obligations.unsettled() != 0
            || source.root != *expected
            || source.d_key != released.d_key
            || source.handoff_id != released.handoff_id
            || source.source_grant_id != *source_id
            || source.original_native_grant_id != original.grant_id
            || source_record.grant.owner_generation != grant.owner_generation
            || original.binding.source_generation != source_record.grant.source_generation
            || original.binding.registration_id != source_record.grant.candidate.registration_id
            || original.binding.source_grant_id != *source_id
            || source.original_turn_id != fresh_ack.proof.first_turn_id
            || source.fresh_grant_id != fresh_ack.proof.fresh_grant_id
            || source.fresh_turn_id != fresh_ack.proof.turn_id
            || source.fresh_receipt_sha256 != fresh_ack.proof.receipt_sha256
            || source.root_terminal_execution_sha256 != digest(&serde_json::to_vec(execution)?)
            || original.phase != "acked"
            || original.binding.root_id != expected.root_id
            || original.binding.owner_generation != grant.owner_generation
            || original.selected_k_json != fresh_ack.proof.selected_k_json
            || original.selected_k_json != attempt.selected_k_json
            || original.turn_id.as_deref() != Some(fresh_ack.proof.first_turn_id.as_str())
            || original.turn_receipt_sha256.as_deref()
                != Some(attempt.candidate.original_turn_receipt_sha256.as_str())
            || original.native_session_id != fresh_ack.proof.native_session_id
            || attempt.candidate.original_source_grant_id != *source_id
            || attempt.candidate.original_native_grant_id != original.grant_id
            || attempt.candidate.original_row_seq != original.binding.row_seq
            || attempt.candidate.fresh.grant_id != fresh_ack.proof.fresh_grant_id
            || attempt.candidate.fresh.source_id != child.source_id
            || attempt.candidate.fresh.attempt_id != child.attempt_id
            || fresh_ack.proof.bash_request_id != child.request_id
            || fresh_ack.proof.fresh_grant_id
                != terminal.delivery_grant_id.as_deref().unwrap_or_default()
            || fresh_ack.proof.turn_id == fresh_ack.proof.first_turn_id
            || fresh_ack.basis != "native_codex_f_assistant_ack"
            || terminal.notification_state != "acked"
            || terminal.ack_basis.as_deref() != Some("native_codex_f_assistant_ack")
            || !matches!(terminal.execution_state.as_str(), "success" | "failure")
            || !terminal.unknown_stages.is_empty()
            || !terminal.unresolved_child_request_ids.is_empty()
            || terminal.root_id != expected.root_id
            || terminal.d_key != released.d_key
            || terminal.handoff_id != released.handoff_id
            || terminal.child_request_id.as_deref() != Some(child.request_id.as_str())
            || execution.root_id != expected.root_id
            || execution.owner_generation != grant.owner_generation
            || child.root_id != expected.root_id
            || child.owner_generation != grant.owner_generation
            || selected_native["grant_id"] != selected.grant_id
            || selected_native["work_id"] != selected.work_id
            || selected_native["native_session_id"] != original.native_session_id
            || selected_native["plan_sha256"] != selected.plan_sha256
            || selected_native["account"] != selected.account
            || selected_native["model"] != selected.model
            || selected_native["provider_pid"] != selected.provider_pid
            || selected_native["provider_starttime"] != selected.provider_starttime
            || selected_native["provider_boot_id"] != selected.provider_boot_id
            || child.cancelled
            || !child.tree_drained
            || !child.output_closed
            || !libc::WIFEXITED(child.wait_status)
            || libc::WEXITSTATUS(child.wait_status) != 0
        {
            return Err(refuse(
                "child W, selected K, source, State or ACK binding changed",
            ));
        }
        Ok(WorkRetirement {
            version: 1,
            root: expected.clone(),
            d_key: released.d_key,
            handoff_id: released.handoff_id,
            work_grant_id: grant.grant_id.clone(),
            work_grant_sha256: grant_sha,
            physical_q: q,
            source_grant_id: source_id.clone(),
            source_retirement_sha256: digest(&serde_json::to_vec(&source)?),
            root_terminal_execution_sha256: digest(&serde_json::to_vec(execution)?),
            parent_output_sha256: digest(&serde_json::to_vec(&execution.parent)?),
            child_event_sha256: digest(&serde_json::to_vec(child)?),
            original_native_grant_id: original.grant_id,
            original_turn_id: fresh_ack.proof.first_turn_id,
            original_turn_receipt_sha256: attempt.candidate.original_turn_receipt_sha256,
            fresh_grant_id: fresh_ack.proof.fresh_grant_id,
            fresh_turn_id: fresh_ack.proof.turn_id,
            fresh_receipt_sha256: fresh_ack.proof.receipt_sha256,
            selected_k_sha256: digest(original.selected_k_json.as_bytes()),
        })
    }

    pub fn retirement(
        &self,
        expected: &RootRecord,
        roots: &RootRegistry,
        grants: &GrantRegistry,
        sources: &SourcePhysicalRegistry,
        sidecar: Option<&BrokerSidecar>,
        lane: Option<&FreshV30Lane>,
        grant: &GrantRecord,
        terminal_dir: &Path,
    ) -> io::Result<Option<WorkRetirement>> {
        let bytes = match root_only_bytes(
            &self.directory,
            &Self::seal_name(&grant.grant_id)?,
            16 * 1024,
        ) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let stored: WorkRetirement = serde_json::from_slice(&bytes)?;
        let sidecar = sidecar.ok_or_else(|| io::Error::other("retired work sidecar absent"))?;
        let lane = lane.ok_or_else(|| io::Error::other("retired work State lane absent"))?;
        let current = self.retirement_evidence(
            expected,
            roots,
            grants,
            sources,
            sidecar,
            lane,
            grant,
            terminal_dir,
        )?;
        if stored != current {
            return Err(io::Error::other("child work retirement seal changed"));
        }
        Ok(Some(stored))
    }

    pub fn retire_after_native_acks(
        &mut self,
        expected: &RootRecord,
        roots: &RootRegistry,
        grants: &GrantRegistry,
        sources: &SourcePhysicalRegistry,
        sidecar: &BrokerSidecar,
        lane: &FreshV30Lane,
        grant: &GrantRecord,
        terminal_dir: &Path,
    ) -> io::Result<WorkRetirement> {
        let seal = self.retirement_evidence(
            expected,
            roots,
            grants,
            sources,
            sidecar,
            lane,
            grant,
            terminal_dir,
        )?;
        if let Some(previous) = self.retirement(
            expected,
            roots,
            grants,
            sources,
            Some(sidecar),
            Some(lane),
            grant,
            terminal_dir,
        )? {
            return if previous == seal {
                Ok(previous)
            } else {
                Err(io::Error::other("conflicting child work retirement seal"))
            };
        }
        if let Err(error) =
            json_artifact::create_new(&self.directory, &Self::seal_name(&grant.grant_id)?, &seal)
        {
            self.poisoned = true;
            return Err(error);
        }
        self.retirement(
            expected,
            roots,
            grants,
            sources,
            Some(sidecar),
            Some(lane),
            grant,
            terminal_dir,
        )?
        .ok_or_else(|| io::Error::other("child work retirement readback absent"))
    }
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
        if roots.has_unrelated_debt(expected) || self.poisoned {
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
        let mut seals = Vec::new();
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
            if let Some(id) = name.strip_suffix(".retired.json") {
                let bytes = root_only_bytes(&directory, &name, 16 * 1024)?;
                let seal: WorkRetirement = serde_json::from_slice(&bytes)?;
                if Self::seal_name(id)? != name
                    || seal.version != 1
                    || seal.work_grant_id != id
                    || seal.physical_q.grant_id != id
                {
                    return Err(io::Error::other("invalid child work retirement seal"));
                }
                seals.push(seal);
                continue;
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
            closed_historical: HashSet::new(),
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
        for seal in seals {
            if !registry
                .live
                .iter()
                .map(|work| &work.record)
                .chain(&registry.debt)
                .any(|work| {
                    work.accepted_grant_id.as_deref() == Some(seal.work_grant_id.as_str())
                        && work.work_incarnation == seal.physical_q.work_incarnation
                        && work.root_id == seal.root.root_id
                })
            {
                return Err(io::Error::other("orphaned child work retirement seal"));
            }
        }
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
            || self
                .debt
                .iter()
                .any(|work| !self.closed_historical.contains(&work.root_id))
            || self.live.iter().any(|work| {
                !self.closed_historical.contains(&work.record.root_id)
                    && work.init.verify().is_err()
            })
    }

    /// Called only after the old Broker validates the matching closed root,
    /// every retired work seal, and the exact caller settlement.
    pub fn admit_closed_historical(&mut self, root_id: &str) {
        self.closed_historical.insert(root_id.to_owned());
    }

    pub(crate) fn broker_root(&self) -> io::Result<&Path> {
        self.directory
            .parent()
            .ok_or_else(|| io::Error::other("work registry has no broker root"))
    }

    pub(crate) fn has_uncertain_write(&self) -> bool {
        self.poisoned
    }

    pub fn debt_records(&self) -> &[WorkRecord] {
        &self.debt
    }

    /// After the caller proves the root terminal, stop holding the PID1 pins of
    /// its works whose pidfd reports the exact exit. Each record stays, as a
    /// restart leaves it; `has_debt`, Q and retirement read records alone.
    pub fn release_terminal_root(&mut self, root_id: &str) {
        let (released, live): (Vec<_>, Vec<_>) = std::mem::take(&mut self.live)
            .into_iter()
            .partition(|work| {
                work.record.root_id == root_id && matches!(work.init.exited(), Ok(true))
            });
        self.live = live;
        self.debt
            .extend(released.into_iter().map(|work| work.record));
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
        || works
            .debt
            .iter()
            .any(|work| !works.closed_historical.contains(&work.root_id))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn work(root_id: &str, init: &PinnedProcess) -> WorkRecord {
        WorkRecord {
            version: 1,
            boot_id: init.boot_id.clone(),
            work_incarnation: uuid::Uuid::new_v4().to_string(),
            root_id: root_id.into(),
            root_init_host_pid: 1,
            root_init_starttime_ticks: 1,
            root_pidns_dev: init.pidns_dev,
            root_pidns_ino: init.pidns_ino,
            work_id: uuid::Uuid::new_v4().to_string(),
            accepted_grant_id: None,
            parent_work_incarnation: None,
            init_host_pid: init.host_pid,
            init_starttime_ticks: init.starttime_ticks,
            pidns_dev: init.pidns_dev,
            pidns_ino: init.pidns_ino,
        }
    }

    fn child(exit_now: bool) -> PinnedProcess {
        let mut process = std::process::Command::new("sleep")
            .arg(if exit_now { "0.2" } else { "30" })
            .spawn()
            .unwrap();
        let pinned = PinnedProcess::open(process.id() as i32).unwrap();
        if exit_now {
            process.wait().unwrap();
        } else {
            std::mem::forget(process);
        }
        pinned
    }

    #[test]
    fn terminal_root_release_keeps_work_records_and_debt_classification() {
        let (terminal, other) = ("terminal-root", "other-root");
        let exited = child(true);
        let running = child(false);
        let other_exited = child(true);
        let running_pid = running.host_pid;
        let records = [
            work(terminal, &exited),
            work(terminal, &running),
            work(other, &other_exited),
        ];
        let mut works = WorkRegistry {
            directory: PathBuf::new(),
            live: records
                .iter()
                .cloned()
                .zip([exited, running, other_exited])
                .map(|(record, init)| LiveWork { record, init })
                .collect(),
            debt: Vec::new(),
            closed_historical: HashSet::new(),
            poisoned: false,
        };
        let descriptors = || fs::read_dir("/proc/self/fd").unwrap().count();
        let before = (works.has_debt(), descriptors());
        assert!(before.0, "exited works of unclosed roots are debt");

        // Only the exited work of the named root drops its two handles.
        works.release_terminal_root(terminal);
        assert_eq!(descriptors(), before.1 - 2);
        assert_eq!(works.has_debt(), before.0);
        assert_eq!(works.debt_records(), &records[..1]);
        assert_eq!(
            works
                .live_works()
                .map(|work| &work.record)
                .collect::<Vec<_>>(),
            vec![&records[1], &records[2]]
        );
        unsafe { libc::kill(running_pid, libc::SIGKILL) };
    }
}
