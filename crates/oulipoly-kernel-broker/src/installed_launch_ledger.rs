//! Durable first-install request/root binding. Publication precedes any future
//! root effect. A published request is never launchable again by resubmission.
use crate::connected_control::ControlGrant;
use crate::entry_registry::ProcessStamp;
use crate::identity::PeerIdentity;
use crate::installed_launch::InstalledLaunchSpec;
use crate::json_artifact;
use crate::root_drain::RootPhysicalCloseProof;
use oulipoly_state::mailbox::{BrokerClosedOwner, FreshRootEffectState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

const REQUEST_ONLY_PROTOCOL: &str = "installed-launch-request-v1";
const ROOT_BOUND_PROTOCOL: &str = "installed-launch-request-v2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestRecord {
    pub protocol: String,
    pub request_id: String,
    pub pair_generation: String,
    pub source_generation: String,
    pub owner_uid: u32,
    pub launcher: ProcessStamp,
    pub spec_sha256: String,
    /// Broker-chosen before any control process or root fork. V1 stays inert.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_id: Option<String>,
}

/// A kernel wait observation for the one control Runner. This is deliberately
/// not a caller result or physical drain certificate.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ControlExitRecord {
    pub protocol: String,
    pub request_id: String,
    pub pair_generation: String,
    pub source_generation: String,
    pub root_id: String,
    pub launcher: ProcessStamp,
    pub control: ProcessStamp,
    pub e_consumed: bool,
    pub code: Option<i32>,
    pub signal: Option<i32>,
}

const CONTROL_EXIT_PROTOCOL: &str = "installed-control-exit-v1";
const TERMINAL_PROTOCOL: &str = "installed-normal-terminal-v1";
const OFFLINE_TERMINAL_PROTOCOL: &str = "installed-offline-terminal-v1";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NormalTerminalCertificate {
    pub protocol: String,
    pub request: RequestRecord,
    pub control_exit: ControlExitRecord,
    pub exit_code: u8,
    pub physical: RootPhysicalCloseProof,
    pub owner: BrokerClosedOwner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor_ack: Option<oulipoly_state::mailbox::FreshSuccessorTerminalAck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_receipt: Option<oulipoly_state::mailbox::FreshOriginalReceiptIdentity>,
}

pub enum Admission {
    New { root_id: String },
    Existing,
}

pub struct InstalledLaunchLedger {
    directory: PathBuf,
    exit_directory: PathBuf,
    terminal_directory: PathBuf,
    pair_generation: String,
    source_generation: String,
}

fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .ok()
        .is_some_and(|id| id.to_string() == value)
}

impl InstalledLaunchLedger {
    /// The serving Broker has already established root and validated the
    /// activated pair. Fixture tests may use a private directory under their
    /// own UID; production serves only as host root.
    pub fn open(state: &Path, pair_generation: &str, source_generation: &str) -> io::Result<Self> {
        Self::open_inner(state, pair_generation, source_generation, true)
    }

    /// A fresh reader must never heal absent ledger storage while deciding
    /// whether historical roots can be ignored for scope classification.
    pub fn open_existing(
        state: &Path,
        pair_generation: &str,
        source_generation: &str,
    ) -> io::Result<Self> {
        Self::open_inner(state, pair_generation, source_generation, false)
    }

    fn open_inner(
        state: &Path,
        pair_generation: &str,
        source_generation: &str,
        create_missing: bool,
    ) -> io::Result<Self> {
        if !canonical_uuid(pair_generation) || !canonical_uuid(source_generation) {
            return Err(io::Error::other("invalid installed launch generation"));
        }
        let directory = state.join("installed-launches");
        let exit_directory = state.join("installed-control-exits");
        let terminal_directory = state.join("installed-normal-terminals");
        for path in [&directory, &exit_directory, &terminal_directory] {
            if create_missing && !path.exists() {
                fs::DirBuilder::new().mode(0o700).create(path)?;
                File::open(state)?.sync_all()?;
            }
        }
        let meta = fs::symlink_metadata(&directory)?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o700
        {
            return Err(io::Error::other("unsafe installed launch ledger directory"));
        }
        json_artifact::require_no_pending(&directory)?;
        for path in [&exit_directory, &terminal_directory] {
            let meta = fs::symlink_metadata(path)?;
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o777 != 0o700
            {
                return Err(io::Error::other("unsafe installed evidence directory"));
            }
            json_artifact::require_no_pending(path)?;
        }
        let ledger = Self {
            directory,
            exit_directory,
            terminal_directory,
            pair_generation: pair_generation.into(),
            source_generation: source_generation.into(),
        };
        let mut roots = std::collections::HashSet::new();
        for entry in fs::read_dir(&ledger.directory)? {
            let entry = entry?;
            let record = ledger.read_path(&entry.path())?;
            if entry.file_name().to_string_lossy() != format!("{}.json", record.request_id) {
                return Err(io::Error::other(
                    "installed launch ledger filename mismatch",
                ));
            }
            if record
                .root_id
                .as_ref()
                .is_some_and(|id| !roots.insert(id.clone()))
            {
                return Err(io::Error::other("duplicate installed launch root ID"));
            }
        }
        for entry in fs::read_dir(&ledger.exit_directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let request_id = name
                .strip_suffix(".json")
                .ok_or_else(|| io::Error::other("installed control exit filename mismatch"))?;
            ledger.read_control_exit(request_id)?;
        }
        for entry in fs::read_dir(&ledger.terminal_directory)? {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            let request_id = name
                .strip_suffix(".json")
                .ok_or_else(|| io::Error::other("installed terminal filename mismatch"))?;
            if !canonical_uuid(request_id) {
                return Err(io::Error::other("installed terminal filename mismatch"));
            }
        }
        Ok(ledger)
    }

    fn read_path(&self, path: &Path) -> io::Result<RequestRecord> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let meta = file.metadata()?;
        let named = fs::symlink_metadata(path)?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o600
            || meta.nlink() != 1
            || meta.len() > 4096
            || meta.dev() != named.dev()
            || meta.ino() != named.ino()
        {
            return Err(io::Error::other("unsafe installed launch ledger record"));
        }
        let record: RequestRecord = json_artifact::read_open(file, path, "installed_launch")?;
        if !matches!(
            (&record.protocol[..], record.root_id.as_deref()),
            (REQUEST_ONLY_PROTOCOL, None) | (ROOT_BOUND_PROTOCOL, Some(_))
        ) || !canonical_uuid(&record.request_id)
            || record
                .root_id
                .as_deref()
                .is_some_and(|id| !canonical_uuid(id))
            || record.pair_generation != self.pair_generation
            || record.source_generation != self.source_generation
            || record.spec_sha256.len() != 64
            || !record
                .spec_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(io::Error::other(
                "installed launch ledger identity mismatch",
            ));
        }
        Ok(record)
    }

    pub fn read(&self, request_id: &str) -> io::Result<RequestRecord> {
        if !canonical_uuid(request_id) {
            return Err(io::Error::other("invalid installed launch request ID"));
        }
        self.read_path(&self.directory.join(format!("{request_id}.json")))
    }

    /// Resolve a prior installed root through the immutable L ledger, then
    /// revalidate its certificate. A root ID alone cannot supply an exit.
    pub fn terminal_for_root(
        &self,
        root_id: &str,
    ) -> io::Result<Option<NormalTerminalCertificate>> {
        for entry in fs::read_dir(&self.directory)? {
            let request = self.read_path(&entry?.path())?;
            if request.root_id.as_deref() == Some(root_id) {
                return self.read_terminal(&request.request_id);
            }
        }
        Ok(None)
    }

    /// The immutable L request that chose this root, if any.
    pub fn request_for_root(&self, root_id: &str) -> io::Result<Option<RequestRecord>> {
        for entry in fs::read_dir(&self.directory)? {
            let request = self.read_path(&entry?.path())?;
            if request.root_id.as_deref() == Some(root_id) {
                return Ok(Some(request));
            }
        }
        Ok(None)
    }

    pub fn read_terminal(&self, request_id: &str) -> io::Result<Option<NormalTerminalCertificate>> {
        let request = self.read(request_id)?;
        let path = self.terminal_directory.join(format!("{request_id}.json"));
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let meta = file.metadata()?;
        let named = fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o600
            || meta.nlink() != 1
            || meta.len() > 65536
            || meta.dev() != named.dev()
            || meta.ino() != named.ino()
        {
            return Err(io::Error::other("unsafe installed terminal record"));
        }
        let certificate: NormalTerminalCertificate =
            json_artifact::read_open(file, &path, "installed_normal_terminal")?;
        let normal = certificate.protocol == TERMINAL_PROTOCOL
            && certificate.physical.normal.is_some()
            && certificate.physical.offline.is_none();
        let offline = certificate.protocol == OFFLINE_TERMINAL_PROTOCOL
            && certificate.physical.offline.is_some()
            && certificate.physical.normal.is_none()
            && certificate
                .physical
                .offline
                .as_ref()
                .is_some_and(|offline| {
                    matches!(
                        (&offline.effect.state, certificate.exit_code),
                        (FreshRootEffectState::ReturnedSuccess, 0)
                            | (FreshRootEffectState::ReturnedFailure, 1..=255)
                    )
                });
        if !(normal || offline)
            || certificate.request != request
            || Some(certificate.control_exit.clone()) != self.read_control_exit(request_id)?
            || certificate.control_exit.code != Some(i32::from(certificate.exit_code))
            || certificate.physical.root_id != request.root_id.as_deref().unwrap_or("")
            || certificate.owner.root_id != certificate.physical.root_id
            || certificate.owner.source_generation != request.source_generation
            || certificate.physical.original_receipt != certificate.original_receipt
            || serde_json::from_str::<RootPhysicalCloseProof>(
                &certificate.owner.physical_proof_json,
            )? != certificate.physical
        {
            return Err(io::Error::other("installed terminal identity mismatch"));
        }
        let state_root = self
            .directory
            .parent()
            .ok_or_else(|| io::Error::other("installed State root absent"))?;
        // This is current acceptance, not a historical certificate lookup.
        // An absent or unreadable State cannot establish an empty child set.
        let unknown =
            |error| io::Error::other(format!("installed_terminal_current_state_unknown: {error}"));
        let lane = oulipoly_state::mailbox::FreshV30Lane::open_at(state_root).map_err(unknown)?;
        if certificate.original_receipt.is_some() {
            let current = crate::root_drain::exact_original_receipt_for_root(
                &lane,
                &certificate.physical.root_id,
            )
            .map_err(|error| unknown(error.to_string()))?;
            if current != certificate.original_receipt {
                return Err(io::Error::other("installed original receipt changed"));
            }
        }
        // A certified close still agrees with the root's current child set:
        // a later C, or a changed member settlement, revokes the reader's
        // acceptance rather than leaving the certificate standing.
        match lane
            .root_child_closure(&certificate.physical.root_id)
            .map_err(unknown)?
        {
            None if certificate.physical.child_members.is_empty() => {}
            Some(closure)
                if closure.refusal.is_none()
                    && closure.members == certificate.physical.child_members => {}
            _ => {
                return Err(io::Error::other("installed terminal child closure changed"));
            }
        }
        Ok(Some(certificate))
    }

    pub fn publish_terminal(
        &self,
        mut certificate: NormalTerminalCertificate,
    ) -> io::Result<NormalTerminalCertificate> {
        certificate.protocol =
            if certificate.physical.offline.is_some() && certificate.physical.normal.is_none() {
                OFFLINE_TERMINAL_PROTOCOL.into()
            } else {
                TERMINAL_PROTOCOL.into()
            };
        let name = format!("{}.json", certificate.request.request_id);
        match json_artifact::create_new(&self.terminal_directory, &name, &certificate) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let stored = self
            .read_terminal(&certificate.request.request_id)?
            .ok_or_else(|| io::Error::other("installed terminal publication absent"))?;
        if stored != certificate {
            return Err(io::Error::other("installed terminal changed"));
        }
        Ok(stored)
    }

    /// Read back a saved direct-child wait only after validating it against
    /// the immutable L record. An absent wait remains unknown across restart.
    pub fn read_control_exit(&self, request_id: &str) -> io::Result<Option<ControlExitRecord>> {
        let request = self.read(request_id)?;
        let path = self.exit_directory.join(format!("{request_id}.json"));
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let meta = file.metadata()?;
        let named = fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o777 != 0o600
            || meta.nlink() != 1
            || meta.len() > 4096
            || meta.dev() != named.dev()
            || meta.ino() != named.ino()
        {
            return Err(io::Error::other("unsafe installed control exit record"));
        }
        let exit: ControlExitRecord =
            json_artifact::read_open(file, &path, "installed_control_exit")?;
        if exit.protocol != CONTROL_EXIT_PROTOCOL
            || exit.request_id != request.request_id
            || exit.pair_generation != request.pair_generation
            || exit.source_generation != request.source_generation
            || Some(exit.root_id.as_str()) != request.root_id.as_deref()
            || exit.launcher != request.launcher
            || exit.control == exit.launcher
            || (exit.code.is_some() == exit.signal.is_some())
            || exit.code.is_some_and(|code| !(0..=255).contains(&code))
            || exit.signal.is_some_and(|signal| signal <= 0)
        {
            return Err(io::Error::other("installed control exit identity mismatch"));
        }
        Ok(Some(exit))
    }

    /// Called only by the Broker holding the original Child after try_wait.
    /// Repeated writes accept the same observation and reject changed facts.
    pub fn record_control_exit(
        &self,
        grant: &ControlGrant,
        status: ExitStatus,
    ) -> io::Result<ControlExitRecord> {
        let request = self.read(&grant.message.request_id)?;
        if request.protocol != ROOT_BOUND_PROTOCOL
            || request.pair_generation != grant.message.pair_generation
            || request.source_generation != grant.message.source_generation
            || request.root_id.as_deref() != Some(grant.message.root_id.as_str())
            || request.launcher == grant.process
        {
            return Err(io::Error::other("installed control wait binding changed"));
        }
        let exit = ControlExitRecord {
            protocol: CONTROL_EXIT_PROTOCOL.into(),
            request_id: request.request_id.clone(),
            pair_generation: request.pair_generation,
            source_generation: request.source_generation,
            root_id: grant.message.root_id.clone(),
            launcher: request.launcher,
            control: grant.process.clone(),
            e_consumed: grant.e_accepted(),
            code: status.code(),
            signal: status.signal(),
        };
        if exit.code.is_some() == exit.signal.is_some() {
            return Err(io::Error::other("unrepresentable control wait status"));
        }
        let name = format!("{}.json", exit.request_id);
        match json_artifact::create_new(&self.exit_directory, &name, &exit) {
            Ok(()) => Ok(exit),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let old = self.read_control_exit(&exit.request_id)?;
                if old.as_ref() == Some(&exit) {
                    Ok(exit)
                } else {
                    Err(io::Error::other("installed control exit changed"))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// E reopens the fsynced L identity before spending an incarnation-local
    /// connected grant. A changed or incomplete record is unknown, not a new
    /// launch opportunity.
    pub fn verify_root_binding(&self, request_id: &str, root_id: &str) -> io::Result<()> {
        let record = self.read(request_id)?;
        if record.protocol != ROOT_BOUND_PROTOCOL || record.root_id.as_deref() != Some(root_id) {
            return Err(io::Error::other("connected control ledger root changed"));
        }
        Ok(())
    }

    fn unused_root_id(&self) -> io::Result<String> {
        let mut used = std::collections::HashSet::new();
        for entry in fs::read_dir(&self.directory)? {
            if let Some(root_id) = self.read_path(&entry?.path())?.root_id {
                used.insert(root_id);
            }
        }
        loop {
            let root_id = uuid::Uuid::new_v4().to_string();
            if !used.contains(&root_id) {
                return Ok(root_id);
            }
        }
    }

    pub fn reserve_request(
        &self,
        spec: &InstalledLaunchSpec,
        peer: &PeerIdentity,
    ) -> io::Result<Admission> {
        if spec.generation != self.pair_generation || !canonical_uuid(&spec.request_id) {
            return Err(io::Error::other("installed launch pair/request mismatch"));
        }
        peer.process.verify()?;
        let record = RequestRecord {
            protocol: ROOT_BOUND_PROTOCOL.into(),
            request_id: spec.request_id.clone(),
            pair_generation: self.pair_generation.clone(),
            source_generation: self.source_generation.clone(),
            owner_uid: peer.uid,
            launcher: ProcessStamp::from(&peer.process),
            spec_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(spec)?)),
            root_id: Some(self.unused_root_id()?),
        };
        let name = format!("{}.json", record.request_id);
        match json_artifact::create_new(&self.directory, &name, &record) {
            Ok(()) => Ok(Admission::New {
                root_id: record.root_id.expect("new request has a root binding"),
            }),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let existing = self.read(&record.request_id)?;
                // The only ignored field is the newly sampled candidate
                // root. A V1 record cannot acquire authority through retry.
                let mut expected = record.clone();
                expected.root_id = existing.root_id.clone();
                if existing.protocol != ROOT_BOUND_PROTOCOL
                    || existing.root_id.is_none()
                    || existing != expected
                {
                    return Err(io::Error::other(
                        "installed launch duplicate identity mismatch",
                    ));
                }
                Ok(Admission::Existing)
            }
            Err(error) => Err(error),
        }
    }

    /// An authenticated observation of the durable request. A bound UUID is
    /// only an identity; no root or terminal evidence exists yet.
    pub fn status(
        &self,
        request_id: &str,
        generation: &str,
        peer: &PeerIdentity,
    ) -> io::Result<String> {
        if generation != self.pair_generation {
            return Err(io::Error::other(
                "installed launch status generation mismatch",
            ));
        }
        peer.process.verify()?;
        let record = self.read(request_id)?;
        if record.owner_uid != peer.uid || record.launcher != ProcessStamp::from(&peer.process) {
            return Err(io::Error::other("installed launch status owner mismatch"));
        }
        Ok(format!("pending {request_id} {generation}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::PinnedProcess;
    use crate::installed_launch::{EntryKind, PROTOCOL as LAUNCH_PROTOCOL};
    use std::process::Command;

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn current_state_absent_and_unreadable_refuse_saved_terminal() {
        if unsafe { libc::geteuid() } != 0 {
            let status = Command::new("unshare")
                .arg("-Ur")
                .arg(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "installed_launch_ledger::tests::current_state_absent_and_unreadable_refuse_saved_terminal",
                    "--nocapture",
                ])
                .status()
                .expect("root-mapped private State fixture required");
            assert!(status.success());
            return;
        }
        use oulipoly_state::mailbox::{
            BoundStateFileIdentity, BrokerStateCloseCursor, EmptyV30BootstrapIdentity,
            FreshRecipientIdentity, FreshRootEffect, FreshRootWorkIntent,
        };
        let private = tempfile::tempdir().unwrap();
        let state_root = private.path().join("broker");
        EmptyV30BootstrapIdentity::bootstrap_at(&state_root).unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let ledger = InstalledLaunchLedger::open(&state_root, &pair, &source).unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let root_id = uuid::Uuid::new_v4().to_string();
        // Constructed certificate: this is a reader control, not evidence of
        // a real provider, owner close, process wait, receipt or installed run.
        let launcher = ProcessStamp {
            host_pid: 10,
            boot_id: "fixture".into(),
            starttime_ticks: 1,
            pidns_dev: 2,
            pidns_ino: 3,
        };
        let request = RequestRecord {
            protocol: ROOT_BOUND_PROTOCOL.into(),
            request_id: request_id.clone(),
            pair_generation: pair.clone(),
            source_generation: source.clone(),
            owner_uid: 0,
            launcher: launcher.clone(),
            spec_sha256: "a".repeat(64),
            root_id: Some(root_id.clone()),
        };
        let mut control = launcher.clone();
        control.host_pid = 11;
        let control_exit = ControlExitRecord {
            protocol: CONTROL_EXIT_PROTOCOL.into(),
            request_id: request_id.clone(),
            pair_generation: pair.clone(),
            source_generation: source.clone(),
            root_id: root_id.clone(),
            launcher,
            control,
            e_consumed: true,
            code: Some(0),
            signal: None,
        };
        json_artifact::create_new(&ledger.directory, &format!("{request_id}.json"), &request)
            .unwrap();
        json_artifact::create_new(
            &ledger.exit_directory,
            &format!("{request_id}.json"),
            &control_exit,
        )
        .unwrap();
        let physical = RootPhysicalCloseProof {
            root_id: root_id.clone(),
            pid1: "terminal_echild_absent".into(),
            pid1_echild_receipt: true,
            pid1_terminal_proof: true,
            pid1_parent_wait_proof: true,
            entry_physical_settled: true,
            entry_original_exited: true,
            work_records: 0,
            work_retired: 0,
            source_physical_records: 0,
            source_physical_retired: 0,
            source_effect: Default::default(),
            successor_ack: None,
            original_receipt: None,
            normal: None,
            offline: Some(crate::root_drain::OfflineRootEvidence {
                effect: FreshRootEffect {
                    handoff_id: "fixture".into(),
                    invocation_uuid: "fixture".into(),
                    session_id: "fixture".into(),
                    actor: FreshRecipientIdentity {
                        host_pid: 12,
                        boot_id: "fixture".into(),
                        starttime_ticks: 1,
                        pidns_dev: 2,
                        pidns_ino: 3,
                    },
                    intent: FreshRootWorkIntent::CliHelp(vec!["--help".into()]),
                    state: FreshRootEffectState::ReturnedSuccess,
                },
            }),
            child_members: Vec::new(),
        };
        let owner = BrokerClosedOwner {
            root_id: root_id.clone(),
            source_generation: source.clone(),
            owner_generation: "fixture".into(),
            root_record_json: "{}".into(),
            physical_proof_json: serde_json::to_string(&physical).unwrap(),
            state_cursor: BrokerStateCloseCursor {
                file: BoundStateFileIdentity {
                    device: 1,
                    inode: 2,
                },
                authority_ordinal: 0,
                admission_id: "no-completion-continuity".into(),
                continuity_digest: "0".repeat(64),
                sidecar_generation: "fixture".into(),
            },
        };
        let certificate = ledger
            .publish_terminal(NormalTerminalCertificate {
                protocol: String::new(),
                request,
                control_exit,
                exit_code: 0,
                physical,
                owner,
                successor_ack: None,
                original_receipt: None,
            })
            .unwrap();
        let terminal_path = ledger.terminal_directory.join(format!("{request_id}.json"));
        let historical_bytes = fs::read(&terminal_path).unwrap();
        let positive = || {
            assert_eq!(
                ledger.read_terminal(&request_id).unwrap(),
                Some(certificate.clone())
            );
            assert_eq!(
                ledger.terminal_for_root(&root_id).unwrap(),
                Some(certificate.clone())
            );
            assert_eq!(fs::read(&terminal_path).unwrap(), historical_bytes);
        };
        positive();
        let state = state_root.join("v30/state.db");
        let retained = state_root.join("v30/retained-state.db");
        fs::rename(&state, &retained).unwrap();
        for condition in ["absent", "unreadable"] {
            if condition == "unreadable" {
                fs::rename(&retained, &state).unwrap();
                fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o200))
                    .unwrap();
            }
            for error in [
                ledger.read_terminal(&request_id).unwrap_err(),
                ledger.terminal_for_root(&root_id).unwrap_err(),
            ] {
                assert!(
                    error
                        .to_string()
                        .starts_with("installed_terminal_current_state_unknown: "),
                    "{condition}: {error}"
                );
            }
            assert_eq!(fs::read(&terminal_path).unwrap(), historical_bytes);
            if condition == "absent" {
                assert!(!state.exists(), "reader recreated missing State");
            }
        }
        fs::set_permissions(&state, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        positive();
    }

    fn peer() -> PeerIdentity {
        PeerIdentity {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            process: PinnedProcess::open(std::process::id() as i32).unwrap(),
        }
    }

    fn spec(pair: &str, request: &str) -> InstalledLaunchSpec {
        InstalledLaunchSpec {
            protocol: LAUNCH_PROTOCOL.into(),
            generation: pair.into(),
            request_id: request.into(),
            kind: EntryKind::Cli,
            args: vec![b"--help".to_vec()],
            environment: vec![],
            stdio_present: [false; 3],
        }
    }

    #[test]
    fn readback_refuses_missing_storage_without_recreating_it() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        assert!(InstalledLaunchLedger::open_existing(state.path(), &pair, &source).is_err());
        assert!(!state.path().join("installed-launches").exists());
        InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        InstalledLaunchLedger::open_existing(state.path(), &pair, &source).unwrap();
        let terminals = state.path().join("installed-normal-terminals");
        fs::remove_dir(&terminals).unwrap();
        assert!(InstalledLaunchLedger::open_existing(state.path(), &pair, &source).is_err());
        assert!(!terminals.exists());
    }

    #[test]
    fn ledger_child() {
        let Ok(action) = std::env::var("AGE319_LEDGER_CHILD_ACTION") else {
            return;
        };
        let state = std::env::var("AGE319_LEDGER_STATE").unwrap();
        let pair = std::env::var("AGE319_LEDGER_PAIR").unwrap();
        let source = std::env::var("AGE319_LEDGER_SOURCE").unwrap();
        let request = std::env::var("AGE319_LEDGER_REQUEST").unwrap();
        let ledger = InstalledLaunchLedger::open(Path::new(&state), &pair, &source).unwrap();
        if action == "reserve" {
            let mut owner = peer();
            owner.process = PinnedProcess::open(
                std::env::var("AGE319_LEDGER_PARENT_PID")
                    .unwrap()
                    .parse()
                    .unwrap(),
            )
            .unwrap();
            let Admission::New { root_id } = ledger
                .reserve_request(&spec(&pair, &request), &owner)
                .unwrap()
            else {
                panic!("first admission did not bind a root");
            };
            assert!(canonical_uuid(&root_id));
            // The reply is intentionally dropped when this process exits.
        } else {
            assert_eq!(action, "wrong_owner");
            assert!(ledger.status(&request, &pair, &peer()).is_err());
            assert!(
                ledger
                    .reserve_request(&spec(&pair, &request), &peer())
                    .is_err()
            );
        }
    }

    fn run_child(action: &str, state: &Path, pair: &str, source: &str, request: &str) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "installed_launch_ledger::tests::ledger_child"])
            .env("AGE319_LEDGER_CHILD_ACTION", action)
            .env("AGE319_LEDGER_STATE", state)
            .env("AGE319_LEDGER_PAIR", pair)
            .env("AGE319_LEDGER_SOURCE", source)
            .env("AGE319_LEDGER_REQUEST", request)
            .env("AGE319_LEDGER_PARENT_PID", std::process::id().to_string())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }

    #[test]
    fn lost_reply_reopens_pending_without_relaunch_and_refuses_wrong_identity() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        // A separate process publishes the admission and exits without a
        // reply. This process then reopens the same durable Broker storage.
        run_child("reserve", state.path(), &pair, &source, &request);
        let restarted = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        let bound = restarted.read(&request).unwrap();
        assert_eq!(bound.protocol, ROOT_BOUND_PROTOCOL);
        let root_id = bound.root_id.unwrap();
        assert!(canonical_uuid(&root_id));
        restarted.verify_root_binding(&request, &root_id).unwrap();
        assert!(
            restarted
                .verify_root_binding(&request, &uuid::Uuid::new_v4().to_string())
                .is_err()
        );
        assert_eq!(
            restarted.status(&request, &pair, &peer()).unwrap(),
            format!("pending {request} {pair}\n")
        );
        assert!(matches!(
            restarted
                .reserve_request(&spec(&pair, &request), &peer())
                .unwrap(),
            Admission::Existing
        ));
        assert_eq!(
            restarted.read(&request).unwrap().root_id.as_deref(),
            Some(root_id.as_str())
        );
        let second_request = uuid::Uuid::new_v4().to_string();
        let Admission::New {
            root_id: second_root,
        } = restarted
            .reserve_request(&spec(&pair, &second_request), &peer())
            .unwrap()
        else {
            panic!("second request did not bind a root");
        };
        assert_ne!(root_id, second_root);
        let mut changed = spec(&pair, &request);
        changed.args = vec![b"diagnostics".to_vec()];
        assert!(restarted.reserve_request(&changed, &peer()).is_err());
        assert!(
            restarted
                .status(&request, &uuid::Uuid::new_v4().to_string(), &peer())
                .is_err()
        );
        let mut other = peer();
        other.uid = other.uid.wrapping_add(1);
        assert!(restarted.status(&request, &pair, &other).is_err());
        assert!(
            restarted
                .reserve_request(&spec(&pair, &request), &other)
                .is_err()
        );
        run_child("wrong_owner", state.path(), &pair, &source, &request);
        assert!(
            InstalledLaunchLedger::open(state.path(), &uuid::Uuid::new_v4().to_string(), &source)
                .is_err()
        );
        assert!(
            InstalledLaunchLedger::open(state.path(), &pair, &uuid::Uuid::new_v4().to_string())
                .is_err()
        );
    }

    #[test]
    fn request_only_record_stays_pending_and_cannot_be_upgraded_by_retry() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        let ledger = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        let mut old = RequestRecord {
            protocol: REQUEST_ONLY_PROTOCOL.into(),
            request_id: request.clone(),
            pair_generation: pair.clone(),
            source_generation: source.clone(),
            owner_uid: peer().uid,
            launcher: ProcessStamp::from(&peer().process),
            spec_sha256: format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&spec(&pair, &request)).unwrap())
            ),
            root_id: None,
        };
        json_artifact::create_new(&ledger.directory, &format!("{request}.json"), &old).unwrap();
        let restarted = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        assert_eq!(
            restarted.status(&request, &pair, &peer()).unwrap(),
            format!("pending {request} {pair}\n")
        );
        assert!(
            restarted
                .reserve_request(&spec(&pair, &request), &peer())
                .is_err()
        );
        old.root_id = Some(uuid::Uuid::new_v4().to_string());
        fs::write(
            ledger.directory.join(format!("{request}.json")),
            serde_json::to_vec(&old).unwrap(),
        )
        .unwrap();
        assert!(InstalledLaunchLedger::open(state.path(), &pair, &source).is_err());
    }

    #[test]
    fn restart_refuses_two_requests_claiming_one_root() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let ledger = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        assert!(matches!(
            ledger
                .reserve_request(&spec(&pair, &first), &peer())
                .unwrap(),
            Admission::New { .. }
        ));
        assert!(matches!(
            ledger
                .reserve_request(&spec(&pair, &second), &peer())
                .unwrap(),
            Admission::New { .. }
        ));
        let mut altered = ledger.read(&second).unwrap();
        altered.root_id = ledger.read(&first).unwrap().root_id;
        fs::write(
            ledger.directory.join(format!("{second}.json")),
            serde_json::to_vec(&altered).unwrap(),
        )
        .unwrap();
        assert!(InstalledLaunchLedger::open(state.path(), &pair, &source).is_err());
    }

    #[test]
    fn control_wait_readback_is_bound_but_never_promotes_pending_to_exit() {
        let state = tempfile::tempdir().unwrap();
        let pair = uuid::Uuid::new_v4().to_string();
        let source = uuid::Uuid::new_v4().to_string();
        let request = uuid::Uuid::new_v4().to_string();
        let ledger = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        let Admission::New { root_id } = ledger
            .reserve_request(&spec(&pair, &request), &peer())
            .unwrap()
        else {
            panic!("request was not new");
        };
        assert!(ledger.read_control_exit(&request).unwrap().is_none());
        let launcher = ProcessStamp::from(&peer().process);
        let mut control = launcher.clone();
        control.host_pid += 1;
        let exit = ControlExitRecord {
            protocol: CONTROL_EXIT_PROTOCOL.into(),
            request_id: request.clone(),
            pair_generation: pair.clone(),
            source_generation: source.clone(),
            root_id,
            launcher,
            control,
            e_consumed: true,
            code: Some(0),
            signal: None,
        };
        json_artifact::create_new(&ledger.exit_directory, &format!("{request}.json"), &exit)
            .unwrap();
        let restarted = InstalledLaunchLedger::open(state.path(), &pair, &source).unwrap();
        assert_eq!(
            restarted.read_control_exit(&request).unwrap(),
            Some(exit.clone())
        );
        assert_eq!(
            restarted.status(&request, &pair, &peer()).unwrap(),
            format!("pending {request} {pair}\n")
        );
        let path = ledger.exit_directory.join(format!("{request}.json"));
        let mut changed = exit.clone();
        changed.root_id = uuid::Uuid::new_v4().to_string();
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(InstalledLaunchLedger::open(state.path(), &pair, &source).is_err());
        fs::write(&path, serde_json::to_vec(&exit).unwrap()).unwrap();
        changed = exit.clone();
        changed.code = None;
        changed.signal = Some(9);
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(InstalledLaunchLedger::open(state.path(), &pair, &source).is_ok());
        changed.signal = Some(0);
        fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(InstalledLaunchLedger::open(state.path(), &pair, &source).is_err());
    }
}
