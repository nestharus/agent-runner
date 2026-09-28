//! Durable first-install request admission. Publication precedes any future
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

const PROTOCOL: &str = "installed-launch-request-v1";

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
}

pub enum Admission {
    New,
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
        for entry in fs::read_dir(&ledger.directory)? {
            let entry = entry?;
            let record = ledger.read_path(&entry.path())?;
            if entry.file_name().to_string_lossy() != format!("{}.json", record.request_id) {
                return Err(io::Error::other(
                    "installed launch ledger filename mismatch",
                ));
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
        if record.protocol != PROTOCOL
            || !canonical_uuid(&record.request_id)
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
            protocol: PROTOCOL.into(),
            request_id: spec.request_id.clone(),
            pair_generation: self.pair_generation.clone(),
            source_generation: self.source_generation.clone(),
            owner_uid: peer.uid,
            launcher: ProcessStamp::from(&peer.process),
            spec_sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(spec)?)),
        };
        let name = format!("{}.json", record.request_id);
        match json_artifact::create_new(&self.directory, &name, &record) {
            Ok(()) => Ok(Admission::New),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if self.read(&record.request_id)? != record {
                    return Err(io::Error::other(
                        "installed launch duplicate identity mismatch",
                    ));
                }
                Ok(Admission::Existing)
            }
            Err(error) => Err(error),
        }
    }

    /// An authenticated observation of the durable request only. No root or
    /// terminal transition exists yet, so every present record is pending.
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
            assert!(matches!(
                ledger
                    .reserve_request(&spec(&pair, &request), &owner)
                    .unwrap(),
                Admission::New
            ));
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
}
