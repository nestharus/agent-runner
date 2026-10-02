//! One durable start per selected W. Publication precedes spawn; an uncertain
//! spawn is never retried. A candidate record is evidence, not admission.
use crate::entry_registry::ProcessStamp;
use crate::identity::{PeerIdentity, PinnedProcess};
use crate::json_artifact;
use crate::phase_record;
use oulipoly_state::mailbox::FreshBashWakeSuccessorDecision;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

pub const FD_ENV: &str = "OULIPOLY_KERNEL_SUCCESSOR_GATE_FD_V1";
pub const ENTRY_ARG: &str = "--installed-successor-candidate";
const START_PROTOCOL: &str = "installed-successor-start-v1";
const CANDIDATE_PROTOCOL: &str = "installed-successor-candidate-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartRecord {
    pub protocol: String,
    pub d_key: String,
    pub decision: FreshBashWakeSuccessorDecision,
    pub pair_generation: String,
    pub source_generation: String,
    pub original: ProcessStamp,
    pub owner_uid: u32,
    pub owner_gid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateRecord {
    pub protocol: String,
    pub offer_request_id: String,
    pub process: ProcessStamp,
    pub owner_uid: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateMessage {
    pub protocol: String,
    pub start: StartRecord,
    pub candidate: CandidateRecord,
}

pub struct Ledger {
    starts: PathBuf,
    candidates: PathBuf,
    pair_generation: String,
    source_generation: String,
}

fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.to_string() == value)
}

impl Ledger {
    pub fn open(state: &Path, pair: &str, source: &str) -> io::Result<Self> {
        if !canonical_uuid(pair) || !canonical_uuid(source) {
            return Err(io::Error::other("invalid successor pair/source"));
        }
        let starts = state.join("installed-successor-starts");
        let candidates = state.join("installed-successor-candidates");
        for path in [&starts, &candidates] {
            if !path.exists() {
                fs::DirBuilder::new().mode(0o700).create(path)?;
                File::open(state)?.sync_all()?;
            }
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o777 != 0o700
            {
                return Err(io::Error::other("unsafe successor ledger directory"));
            }
            json_artifact::require_no_pending(path)?;
        }
        let ledger = Self {
            starts,
            candidates,
            pair_generation: pair.into(),
            source_generation: source.into(),
        };
        for entry in fs::read_dir(&ledger.starts)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().to_string();
            let Some(id) = id.strip_suffix(".json") else {
                return Err(io::Error::other("invalid successor start filename"));
            };
            ledger.read_start(id)?;
        }
        for entry in fs::read_dir(&ledger.candidates)? {
            let entry = entry?;
            let id = entry.file_name().to_string_lossy().to_string();
            let Some(id) = id.strip_suffix(".json") else {
                return Err(io::Error::other("invalid successor candidate filename"));
            };
            ledger.read_candidate(id)?;
        }
        Ok(ledger)
    }

    pub fn read_start(&self, offer: &str) -> io::Result<Option<StartRecord>> {
        if !canonical_uuid(offer) {
            return Err(io::Error::other("invalid successor offer ID"));
        }
        let path = self.starts.join(format!("{offer}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let record: StartRecord = json_artifact::read(&path, "successor_start")?;
        if record.protocol != START_PROTOCOL
            || !canonical_uuid(&record.d_key)
            || record.decision.offer_request_id != offer
            || record.pair_generation != self.pair_generation
            || record.source_generation != self.source_generation
            || record.original.host_pid != record.decision.obligation.original_identity.host_pid
            || record.original.boot_id != record.decision.obligation.original_identity.boot_id
            || record.original.starttime_ticks
                != record.decision.obligation.original_identity.starttime_ticks
            || record.original.pidns_dev != record.decision.obligation.original_identity.pidns_dev
            || record.original.pidns_ino != record.decision.obligation.original_identity.pidns_ino
        {
            return Err(io::Error::other("successor start identity changed"));
        }
        Ok(Some(record))
    }

    pub fn reserve(
        &self,
        d_key: &str,
        decision: &FreshBashWakeSuccessorDecision,
        peer: &PeerIdentity,
    ) -> io::Result<StartRecord> {
        let offer = &decision.offer_request_id;
        if self.read_start(offer)?.is_some() {
            return Err(io::Error::other("successor start already spent"));
        }
        if decision.obligation.original_identity.host_pid != peer.process.host_pid
            || decision.obligation.original_identity.boot_id != peer.process.boot_id
            || decision.obligation.original_identity.starttime_ticks != peer.process.starttime_ticks
            || decision.obligation.original_identity.pidns_dev != peer.process.pidns_dev
            || decision.obligation.original_identity.pidns_ino != peer.process.pidns_ino
        {
            return Err(io::Error::other(
                "successor start original/selected W changed",
            ));
        }
        peer.process.verify()?;
        if peer.process.exited()? {
            return Err(io::Error::other("successor original already exited"));
        }
        let record = StartRecord {
            protocol: START_PROTOCOL.into(),
            d_key: d_key.into(),
            decision: decision.clone(),
            pair_generation: self.pair_generation.clone(),
            source_generation: self.source_generation.clone(),
            original: ProcessStamp::from(&peer.process),
            owner_uid: peer.uid,
            owner_gid: peer.gid,
        };
        json_artifact::create_new(&self.starts, &format!("{offer}.json"), &record)?;
        if self.read_start(offer)?.as_ref() != Some(&record) {
            return Err(io::Error::other("successor start readback changed"));
        }
        Ok(record)
    }

    pub fn read_candidate(&self, offer: &str) -> io::Result<Option<CandidateRecord>> {
        let start = self
            .read_start(offer)?
            .ok_or_else(|| io::Error::other("successor start absent"))?;
        let path = self.candidates.join(format!("{offer}.json"));
        if !path.exists() {
            return Ok(None);
        }
        let record: CandidateRecord = json_artifact::read(&path, "successor_candidate")?;
        if record.protocol != CANDIDATE_PROTOCOL
            || record.offer_request_id != offer
            || record.owner_uid != start.owner_uid
            || record.process == start.original
        {
            return Err(io::Error::other("successor candidate identity changed"));
        }
        Ok(Some(record))
    }

    pub fn record_candidate(
        &self,
        start: &StartRecord,
        record: &CandidateRecord,
    ) -> io::Result<()> {
        if self.read_start(&start.decision.offer_request_id)?.as_ref() != Some(start)
            || self
                .read_candidate(&start.decision.offer_request_id)?
                .is_some()
            || record.protocol != CANDIDATE_PROTOCOL
            || record.offer_request_id != start.decision.offer_request_id
            || record.owner_uid != start.owner_uid
            || record.process == start.original
        {
            return Err(io::Error::other("successor candidate cannot be recorded"));
        }
        json_artifact::create_new(
            &self.candidates,
            &format!("{}.json", record.offer_request_id),
            &record,
        )?;
        if self.read_candidate(&record.offer_request_id)?.as_ref() != Some(&record) {
            return Err(io::Error::other("successor candidate readback changed"));
        }
        Ok(())
    }
}

pub struct LiveCandidate {
    pub record: CandidateRecord,
    child: Child,
    channel: UnixStream,
    offered: bool,
}

impl LiveCandidate {
    pub fn spawn(
        image: &File,
        start: &StartRecord,
        socket: &Path,
        manifest: Option<&Path>,
    ) -> io::Result<Self> {
        let (broker, gate) = UnixStream::pair()?;
        let gate_fd = unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if gate_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let gate = unsafe { UnixStream::from_raw_fd(gate_fd) };
        let image_fd = unsafe { libc::fcntl(image.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
        if image_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pinned_image = unsafe { File::from_raw_fd(image_fd) };
        let mut command = Command::new(format!("/proc/self/fd/{image_fd}"));
        command
            .arg(ENTRY_ARG)
            .env_clear()
            .env(FD_ENV, gate_fd.to_string())
            .env("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        if let Some(manifest) = manifest {
            command
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_PAIR_V1", manifest)
                .env("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1", socket);
            command.stderr(Stdio::from(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .mode(0o600)
                    .open(socket.with_file_name("successor-child.err"))?,
            ));
        } else {
            command.stderr(Stdio::null());
        }
        let uid = start.owner_uid;
        let gid = start.owner_gid;
        let private_root_mapped = std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| {
                let mut fields = map.split_ascii_whitespace();
                fields.next() == Some("0")
                    && fields.next().is_some_and(|host_uid| host_uid != "0")
                    && fields.next() == Some("1")
            });
        unsafe {
            command.pre_exec(move || {
                let current_uid = libc::geteuid();
                if (current_uid == 0
                    && !private_root_mapped
                    && libc::setgroups(0, std::ptr::null()) != 0)
                    || (current_uid != 0 && current_uid != uid)
                    || libc::setresgid(gid, gid, gid) != 0
                    || libc::setresuid(uid, uid, uid) != 0
                    || libc::fcntl(gate_fd, libc::F_SETFD, 0) != 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        drop(gate);
        let prepared = (|| -> io::Result<CandidateRecord> {
            let process = PinnedProcess::open(child.id() as i32)?;
            let broker_process = PinnedProcess::open(std::process::id() as i32)?;
            if !process.same_executable_as(image)? || !process.direct_child_of(&broker_process)? {
                return Err(io::Error::other(
                    "successor did not retain Broker parent/image",
                ));
            }
            Ok(CandidateRecord {
                protocol: CANDIDATE_PROTOCOL.into(),
                offer_request_id: start.decision.offer_request_id.clone(),
                process: ProcessStamp::from(&process),
                owner_uid: uid,
            })
        })();
        let record = match prepared {
            Ok(record) => record,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        drop(pinned_image);
        observe_exit(
            &start.decision.offer_request_id,
            record.process.host_pid,
            record.process.starttime_ticks,
        );
        Ok(Self {
            record,
            child,
            channel: broker,
            offered: false,
        })
    }

    pub fn issue(&mut self, start: &StartRecord, candidate: &CandidateRecord) -> io::Result<()> {
        if self.record != *candidate {
            return Err(io::Error::other("successor process changed"));
        }
        let message = GateMessage {
            protocol: "installed-successor-gate-v1".into(),
            start: start.clone(),
            candidate: candidate.clone(),
        };
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.channel.write_all(&bytes)
    }

    pub fn consume_offer(&mut self, peer: &PeerIdentity) -> io::Result<()> {
        if self.offered
            || self.child.try_wait()?.is_some()
            || ProcessStamp::from(&peer.process) != self.record.process
            || peer.uid != self.record.owner_uid
        {
            return Err(io::Error::other(
                "successor live gate changed or already consumed",
            ));
        }
        peer.process.verify()?;
        let mut byte = [0u8; 1];
        self.channel.set_nonblocking(true)?;
        let result = self.channel.read_exact(&mut byte);
        self.channel.set_nonblocking(false)?;
        result?;
        if byte != [b'R'] {
            return Err(io::Error::other("successor gate proof invalid"));
        }
        self.offered = true;
        Ok(())
    }

    pub fn verify_offered(&mut self, peer: &PeerIdentity) -> io::Result<()> {
        if !self.offered
            || self.child.try_wait()?.is_some()
            || ProcessStamp::from(&peer.process) != self.record.process
            || peer.uid != self.record.owner_uid
        {
            return Err(io::Error::other("successor live offered peer absent"));
        }
        peer.process.verify()
    }

    pub fn verify_offered_stamp(&mut self, stamp: &ProcessStamp) -> io::Result<()> {
        if !self.offered || self.child.try_wait()?.is_some() || &self.record.process != stamp {
            return Err(io::Error::other("successor live candidate absent"));
        }
        Ok(())
    }

    pub fn reap_after_ack(self) {
        let mut child = self.child;
        let offer = self.record.offer_request_id;
        let (pid, starttime) = (
            self.record.process.host_pid,
            self.record.process.starttime_ticks,
        );
        let _ = std::thread::Builder::new()
            .name("installed-successor-reaper".into())
            .spawn(move || {
                let exit = match child.wait() {
                    Ok(status) => exit_of(status),
                    Err(_) => phase_record::SuccessorExit::Unobserved("reaper-wait-failed"),
                };
                phase_record::successor_exited(&offer, pid, starttime, "reaper", &exit);
            });
    }
}

/// Blocks until the pidfd's child exits and returns how, leaving the child
/// unreaped (`WNOWAIT`): its parent's own wait still receives the status.
fn wait_exit_unreaped(pidfd: &std::os::fd::OwnedFd) -> phase_record::SuccessorExit {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = loop {
        let rc = unsafe {
            libc::waitid(
                libc::P_PIDFD,
                pidfd.as_raw_fd() as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc != 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        break rc;
    };
    if rc != 0 {
        // ECHILD: the Broker's own reaper took the status first and records
        // it itself.
        return phase_record::SuccessorExit::Unobserved("reaped-before-observed");
    }
    let status = unsafe { info.si_status() };
    match info.si_code {
        libc::CLD_EXITED => phase_record::SuccessorExit::Exited(status),
        libc::CLD_KILLED | libc::CLD_DUMPED => phase_record::SuccessorExit::Signaled(status),
        _ => phase_record::SuccessorExit::Unobserved("status-unclassified"),
    }
}

fn exit_of(status: std::process::ExitStatus) -> phase_record::SuccessorExit {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => phase_record::SuccessorExit::Exited(code),
        (None, Some(signal)) => phase_record::SuccessorExit::Signaled(signal),
        (None, None) => phase_record::SuccessorExit::Unobserved("status-unclassified"),
    }
}

/// Records the spawn, then waits on the exact child through a pidfd without
/// reaping it, and records how it ended. A successor that fails at entry
/// (before or after its offer) otherwise leaves no trace: its stderr is null
/// and nothing reaps it before ACK. Its environment, descriptors and the
/// Broker's own waits are unchanged; the status stays for the Broker's wait.
fn observe_exit(offer: &str, pid: i32, starttime: u64) {
    phase_record::successor_spawned(offer, pid, starttime);
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if pidfd < 0 {
        let exit = phase_record::SuccessorExit::Unobserved("pidfd-unavailable");
        phase_record::successor_exited(offer, pid, starttime, "observer", &exit);
        return;
    }
    let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(pidfd) };
    let observed = offer.to_owned();
    let spawned = std::thread::Builder::new()
        .name("installed-successor-observer".into())
        .stack_size(64 * 1024)
        .spawn(move || {
            let exit = wait_exit_unreaped(&pidfd);
            phase_record::successor_exited(&observed, pid, starttime, "observer", &exit);
        });
    if spawned.is_err() {
        let exit = phase_record::SuccessorExit::Unobserved("observer-unavailable");
        phase_record::successor_exited(offer, pid, starttime, "observer", &exit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_state::mailbox::FreshRecipientIdentity;

    fn pidfd_of(child: &Child) -> std::os::fd::OwnedFd {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as i32, 0) } as i32;
        assert!(fd >= 0, "{}", io::Error::last_os_error());
        unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) }
    }

    /// The observer reports a failed entry's status and a killed one's
    /// signal, and the parent's own wait still receives the same status.
    #[test]
    fn unreaped_exit_observation_leaves_the_parent_wait_unchanged() {
        use std::os::unix::process::ExitStatusExt;
        let mut failed = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let pidfd = pidfd_of(&failed);
        assert_eq!(
            wait_exit_unreaped(&pidfd),
            phase_record::SuccessorExit::Exited(7)
        );
        // Still a zombie for its parent: observing it did not reap it.
        assert_eq!(failed.wait().unwrap().code(), Some(7));
        // Once the parent reaped it, the status is not invented.
        assert_eq!(
            wait_exit_unreaped(&pidfd),
            phase_record::SuccessorExit::Unobserved("reaped-before-observed")
        );

        let mut killed = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pidfd = pidfd_of(&killed);
        let observer = std::thread::spawn(move || wait_exit_unreaped(&pidfd));
        unsafe { libc::kill(killed.id() as i32, libc::SIGKILL) };
        assert_eq!(
            observer.join().unwrap(),
            phase_record::SuccessorExit::Signaled(libc::SIGKILL)
        );
        assert_eq!(killed.wait().unwrap().signal(), Some(libc::SIGKILL));
        assert!(matches!(
            exit_of(killed.wait().unwrap()),
            phase_record::SuccessorExit::Signaled(signal) if signal == libc::SIGKILL
        ));
    }

    #[test]
    fn spent_start_survives_restart_and_candidate_is_immutable() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let offer = uuid::Uuid::new_v4().to_string();
        let d_key = uuid::Uuid::new_v4().to_string();
        let process = PinnedProcess::open(std::process::id() as i32).unwrap();
        let peer = PeerIdentity {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            process,
        };
        let original = FreshRecipientIdentity {
            host_pid: peer.process.host_pid,
            boot_id: peer.process.boot_id.clone(),
            starttime_ticks: peer.process.starttime_ticks,
            pidns_dev: peer.process.pidns_dev,
            pidns_ino: peer.process.pidns_ino,
        };
        let wake = uuid::Uuid::new_v4().to_string();
        let decision: FreshBashWakeSuccessorDecision = serde_json::from_value(serde_json::json!({
            "wake_request_id":wake,
            "offer_request_id":offer,
            "obligation": {
                "request_id":wake,
                "session_id":uuid::Uuid::new_v4().to_string(),
                "seq":1,
                "source_id":uuid::Uuid::new_v4().to_string(),
                "attempt_id":uuid::Uuid::new_v4().to_string(),
                "lane_id":uuid::Uuid::new_v4().to_string(),
                "source_generation":uuid::Uuid::new_v4().to_string(),
                "root_id":uuid::Uuid::new_v4().to_string(),
                "owner_generation":uuid::Uuid::new_v4().to_string(),
                "original_identity":original,
                "payload_sha256":"a".repeat(64),
                "payload_byte_len":12,
                "recorded_at":"now",
            },
            "decided_at":"now",
        }))
        .unwrap();
        let ledger = Ledger::open(state.path(), &pair, &source).unwrap();
        let start = ledger.reserve(&d_key, &decision, &peer).unwrap();
        assert!(ledger.reserve(&d_key, &decision, &peer).is_err());
        let reopened = Ledger::open(state.path(), &pair, &source).unwrap();
        assert_eq!(reopened.read_start(&offer).unwrap(), Some(start.clone()));
        assert!(reopened.reserve(&d_key, &decision, &peer).is_err());
        assert!(Ledger::open(state.path(), &uuid::Uuid::new_v4().to_string(), &source).is_err());
        let mut candidate = CandidateRecord {
            protocol: CANDIDATE_PROTOCOL.into(),
            offer_request_id: offer.clone(),
            process: start.original.clone(),
            owner_uid: start.owner_uid,
        };
        assert!(reopened.record_candidate(&start, &candidate).is_err());
        candidate.process.host_pid += 1;
        reopened.record_candidate(&start, &candidate).unwrap();
        assert!(reopened.record_candidate(&start, &candidate).is_err());
        let final_read = Ledger::open(state.path(), &pair, &source).unwrap();
        assert_eq!(final_read.read_candidate(&offer).unwrap(), Some(candidate));
    }
}
