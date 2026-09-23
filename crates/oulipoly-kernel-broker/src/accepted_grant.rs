//! Durable, one-use accepted-work preparation. The host guardian can submit
//! its exact positive receipt. A consumed record is durable launch debt, never
//! by itself proof of execution or physical drain.
use crate::entry_registry::{EntryRegistry, ProcessStamp};
use crate::identity::{PeerIdentity, PinnedProcess, host_proc_file};
use crate::native_receipt::{BoundNativeAuthority, verify as verify_native_receipt};
use crate::registry::RootRegistry;
use crate::work_registry::WorkRegistry;
use oulipoly_state::mailbox::{MailboxDb, NativeGrantBinding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
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
    state_root: PathBuf,
    meta: IntentMeta,
    #[serde(default)]
    registration_authority: Option<Vec<u8>>,
}

#[derive(Deserialize)]
struct IntentMeta {
    cwd: PathBuf,
    #[serde(default)]
    owner_session_id: Option<String>,
    #[serde(default)]
    owner_invocation_uuid: Option<String>,
    #[serde(default)]
    delivery_helper: Option<HelperProvenance>,
}

#[derive(Clone, Deserialize)]
struct HelperProvenance {
    path: PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    sha256: String,
    interpreter: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SealedHelper {
    pub image: FileStamp,
    pub sha256: String,
    pub owner_session_id: String,
    pub owner_invocation_uuid: String,
    pub registration_authority_sha256: String,
}

impl SealedHelper {
    pub fn matches_live_executable(&self, process: &PinnedProcess) -> io::Result<bool> {
        process.verify()?;
        let executable = host_proc_file(&format!("{}/exe", process.host_pid))?;
        let matches = matches_pinned_image(&executable, &self.image, &self.sha256)?;
        process.verify()?;
        Ok(matches)
    }
}

fn matches_pinned_image(file: &File, pinned: &FileStamp, sha256: &str) -> io::Result<bool> {
    let stamp = FileStamp::of(file)?;
    // Bash executes its handle-bound helper from a sealed memfd. Its inode
    // differs from the on-disk H snapshot, but the executable must still
    // have those exact bytes and be unable to change after V.
    if stamp != *pinned && !sealed_executable(file)? {
        return Ok(false);
    }
    Ok(image_digest(file)? == sha256)
}

fn sealed_executable(file: &File) -> io::Result<bool> {
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    if seals < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EINVAL) {
            return Ok(false);
        }
        return Err(error);
    }
    const REQUIRED: i32 =
        libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    Ok(seals & REQUIRED == REQUIRED)
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
    /// The original broker-attested Runner join remains an exact historical
    /// owner binding even if that child exits before long-lived nested work.
    pub joined_child: ProcessStamp,
    pub work_id: String,
    pub parent_grant_id: Option<String>,
    pub parent_work_incarnation: Option<String>,
    pub accepted_sha256: String,
    pub request_sha256: String,
    pub initiator: SourceIdentity,
    pub artifacts: GrantArtifacts,
    /// Present only for a v3 grant whose accepted intent pinned the exact
    /// native Runner helper image and owner session before K was consumed.
    #[serde(default)]
    pub sealed_helper: Option<SealedHelper>,
    /// Fsynced before a K namespace fork. Never reset on timeout.
    pub consumed: bool,
}

/// A native continuation is a separate protocol from original-work H/K.
/// Version 4 can be spent once. A spent record is custody debt, not proof of
/// worker creation, attach, gate release, or Q.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeGrantRecord {
    pub version: u32,
    pub kind: String,
    pub grant_id: String,
    pub attempt_id: String,
    pub root_id: String,
    pub root_init: ProcessStamp,
    pub domain_id: String,
    pub supervisor_authority_id: String,
    pub owner_generation: String,
    pub owner_uid: u32,
    pub guardian: ProcessStamp,
    pub joined_child: ProcessStamp,
    pub receipt_sha256: String,
    pub accepted_snapshot_sha256: String,
    pub custodian_request_sha256: String,
    pub directory: FileStamp,
    pub request: FileStamp,
    pub receipt: FileStamp,
    pub request_byte_len: u64,
    pub receipt_byte_len: u64,
    pub state: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileStamp {
    pub device: u64,
    pub inode: u64,
}

impl FileStamp {
    fn of(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GrantArtifacts {
    pub executable: FileStamp,
    pub intent: FileStamp,
    pub cwd: FileStamp,
    pub state_dir: FileStamp,
    pub accepted: FileStamp,
}

impl GrantArtifacts {
    fn pinned(
        executable: &File,
        intent: &File,
        cwd: &File,
        state_dir: &File,
        accepted: &File,
    ) -> io::Result<Self> {
        Ok(Self {
            executable: FileStamp::of(executable)?,
            intent: FileStamp::of(intent)?,
            cwd: FileStamp::of(cwd)?,
            state_dir: FileStamp::of(state_dir)?,
            accepted: FileStamp::of(accepted)?,
        })
    }

    fn valid(&self) -> bool {
        [
            &self.executable,
            &self.intent,
            &self.cwd,
            &self.state_dir,
            &self.accepted,
        ]
        .iter()
        .all(|stamp| stamp.inode != 0)
    }
}

#[derive(Debug)]
pub struct GrantRegistry {
    directory: PathBuf,
    records: Vec<GrantRecord>,
    native_records: Vec<NativeGrantRecord>,
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

fn image_digest(file: &File) -> io::Result<String> {
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    let mut offset = 0;
    loop {
        let count = file.read_at(&mut bytes, offset)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
        offset += count as u64;
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn sealed_helper(
    state_dir: &File,
    intent: &IntentIdentity,
    runner_image: &File,
) -> io::Result<Option<SealedHelper>> {
    let meta = &intent.meta;
    // Standalone original work can include a delivery-helper snapshot, but
    // carries no native owner permission. Once any native owner field is
    // present, H must pin the complete set before K can launch the work.
    if meta.owner_session_id.is_none()
        && meta.owner_invocation_uuid.is_none()
        && intent.registration_authority.is_none()
    {
        return Ok(None);
    }
    let (Some(provenance), Some(session), Some(invocation), Some(authority)) = (
        &meta.delivery_helper,
        &meta.owner_session_id,
        &meta.owner_invocation_uuid,
        &intent.registration_authority,
    ) else {
        return Err(io::Error::other("incomplete sealed helper owner binding"));
    };
    if session.is_empty()
        || uuid::Uuid::parse_str(invocation).is_err()
        || provenance.interpreter.is_some()
        || !valid_digest(&provenance.sha256)
        || fs::canonicalize(&provenance.path)?
            != fs::canonicalize(
                intent
                    .state_root
                    .join(&intent.handle)
                    .join("delivery-helper"),
            )?
    {
        return Err(io::Error::other("invalid sealed helper provenance"));
    }
    if authority.len() != 64
        || !authority
            .iter()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(io::Error::other(
            "invalid sealed helper registration authority",
        ));
    }
    let name = std::ffi::CString::new("delivery-helper").unwrap();
    let fd = unsafe {
        libc::openat(
            state_dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let image = unsafe { File::from_raw_fd(fd) };
    let meta = image.metadata()?;
    if !meta.is_file()
        || meta.dev() != provenance.device
        || meta.ino() != provenance.inode
        || meta.len() != provenance.size
        || image_digest(&image)? != provenance.sha256
        || image_digest(runner_image)? != provenance.sha256
    {
        return Err(io::Error::other(
            "sealed helper image differs from accepted Runner",
        ));
    }
    Ok(Some(SealedHelper {
        image: FileStamp::of(&image)?,
        sha256: provenance.sha256.clone(),
        owner_session_id: session.clone(),
        owner_invocation_uuid: invocation.clone(),
        registration_authority_sha256: digest(authority),
    }))
}

fn valid_stamp(stamp: &ProcessStamp) -> bool {
    stamp.host_pid > 0
        && !stamp.boot_id.is_empty()
        && stamp.starttime_ticks > 0
        && stamp.pidns_ino > 0
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
    /// Revalidate every descriptor immediately before consuming a prepared H
    /// grant. A caller cannot switch the accepted executable or receipt between
    /// preparation and the irreversible launch transition.
    pub fn validate_launch_artifacts(
        &self,
        grant_id: &str,
        executable: &File,
        intent: &File,
        cwd: &File,
        state_dir: &File,
        accepted: &File,
    ) -> io::Result<GrantRecord> {
        let record = self
            .records
            .iter()
            .find(|record| record.grant_id == grant_id && !record.consumed)
            .ok_or_else(|| io::Error::other("unavailable one-use accepted grant"))?;
        if record.artifacts.executable != FileStamp::of(executable)?
            || record.artifacts.intent != FileStamp::of(intent)?
            || record.artifacts.cwd != FileStamp::of(cwd)?
            || record.artifacts.state_dir != FileStamp::of(state_dir)?
            || record.artifacts.accepted != FileStamp::of(accepted)?
            || !executable.metadata()?.is_file()
            || !cwd.metadata()?.is_dir()
            || !state_dir.metadata()?.is_dir()
            || !same_file_in_directory(state_dir, ACCEPTED, accepted)?
            || !same_file_in_directory(state_dir, INTENT, intent)?
        {
            return Err(io::Error::other(
                "accepted launch descriptor identity changed",
            ));
        }
        let accepted_bytes = read_bounded(accepted)?;
        let intent_bytes = read_bounded(intent)?;
        let receipt = validate_artifacts(
            &accepted_bytes,
            &intent_bytes,
            &record.root_id,
            &record.work_id,
            &record.request_sha256,
            &record.accepted_sha256,
            &record.owner_generation,
            &record.supervisor_authority_id,
        )?;
        if receipt.initiator != record.initiator {
            return Err(io::Error::other("accepted source identity changed"));
        }
        Ok(record.clone())
    }

    pub fn open(directory: impl AsRef<Path>) -> io::Result<Self> {
        let directory = directory.as_ref().to_path_buf();
        let mut records = Vec::new();
        let mut native_records = Vec::new();
        let mut ids = HashSet::new();
        let mut works = HashSet::new();
        let mut native_attempts = HashSet::new();
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry.file_type()?.is_file() || !name.ends_with(".json") {
                return Err(io::Error::other("unrecognized accepted grant entry"));
            }
            let bytes = fs::read(entry.path())?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            let version = value.get("version").and_then(|v| v.as_u64());
            if version == Some(4) {
                let record: NativeGrantRecord = serde_json::from_slice(&bytes)?;
                if record.kind != "native-continuation-v1"
                    || !matches!(record.state.as_str(), "prepared" | "consumed")
                    || uuid::Uuid::parse_str(&record.grant_id).is_err()
                    || format!("{}.json", record.grant_id) != name
                    || uuid::Uuid::parse_str(&record.attempt_id).is_err()
                    || uuid::Uuid::parse_str(&record.root_id).is_err()
                    || uuid::Uuid::parse_str(&record.domain_id).is_err()
                    || uuid::Uuid::parse_str(&record.supervisor_authority_id).is_err()
                    || uuid::Uuid::parse_str(&record.owner_generation).is_err()
                    || !valid_stamp(&record.root_init)
                    || !valid_stamp(&record.guardian)
                    || !valid_stamp(&record.joined_child)
                    || !valid_digest(&record.receipt_sha256)
                    || !valid_digest(&record.accepted_snapshot_sha256)
                    || !valid_digest(&record.custodian_request_sha256)
                    || record.directory.inode == 0
                    || record.request.inode == 0
                    || record.receipt.inode == 0
                    || record.request_byte_len > 4 * 1024 * 1024
                    || record.receipt_byte_len > 1024 * 1024
                    || !ids.insert(record.grant_id.clone())
                    || !native_attempts.insert(record.attempt_id.clone())
                    || !works.insert((record.root_id.clone(), record.attempt_id.clone()))
                {
                    return Err(io::Error::other("invalid or duplicate native grant"));
                }
                native_records.push(record);
                continue;
            }
            if !matches!(version, Some(2 | 3)) {
                return Err(io::Error::other("unsupported accepted grant version"));
            }
            let record: GrantRecord = serde_json::from_slice(&bytes)?;
            if !matches!(record.version, 2 | 3)
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
                || !valid_stamp(&record.root_init)
                || !valid_stamp(&record.guardian)
                || !valid_stamp(&record.joined_child)
                || !record.artifacts.valid()
                || (record.version == 3) != record.sealed_helper.is_some()
                || record.sealed_helper.as_ref().is_some_and(|helper| {
                    helper.image.inode == 0
                        || !valid_digest(&helper.sha256)
                        || helper.owner_session_id.is_empty()
                        || uuid::Uuid::parse_str(&helper.owner_invocation_uuid).is_err()
                        || !valid_digest(&helper.registration_authority_sha256)
                })
                || record
                    .parent_work_incarnation
                    .as_ref()
                    .is_some_and(|parent| uuid::Uuid::parse_str(parent).is_err())
                || record
                    .parent_grant_id
                    .as_ref()
                    .is_some_and(|parent| uuid::Uuid::parse_str(parent).is_err())
                || record.parent_grant_id.is_some() != record.parent_work_incarnation.is_some()
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
            native_records,
            poisoned: false,
        })
    }

    pub fn records(&self) -> &[GrantRecord] {
        &self.records
    }

    pub fn native_record(&self, attempt_id: &str) -> Option<&NativeGrantRecord> {
        self.native_records
            .iter()
            .find(|r| r.attempt_id == attempt_id)
    }

    /// The irreversible native K boundary. The broker supplies a live State
    /// connection and the same pinned evidence used by N; a caller-supplied
    /// binding or grant ID alone cannot spend the grant. Call this only once a
    /// fixed gated worker launch is ready. A consumed record is never reset,
    /// including after a lost reply or broker restart.
    #[expect(
        clippy::too_many_arguments,
        reason = "independent broker, State, process, and file authorities"
    )]
    pub fn consume_native(
        &mut self,
        grant_id: &str,
        roots: &RootRegistry,
        entries: &EntryRegistry,
        works: &WorkRegistry,
        caller: &PeerIdentity,
        host_namespace: &File,
        runner_image: &File,
        directory: &File,
        request: &File,
        receipt: &File,
        state: &MailboxDb,
    ) -> io::Result<NativeGrantRecord> {
        if self.has_debt() || roots.has_debt() || entries.has_debt() || works.has_debt() {
            return Err(io::Error::other("native K registry debt"));
        }
        let record = self
            .native_records
            .iter()
            .find(|r| r.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("unknown native grant"))?
            .clone();
        if record.state != "prepared" {
            return Err(io::Error::other("native grant already spent"));
        }
        let entry = entries
            .record(&record.root_id)
            .ok_or_else(|| io::Error::other("native entry absent"))?;
        let root = roots
            .live_roots()
            .find(|r| r.record.root_id == record.root_id)
            .ok_or_else(|| io::Error::other("native root absent"))?;
        if !entry.join_consumed
            || entry.guardian.as_ref() != Some(&record.guardian)
            || entry.joined_child.as_ref() != Some(&record.joined_child)
            || entry.domain_id.as_deref() != Some(record.domain_id.as_str())
            || entry.supervisor_authority_id.as_deref()
                != Some(record.supervisor_authority_id.as_str())
            || entry.owner_uid != record.owner_uid
            || root.record.owner_uid != record.owner_uid
            || ProcessStamp::from(&root.init) != record.root_init
            || ProcessStamp::from(&caller.process) != record.guardian
            || caller.uid != record.owner_uid
            || !caller.process.in_namespace(host_namespace)?
            || !caller.process.same_executable_as(runner_image)?
        {
            return Err(io::Error::other("native K owner/root incarnation changed"));
        }
        let bound = BoundNativeAuthority {
            root_id: &record.root_id,
            domain_id: &record.domain_id,
            supervisor_authority_id: &record.supervisor_authority_id,
            owner_generation: &record.owner_generation,
            owner_uid: record.owner_uid,
            guardian: &record.guardian,
            host_namespace,
            runner_image,
            receipt_sha256: &record.receipt_sha256,
        };
        let verified = verify_native_receipt(caller, &bound, directory, request, receipt)?;
        if verified.attempt_id != record.attempt_id
            || verified.accepted_snapshot_sha256 != record.accepted_snapshot_sha256
            || verified.custodian_request_sha256 != record.custodian_request_sha256
            || verified.receipt_sha256 != record.receipt_sha256
            || FileStamp::of(directory)? != record.directory
            || FileStamp::of(request)? != record.request
            || FileStamp::of(receipt)? != record.receipt
            || request.metadata()?.len() != record.request_byte_len
            || receipt.metadata()?.len() != record.receipt_byte_len
        {
            return Err(io::Error::other("native K evidence changed since prepare"));
        }
        let binding: NativeGrantBinding = state
            .native_grant_binding(&record.attempt_id)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("native K State binding absent"))?;
        if binding.attempt_id != record.attempt_id
            || binding.grant_id != record.grant_id
            || binding.protocol != record.kind
            || binding.accepted_revision != 2
            || binding.domain_id != record.domain_id
            || binding.kernel_root_id != record.root_id
            || binding.supervisor_authority_id != record.supervisor_authority_id
            || binding.owner_generation != record.owner_generation
            || binding.guardian_identity.pid != i64::from(record.guardian.host_pid)
            || binding.guardian_identity.boot_id != record.guardian.boot_id
            || binding.guardian_identity.starttime_ticks != record.guardian.starttime_ticks as i64
            || binding.accepted_snapshot_sha256 != record.accepted_snapshot_sha256
            || binding.custodian_request_sha256 != record.custodian_request_sha256
        {
            return Err(io::Error::other("native K State/grant binding conflict"));
        }
        root.init.verify()?;
        caller.process.verify()?;
        self.consume_native_record(grant_id)
    }

    fn consume_native_record(&mut self, grant_id: &str) -> io::Result<NativeGrantRecord> {
        if self.has_debt() {
            return Err(io::Error::other("uncertain grant registry"));
        }
        let index = self
            .native_records
            .iter()
            .position(|r| r.grant_id == grant_id)
            .ok_or_else(|| io::Error::other("unknown native grant"))?;
        if self.native_records[index].state != "prepared" {
            return Err(io::Error::other("native grant already spent"));
        }
        let mut consumed = self.native_records[index].clone();
        consumed.state = "consumed".into();
        let path = self.directory.join(format!("{grant_id}.json"));
        let temp = self.directory.join(format!("{grant_id}.tmp"));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temp)?;
            serde_json::to_writer(&mut file, &consumed)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp, &path)?;
            File::open(&self.directory)?.sync_all()
        })();
        if let Err(error) = result {
            self.poisoned = true;
            return Err(error);
        }
        self.native_records[index] = consumed.clone();
        Ok(consumed)
    }

    pub fn has_debt(&self) -> bool {
        self.poisoned
    }

    /// Native prepare is idempotent only for the exact same authenticated
    /// evidence. A lost reply can be retried after broker restart without
    /// issuing another grant. This never authorizes K.
    #[expect(
        clippy::too_many_arguments,
        reason = "independent authority and descriptor checks"
    )]
    pub fn prepare_native(
        &mut self,
        roots: &RootRegistry,
        entries: &EntryRegistry,
        works: &WorkRegistry,
        caller: &PeerIdentity,
        host_namespace: &File,
        runner_image: &File,
        directory: &File,
        request: &File,
        receipt: &File,
        root_id: &str,
        attempt_id: &str,
        owner_generation: &str,
        receipt_sha256: &str,
    ) -> io::Result<NativeGrantRecord> {
        if self.has_debt()
            || roots.has_debt()
            || entries.has_debt()
            || works.has_debt()
            || !directory.metadata()?.is_dir()
            || !valid_digest(receipt_sha256)
            || uuid::Uuid::parse_str(attempt_id).is_err()
            || uuid::Uuid::parse_str(owner_generation).is_err()
        {
            return Err(io::Error::other(
                "native grant registry or evidence uncertain",
            ));
        }
        let entry = entries
            .record(root_id)
            .ok_or_else(|| io::Error::other("entry absent"))?;
        let root = roots
            .live_roots()
            .find(|r| r.record.root_id == root_id)
            .ok_or_else(|| io::Error::other("live root absent"))?;
        let guardian = ProcessStamp::from(&caller.process);
        if !entry.join_consumed
            || entry.guardian.as_ref() != Some(&guardian)
            || entry.owner_uid != caller.uid
            || root.record.owner_uid != caller.uid
            || entry.joined_child.is_none()
            || entry.domain_id.is_none()
            || entry.supervisor_authority_id.is_none()
            || !caller.process.in_namespace(host_namespace)?
            || !caller.process.same_executable_as(runner_image)?
        {
            return Err(io::Error::other("caller is not bound host guardian"));
        }
        let bound = BoundNativeAuthority {
            root_id,
            domain_id: entry.domain_id.as_deref().unwrap(),
            supervisor_authority_id: entry.supervisor_authority_id.as_deref().unwrap(),
            owner_generation,
            owner_uid: entry.owner_uid,
            guardian: entry.guardian.as_ref().unwrap(),
            host_namespace,
            runner_image,
            receipt_sha256,
        };
        let verified = verify_native_receipt(caller, &bound, directory, request, receipt)?;
        if verified.attempt_id != attempt_id {
            return Err(io::Error::other(
                "native attempt ID differs from accepted receipt",
            ));
        }
        root.init.verify()?;
        caller.process.verify()?;
        let existing = self.native_record(attempt_id);
        let record = NativeGrantRecord {
            version: 4,
            kind: "native-continuation-v1".into(),
            grant_id: existing
                .map_or_else(|| uuid::Uuid::new_v4().to_string(), |r| r.grant_id.clone()),
            attempt_id: attempt_id.into(),
            root_id: root_id.into(),
            root_init: ProcessStamp::from(&root.init),
            domain_id: entry.domain_id.as_ref().unwrap().clone(),
            supervisor_authority_id: entry.supervisor_authority_id.as_ref().unwrap().clone(),
            owner_generation: owner_generation.into(),
            owner_uid: caller.uid,
            guardian,
            joined_child: entry.joined_child.as_ref().unwrap().clone(),
            receipt_sha256: verified.receipt_sha256,
            accepted_snapshot_sha256: verified.accepted_snapshot_sha256,
            custodian_request_sha256: verified.custodian_request_sha256,
            directory: FileStamp::of(directory)?,
            request: FileStamp::of(request)?,
            receipt: FileStamp::of(receipt)?,
            request_byte_len: request.metadata()?.len(),
            receipt_byte_len: receipt.metadata()?.len(),
            state: "prepared".into(),
        };
        if let Some(existing) = existing {
            if existing != &record {
                return Err(io::Error::other(
                    "native grant replay conflicts with durable evidence",
                ));
            }
            return Ok(existing.clone());
        }
        if self.records.iter().any(|r| {
            r.grant_id == record.grant_id || r.root_id == root_id && r.work_id == attempt_id
        }) || self
            .native_records
            .iter()
            .any(|r| r.grant_id == record.grant_id)
        {
            return Err(io::Error::other("native grant collides with existing work"));
        }
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
        self.native_records.push(record.clone());
        Ok(record)
    }

    /// The caller must be the live, exact host guardian already bound to this
    /// root. The positive receipt and intent are read through pinned descriptors
    /// from the guardian's accepted state directory. The caller supplies the
    /// digest of its in-memory positive receipt so a mutable file cannot alter
    /// registration or initiator identity after acceptance. This prepares debt
    /// only; execution requires the separate one-use K operation.
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
        executable: &File,
        cwd: &File,
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
            || self
                .native_records
                .iter()
                .any(|record| record.root_id == root_id && record.attempt_id == work_id)
            || !caller.process.in_namespace(host_namespace)?
            || !caller.process.same_executable_as(runner_image)?
            || !cwd.metadata()?.is_dir()
            || !executable.metadata()?.is_file()
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
        let intent_identity: IntentIdentity = serde_json::from_slice(&intent_bytes)?;
        let helper = sealed_helper(state_dir, &intent_identity, runner_image)?;
        let expected_state =
            fs::metadata(intent_identity.state_root.join(&intent_identity.handle))?;
        let expected_cwd = fs::metadata(intent_identity.meta.cwd)?;
        let actual_state = state_dir.metadata()?;
        let actual_cwd = cwd.metadata()?;
        if (expected_state.dev(), expected_state.ino()) != (actual_state.dev(), actual_state.ino())
            || (expected_cwd.dev(), expected_cwd.ino()) != (actual_cwd.dev(), actual_cwd.ino())
        {
            return Err(io::Error::other("accepted state or cwd descriptor changed"));
        }
        // The accepted receipt carries the original source incarnation, not a
        // free-standing work UUID. Pin that live process in the host observer
        // before recording a grant. Namespace-local PID confusion refuses the
        // request until explicit PID-domain transport is integrated.
        let source_pid = i32::try_from(receipt.initiator.pid)
            .map_err(|_| io::Error::other("invalid source host PID"))?;
        let source = PinnedProcess::open(source_pid)?;
        if source.boot_id != receipt.initiator.boot_id
            || source.starttime_ticks
                != u64::try_from(receipt.initiator.starttime_ticks)
                    .map_err(|_| io::Error::other("invalid source starttime"))?
            || !source.same_executable_as(executable)?
        {
            return Err(io::Error::other("accepted source incarnation changed"));
        }
        let parent = match &receipt.registration {
            Registration::Root => {
                if !source.in_namespace(root.init.namespace())? {
                    return Err(io::Error::other("source is outside exact root namespace"));
                }
                None
            }
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
                if !source.in_namespace(parent_work.init.namespace())? {
                    return Err(io::Error::other(
                        "source is outside causal parent namespace",
                    ));
                }
                Some((
                    parent.grant_id.clone(),
                    parent_work.record.work_incarnation.clone(),
                ))
            }
        };
        caller.process.verify()?;
        root.init.verify()?;
        source.verify()?;
        let record = GrantRecord {
            version: if helper.is_some() { 3 } else { 2 },
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: root_id.to_owned(),
            root_init: ProcessStamp::from(&root.init),
            owner_uid: caller.uid,
            supervisor_authority_id: supervisor.clone(),
            owner_generation: owner_generation.to_owned(),
            guardian: ProcessStamp::from(&caller.process),
            joined_child: joined.clone(),
            work_id: work_id.to_owned(),
            parent_grant_id: parent.as_ref().map(|parent| parent.0.clone()),
            parent_work_incarnation: parent.map(|parent| parent.1),
            accepted_sha256: digest(&accepted_bytes),
            request_sha256: request_sha256.to_owned(),
            initiator: receipt.initiator,
            artifacts: GrantArtifacts::pinned(executable, intent, cwd, state_dir, accepted)?,
            sealed_helper: helper,
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
    #[expect(
        clippy::too_many_arguments,
        reason = "the caller, root, entry, and causal work registries are independent trust checks"
    )]
    pub fn consume(
        &mut self,
        grant_id: &str,
        roots: &RootRegistry,
        entries: &EntryRegistry,
        works: &WorkRegistry,
        caller: &PeerIdentity,
        host_namespace: &File,
        runner_image: &File,
    ) -> io::Result<GrantRecord> {
        if roots.has_debt()
            || entries.has_debt()
            || works.has_debt()
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
            || entry.joined_child.as_ref() != Some(&record.joined_child)
        {
            return Err(io::Error::other("accepted grant binding changed"));
        }
        if let (Some(parent_grant_id), Some(parent_incarnation)) =
            (&record.parent_grant_id, &record.parent_work_incarnation)
        {
            let parent_grant = self
                .records
                .iter()
                .find(|parent| parent.grant_id == *parent_grant_id && parent.consumed)
                .ok_or_else(|| io::Error::other("causal parent grant changed"))?;
            let parent_work = works.live_works().find(|work| {
                work.record.root_id == record.root_id
                    && work.record.work_incarnation == *parent_incarnation
                    && work.record.accepted_grant_id.as_deref() == Some(parent_grant_id.as_str())
            });
            if parent_grant.root_init != record.root_init
                || parent_work.is_none_or(|work| work.init.verify().is_err())
            {
                return Err(io::Error::other("causal parent namespace changed"));
            }
        }
        root.init.verify()?;
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

    #[test]
    fn unsupported_or_partial_native_records_refuse_registry_restart() {
        let dir = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4();
        let path = dir.path().join(format!("{id}.json"));
        fs::write(&path, format!(r#"{{"version":5,"grant_id":"{id}"}}"#)).unwrap();
        assert!(GrantRegistry::open(dir.path()).is_err());
        fs::write(
            &path,
            format!(r#"{{"version":4,"grant_id":"{id}","kind":"native-continuation-v1"}}"#),
        )
        .unwrap();
        assert!(GrantRegistry::open(dir.path()).is_err());
    }

    #[test]
    fn sealed_helper_image_requires_exact_bytes_and_immutable_memfd() {
        let name = c"age319-sealed-helper-test";
        let raw = unsafe {
            libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
        };
        assert!(raw >= 0);
        let mut memfd = unsafe { File::from_raw_fd(raw) };
        memfd.write_all(b"pinned-runner-image").unwrap();
        let source = tempfile::tempfile().unwrap();
        let different_stamp = FileStamp::of(&source).unwrap();
        let digest = image_digest(&memfd).unwrap();
        assert!(!matches_pinned_image(&memfd, &different_stamp, &digest).unwrap());
        let seals =
            libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
        assert_eq!(unsafe { libc::fcntl(raw, libc::F_ADD_SEALS, seals) }, 0);
        assert!(matches_pinned_image(&memfd, &different_stamp, &digest).unwrap());
        assert!(!matches_pinned_image(&memfd, &different_stamp, &"00".repeat(32)).unwrap());
        assert!(matches_pinned_image(&memfd, &FileStamp::of(&memfd).unwrap(), &digest).unwrap());
    }

    #[test]
    fn standalone_original_work_keeps_v2_grant_without_native_owner() {
        let intent = IntentIdentity {
            protocol: "original-work-v1".into(),
            work_id: "standalone".into(),
            root_id: uuid::Uuid::new_v4().to_string(),
            handle: "standalone".into(),
            state_root: PathBuf::from("/not-opened"),
            meta: IntentMeta {
                cwd: PathBuf::from("/"),
                owner_session_id: None,
                owner_invocation_uuid: None,
                delivery_helper: Some(HelperProvenance {
                    path: PathBuf::from("/not-opened/standalone/delivery-helper"),
                    device: 1,
                    inode: 1,
                    size: 1,
                    sha256: "11".repeat(32),
                    interpreter: None,
                }),
            },
            registration_authority: None,
        };
        assert!(
            sealed_helper(
                &File::open("/").unwrap(),
                &intent,
                &File::open("/proc/self/exe").unwrap(),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn partial_native_owner_fields_refuse_instead_of_downgrading_to_v2() {
        let state_dir = File::open("/").unwrap();
        let runner_image = File::open("/proc/self/exe").unwrap();
        let provenance = HelperProvenance {
            path: PathBuf::from("/not-opened/work/delivery-helper"),
            device: 1,
            inode: 1,
            size: 1,
            sha256: "11".repeat(32),
            interpreter: None,
        };
        for fields in 1..15_u8 {
            let intent = IntentIdentity {
                protocol: "original-work-v1".into(),
                work_id: "work".into(),
                root_id: uuid::Uuid::new_v4().to_string(),
                handle: "work".into(),
                state_root: PathBuf::from("/not-opened"),
                meta: IntentMeta {
                    cwd: PathBuf::from("/"),
                    delivery_helper: (fields & 1 != 0).then(|| provenance.clone()),
                    owner_session_id: (fields & 2 != 0).then(|| "session".into()),
                    owner_invocation_uuid: (fields & 4 != 0)
                        .then(|| uuid::Uuid::new_v4().to_string()),
                },
                registration_authority: (fields & 8 != 0).then(|| b"11".repeat(32)),
            };
            let result = sealed_helper(&state_dir, &intent, &runner_image);
            if fields == 1 {
                assert!(result.unwrap().is_none(), "standalone helper snapshot");
            } else {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "incomplete sealed helper owner binding",
                    "fields {fields:04b}"
                );
            }
        }
    }
    use serde_json::json;
    use std::os::unix::fs::symlink;

    fn evidence(nested: bool) -> (Vec<u8>, Vec<u8>, String, String, String) {
        let root = uuid::Uuid::new_v4().to_string();
        let supervisor = uuid::Uuid::new_v4().to_string();
        let generation = uuid::Uuid::new_v4().to_string();
        let intent = serde_json::to_vec(&json!({
            "protocol": "original-work-v1", "work_id": "work-a",
            "root_id": root, "handle": "work-a", "state_root": "/tmp",
            "meta": {"cwd": "/tmp"}
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
            version: 2,
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            root_init: ProcessStamp::from(guardian),
            owner_uid: unsafe { libc::getuid() },
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian: ProcessStamp::from(guardian),
            joined_child: ProcessStamp::from(guardian),
            work_id: "accepted-a".into(),
            parent_grant_id: None,
            parent_work_incarnation: None,
            accepted_sha256: digest(b"accepted"),
            request_sha256: digest(b"intent"),
            initiator: SourceIdentity {
                pid: 42,
                boot_id: "boot".into(),
                starttime_ticks: 7,
            },
            artifacts: GrantArtifacts::pinned(
                &File::open("/proc/self/exe").unwrap(),
                &File::open("/dev/null").unwrap(),
                &File::open("/tmp").unwrap(),
                &File::open("/tmp").unwrap(),
                &File::open("/dev/null").unwrap(),
            )
            .unwrap(),
            sealed_helper: None,
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

        // The older ledger lacked descriptor inode bindings. Refuse it on
        // restart instead of silently upgrading an unbound accepted grant.
        let old = tempfile::tempdir().unwrap();
        let mut old_record = record;
        old_record.version = 1;
        fs::write(
            old.path().join(format!("{}.json", old_record.grant_id)),
            serde_json::to_vec(&old_record).unwrap(),
        )
        .unwrap();
        assert!(GrantRegistry::open(old.path()).is_err());

        let unpaired = tempfile::tempdir().unwrap();
        let mut unpaired_record = self::record(&guardian);
        unpaired_record.parent_grant_id = Some(uuid::Uuid::new_v4().to_string());
        fs::write(
            unpaired
                .path()
                .join(format!("{}.json", unpaired_record.grant_id)),
            serde_json::to_vec(&unpaired_record).unwrap(),
        )
        .unwrap();
        assert!(GrantRegistry::open(unpaired.path()).is_err());
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
