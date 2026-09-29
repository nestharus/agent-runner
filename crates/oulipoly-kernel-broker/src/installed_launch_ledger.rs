//! Durable first-install request/root binding. Publication precedes any future
//! root effect. A published request is never launchable again by resubmission.
use crate::entry_registry::ProcessStamp;
use crate::identity::PeerIdentity;
use crate::installed_launch::InstalledLaunchSpec;
use crate::json_artifact;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

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

pub enum Admission {
    New { root_id: String },
    Existing,
}

pub struct InstalledLaunchLedger {
    directory: PathBuf,
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
        if !canonical_uuid(pair_generation) || !canonical_uuid(source_generation) {
            return Err(io::Error::other("invalid installed launch generation"));
        }
        let directory = state.join("installed-launches");
        if !directory.exists() {
            fs::DirBuilder::new().mode(0o700).create(&directory)?;
            File::open(state)?.sync_all()?;
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
        let ledger = Self {
            directory,
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

    fn read(&self, request_id: &str) -> io::Result<RequestRecord> {
        if !canonical_uuid(request_id) {
            return Err(io::Error::other("invalid installed launch request ID"));
        }
        self.read_path(&self.directory.join(format!("{request_id}.json")))
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
}
