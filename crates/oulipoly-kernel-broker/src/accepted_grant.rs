//! Durable, one-use accepted-work preparation. This has deliberately no
//! socket operation or worker launch: the host guardian must transfer its
//! accepted descriptors into a gated nested launch before this may authorize
//! execution. A record here is debt, never proof of physical drain.
use crate::entry_registry::{EntryRegistry, ProcessStamp};
use crate::identity::{PeerIdentity, PinnedProcess};
use crate::registry::RootRegistry;
use crate::work_registry::WorkRegistry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const ACCEPTED: &str = "root-work-accepted-v1.json";
const INTENT: &str = "root-work-intent-v1.json";
const MAX_ARTIFACT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub pid: i64,
    pub boot_id: String,
    pub starttime_ticks: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Registration {
    Root,
    Nested {
        parent_work_id: String,
        parent_capability_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Acceptance {
    pub protocol: String,
    pub work_id: String,
    pub request_sha256: String,
    pub root_id: String,
    pub supervisor_authority_id: String,
    pub owner_generation: String,
    pub initiator: SourceIdentity,
    pub registration: Registration,
    pub cancel_capability_sha256: String,
}

#[derive(Deserialize)]
struct IntentIdentity {
    protocol: String,
    work_id: String,
    root_id: String,
    handle: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantRecord {
    pub version: u32,
    pub grant_id: String,
    pub root_id: String,
    pub root_init: ProcessStamp,
    pub owner_uid: u32,
    pub supervisor_authority_id: String,
    pub owner_generation: String,
    pub guardian: ProcessStamp,
    pub work_id: String,
    pub parent_work_incarnation: Option<String>,
    pub accepted_sha256: String,
    pub request_sha256: String,
    pub initiator: SourceIdentity,
    /// Fsynced before any future namespace fork. Never reset on timeout.
    pub consumed: bool,
}

#[derive(Debug)]
pub struct GrantRegistry {
    directory: PathBuf,
    records: Vec<GrantRecord>,
    poisoned: bool,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn read_bounded(file: &File) -> io::Result<Vec<u8>> {
    let size = file.metadata()?.len();
    if size > MAX_ARTIFACT {
        return Err(io::Error::other("accepted-work artifact too large"));
    }
    let mut bytes = vec![0; size as usize];
    file.read_exact_at(&mut bytes, 0)?;
    Ok(bytes)
}

fn same_file_in_directory(directory: &File, name: &str, file: &File) -> io::Result<bool> {
    let name = std::ffi::CString::new(name).expect("constant artifact name");
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            std::os::fd::AsRawFd::as_raw_fd(directory),
            name.as_ptr(),
            &mut stat,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let metadata = file.metadata()?;
    Ok(metadata.is_file()
        && metadata.dev() == stat.st_dev
        && metadata.ino() == stat.st_ino
        && stat.st_mode & libc::S_IFMT == libc::S_IFREG)
}

#[expect(
    clippy::too_many_arguments,
    reason = "all acceptance bindings must be explicit"
)]
fn validate_artifacts(
    accepted: &[u8],
    intent: &[u8],
    root_id: &str,
    work_id: &str,
    request_sha256: &str,
    accepted_sha256: &str,
    owner_generation: &str,
    supervisor_authority_id: &str,
) -> io::Result<Acceptance> {
    if !valid_digest(request_sha256)
        || !valid_digest(accepted_sha256)
        || digest(accepted) != accepted_sha256
        || digest(intent) != request_sha256
        || uuid::Uuid::parse_str(root_id).is_err()
        || uuid::Uuid::parse_str(owner_generation).is_err()
        || uuid::Uuid::parse_str(supervisor_authority_id).is_err()
        || work_id.is_empty()
        || work_id.len() > 256
        || work_id.contains('\0')
    {
        return Err(io::Error::other("invalid accepted-work request binding"));
    }
    let receipt: Acceptance = serde_json::from_slice(accepted)?;
    let identity: IntentIdentity = serde_json::from_slice(intent)?;
    if receipt.protocol != "original-work-v1"
        || identity.protocol != receipt.protocol
        || receipt.work_id != work_id
        || identity.work_id != work_id
        || identity.handle != work_id
        || receipt.root_id != root_id
        || identity.root_id != root_id
        || receipt.request_sha256 != request_sha256
        || receipt.owner_generation != owner_generation
        || receipt.supervisor_authority_id != supervisor_authority_id
        || !valid_digest(&receipt.cancel_capability_sha256)
        || receipt.initiator.pid <= 0
        || receipt.initiator.starttime_ticks <= 0
        || receipt.initiator.boot_id.is_empty()
        || matches!(&receipt.registration, Registration::Nested { parent_work_id, parent_capability_sha256 }
            if parent_work_id.is_empty() || !valid_digest(parent_capability_sha256))
    {
        return Err(io::Error::other("accepted-work receipt or intent mismatch"));
    }
    Ok(receipt)
}

impl GrantRegistry {
    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let mut records = Vec::new();
        let mut ids = HashSet::new();
        let mut works = HashSet::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type()?.is_file() || !name.ends_with(".json") {
                return Err(io::Error::other("unrecognized accepted grant entry"));
            }
            let record: GrantRecord = serde_json::from_slice(&fs::read(entry.path())?)?;
            if record.version != 1
                || uuid::Uuid::parse_str(&record.grant_id).is_err()
                || format!("{}.json", record.grant_id) != name
                || uuid::Uuid::parse_str(&record.root_id).is_err()
                || uuid::Uuid::parse_str(&record.owner_generation).is_err()
                || uuid::Uuid::parse_str(&record.supervisor_authority_id).is_err()
                || !valid_digest(&record.accepted_sha256)
                || !valid_digest(&record.request_sha256)
                || record.work_id.is_empty()
                || record.work_id.len() > 256
                || record.work_id.contains('\0')
                || record.initiator.pid <= 0
                || record.initiator.starttime_ticks <= 0
                || record.initiator.boot_id.is_empty()
                || record
                    .parent_work_incarnation
                    .as_ref()
                    .is_some_and(|parent| uuid::Uuid::parse_str(parent).is_err())
                || !ids.insert(record.grant_id.clone())
                || !works.insert((record.root_id.clone(), record.work_id.clone()))
            {
                return Err(io::Error::other("invalid or duplicate accepted grant"));
            }
            records.push(record);
        }
        Ok(Self {
            directory,
            records,
            poisoned: false,
        })
    }

    pub fn records(&self) -> &[GrantRecord] {
        &self.records
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned
    }

    /// The caller must be the live, exact host guardian already bound to this
    /// root. The positive receipt and intent are read through pinned descriptors
    /// from the guardian's accepted state directory. The caller supplies the
    /// digest of its in-memory positive receipt so a mutable file cannot alter
    /// registration or initiator identity after acceptance. This prepares debt only;
    /// no worker can execute through this method.
    #[expect(
        clippy::too_many_arguments,
        reason = "all acceptance bindings must be explicit"
    )]
    pub fn prepare(
        &mut self,
        roots: &RootRegistry,
        entries: &EntryRegistry,
        works: &WorkRegistry,
        caller: &PeerIdentity,
        host_namespace: &File,
        runner_image: &File,
        state_dir: &File,
        accepted: &File,
        intent: &File,
        root_id: &str,
        work_id: &str,
        request_sha256: &str,
        accepted_sha256: &str,
        owner_generation: &str,
    ) -> io::Result<GrantRecord> {
        if self.has_debt()
            || roots.has_debt()
            || entries.has_debt()
            || works.has_debt()
            || self
                .records
                .iter()
                .any(|record| record.root_id == root_id && record.work_id == work_id)
            || !caller.process.in_namespace(host_namespace)?
            || !caller.process.same_executable_as(runner_image)?
            || !state_dir.metadata()?.is_dir()
            || !same_file_in_directory(state_dir, ACCEPTED, accepted)?
            || !same_file_in_directory(state_dir, INTENT, intent)?
        {
            return Err(io::Error::other("accepted grant preparation denied"));
        }
        let entry = entries
            .record(root_id)
            .ok_or_else(|| io::Error::other("entry absent"))?;
        let root = roots
            .live_roots()
            .find(|root| root.record.root_id == root_id)
            .ok_or_else(|| io::Error::other("live root absent"))?;
        if !entry.join_consumed
            || entry.joined_child.is_none()
            || entry.guardian.as_ref() != Some(&ProcessStamp::from(&caller.process))
            || entry.owner_uid != caller.uid
            || root.record.owner_uid != caller.uid
            || entry.domain_id.is_none()
            || entry.supervisor_authority_id.is_none()
        {
            return Err(io::Error::other("caller is not bound host guardian"));
        }
        let joined = entry.joined_child.as_ref().unwrap();
        let joined_live = PinnedProcess::open(joined.host_pid)?;
        if ProcessStamp::from(&joined_live) != *joined
            || !joined_live.in_namespace(root.init.namespace())?
            || !joined_live.direct_child_of(&root.init)?
        {
            return Err(io::Error::other("joined owner is not live"));
        }
        let supervisor = entry.supervisor_authority_id.as_ref().unwrap();
        let accepted_bytes = read_bounded(accepted)?;
        let intent_bytes = read_bounded(intent)?;
        let receipt = validate_artifacts(
            &accepted_bytes,
            &intent_bytes,
            root_id,
            work_id,
            request_sha256,
            accepted_sha256,
            owner_generation,
            supervisor,
        )?;
        let parent = match &receipt.registration {
            Registration::Root => None,
            Registration::Nested { parent_work_id, .. } => {
                let parent = self
                    .records
                    .iter()
                    .find(|record| {
                        record.root_id == root_id
                            && record.work_id == *parent_work_id
                            && record.owner_generation == owner_generation
                            && record.consumed
                    })
                    .ok_or_else(|| io::Error::other("causal parent grant absent"))?;
                let parent_work = works
                    .live_works()
                    .find(|work| {
                        work.record.root_id == root_id
                            && work.record.work_id == *parent_work_id
                            && work.record.accepted_grant_id.as_deref()
                                == Some(parent.grant_id.as_str())
                            && work.init.verify().is_ok()
                    })
                    .ok_or_else(|| io::Error::other("causal parent namespace absent"))?;
                if parent.root_init != ProcessStamp::from(&root.init) {
                    return Err(io::Error::other("causal parent root changed"));
                }
                Some(parent_work.record.work_incarnation.clone())
            }
        };
        caller.process.verify()?;
        root.init.verify()?;
        joined_live.verify()?;
        let record = GrantRecord {
            version: 1,
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: root_id.to_owned(),
            root_init: ProcessStamp::from(&root.init),
            owner_uid: caller.uid,
            supervisor_authority_id: supervisor.clone(),
            owner_generation: owner_generation.to_owned(),
            guardian: ProcessStamp::from(&caller.process),
            work_id: work_id.to_owned(),
            parent_work_incarnation: parent,
            accepted_sha256: digest(&accepted_bytes),
            request_sha256: request_sha256.to_owned(),
            initiator: receipt.initiator,
            consumed: false,
        };
        self.create(record.clone())?;
        Ok(record)
    }

    fn create(&mut self, record: GrantRecord) -> io::Result<()> {
        let path = self.directory.join(format!("{}.json", record.grant_id));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
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
        self.records.push(record);
        Ok(())
    }

    /// Check the original host guardian and live root again immediately before
    /// the irreversible pre-fork transition. A lost response or failed fork
    /// retains the consumed record and can never replay this grant.
    pub fn consume(
        &mut self,
        grant_id: &str,
        roots: &RootRegistry,
        entries: &EntryRegistry,
        caller: &PeerIdentity,
        host_namespace: &File,
        runner_image: &File,
    ) -> io::Result<GrantRecord> {
        if roots.has_debt()
            || entries.has_debt()
            || !caller.process.in_namespace(host_namespace)?
            || !caller.process.same_executable_as(runner_image)?
        {
            return Err(io::Error::other("accepted grant launch authority changed"));
        }
        let record = self
            .records
            .iter()
            .find(|record| record.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("unknown grant"))?;
        let entry = entries
            .record(&record.root_id)
            .ok_or_else(|| io::Error::other("bound entry absent"))?;
        let root = roots
            .live_roots()
            .find(|root| root.record.root_id == record.root_id)
            .ok_or_else(|| io::Error::other("live root absent"))?;
        if entry.guardian.as_ref() != Some(&ProcessStamp::from(&caller.process))
            || entry.owner_uid != caller.uid
            || record.owner_uid != caller.uid
            || !entry.join_consumed
            || entry.supervisor_authority_id.as_deref()
                != Some(record.supervisor_authority_id.as_str())
            || record.root_init != ProcessStamp::from(&root.init)
            || record.guardian != ProcessStamp::from(&caller.process)
        {
            return Err(io::Error::other("accepted grant binding changed"));
        }
        let joined = entry
            .joined_child
            .as_ref()
            .ok_or_else(|| io::Error::other("joined owner absent"))?;
        let joined_live = PinnedProcess::open(joined.host_pid)?;
        if *joined != ProcessStamp::from(&joined_live)
            || !joined_live.in_namespace(root.init.namespace())?
            || !joined_live.direct_child_of(&root.init)?
        {
            return Err(io::Error::other(
                "joined owner changed before grant consumption",
            ));
        }
        root.init.verify()?;
        joined_live.verify()?;
        self.consume_record(grant_id, &caller.process)
    }

    fn consume_record(
        &mut self,
        grant_id: &str,
        guardian: &PinnedProcess,
    ) -> io::Result<GrantRecord> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain grant registry"));
        }
        let index = self
            .records
            .iter()
            .position(|record| record.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("unknown grant"))?;
        let current = &self.records[index];
        if current.consumed || current.guardian != ProcessStamp::from(guardian) {
            return Err(io::Error::other("spent grant or guardian changed"));
        }
        guardian.verify()?;
        let mut consumed = current.clone();
        consumed.consumed = true;
        let path = self.directory.join(format!("{grant_id}.json"));
        let temp = self.directory.join(format!("{grant_id}.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(temp)?;
            serde_json::to_writer(&mut file, &consumed)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(self.directory.join(format!("{grant_id}.tmp")), path)?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.records[index] = consumed.clone();
        Ok(consumed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;

    fn evidence(nested: bool) -> (Vec<u8>, Vec<u8>, String, String, String) {
        let root = uuid::Uuid::new_v4().to_string();
        let supervisor = uuid::Uuid::new_v4().to_string();
        let generation = uuid::Uuid::new_v4().to_string();
        let intent = serde_json::to_vec(&json!({
            "protocol": "original-work-v1", "work_id": "work-a",
            "root_id": root, "handle": "work-a", "meta": {"cwd": "/tmp"}
        }))
        .unwrap();
        let accepted = serde_json::to_vec(&json!({
            "protocol": "original-work-v1", "work_id": "work-a",
            "request_sha256": digest(&intent), "root_id": root,
            "supervisor_authority_id": supervisor, "owner_generation": generation,
            "initiator": {"pid": 42, "boot_id": "boot", "starttime_ticks": 7},
            "registration": if nested { json!({"kind": "nested", "parent_work_id": "work-p", "parent_capability_sha256": digest(b"parent")}) }
                else { json!({"kind": "root"}) },
            "cancel_capability_sha256": digest(b"cancel")
        })).unwrap();
        (accepted, intent, root, supervisor, generation)
    }

    #[test]
    fn positive_receipt_binds_exact_intent_owner_and_parent_claim() {
        for nested in [false, true] {
            let (accepted, intent, root, supervisor, generation) = evidence(nested);
            let request = digest(&intent);
            let accepted_hash = digest(&accepted);
            let check = |root_id: &str,
                         work_id: &str,
                         request_hash: &str,
                         receipt_hash: &str,
                         owner: &str,
                         authority: &str| {
                validate_artifacts(
                    &accepted,
                    &intent,
                    root_id,
                    work_id,
                    request_hash,
                    receipt_hash,
                    owner,
                    authority,
                )
            };
            let receipt = check(
                &root,
                "work-a",
                &request,
                &accepted_hash,
                &generation,
                &supervisor,
            )
            .unwrap();
            assert_eq!(
                matches!(receipt.registration, Registration::Nested { .. }),
                nested
            );
            assert!(
                check(
                    "wrong",
                    "work-a",
                    &request,
                    &accepted_hash,
                    &generation,
                    &supervisor
                )
                .is_err()
            );
            assert!(
                check(
                    &root,
                    "work-b",
                    &request,
                    &accepted_hash,
                    &generation,
                    &supervisor
                )
                .is_err()
            );
            assert!(
                check(
                    &root,
                    "work-a",
                    &digest(b"other"),
                    &accepted_hash,
                    &generation,
                    &supervisor
                )
                .is_err()
            );
            assert!(
                check(
                    &root,
                    "work-a",
                    &request,
                    &digest(b"altered receipt"),
                    &generation,
                    &supervisor
                )
                .is_err()
            );
            assert!(
                check(
                    &root,
                    "work-a",
                    &request,
                    &accepted_hash,
                    &uuid::Uuid::new_v4().to_string(),
                    &supervisor
                )
                .is_err()
            );
            assert!(
                check(
                    &root,
                    "work-a",
                    &request,
                    &accepted_hash,
                    &generation,
                    &uuid::Uuid::new_v4().to_string()
                )
                .is_err()
            );
        }
    }

    fn record(guardian: &PinnedProcess) -> GrantRecord {
        GrantRecord {
            version: 1,
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            root_init: ProcessStamp::from(guardian),
            owner_uid: unsafe { libc::getuid() },
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian: ProcessStamp::from(guardian),
            work_id: "accepted-a".into(),
            parent_work_incarnation: None,
            accepted_sha256: digest(b"accepted"),
            request_sha256: digest(b"intent"),
            initiator: SourceIdentity {
                pid: 42,
                boot_id: "boot".into(),
                starttime_ticks: 7,
            },
            consumed: false,
        }
    }

    #[test]
    fn consumed_grant_survives_restart_and_cannot_replay() {
        let dir = tempfile::tempdir().unwrap();
        let guardian = PinnedProcess::open(std::process::id() as i32).unwrap();
        let mut grants = GrantRegistry::open(dir.path()).unwrap();
        let record = record(&guardian);
        grants.create(record.clone()).unwrap();
        let mut restarted = GrantRegistry::open(dir.path()).unwrap();
        assert!(!restarted.records()[0].consumed);
        let consumed = restarted
            .consume_record(&record.grant_id, &guardian)
            .unwrap();
        assert!(consumed.consumed);
        let mut restarted = GrantRegistry::open(dir.path()).unwrap();
        assert!(restarted.records()[0].consumed);
        assert!(
            restarted
                .consume_record(&record.grant_id, &guardian)
                .is_err()
        );
        assert!(restarted.create(record).is_err());
        assert!(restarted.has_debt());
    }

    #[test]
    fn malformed_or_duplicate_grant_file_stops_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let guardian = PinnedProcess::open(std::process::id() as i32).unwrap();
        let record = record(&guardian);
        let mut grants = GrantRegistry::open(dir.path()).unwrap();
        grants.create(record.clone()).unwrap();
        let duplicate = dir.path().join(format!("{}.json", uuid::Uuid::new_v4()));
        fs::write(duplicate, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(GrantRegistry::open(dir.path()).is_err());
    }

    #[test]
    fn artifact_descriptor_must_name_exact_nonsymlink_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let state = File::open(dir.path()).unwrap();
        let accepted_path = dir.path().join(ACCEPTED);
        fs::write(&accepted_path, b"accepted").unwrap();
        let accepted = File::open(&accepted_path).unwrap();
        assert!(same_file_in_directory(&state, ACCEPTED, &accepted).unwrap());
        let other = dir.path().join("other");
        fs::write(&other, b"accepted").unwrap();
        assert!(!same_file_in_directory(&state, ACCEPTED, &File::open(other).unwrap()).unwrap());
        fs::remove_file(&accepted_path).unwrap();
        symlink("other", &accepted_path).unwrap();
        assert!(!same_file_in_directory(&state, ACCEPTED, &accepted).unwrap());
    }
}
