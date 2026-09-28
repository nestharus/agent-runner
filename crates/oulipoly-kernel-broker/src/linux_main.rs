//! Opt-in host-root broker for the pinned guardian and one-use root child join.

const SOURCE_TICKET_TTL: std::time::Duration = std::time::Duration::from_secs(30);
const BROKER_ACCEPT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
#[cfg(feature = "age319-private-broker-fixture")]
const ORDINARY_BASH_COMPLETION_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);
const BROKER_INGRESS_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const RELEASED_HANDOFF_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const FRESH_V30_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const FRESH_V30_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const FRESH_HANDOFF_QUEUE_CAPACITY: usize = 8;
const RELEASED_HANDOFF_REPLY_CAPACITY: usize = 1;

const REQUEST_RECEIVE_BUFFER_BYTES: usize = 64 * 1024;
#[cfg(feature = "age319-private-broker-fixture")]
const SYNC_STREAM_VERIFY_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(feature = "age319-private-broker-fixture")]
#[path = "fresh_provider.rs"]
mod fresh_provider;
// Opt-in AGE-319 route and original-provider writer index. Account/effect/manual
// eligibility readers still use retained evidence; this is not activation.
#[cfg(feature = "age319-private-broker-fixture")]
#[path = "fresh_index.rs"]
#[allow(dead_code)]
mod fresh_index;
#[cfg(feature = "age319-private-broker-fixture")]
#[path = "manual_quota.rs"]
mod manual_quota;
#[path = "namespace_helper_reaper.rs"]
mod namespace_helper_reaper;
#[path = "native_work.rs"]
mod native_work;
#[path = "normal_physical.rs"]
mod normal_physical;
#[cfg(feature = "age319-private-broker-fixture")]
#[path = "private_installed_exec.rs"]
mod private_installed_exec;
#[path = "released_handoff.rs"]
mod released_handoff;
#[path = "root_join.rs"]
mod root_join;
#[path = "source_decision_journal.rs"]
mod source_decision_journal;
#[path = "source_launch.rs"]
mod source_launch;
#[cfg(feature = "age319-private-broker-fixture")]
#[path = "v2_wake.rs"]
mod v2_wake;
#[path = "work_launch.rs"]
mod work_launch;
use base64::Engine as _;
#[cfg(feature = "age319-private-broker-fixture")]
use oulipoly_kernel_broker::accepted_grant::DelegatedRootHGrant;
use oulipoly_kernel_broker::accepted_grant::{Acceptance, GrantRegistry};
use oulipoly_kernel_broker::cutover_gate::EntryGate;
use oulipoly_kernel_broker::entry_registry::{
    EntryRegistry, EntryTerminalSettlement, ProcessStamp,
};
use oulipoly_kernel_broker::identity::{
    PeerIdentity, PinnedProcess, host_proc_file, host_proc_uid, install_detached_host_proc,
};
use oulipoly_kernel_broker::installed_launch::{self, InstalledLaunchSpec};
use oulipoly_kernel_broker::installed_pair::{self, InstalledPair};
use oulipoly_kernel_broker::native_receipt::{
    BoundNativeAuthority, verify as verify_native_receipt,
};
use oulipoly_kernel_broker::protocol::{
    AcceptedWorkSpec, DelegatedRootHProof, ExactSourceDecisionRequest,
    ExactSourceDecisionVerification, ExactSourceIssuerKind, FreshChildRequest,
    FreshRecipientRequest, FreshRootEffectRequest, JoinSpec, JoinedChildWitness,
    LaunchAcceptedWorkSpec, NativeKSpec, NativePrepareSpec, OwnerDiscoveryReadback,
    OwnerDiscoveryRequest, OwnerPidBinding, OwnerWitness, PostcommitHChallenge,
    PostcommitHReadback, ProcessWitness, SourceControlUse, SourceScope, SourceSocketWitness,
    SourceTicketUse, SourceWitnessProbe, StateGenerationSpec, StateReadSpec, StateWriteAction,
    StateWriteSpec,
};
use oulipoly_kernel_broker::registry::{OwnerCloseIntent, RootRecord, RootRegistry};
use oulipoly_kernel_broker::source_acceptance::{
    accept_v2_completion_source, capture_and_stage_v2_evidence, commit_v2_evidence,
    decide_v2_source_retention_release, deliver_v2_source_retention_release,
    read_v2_recipient_custody,
};
use oulipoly_kernel_broker::source_physical::{SourceObservation, SourcePhysicalRegistry};
use oulipoly_kernel_broker::work_registry::{
    Scope, WorkRegistry, classify_scope, classify_scope_readback,
};
use oulipoly_kernel_broker::{root_drain, root_pid1};
use oulipoly_state::mailbox::{
    BrokerReleaseEvidence, BrokerSidecar, BrokerSourceEffectGrant, ExactProcessEvidence,
    FreshBashListenerPolicy, FreshDeliverySubmission, FreshNativeFPreparation,
    FreshRecipientIdentity, FreshReleasedHandoff, FreshRootTerminalReadback, FreshV30Lane,
    FreshV30LaneIdentity, PreparedBrokerOwner, PreparedProcessStamp, RuntimeGenerationRow,
};
use sha2::{Digest, Sha256};
#[cfg(feature = "age319-private-broker-fixture")]
use std::collections::HashMap;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
#[cfg(feature = "age319-private-broker-fixture")]
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::IntoRawFd;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";
const STATE: &str = "/var/lib/oulipoly-kernel-broker";
const FRESH_SOCKET: &str = "/run/oulipoly-kernel-broker/v30.sock";
const RUNNER: &str = "/usr/local/libexec/oulipoly/oulipoly-agent-runner";

fn verify_native_f_resident(
    generation: &RuntimeGenerationRow,
    path: &str,
    device: u64,
    inode: u64,
    instance: &str,
    settings: &str,
    session: &str,
) -> Result<(), String> {
    let reply = oulipoly_runtime::executor::cli::pty_broker::query_pty_generation_identity(
        Path::new(path),
        device,
        inode,
    )?;
    let (ExactProcessEvidence::Recorded(creator), ExactProcessEvidence::Recorded(provider)) = (
        &generation.creator_process_evidence,
        &generation.exact_process_evidence,
    ) else {
        return Err("native F exact resident process identity unavailable".into());
    };
    if reply.generation_id != generation.generation_id.to_string()
        || reply.spawn_invocation_uuid != generation.spawn_invocation_uuid
        || &reply.creator_process != creator
        || &reply.provider_process != provider
        || reply.provider_account != generation.provider_name
        || reply.provider_instance_id != instance
        || reply.settings_id != settings
        || reply.provider_session_id != session
        || generation.session_id.as_deref() != Some(session)
        || generation.pty_control_path.as_deref() != Some(path)
    {
        return Err("native F resident PTY attestation mismatched selected generation".into());
    }
    Ok(())
}

fn verify_native_f_readback(
    lane: &FreshV30Lane,
    record: &FreshNativeFPreparation,
) -> Result<(), String> {
    lane.attest_native_f_preparation(record, |generation, path, device, inode| {
        verify_native_f_resident(
            generation,
            path,
            device,
            inode,
            &record.provider_instance_id,
            &record.settings_id,
            &record.provider_session_id,
        )
    })
}

/// The private physical provider owns this native session file. A typed page
/// supplied by the original root is corroborated against that file and the
/// pinned live provider; a constructed F socket request alone is insufficient.
#[cfg(feature = "age319-private-broker-fixture")]
fn verify_private_native_f_source(
    lane: &FreshV30Lane,
    prepared: &FreshNativeFPreparation,
    observed: &oulipoly_state::mailbox::FreshNativeFObservedTurn,
) -> Result<(), String> {
    if !private_fixture() {
        return Err("native F provider source verifier unavailable; pending".into());
    }
    verify_native_f_readback(lane, prepared)?;
    let gate = PathBuf::from(
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
            .map_err(|_| "native F provider source gate absent")?,
    );
    let path = gate.join("provider-native-session.json");
    let metadata = fs::symlink_metadata(&path).map_err(|_| "native F provider source absent")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4 * 1024 * 1024
    {
        return Err("native F provider source file invalid".into());
    }
    let native: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?)
            .map_err(|_| "native F provider source malformed")?;
    let source_turns = native["turns"]
        .as_array()
        .ok_or("native F provider turns absent")?;
    if native["format"] != "age319-interactive-native-session/v1"
        || native["session_id"] != prepared.provider_session_id
        || native["controlling_tty"] != true
        || source_turns.len() != 1
        || observed.anchor_token
            != format!(
                "age319-native:{}:0",
                native["store_nonce"]
                    .as_str()
                    .ok_or("native F store nonce absent")?
            )
        || source_turns[0]["turn_id"] != observed.turn_id
        || source_turns[0]["body"] != observed.body
        || source_turns[0]["nonce"] != observed.nonce
        || source_turns[0]["session_id"] != prepared.provider_session_id
        || source_turns[0]["payload_sha256"] != prepared.payload_sha256
    {
        return Err("native F provider-authored turn differs from typed page".into());
    }
    Ok(())
}

#[cfg(feature = "age319-private-broker-fixture")]
fn certify_native_f_with_source(
    lane: &mut FreshV30Lane,
    request: &str,
    recipient: &FreshRecipientIdentity,
    observed: &oulipoly_state::mailbox::FreshNativeFObservedTurn,
) -> Result<oulipoly_state::mailbox::FreshNativeFReceipt, String> {
    let prepared = lane
        .read_native_f_preparation(request, recipient)?
        .ok_or("native F preparation absent")?;
    verify_private_native_f_source(lane, &prepared, observed)?;
    lane.certify_native_f_receipt(request, recipient, observed)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn verify_native_f_committed_receipt(
    lane: &FreshV30Lane,
    request: &str,
    recipient: &FreshRecipientIdentity,
) -> Result<(), String> {
    let Some(receipt) = lane.read_native_f_receipt(request, recipient)? else {
        return Ok(());
    };
    let prepared = lane
        .read_native_f_preparation(request, recipient)?
        .ok_or("native F committed receipt preparation absent")?;
    verify_private_native_f_source(lane, &prepared, &receipt.observation)?;
    if receipt.preparation_request_id != prepared.preparation_request_id
        || receipt.grant_id != prepared.grant_id
        || receipt.recipient_identity != *recipient
        || receipt.envelope_sha256 != prepared.envelope_sha256
        || receipt.payload_sha256 != prepared.payload_sha256
    {
        return Err("native F committed receipt lineage changed".into());
    }
    Ok(())
}

#[cfg(not(feature = "age319-private-broker-fixture"))]
fn certify_native_f_with_source(
    _lane: &mut FreshV30Lane,
    _request: &str,
    _recipient: &FreshRecipientIdentity,
    _observed: &oulipoly_state::mailbox::FreshNativeFObservedTurn,
) -> Result<oulipoly_state::mailbox::FreshNativeFReceipt, String> {
    Err("native F provider source verifier unavailable; pending".into())
}

#[cfg(not(feature = "age319-private-broker-fixture"))]
fn verify_native_f_committed_receipt(
    lane: &FreshV30Lane,
    request: &str,
    recipient: &FreshRecipientIdentity,
) -> Result<(), String> {
    // A receipt imported from another Broker image is a historical State fact,
    // not independent provider-native evidence for this image. Preserve an
    // absent-receipt status read, but never use an existing receipt for ACK or
    // as a currently certified readback without a supported source verifier.
    if lane.read_native_f_receipt(request, recipient)?.is_some() {
        return Err("native F provider source verifier unavailable; pending".into());
    }
    Ok(())
}

fn read_native_f_receipt_with_source(
    lane: &FreshV30Lane,
    request: &str,
    recipient: &FreshRecipientIdentity,
) -> Result<Option<oulipoly_state::mailbox::FreshNativeFReceipt>, String> {
    verify_native_f_committed_receipt(lane, request, recipient)?;
    lane.read_native_f_receipt(request, recipient)
}

fn acknowledge_native_f_receipt_with_source(
    lane: &mut FreshV30Lane,
    request: &str,
    token: &str,
    recipient: &FreshRecipientIdentity,
) -> Result<oulipoly_state::mailbox::FreshDeliveryReadback, String> {
    verify_native_f_committed_receipt(lane, request, recipient)?;
    lane.acknowledge_native_f_receipt(request, token, recipient)
}

fn read_native_f_auto_ack_with_source(
    lane: &FreshV30Lane,
    request: &str,
    recipient: &FreshRecipientIdentity,
) -> Result<Option<oulipoly_state::mailbox::FreshNativeFAutoAck>, String> {
    verify_native_f_committed_receipt(lane, request, recipient)?;
    lane.read_native_f_auto_ack(request, recipient)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_fixture() -> bool {
    (unsafe { libc::geteuid() }) == 0
        && fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
}
#[cfg(feature = "age319-private-broker-fixture")]
fn open_exact_sync_stream(
    directory: &Path,
    grant: &str,
    suffix: &str,
    len: u64,
    hash: &str,
) -> io::Result<File> {
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join(format!("{grant}.{suffix}")))?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != 0 || meta.nlink() != 1 || meta.len() != len {
        return Err(io::Error::other("sync stream custody changed"));
    }
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut buffer = [0u8; SYNC_STREAM_VERIFY_BUFFER_BYTES];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        count = count
            .checked_add(read as u64)
            .ok_or_else(|| io::Error::other("sync stream length overflow"))?;
        digest.update(&buffer[..read]);
    }
    if count != len || format!("{:x}", digest.finalize()) != hash || file.metadata()?.len() != len {
        return Err(io::Error::other("sync stream hash changed"));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn private_native_f_drop_reply(stage: &str) {
    if !private_fixture()
        || std::env::var("AGE319_PRIVATE_NATIVE_F_DROP_REPLY_V1")
            .ok()
            .as_deref()
            != Some(stage)
    {
        return;
    }
    if let Ok(gate) = std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
        let _ = fs::write(Path::new(&gate).join("interactive-f-reply-dropped"), stage);
    }
    std::process::exit(82);
}
#[cfg(not(feature = "age319-private-broker-fixture"))]
fn private_fixture() -> bool {
    false
}

fn checked_root_path(path: &Path, directory: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("nonabsolute installed path"));
    }
    let mut current = path;
    loop {
        let meta = fs::symlink_metadata(current)?;
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 || meta.file_type().is_symlink() {
            return Err(io::Error::other("untrusted installed path"));
        }
        if current == path
            && (if directory {
                !meta.is_dir()
            } else {
                !meta.is_file()
            })
        {
            return Err(io::Error::other("wrong installed file type"));
        }
        if current == Path::new("/") {
            break;
        }
        current = current
            .parent()
            .ok_or_else(|| io::Error::other("bad path"))?;
    }
    Ok(())
}

// The current installed Runner still has direct user-sidecar writers. A v30
// database must not admit that image into a new root or let it exercise the
// old source/control/work endpoint. Y/R/W and v30 N/t are generation-bound
// broker authority checks; I is a read-only live route observation. None can
// create a root on their own. Remove this gate only with the complete
// production caller routing and image-version
// admission protocol, never as part of ordinary broker startup.
fn require_cutover_entry_route(
    operation: u8,
    broker_owned_sidecar: bool,
    gate_closed: bool,
) -> io::Result<()> {
    if matches!(
        operation,
        b'i' | b'v' | b'X' | b'x' | b'@' | b'[' | 0x7f | 0x80 | 0x81 | 0x82 | 0x83
    ) {
        return Ok(());
    }
    if gate_closed {
        return Err(io::Error::other("broker entry gate is durably closed"));
    }
    if matches!(operation, b':' | b';' | b'+') && !broker_owned_sidecar {
        return Err(io::Error::other(
            "exact source decision requires v30 sidecar",
        ));
    }
    if !broker_owned_sidecar
        && matches!(
            operation,
            b'e' | b'p' | b'g' | b'a' | b'j' | b't' | b'q' | b'z'
        )
    {
        return Err(io::Error::other("v30 entry requires broker-owned sidecar"));
    }
    if broker_owned_sidecar
        && !matches!(
            operation,
            b'=' | b'Y'
                | b'R'
                | b'W'
                | b'I'
                | b'e'
                | b'p'
                | b'g'
                | b'a'
                | b'j'
                | b'N'
                | b't'
                | b'q'
                | b'z'
                | b':'
                | b';'
        )
    {
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_fixture()
            && matches!(
                operation,
                b'L' | b'l' | b'M' | b'&' | b'+' | b's' | b'T' | b'H' | b'K' | b'B' | b'Q' | b'Z'
            )
        {
            return Ok(());
        }
        return Err(io::Error::other(
            "broker-owned v30 sidecar requires installed v30 Runner entry routing",
        ));
    }
    Ok(())
}

#[derive(Debug)]
enum RequestPayload {
    None,
    RootDrain {
        expected: RootRecord,
    },
    OwnerCloseIntent {
        request: oulipoly_kernel_broker::protocol::OwnerCloseIntentRequest,
    },
    FreshChildRequest {
        request: FreshChildRequest,
    },
    FreshSessionRequest {
        request_id: String,
    },
    FreshRootEffectRequest {
        request: FreshRootEffectRequest,
    },
    #[allow(
        dead_code,
        reason = "fresh Bash lane is closed without the private fixture"
    )]
    FreshBashChildRequest {
        request_id: String,
        listener_policy: Option<FreshBashListenerPolicy>,
        #[cfg(feature = "age319-private-broker-fixture")]
        ordinary_command: Option<fresh_provider::OrdinaryBashCommand>,
        #[cfg(feature = "age319-private-broker-fixture")]
        ordinary_k_digest: Option<[u8; 32]>,
    },
    #[allow(
        dead_code,
        reason = "fresh Bash lane is closed without the private fixture"
    )]
    FreshBashPrivateResult {
        result: oulipoly_state::mailbox::FreshBashPrivateResult,
    },
    FreshNormalModelRequest {
        request: FreshRootEffectRequest,
        config_dir: File,
    },
    FreshNormalPlanRequest {
        request: FreshRootEffectRequest,
        config_dir: File,
        cwd: File,
        environment: File,
    },
    FreshNormalPublicationRequest {
        request: FreshRootEffectRequest,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    FreshProviderRequest {
        request: FreshRootEffectRequest,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    PrivateNativeBashExec {
        request: oulipoly_kernel_broker::protocol::PrivateNativeBashExec,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    FreshInteractivePtyHandoff {
        request: oulipoly_kernel_broker::protocol::PrivateFreshPtyHandoff,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    FreshRouteRequest {
        request: oulipoly_kernel_broker::protocol::FreshRouteRequest,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    FreshAccountEffectRequest {
        request: oulipoly_kernel_broker::protocol::FreshAccountEffectRequest,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    ManualQuotaRequest {
        request: oulipoly_kernel_broker::protocol::ManualQuotaRequest,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    ManualQuotaObserve {
        operation_id: String,
    },
    FreshRecipientRequest {
        request: FreshRecipientRequest,
    },
    Prepare {
        root_id: String,
        guardian_pid: i32,
    },
    Bind {
        root_id: String,
        domain_id: String,
        supervisor_id: String,
    },
    Read {
        root_id: String,
    },
    Join {
        spec: JoinSpec,
        descriptors: [File; 5],
    },
    VerifyOwner {
        witness: OwnerWitness,
        socket: File,
    },
    DiscoverOwner {
        request: OwnerDiscoveryRequest,
        socket: File,
    },
    ExactSourceDecision {
        request: ExactSourceDecisionRequest,
        socket: File,
        registration: File,
    },
    VerifyExactSourceDecision {
        request: ExactSourceDecisionVerification,
        socket: File,
        registration: File,
    },
    PostcommitH {
        request: PostcommitHChallenge,
        socket: File,
        registration: File,
        retained_guardian: File,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    PrivateSourceWitness {
        probe: oulipoly_kernel_broker::protocol::PrivateSourceWitnessProbe,
        socket: File,
        registration: File,
    },
    VerifySourceSocket {
        witness: SourceSocketWitness,
        socket: File,
    },
    ConsumeSourceTicket {
        spec: SourceTicketUse,
        socket: File,
    },
    VerifyJoinedChild {
        witness: JoinedChildWitness,
    },
    PrepareAcceptedWork {
        spec: AcceptedWorkSpec,
        descriptors: [File; 5],
    },
    PrepareNative {
        spec: NativePrepareSpec,
        descriptors: [File; 3],
    },
    NativeK {
        spec: NativeKSpec,
        descriptors: [File; 4],
    },
    NativeKV30 {
        spec: NativeKSpec,
        descriptors: [File; 3],
    },
    LaunchAcceptedWork {
        spec: LaunchAcceptedWorkSpec,
        descriptors: [File; 7],
    },
    ObserveAcceptedWork {
        grant_id: String,
    },
    CancelAcceptedWork {
        grant_id: String,
    },
    StateRead {
        spec: StateReadSpec,
    },
    StateWrite {
        spec: StateWriteSpec,
    },
    StateGeneration {
        spec: StateGenerationSpec,
    },
    InstalledLaunch {
        spec: InstalledLaunchSpec,
        descriptors: Vec<File>,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    PrivateLaunchStatus {
        request_id: String,
        generation: String,
        cancel: bool,
    },
}

#[cfg(feature = "age319-private-broker-fixture")]
fn read_ordinary_bash_command_descriptor(
    mut file: File,
) -> io::Result<fresh_provider::OrdinaryBashCommand> {
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    let required = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    let metadata = file.metadata()?;
    if seals < 0
        || seals & required != required
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > 64 * 1024
    {
        return Err(io::Error::other(
            "ordinary Bash C source descriptor unsealed or invalid",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(metadata.len() + 1).read_to_end(&mut bytes)?;
    if bytes.len() != metadata.len() as usize {
        return Err(io::Error::other("ordinary Bash C descriptor size changed"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

fn recv_request(
    stream: &mut UnixStream,
) -> io::Result<(u8, RequestPayload, libc::ucred, PinnedProcess)> {
    let fd = stream.as_raw_fd();
    let one: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            &one as *const _ as *const _,
            std::mem::size_of_val(&one) as _,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut original = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut original_len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            original.as_mut_ptr() as *mut _,
            &mut original_len,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if original_len as usize != std::mem::size_of::<libc::ucred>() {
        return Err(io::Error::other("bad peer credentials"));
    }
    let original = unsafe { original.assume_init() };
    let process = PinnedProcess::open(original.pid)?;
    // A queued request from a process that died before accept cannot answer a
    // fresh challenge. The pidfd/starttime pinned before the request must stay
    // live through classification and dispatch.
    let challenge = *uuid::Uuid::new_v4().as_bytes();
    stream.write_all(&challenge)?;
    let mut request = [0u8; REQUEST_RECEIVE_BUFFER_BYTES];
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 128];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len();
    let read = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if read < 0 {
        // A failed recvmsg installed no ancillary descriptors.
        return Err(io::Error::last_os_error());
    }
    let mut credentials = None;
    let mut descriptors = Vec::new();
    let mut invalid_ancillary = false;
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !cmsg.is_null() {
        let header = unsafe { &*cmsg };
        if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_CREDENTIALS {
            if credentials.is_some()
                || header.cmsg_len as usize
                    != unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as _) } as usize
            {
                invalid_ancillary = true;
            } else {
                credentials = Some(unsafe { *(libc::CMSG_DATA(cmsg) as *const libc::ucred) });
            }
        } else if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
            let base = unsafe { libc::CMSG_LEN(0) } as usize;
            let bytes = (header.cmsg_len as usize).saturating_sub(base);
            if (header.cmsg_len as usize) >= base
                && bytes.is_multiple_of(std::mem::size_of::<i32>())
            {
                for index in 0..bytes / std::mem::size_of::<i32>() {
                    let received = unsafe { *(libc::CMSG_DATA(cmsg) as *const i32).add(index) };
                    descriptors.push(unsafe { File::from_raw_fd(received) });
                }
            }
            if (header.cmsg_len as usize) < base
                || !bytes.is_multiple_of(std::mem::size_of::<i32>())
            {
                invalid_ancillary = true;
            }
        } else {
            invalid_ancillary = true;
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
    }
    let valid_length = match request[0] {
        b'G' | b'g' => read == 65,
        b'P' | b'p' => read == 37,
        b'Q' | b'Z' | b'q' | b'z' | b'D' | b'd' | b'c' => read == 33,
        b'C' => read == 33 || read == 34,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'X' => read == 34,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'%' | b'!' | b'<' => read == 33,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'u' | b'v' => read == 33,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'^' => read == 65,
        // Legacy E has no body; fresh Bash E carries a request UUID on its
        // separate socket. Preserve both exact wire shapes for pinned images.
        b'E' => read == 17 || read == 33,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'l' | b'M' => read == 49,
        b'A' | b'a' => read == 33,
        b'J' | b'j' => (18..=48 * 1024 + 17).contains(&read),
        b'L' => (18..=48 * 1024 + 17).contains(&read),
        b':' | b';' | b'+' => (18..=8192 + 17).contains(&read),
        #[cfg(feature = "age319-private-broker-fixture")]
        b'&' => (18..=2048 + 17).contains(&read),
        b'=' | b'V' | b'S' | b's' | b'T' | b'H' | b'K' | b'B' | b'N' | b'k' | b't' | b'R'
        | b'W' | b'Y' | b'0' | b'1' | b'2' | b'3' | b'4' => (18..=2048 + 17).contains(&read),
        #[cfg(feature = "age319-private-broker-fixture")]
        b'5' | b'6' | b'7' | b'8' | b'9' | b'b' | b'y' | b'x' | b'$' | b'*' | b'/' | b'>'
        | b'_' => (18..=2048 + 17).contains(&read),
        #[cfg(feature = "age319-private-broker-fixture")]
        b'h' | b'f' | b'(' | b')' | b'm' | b'n' | b'o' | b'w' | b'r' => {
            (18..=48 * 1024 + 17).contains(&read)
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        b'#' | b'{' | b'}' | b']' | b'|' | b'~' | b'?' => (18..=2048 + 17).contains(&read),
        b'@' | b'[' | 0x7f | 0x80 | 0x81 | 0x82 | 0x83 | 0x84 | 0x85 | 0x86 | 0x87 | 0x88
        | 0x89 | 0x8a | 0x8b | 0x8c | 0x8d => (18..=2048 + 17).contains(&read),
        b'F' => (18..=8192 + 17).contains(&read),
        b'O' => (18..=1024 + 17).contains(&read),
        b'U' => (18..=512 + 17).contains(&read),
        _ => read == 17,
    };
    if !valid_length
        || request.get(1..17) != Some(challenge.as_slice())
        || msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
    {
        return Err(io::Error::other("invalid challenged request"));
    }
    if invalid_ancillary
        || match request[0] {
            b'J' | b'j' | b'H' => descriptors.len() != 5,
            b'N' | b't' => descriptors.len() != 3,
            b'k' => descriptors.len() != 4,
            b'K' => descriptors.len() != 7,
            0x84 | 0x85 => descriptors.len() != 1,
            0x86 | 0x87 | 0x88 | 0x89 | 0x8a | 0x8b => descriptors.len() != 3,
            0x8c => descriptors.len() != 2,
            0x8d => !descriptors.is_empty(),
            #[cfg(feature = "age319-private-broker-fixture")]
            b'5' | b'b' => descriptors.len() != 4,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'9' => !(read == 33 && descriptors.is_empty()) && descriptors.len() != 4,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'h' | b'(' => descriptors.len() != 5,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'X' | b'^' | b'f' | b')' | b'w' | b'~' | b'?' => descriptors.len() != 1,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'#' => descriptors.len() != 2,
            b':' | b';' => descriptors.len() != 2,
            b'+' => descriptors.len() != 3,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'&' => descriptors.len() != 2,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'{' => descriptors.len() != 7,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'}' => descriptors.len() != 8,
            #[cfg(feature = "age319-private-broker-fixture")]
            b']' => !matches!(descriptors.len(), 0 | 2),
            b'L' => !(1..=4).contains(&descriptors.len()),
            b'=' | b'V' | b'S' | b's' | b'T' => descriptors.len() != 1,
            _ => !descriptors.is_empty(),
        }
    {
        return Err(io::Error::other("unsupported request ancillary data"));
    }
    let credentials =
        credentials.ok_or_else(|| io::Error::other("missing per-request credentials"))?;
    // The kernel resolves an explicitly supplied SCM_CREDENTIALS PID in the
    // sender's PID namespace before translating it to this host broker. A
    // privileged child namespace sender cannot name an ancestor-namespace
    // connector, even if that connector transferred this socket. Keep this
    // equality check and the pinned connector verification together; neither
    // SO_PEERCRED nor SCM_CREDENTIALS alone proves the current sender.
    if (credentials.pid, credentials.uid, credentials.gid)
        != (original.pid, original.uid, original.gid)
    {
        return Err(io::Error::other("transferred/inherited socket sender"));
    }
    process.verify()?;
    let payload = match request[0] {
        b'@' | b'[' | 0x7f => RequestPayload::RootDrain {
            expected: serde_json::from_slice(&request[17..read as usize])?,
        },
        0x80 | 0x81 | 0x82 | 0x83 => RequestPayload::OwnerCloseIntent {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'U' => RequestPayload::FreshChildRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'F' => RequestPayload::FreshRecipientRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'0' | b'1' | b'2' | b'3' | b'4' => RequestPayload::FreshRootEffectRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        0x84 | 0x85 => RequestPayload::FreshNormalModelRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            config_dir: descriptors.into_iter().next().unwrap(),
        },
        0x86 | 0x87 | 0x88 | 0x89 | 0x8a | 0x8b => {
            let mut descriptors = descriptors.into_iter();
            RequestPayload::FreshNormalPlanRequest {
                request: serde_json::from_slice(&request[17..read as usize])?,
                config_dir: descriptors.next().unwrap(),
                cwd: descriptors.next().unwrap(),
                environment: descriptors.next().unwrap(),
            }
        }
        0x8c | 0x8d => RequestPayload::FreshNormalPublicationRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'5' | b'6' | b'7' | b'b' | b'y' | b'x' | b'*' | b'/' | b'>' | b'_' => {
            RequestPayload::FreshProviderRequest {
                request: serde_json::from_slice(&request[17..read as usize])?,
                descriptors,
            }
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        b'$' => RequestPayload::PrivateNativeBashExec {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'8' | b'9' if read != 33 || !descriptors.is_empty() => {
            RequestPayload::FreshProviderRequest {
                request: serde_json::from_slice(&request[17..read as usize])?,
                descriptors,
            }
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        b'h' | b'f' | b'(' | b')' => RequestPayload::FreshRouteRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'#' | b'{' | b'}' | b']' | b'|' | b'~' | b'?' => {
            RequestPayload::FreshInteractivePtyHandoff {
                request: serde_json::from_slice(&request[17..read as usize])?,
                descriptors,
            }
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        b'm' | b'n' | b'o' => RequestPayload::FreshAccountEffectRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'w' => RequestPayload::ManualQuotaRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'r' => RequestPayload::ManualQuotaObserve {
            operation_id: serde_json::from_slice::<
                oulipoly_kernel_broker::protocol::ManualQuotaObserveRequest,
            >(&request[17..read as usize])?
            .operation_id,
        },
        b'D' | b'd' => RequestPayload::FreshSessionRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'C' | b'c' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            listener_policy: if request[0] == b'C' {
                Some(match request.get(33).copied().unwrap_or(0) {
                    0 => FreshBashListenerPolicy::ResponseOnly,
                    1 => FreshBashListenerPolicy::Notify,
                    _ => return Err(io::Error::other("fresh Bash C listener policy invalid")),
                })
            } else {
                None
            },
            #[cfg(feature = "age319-private-broker-fixture")]
            ordinary_command: if request[0] == b'C' && read > 34 {
                Some(serde_json::from_slice(&request[34..read as usize])?)
            } else {
                None
            },
            #[cfg(feature = "age319-private-broker-fixture")]
            ordinary_k_digest: None,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'X' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            listener_policy: Some(match request[33] {
                0 => FreshBashListenerPolicy::ResponseOnly,
                1 => FreshBashListenerPolicy::Notify,
                _ => return Err(io::Error::other("ordinary Bash C listener policy invalid")),
            }),
            ordinary_command: Some(read_ordinary_bash_command_descriptor(
                descriptors.into_iter().next().unwrap(),
            )?),
            ordinary_k_digest: None,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'%' | b'!' | b'8' | b'9' | b'v' | b'u' | b'<' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            listener_policy: None,
            ordinary_command: None,
            ordinary_k_digest: None,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'^' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            listener_policy: None,
            ordinary_command: Some(read_ordinary_bash_command_descriptor(
                descriptors.into_iter().next().unwrap(),
            )?),
            ordinary_k_digest: Some(request[33..65].try_into().unwrap()),
        },
        b'E' if read == 33 => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            listener_policy: None,
            #[cfg(feature = "age319-private-broker-fixture")]
            ordinary_command: None,
            #[cfg(feature = "age319-private-broker-fixture")]
            ordinary_k_digest: None,
        },
        b'O' => RequestPayload::FreshBashPrivateResult {
            result: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'P' | b'p' => RequestPayload::Prepare {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            guardian_pid: i32::from_ne_bytes(request[33..37].try_into().unwrap()),
        },
        b'G' | b'g' => RequestPayload::Bind {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            domain_id: uuid::Uuid::from_bytes(request[33..49].try_into().unwrap()).to_string(),
            supervisor_id: uuid::Uuid::from_bytes(request[49..65].try_into().unwrap()).to_string(),
        },
        b'A' | b'a' => RequestPayload::Read {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'Q' | b'q' => RequestPayload::ObserveAcceptedWork {
            grant_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'Z' | b'z' => RequestPayload::CancelAcceptedWork {
            grant_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'J' | b'j' => RequestPayload::Join {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("join descriptors"))?,
        },
        b'V' => RequestPayload::VerifyOwner {
            witness: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
        },
        b'=' => RequestPayload::DiscoverOwner {
            request: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
        },
        b':' => RequestPayload::ExactSourceDecision {
            request: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
            registration: descriptors.remove(0),
        },
        b';' => RequestPayload::VerifyExactSourceDecision {
            request: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
            registration: descriptors.remove(0),
        },
        b'+' => RequestPayload::PostcommitH {
            request: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
            registration: descriptors.remove(0),
            retained_guardian: descriptors.remove(0),
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'&' => RequestPayload::PrivateSourceWitness {
            probe: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
            registration: descriptors.remove(0),
        },
        b'S' | b's' => RequestPayload::VerifySourceSocket {
            witness: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
        },
        b'T' => RequestPayload::ConsumeSourceTicket {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            socket: descriptors.remove(0),
        },
        b'B' => RequestPayload::VerifyJoinedChild {
            witness: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'H' => RequestPayload::PrepareAcceptedWork {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("accepted-work descriptors"))?,
        },
        b'N' => RequestPayload::PrepareNative {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("native prepare descriptors"))?,
        },
        b'k' => RequestPayload::NativeK {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("native K descriptors"))?,
        },
        b't' => RequestPayload::NativeKV30 {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("v30 native K descriptors"))?,
        },
        b'K' => RequestPayload::LaunchAcceptedWork {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("accepted launch descriptors"))?,
        },
        b'R' => RequestPayload::StateRead {
            spec: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'W' => RequestPayload::StateWrite {
            spec: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'Y' => RequestPayload::StateGeneration {
            spec: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'L' => RequestPayload::InstalledLaunch {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'l' | b'M' => RequestPayload::PrivateLaunchStatus {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            generation: uuid::Uuid::from_bytes(request[33..49].try_into().unwrap()).to_string(),
            cancel: request[0] == b'M',
        },
        _ => RequestPayload::None,
    };
    Ok((request[0], payload, credentials, process))
}

fn peer_from_request(stream: &mut UnixStream) -> io::Result<(u8, RequestPayload, PeerIdentity)> {
    let (operation, payload, credentials, process) = recv_request(stream)?;
    Ok((
        operation,
        payload,
        PeerIdentity {
            uid: credentials.uid,
            gid: credentials.gid,
            process,
        },
    ))
}

fn root_launch_admitted(peer: &PeerIdentity, scope: &Scope, host_namespace: &File) -> bool {
    matches!(scope, Scope::Outside)
        && (peer.uid >= 1000 || private_fixture() && peer.uid == 0)
        && matches!(peer.process.in_namespace(host_namespace), Ok(true))
}

fn state_actor_matches(
    peer: &PeerIdentity,
    guardian: &PinnedProcess,
    guardian_stamp: &ProcessStamp,
    owner: &oulipoly_state::mailbox::CompletionDomainOwner,
    runner_image: &File,
) -> io::Result<bool> {
    let exact_guardian = oulipoly_state::completion_continuation::SourceProcessIdentity {
        pid: i64::from(guardian.host_pid),
        boot_id: guardian.boot_id.clone(),
        starttime_ticks: i64::try_from(guardian.starttime_ticks)
            .map_err(|_| io::Error::other("guardian starttime overflow"))?,
    };
    if owner.guardian_identity != exact_guardian {
        return Ok(false);
    }
    if ProcessStamp::from(&peer.process) == *guardian_stamp {
        return Ok(true);
    }
    Ok(peer.process.direct_child_of(guardian)?
        && peer.process.same_executable_as(runner_image)?
        && owner.driver_identity.pid == i64::from(peer.process.host_pid)
        && owner.driver_identity.boot_id == peer.process.boot_id
        && owner.driver_identity.starttime_ticks
            == i64::try_from(peer.process.starttime_ticks)
                .map_err(|_| io::Error::other("driver starttime overflow"))?)
}

/// Registry and kernel process identity select the State actor. A claimed
/// root/owner UUID or matching UID cannot turn a sibling into that actor.
#[cfg(feature = "age319-private-broker-fixture")]
fn verify_delegated_root_h_source(
    state_root: &Path,
    peer: &PeerIdentity,
    root_id: &str,
    request_id: &str,
) -> io::Result<(String, oulipoly_state::mailbox::FreshRootHDelegation)> {
    if !private_fixture() {
        return Err(io::Error::other("delegated root H is private only"));
    }
    let bash_path = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1")
        .ok_or_else(|| io::Error::other("delegated root H Bash image absent"))?;
    let bash_image = File::open(bash_path)?;
    let lane = FreshV30Lane::open_at(state_root).map_err(io::Error::other)?;
    let (root, root_actor, parent) = fresh_bash_parent(state_root, &lane, peer, Some(&bash_image))?;
    if root.old_release.prepared.root_id != root_id {
        return Err(io::Error::other("delegated root H work scope changed"));
    }
    let selected =
        fresh_provider::selected_root_h_k(&state_root.join("v30/fresh-provider"), &root, &parent)?;
    let mut child = lane
        .read_bash_child(request_id)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("delegated root H child absent"))?;
    child.session = lane
        .read_session(&child.d_key)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("delegated root H child D absent"))?;
    let actor = FreshRecipientIdentity {
        host_pid: peer.process.host_pid,
        boot_id: peer.process.boot_id.clone(),
        starttime_ticks: peer.process.starttime_ticks,
        pidns_dev: peer.process.pidns_dev,
        pidns_ino: peer.process.pidns_ino,
    };
    if child.actor != actor
        || child.parent_work_grant_id != parent.grant_id()
        || child.parent_work_id != parent.work_id()
    {
        return Err(io::Error::other(
            "delegated root H child actor or K changed",
        ));
    }
    let delegation = lane
        .require_consumed_root_h_delegation(&root, &root_actor, &child, &selected)
        .map_err(io::Error::other)?;
    peer.process.verify()?;
    if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
        let _ = fs::write(
            Path::new(&gate).join("delegated-h-broker-stage"),
            b"source-verified",
        );
    }
    Ok((parent.work_id().to_owned(), delegation))
}

#[cfg(feature = "age319-private-broker-fixture")]
fn verify_delegated_root_h_grant(
    proof: &DelegatedRootHProof,
    receipt: &Acceptance,
    spec: &AcceptedWorkSpec,
    consumed: &BTreeMap<String, DelegatedRootHProof>,
    state_root: &Path,
) -> io::Result<DelegatedRootHGrant> {
    if !private_fixture()
        || consumed.get(&proof.ticket_id) != Some(proof)
        || receipt.delegated_root_h.as_ref() != Some(proof)
        || proof.work_id != spec.work_id
        || proof.witness.root_id != spec.root_id
        || proof.witness.supervisor_id != receipt.supervisor_authority_id
        || !matches!(proof.witness.scope, SourceScope::Root)
        || receipt.initiator.pid != i64::from(proof.witness.source.host_pid)
        || receipt.initiator.boot_id != proof.witness.source.boot_id
        || receipt.initiator.starttime_ticks
            != i64::try_from(proof.witness.source.starttime_ticks)
                .map_err(|_| io::Error::other("delegated H source starttime overflow"))?
    {
        return Err(io::Error::other("delegated H ticket or acceptance changed"));
    }
    let source = PinnedProcess::open(proof.witness.source.host_pid)?;
    if !witness_matches(&proof.witness.source, &source)? {
        return Err(io::Error::other("delegated H source incarnation changed"));
    }
    let peer = PeerIdentity {
        uid: host_proc_uid(source.host_pid)?,
        gid: 0,
        process: source,
    };
    let (work_id, delegation) = verify_delegated_root_h_source(
        state_root,
        &peer,
        &spec.root_id,
        proof
            .witness
            .delegated_root_h_request_id
            .as_deref()
            .ok_or_else(|| io::Error::other("delegated H child selector absent"))?,
    )?;
    if work_id != delegation.selected_k.work_id
        || delegation.root_id != spec.root_id
        || delegation.owner_generation != spec.owner_generation
        || delegation.child_actor.host_pid != peer.process.host_pid
        || delegation.child_actor.boot_id != peer.process.boot_id
        || delegation.child_actor.starttime_ticks != peer.process.starttime_ticks
        || delegation.root_endpoint.is_empty()
    {
        return Err(io::Error::other(
            "delegated H selected K or original owner changed",
        ));
    }
    Ok(DelegatedRootHGrant {
        proof: proof.clone(),
        selected_k: delegation.selected_k,
        listener_policy: delegation.listener_policy,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent broker and State authority inputs"
)]
fn read_broker_state(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerContinuationReadback> {
    if !matches!(
        spec.protocol.as_str(),
        "broker-state-read-v1" | "broker-entry-running-readback-v30"
    ) || roots.has_debt()
        || entries.has_uncertain_write()
        || works.has_debt()
        || !matches!(
            classify_scope(peer, host_namespace, roots, works),
            Scope::Outside
        )
    {
        return Err(io::Error::other("broker State read admission refused"));
    }
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == spec.root_id)
        .ok_or_else(|| io::Error::other("broker State root absent"))?;
    let entry = entries
        .record(&spec.root_id)
        .ok_or_else(|| io::Error::other("broker State entry absent"))?;
    if !entry.join_consumed
        || entry.joined_child.is_none()
        || entry.owner_uid != peer.uid
        || root.record.owner_uid != peer.uid
        || entry.domain_id.is_none()
        || entry.supervisor_authority_id.is_none()
    {
        return Err(io::Error::other("broker State root binding absent"));
    }
    root.init.verify()?;
    let guardian_stamp = entry
        .guardian
        .as_ref()
        .ok_or_else(|| io::Error::other("broker State guardian absent"))?;
    let guardian = PinnedProcess::open(guardian_stamp.host_pid)?;
    if ProcessStamp::from(&guardian) != *guardian_stamp || !guardian.in_namespace(host_namespace)? {
        return Err(io::Error::other("broker State guardian changed"));
    }
    let readback = sidecar
        .read_exact_continuation(
            &spec.source_generation,
            &spec.root_id,
            entry.domain_id.as_deref().unwrap(),
            entry.supervisor_authority_id.as_deref().unwrap(),
            &spec.owner_generation,
            spec.attempt_id.as_deref(),
        )
        .map_err(io::Error::other)?;
    let exact_entry_read = if spec.protocol == "broker-entry-running-readback-v30"
        && spec.attempt_id.is_none()
        && entry.entry == ProcessStamp::from(&peer.process)
        && peer.process.same_executable_as(runner_image)?
        && let Some(stamp) = &entry.prepared_driver
    {
        let driver = PinnedProcess::open(stamp.host_pid)?;
        let matches = ProcessStamp::from(&driver) == *stamp
            && driver.direct_child_of(&guardian)?
            && driver.same_executable_as(runner_image)?
            && driver.in_namespace(host_namespace)?
            && i64::from(stamp.host_pid) == readback.owner.driver_identity.pid
            && stamp.starttime_ticks as i64 == readback.owner.driver_identity.starttime_ticks;
        driver.verify()?;
        matches
    } else {
        false
    };
    if !exact_entry_read
        && !state_actor_matches(
            peer,
            &guardian,
            guardian_stamp,
            &readback.owner,
            runner_image,
        )?
    {
        return Err(io::Error::other(
            "broker State caller is not exact entry, guardian or driver",
        ));
    }
    guardian.verify()?;
    peer.process.verify()?;
    Ok(readback)
}

fn encode_state_readback(
    readback: &oulipoly_state::mailbox::BrokerContinuationReadback,
) -> io::Result<String> {
    let mut response = serde_json::to_string(readback)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other("broker State readback too large"));
    }
    Ok(response)
}

fn broker_state_generation(
    spec: StateGenerationSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<String> {
    if !matches!(
        spec.protocol.as_str(),
        "broker-state-generation-v1" | "broker-prepared-generation-v30"
    ) || roots.has_debt()
        || entries.has_uncertain_write()
        || works.has_debt()
        || !matches!(
            classify_scope(peer, host_namespace, roots, works),
            Scope::Outside
        )
    {
        return Err(io::Error::other(
            "broker State generation admission refused",
        ));
    }
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == spec.root_id)
        .ok_or_else(|| io::Error::other("broker State root absent"))?;
    let entry = entries
        .record(&spec.root_id)
        .ok_or_else(|| io::Error::other("broker State entry absent"))?;
    if !entry.join_consumed
        || entry.joined_child.is_none()
        || root.record.owner_uid != peer.uid
        || entry.owner_uid != peer.uid
        || entry.domain_id.is_none()
        || entry.supervisor_authority_id.is_none()
        || entry.guardian.as_ref() != Some(&ProcessStamp::from(&peer.process))
    {
        return Err(io::Error::other(
            "broker State generation requires exact guardian",
        ));
    }
    root.init.verify()?;
    peer.process.verify()?;
    Ok(format!(
        "state-generation {}\n",
        sidecar.source_generation()
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent broker and State authority inputs"
)]
fn write_broker_state(
    spec: StateWriteSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &mut BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerContinuationReadback> {
    if spec.protocol != "broker-state-write-v1"
        || spec.source_generation != sidecar.source_generation()
    {
        return Err(io::Error::other(
            "broker State write version/generation conflict",
        ));
    }
    // The v30 prepared/held protocol has no committed release or child
    // post-gate attestation yet. Its old Publish action would commit a v18
    // running row before either exists, so keep this wire action closed.
    if matches!(&spec.action, StateWriteAction::Publish { .. }) {
        return Err(io::Error::other(
            "v30 running-owner publication requires held-J release protocol",
        ));
    }
    let read_spec = |attempt_id: Option<String>| StateReadSpec {
        protocol: "broker-state-read-v1".into(),
        source_generation: spec.source_generation.clone(),
        root_id: spec.root_id.clone(),
        owner_generation: spec.owner_generation.clone(),
        attempt_id,
    };
    match spec.action {
        StateWriteAction::Release => Err(io::Error::other("release requires held v30 protocol")),
        StateWriteAction::ReserveSourceGrant => Err(io::Error::other(
            "source grant reservation requires broker-selected v30 protocol",
        )),
        StateWriteAction::Prepare { .. } => {
            Err(io::Error::other("prepared owner requires v30 protocol"))
        }
        StateWriteAction::Revoke { attempt } => {
            let before = read_broker_state(
                read_spec(Some(attempt.attempt_id.clone())),
                peer,
                host_namespace,
                runner_image,
                roots,
                works,
                entries,
                sidecar,
            )?;
            if !before.broker_owned
                || before.owner.driver_identity.pid != i64::from(peer.process.host_pid)
                || before.attempt.as_ref() != Some(&attempt)
            {
                return Err(io::Error::other(
                    "broker withdrawal requires exact driver proposal",
                ));
            }
            sidecar
                .revoke_exact_unaccepted_attempt(&before.owner, &spec.root_id, &attempt)
                .map_err(io::Error::other)
        }
        StateWriteAction::Publish {
            driver_pid,
            endpoint,
        } => {
            if roots.has_debt()
                || entries.has_uncertain_write()
                || works.has_debt()
                || !matches!(
                    classify_scope(peer, host_namespace, roots, works),
                    Scope::Outside
                )
                || endpoint.is_empty()
                || endpoint.len() > 1024
            {
                return Err(io::Error::other(
                    "broker owner publication admission refused",
                ));
            }
            let root = roots
                .live_roots()
                .find(|root| root.record.root_id == spec.root_id)
                .ok_or_else(|| io::Error::other("broker owner root absent"))?;
            let entry = entries
                .record(&spec.root_id)
                .ok_or_else(|| io::Error::other("broker owner entry absent"))?;
            if !entry.join_consumed
                || entry.joined_child.is_none()
                || entry.owner_uid != peer.uid
                || root.record.owner_uid != peer.uid
                || entry.guardian.as_ref() != Some(&ProcessStamp::from(&peer.process))
            {
                return Err(io::Error::other(
                    "broker owner caller is not bound guardian",
                ));
            }
            root.init.verify()?;
            let driver = PinnedProcess::open(driver_pid)?;
            if !driver.direct_child_of(&peer.process)?
                || !driver.same_executable_as(runner_image)?
                || !driver.in_namespace(host_namespace)?
            {
                return Err(io::Error::other("broker owner driver is not exact child"));
            }
            let identity = |process: &PinnedProcess| -> io::Result<_> {
                Ok(
                    oulipoly_state::completion_continuation::SourceProcessIdentity {
                        pid: i64::from(process.host_pid),
                        boot_id: process.boot_id.clone(),
                        starttime_ticks: i64::try_from(process.starttime_ticks)
                            .map_err(|_| io::Error::other("process starttime overflow"))?,
                    },
                )
            };
            let owner = oulipoly_state::mailbox::CompletionDomainOwner {
                protocol: oulipoly_state::completion_continuation::PROTOCOL.into(),
                domain_id: entry
                    .domain_id
                    .clone()
                    .ok_or_else(|| io::Error::other("entry domain absent"))?,
                supervisor_authority_id: entry
                    .supervisor_authority_id
                    .clone()
                    .ok_or_else(|| io::Error::other("entry supervisor absent"))?,
                owner_generation: spec.owner_generation,
                guardian_identity: identity(&peer.process)?,
                driver_identity: identity(&driver)?,
                endpoint,
            };
            peer.process.verify()?;
            driver.verify()?;
            sidecar
                .publish_exact_owner(&owner, &spec.root_id)
                .map_err(io::Error::other)
        }
        StateWriteAction::Reserve { attempt } => {
            let before = read_broker_state(
                read_spec(None),
                peer,
                host_namespace,
                runner_image,
                roots,
                works,
                entries,
                sidecar,
            )?;
            if !before.broker_owned
                || attempt.owner_generation != spec.owner_generation
                || before.owner.guardian_identity.pid == i64::from(peer.process.host_pid)
                || before.owner.driver_identity.pid != i64::from(peer.process.host_pid)
            {
                return Err(io::Error::other("broker reservation requires exact driver"));
            }
            sidecar
                .reserve_exact_attempt(&before.owner, &spec.root_id, &attempt)
                .map_err(io::Error::other)
        }
        StateWriteAction::Accept { attempt_id } => {
            let before = read_broker_state(
                read_spec(Some(attempt_id)),
                peer,
                host_namespace,
                runner_image,
                roots,
                works,
                entries,
                sidecar,
            )?;
            if !before.broker_owned
                || before.owner.guardian_identity.pid != i64::from(peer.process.host_pid)
            {
                return Err(io::Error::other(
                    "broker acceptance requires exact guardian",
                ));
            }
            let attempt = before
                .attempt
                .ok_or_else(|| io::Error::other("broker attempt absent"))?;
            let (_, readback) = sidecar
                .accept_exact_attempt(&before.owner, &spec.root_id, &attempt)
                .map_err(io::Error::other)?;
            Ok(readback)
        }
        StateWriteAction::Repair { .. } | StateWriteAction::LaunchSourceGrant => {
            Err(io::Error::other("bounded repair requires v30 protocol"))
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained State authorities are independent"
)]
fn read_bounded_repair(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerRepairReadback> {
    if spec.protocol != "broker-repair-read-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other(
            "broker bounded repair read version conflict",
        ));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            ..spec
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker bounded repair requires exact driver",
        ));
    }
    sidecar
        .read_bounded_repair(&exact.source_generation, &exact.root_id, &exact.owner)
        .map_err(io::Error::other)
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained source authorities are independent"
)]
fn read_bounded_source_selection(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerSourceSelection> {
    if spec.protocol != "broker-source-selection-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("broker source selection version conflict"));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            ..spec
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker source selection requires exact driver",
        ));
    }
    sidecar
        .read_bounded_source_selection(&exact.source_generation, &exact.root_id, &exact.owner)
        .map_err(io::Error::other)
}

fn encode_source_selection(
    selection: &oulipoly_state::mailbox::BrokerSourceSelection,
) -> io::Result<String> {
    let mut response = serde_json::to_string(selection)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other(
            "broker source selection readback too large",
        ));
    }
    Ok(response)
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained recipient authorities are independent"
)]
fn read_bounded_recipient_selection(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &mut BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerRecipientSelection> {
    if spec.protocol != "broker-recipient-selection-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other(
            "broker recipient selection version conflict",
        ));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            ..spec
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker recipient selection requires exact driver",
        ));
    }
    sidecar
        .read_bounded_recipient_selection(&exact.source_generation, &exact.root_id, &exact.owner)
        .map_err(io::Error::other)
}

fn encode_recipient_selection(
    selection: &oulipoly_state::mailbox::BrokerRecipientSelection,
) -> io::Result<String> {
    let mut response = serde_json::to_string(selection)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other(
            "broker recipient selection readback too large",
        ));
    }
    Ok(response)
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained State authorities are independent"
)]
fn read_source_effect_grant(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &BrokerSidecar,
) -> io::Result<Option<oulipoly_state::mailbox::BrokerSourceEffectGrant>> {
    if spec.protocol != "broker-source-grant-read-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other(
            "broker source grant read version conflict",
        ));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            ..spec
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker source grant read requires exact driver",
        ));
    }
    sidecar
        .read_source_effect_grant(&exact.source_generation, &exact.root_id, &exact.owner)
        .map_err(io::Error::other)
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained State authorities are independent"
)]
fn reserve_source_effect_grant(
    spec: StateWriteSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &mut BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerSourceEffectGrant> {
    if spec.protocol != "broker-source-grant-reserve-v30"
        || !matches!(spec.action, StateWriteAction::ReserveSourceGrant)
    {
        return Err(io::Error::other(
            "broker source grant reservation version conflict",
        ));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            source_generation: spec.source_generation,
            root_id: spec.root_id,
            owner_generation: spec.owner_generation,
            attempt_id: None,
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker source grant reservation requires exact driver",
        ));
    }
    sidecar
        .reserve_source_effect_grant(&exact.source_generation, &exact.root_id, &exact.owner)
        .map_err(io::Error::other)
}

fn encode_source_effect_grant(
    grant: &Option<oulipoly_state::mailbox::BrokerSourceEffectGrant>,
) -> io::Result<String> {
    let mut response = serde_json::to_string(grant)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other("broker source grant readback too large"));
    }
    Ok(response)
}

#[expect(
    clippy::too_many_arguments,
    reason = "broker actor and retained State authorities are independent"
)]
fn write_bounded_repair(
    spec: StateWriteSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    sidecar: &mut BrokerSidecar,
) -> io::Result<oulipoly_state::mailbox::BrokerRepairReadback> {
    let StateWriteAction::Repair { expected_ordinal } = spec.action else {
        return Err(io::Error::other(
            "broker bounded repair write action conflict",
        ));
    };
    if spec.protocol != "broker-repair-write-v30" || expected_ordinal < 0 {
        return Err(io::Error::other(
            "broker bounded repair write version/cursor conflict",
        ));
    }
    let exact = read_broker_state(
        StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            source_generation: spec.source_generation,
            root_id: spec.root_id,
            owner_generation: spec.owner_generation,
            attempt_id: None,
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        sidecar,
    )?;
    if !exact.broker_owned || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid) {
        return Err(io::Error::other(
            "broker bounded repair requires exact driver",
        ));
    }
    sidecar
        .repair_bounded_suffix(
            &exact.source_generation,
            &exact.root_id,
            &exact.owner,
            expected_ordinal,
        )
        .map_err(io::Error::other)
}

fn encode_repair_readback(
    readback: &oulipoly_state::mailbox::BrokerRepairReadback,
) -> io::Result<String> {
    let mut response = serde_json::to_string(readback)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other("broker repair readback too large"));
    }
    Ok(response)
}

fn witness_matches(witness: &ProcessWitness, process: &PinnedProcess) -> io::Result<bool> {
    process.verify()?;
    Ok(witness.host_pid == process.host_pid
        && witness.boot_id == process.boot_id
        && witness.starttime_ticks == process.starttime_ticks)
}

#[expect(
    clippy::too_many_arguments,
    reason = "owner verification checks independent root, work, grant, image and socket trust roots"
)]
fn verify_owner_socket(
    witness: OwnerWitness,
    socket: File,
    peer: &PeerIdentity,
    runner_image: &File,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
) -> io::Result<String> {
    for id in [&witness.root_id, &witness.domain_id, &witness.supervisor_id] {
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid owner witness ID"))?;
    }
    // Entry debt includes the normal exit of the original host entry after
    // its one-use join. V is read-only: the live root and the exact joined
    // child, guardian, work and grant bindings below carry its authority.
    if roots.has_debt() || entries.has_uncertain_write() {
        return Err(io::Error::other("uncertain owner witness caller"));
    }
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == witness.root_id)
        .ok_or_else(|| io::Error::other("owner witness root absent"))?;
    let entry = entries
        .record(&witness.root_id)
        .ok_or_else(|| io::Error::other("owner witness entry absent"))?;
    if !entry.join_consumed
        || entry.owner_uid != peer.uid
        || entry.owner_uid != root.record.owner_uid
        || entry.domain_id.as_deref() != Some(&witness.domain_id)
        || entry.supervisor_authority_id.as_deref() != Some(&witness.supervisor_id)
    {
        return Err(io::Error::other("owner witness root binding changed"));
    }
    let joined_child = entry
        .joined_child
        .as_ref()
        .ok_or_else(|| io::Error::other("joined child absent"))?;
    let guardian_stamp = entry
        .guardian
        .as_ref()
        .ok_or_else(|| io::Error::other("owner witness guardian absent"))?;
    let original_child = joined_child == &ProcessStamp::from(&peer.process)
        && peer.process.direct_child_of(&root.init)?
        && peer.process.in_namespace(root.init.namespace())?
        && peer.process.same_executable_as(runner_image)?;
    if !original_child {
        // A sealed helper is accepted only from the exact live nested work
        // whose H grant was consumed by K. The work record and grant must
        // agree on the root, work, PID1 incarnation and original owner. The
        // accepted intent also pinned this helper inode and Runner digest.
        if works.has_debt() || grants.has_debt() {
            return Err(io::Error::other("uncertain sealed helper work"));
        }
        let Scope::Work {
            root_id,
            work_id,
            work_incarnation,
        } = classify_scope(peer, host_namespace, roots, works)
        else {
            return Err(io::Error::other("owner witness is outside consumed work"));
        };
        if root_id != witness.root_id {
            return Err(io::Error::other("owner helper root mismatch"));
        }
        if witness.work_id.as_deref() != Some(work_id.as_str()) {
            return Err(io::Error::other("owner helper work ID mismatch"));
        }
        let work = works
            .live_works()
            .find(|work| {
                work.record.root_id == root_id
                    && work.record.work_id == work_id
                    && work.record.work_incarnation == work_incarnation
            })
            .ok_or_else(|| io::Error::other("owner helper work absent"))?;
        let grant = grants
            .records()
            .iter()
            .find(|grant| {
                grant.grant_id == work.record.accepted_grant_id.as_deref().unwrap_or("")
                    && grant.root_id == root_id
                    && grant.work_id == work_id
                    && grant.consumed
            })
            .ok_or_else(|| io::Error::other("owner helper consumed grant absent"))?;
        let helper = grant
            .sealed_helper
            .as_ref()
            .ok_or_else(|| io::Error::other("owner helper was not pinned at H"))?;
        if grant.version != 3
            || witness.work_id.as_deref() != Some(grant.work_id.as_str())
            || grant.owner_uid != peer.uid
            || grant.root_init != ProcessStamp::from(&root.init)
            || grant.guardian != *guardian_stamp
            || grant.joined_child != *joined_child
            || grant.supervisor_authority_id != witness.supervisor_id
            || witness.owner_generation.as_deref() != Some(&grant.owner_generation)
        {
            return Err(io::Error::other("owner helper grant incarnation mismatch"));
        }
        if witness.registration_authority_sha256.as_deref()
            != Some(&helper.registration_authority_sha256)
            || witness.owner_session_id.as_deref() != Some(helper.owner_session_id.as_str())
            || witness.owner_invocation_uuid.as_deref()
                != Some(helper.owner_invocation_uuid.as_str())
        {
            return Err(io::Error::other(
                "owner helper registration witness mismatch",
            ));
        }
        if !helper.matches_live_executable(&peer.process)? {
            return Err(io::Error::other("owner helper pinned image mismatch"));
        }
        work.init.verify()?;
    }
    let guardian = PinnedProcess::open(guardian_stamp.host_pid)?;
    if ProcessStamp::from(&guardian) != *guardian_stamp
        || !witness_matches(&witness.guardian, &guardian)?
        || !guardian.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "owner witness guardian incarnation mismatch",
        ));
    }
    let driver = PinnedProcess::open(witness.driver.host_pid)?;
    if !witness_matches(&witness.driver, &driver)?
        || !driver.direct_child_of(&guardian)?
        || !driver.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "owner witness driver incarnation mismatch",
        ));
    }
    // Getsockopt is evaluated by this broker in the host PID namespace. The
    // child's SO_PEERCRED PID for this outside guardian can be zero/unmapped.
    let mut kind: libc::c_int = 0;
    let mut kind_len = std::mem::size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut libc::c_int).cast(),
            &mut kind_len,
        )
    } != 0
        || kind != libc::SOCK_STREAM
        || kind_len as usize != std::mem::size_of_val(&kind)
    {
        return Err(io::Error::other("owner witness is not a stream socket"));
    }
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of_val(&credentials)
        || credentials.pid != guardian.host_pid
        || credentials.uid != peer.uid
    {
        return Err(io::Error::other(
            "owner socket peer is not pinned host guardian",
        ));
    }
    guardian.verify()?;
    driver.verify()?;
    peer.process.verify()?;
    Ok(format!("verified-owner {}\n", witness.root_id))
}

#[expect(
    clippy::too_many_arguments,
    reason = "discovery joins live peer, held release, H/K and original State"
)]
fn discover_owner(
    request: OwnerDiscoveryRequest,
    socket: File,
    peer: &PeerIdentity,
    runner_image: &File,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &BrokerSidecar,
) -> io::Result<String> {
    let root_id = request.root_id.clone();
    uuid::Uuid::parse_str(&root_id).map_err(|_| io::Error::other("invalid discovery root"))?;
    let release = sidecar
        .read_released_owner_for_root(&root_id)
        .map_err(io::Error::other)?;
    let owner = &release.owner;
    if request
        .expected_owner_generation
        .as_deref()
        .is_some_and(|expected| expected != owner.owner_generation)
    {
        return Err(io::Error::other("owner discovery stale generation"));
    }
    let process = |pid: &oulipoly_state::completion_continuation::SourceProcessIdentity| -> io::Result<ProcessWitness> {
        Ok(ProcessWitness {
            host_pid: i32::try_from(pid.pid).map_err(|_| io::Error::other("invalid owner PID"))?,
            boot_id: pid.boot_id.clone(),
            starttime_ticks: u64::try_from(pid.starttime_ticks).map_err(|_| io::Error::other("invalid owner starttime"))?,
        })
    };
    let mut witness = OwnerWitness {
        root_id: root_id.clone(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: process(&owner.guardian_identity)?,
        driver: process(&owner.driver_identity)?,
        owner_generation: Some(owner.owner_generation.clone()),
        work_id: None,
        owner_session_id: None,
        owner_invocation_uuid: None,
        registration_authority_sha256: None,
    };
    let sealed = if let Scope::Work {
        root_id: scope_root,
        work_id,
        work_incarnation,
    } = classify_scope(peer, host_namespace, roots, works)
    {
        if scope_root != root_id || works.has_debt() || grants.has_debt() {
            return Err(io::Error::other("owner discovery work root/debt conflict"));
        }
        let work = works
            .live_works()
            .find(|work| {
                work.record.root_id == root_id
                    && work.record.work_id == work_id
                    && work.record.work_incarnation == work_incarnation
            })
            .ok_or_else(|| io::Error::other("owner discovery work absent"))?;
        let grant = grants
            .records()
            .iter()
            .find(|grant| {
                grant.consumed
                    && grant.grant_id == work.record.accepted_grant_id.as_deref().unwrap_or("")
                    && grant.root_id == root_id
                    && grant.work_id == work_id
            })
            .ok_or_else(|| io::Error::other("owner discovery consumed H/K absent"))?;
        let helper = grant
            .sealed_helper
            .as_ref()
            .ok_or_else(|| io::Error::other("owner discovery sealed helper absent"))?;
        witness.work_id = Some(grant.work_id.clone());
        witness.owner_session_id = Some(helper.owner_session_id.clone());
        witness.owner_invocation_uuid = Some(helper.owner_invocation_uuid.clone());
        witness.registration_authority_sha256 = Some(helper.registration_authority_sha256.clone());
        let capability_digest = helper
            .state_capability_digest
            .clone()
            .ok_or_else(|| io::Error::other("owner discovery H lacks original State digest"))?;
        Some((
            helper.owner_invocation_uuid.clone(),
            helper.owner_session_id.clone(),
            capability_digest,
            ProcessStamp::from(&work.init),
        ))
    } else {
        None
    };
    verify_owner_socket(
        witness.clone(),
        socket,
        peer,
        runner_image,
        host_namespace,
        roots,
        works,
        entries,
        grants,
    )?;
    let owner_generation = owner.owner_generation.as_str();
    let gate = held
        .get(&root_id)
        .ok_or_else(|| io::Error::other("owner discovery held D absent"))?;
    let release_id = gate
        .release_id()
        .ok_or_else(|| io::Error::other("owner discovery D not released"))?;
    if &release.release_id != release_id
        || release.owner.domain_id != witness.domain_id
        || release.owner.supervisor_authority_id != witness.supervisor_id
        || release.owner.owner_generation != owner_generation
        || release.prepared.joined_child
            != prepared_stamp(
                entries
                    .record(&root_id)
                    .and_then(|row| row.joined_child.as_ref())
                    .ok_or_else(|| io::Error::other("owner discovery joined child absent"))?,
            )
    {
        return Err(io::Error::other("owner discovery released binding changed"));
    }
    let joined = PinnedProcess::open(release.prepared.joined_child.host_pid)?;
    let root = roots
        .live_roots()
        .find(|row| row.record.root_id == root_id)
        .ok_or_else(|| io::Error::other("owner discovery root absent"))?;
    if prepared_stamp(&ProcessStamp::from(&joined)) != release.prepared.joined_child
        || !joined.direct_child_of(&root.init)?
        || !joined.in_namespace(root.init.namespace())?
        || !joined.same_executable_as(runner_image)?
    {
        return Err(io::Error::other("owner discovery joined child changed"));
    }
    joined.verify()?;
    let pid_binding =
        if let (Some(pid), Some((invocation_uuid, session_id, capability_digest, work_init))) =
            (request.query_pid, sealed.as_ref())
        {
            if pid == work_init.host_pid {
                let current = PinnedProcess::open(pid)?;
                if ProcessStamp::from(&current) != *work_init {
                    return Err(io::Error::other("owner discovery work PID1 changed"));
                }
                current.verify()?;
                sidecar
                    .read_bound_invocation_session(invocation_uuid, session_id, capability_digest)
                    .map_err(io::Error::other)?;
                Some(OwnerPidBinding {
                    pid,
                    invocation_uuid: invocation_uuid.clone(),
                    session_id: session_id.clone(),
                })
            } else {
                None
            }
        } else {
            None
        };
    // Even a domain-only read checks the original State binding for the H
    // owner; no copied mailbox or caller-selected path is consulted.
    if let Some((invocation_uuid, session_id, capability_digest, _)) = sealed {
        sidecar
            .read_bound_invocation_session(&invocation_uuid, &session_id, &capability_digest)
            .map_err(io::Error::other)?;
    }
    peer.process.verify()?;
    let response = OwnerDiscoveryReadback {
        root_id,
        source_generation: sidecar.source_generation().into(),
        release_id: release.release_id,
        owner: release.owner,
        pid_binding,
    };
    let encoded = serde_json::to_string(&response)?;
    if encoded.len() > 4096 {
        return Err(io::Error::other("owner discovery reply too large"));
    }
    Ok(format!("{encoded}\n"))
}

fn exact_registration_snapshot(file: &File, path: &Path) -> io::Result<(u64, u64, Vec<u8>)> {
    use std::os::unix::fs::FileExt;
    let fd_meta = file.metadata()?;
    let path_meta = fs::symlink_metadata(path)?;
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || !fd_meta.is_file()
        || !path_meta.is_file()
        || path_meta.file_type().is_symlink()
        || fd_meta.nlink() != 1
        || (fd_meta.dev(), fd_meta.ino(), fd_meta.len())
            != (path_meta.dev(), path_meta.ino(), path_meta.len())
        || fd_meta.len() == 0
        || fd_meta.len() > oulipoly_state::completion_continuation::MAX_REGISTRATION_BYTES as u64
    {
        return Err(io::Error::other(
            "private registration FD/path identity mismatch",
        ));
    }
    let independently_opened = oulipoly_state::completion_continuation::open_source_file(
        path.parent()
            .ok_or_else(|| io::Error::other("registration parent absent"))?,
        path.file_name()
            .ok_or_else(|| io::Error::other("registration name absent"))?
            .to_str()
            .ok_or_else(|| io::Error::other("registration name not UTF-8"))?,
        oulipoly_state::completion_continuation::MAX_REGISTRATION_BYTES,
    )
    .map_err(io::Error::other)?;
    let opened_meta = independently_opened.metadata()?;
    if (opened_meta.dev(), opened_meta.ino(), opened_meta.len())
        != (fd_meta.dev(), fd_meta.ino(), fd_meta.len())
    {
        return Err(io::Error::other("private registration path changed"));
    }
    let mut bytes = vec![0u8; fd_meta.len() as usize];
    let mut path_bytes = vec![0u8; bytes.len()];
    for (source, target) in [(file, &mut bytes), (&independently_opened, &mut path_bytes)] {
        let mut offset = 0;
        while offset < target.len() {
            let read = source.read_at(&mut target[offset..], offset as u64)?;
            if read == 0 {
                return Err(io::Error::other("private registration read shortened"));
            }
            offset += read;
        }
    }
    if bytes != path_bytes
        || file.metadata()?.len() != fd_meta.len()
        || independently_opened.metadata()?.len() != fd_meta.len()
        || fs::symlink_metadata(path)?.ino() != fd_meta.ino()
    {
        return Err(io::Error::other("private registration bytes changed"));
    }
    Ok((fd_meta.dev(), fd_meta.ino(), bytes))
}

/// Bind the creator in Bash's immutable registration to the *different*
/// sealed Runner issuer. A copied registration or a helper in another work
/// cannot manufacture the accepted H initiator, one-use K and direct worker.
fn exact_consumed_h_worker(
    probe: &SourceWitnessProbe,
    source: &oulipoly_state::completion_continuation::SourceRegistration,
    peer: &PeerIdentity,
    roots: &RootRegistry,
    works: &WorkRegistry,
    grants: &GrantRegistry,
) -> io::Result<ProcessStamp> {
    use std::io::Read;
    use std::os::unix::fs::FileExt;
    let work_id = probe
        .owner
        .work_id
        .as_deref()
        .ok_or_else(|| io::Error::other("consumed H work ID absent"))?;
    if works.has_debt() || grants.has_debt() {
        return Err(io::Error::other("consumed H work/grant debt"));
    }
    let work = works
        .live_works()
        .find(|work| work.record.root_id == probe.owner.root_id && work.record.work_id == work_id)
        .ok_or_else(|| io::Error::other("consumed H work absent"))?;
    let grant = grants
        .records()
        .iter()
        .find(|grant| {
            grant.grant_id == work.record.accepted_grant_id.as_deref().unwrap_or("")
                && grant.root_id == probe.owner.root_id
                && grant.work_id == work_id
                && grant.consumed
                && grant.version == 3
        })
        .ok_or_else(|| io::Error::other("consumed H grant absent"))?;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == probe.owner.root_id)
        .ok_or_else(|| io::Error::other("consumed H root absent"))?;
    let helper = grant
        .sealed_helper
        .as_ref()
        .ok_or_else(|| io::Error::other("consumed H helper absent"))?;
    if grant.owner_uid != peer.uid
        || grant.root_init != ProcessStamp::from(&root.init)
        || grant.supervisor_authority_id != probe.owner.supervisor_id
        || grant.owner_generation != probe.owner_generation
        || grant.guardian.host_pid != probe.owner.guardian.host_pid
        || grant.guardian.boot_id != probe.owner.guardian.boot_id
        || grant.guardian.starttime_ticks != probe.owner.guardian.starttime_ticks
        || helper.sha256 != source.helper.sha256
        || helper.owner_session_id != source.owner_session_id
        || helper.owner_invocation_uuid != source.owner_invocation_uuid
        || probe.owner.work_id.as_deref() != Some(grant.work_id.as_str())
    {
        return Err(io::Error::other("consumed H helper/grant binding changed"));
    }
    if let Some(delegated) = &grant.delegated_root_h {
        let expected_mode = match delegated.listener_policy.as_str() {
            "notify" => "async",
            "response_only" => "sync",
            _ => return Err(io::Error::other("consumed H listener policy invalid")),
        };
        if source.delivery_mode != expected_mode {
            return Err(io::Error::other("consumed H source delivery mode changed"));
        }
    }
    // verify_owner_socket already checked this exact peer against the sealed
    // executable; repeating a full image digest here would widen the gap
    // before the final live-process recheck.
    let initiator = PinnedProcess::open(
        i32::try_from(grant.initiator.pid)
            .map_err(|_| io::Error::other("consumed H initiator PID invalid"))?,
    )?;
    let initiator_namespace_matches = if let Some(binding) = &grant.delegated_root_h {
        // The accepted source was the selected K-owned Bash child. Its
        // namespace was proved against the consumed K at H preparation and
        // is carried by the immutable, one-use grant across Q admission.
        witness_matches(&binding.proof.witness.source, &initiator)?
    } else {
        initiator.in_namespace(root.init.namespace())?
    };
    if grant.initiator.boot_id != initiator.boot_id
        || u64::try_from(grant.initiator.starttime_ticks).ok() != Some(initiator.starttime_ticks)
        || !initiator_namespace_matches
    {
        return Err(io::Error::other("consumed H initiator incarnation changed"));
    }
    let worker = PinnedProcess::open(
        i32::try_from(source.registering_caller.pid)
            .map_err(|_| io::Error::other("consumed H registration worker PID invalid"))?,
    )?;
    if worker.boot_id != source.registering_caller.boot_id
        || i64::try_from(worker.starttime_ticks).ok()
            != Some(source.registering_caller.starttime_ticks)
        || !worker.direct_child_of(&work.init)?
        || !worker.in_namespace(work.init.namespace())?
        || !worker.same_executable_as(&host_proc_file(&format!("{}/exe", initiator.host_pid))?)?
        || !peer.process.direct_child_of(&worker)?
    {
        return Err(io::Error::other(
            "consumed H Bash worker/helper parentage changed",
        ));
    }
    let image = host_proc_file(&format!("{}/exe", initiator.host_pid))?;
    let image_meta = image.metadata()?;
    if (image_meta.dev(), image_meta.ino())
        != (
            grant.artifacts.executable.device,
            grant.artifacts.executable.inode,
        )
    {
        return Err(io::Error::other("consumed H Bash image changed"));
    }
    let path = probe
        .accepted_intent_path
        .as_ref()
        .ok_or_else(|| io::Error::other("consumed H accepted intent path absent"))?;
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(io::Error::other("consumed H accepted intent path invalid"));
    }
    let mut intent = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = intent.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if !meta.is_file()
        || !named.is_file()
        || named.file_type().is_symlink()
        || (meta.dev(), meta.ino()) != (grant.artifacts.intent.device, grant.artifacts.intent.inode)
        || (named.dev(), named.ino()) != (meta.dev(), meta.ino())
        || meta.len() == 0
        || meta.len() > 1024 * 1024
    {
        return Err(io::Error::other("consumed H accepted intent inode changed"));
    }
    let mut bytes = Vec::new();
    intent.read_to_end(&mut bytes)?;
    if bytes.len() as u64 != meta.len()
        || format!("{:x}", Sha256::digest(&bytes)) != grant.request_sha256
    {
        return Err(io::Error::other("consumed H accepted intent bytes changed"));
    }
    let selected: serde_json::Value = serde_json::from_slice(&bytes)?;
    if selected["work_id"] != work_id || selected["root_id"] != probe.owner.root_id {
        return Err(io::Error::other(
            "consumed H accepted intent work/root changed",
        ));
    }
    if selected["meta"]["delivery_helper"]["environment_sha256"] != source.helper.environment_sha256
    {
        return Err(io::Error::other(
            "consumed H/K helper environment selection changed",
        ));
    }
    let helper_environment_path =
        Path::new(&source.handle_dir).join("delivery-helper-environment.json");
    let mut helper_environment = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&helper_environment_path)?;
    let helper_environment_meta = helper_environment.metadata()?;
    if !helper_environment_meta.is_file()
        || helper_environment_meta.len() == 0
        || helper_environment_meta.len() > 1024 * 1024
        || fs::symlink_metadata(&helper_environment_path)?.ino() != helper_environment_meta.ino()
    {
        return Err(io::Error::other(
            "consumed H/K helper environment file changed",
        ));
    }
    let mut helper_environment_bytes = Vec::new();
    helper_environment.read_to_end(&mut helper_environment_bytes)?;
    if helper_environment_bytes.len() as u64 != helper_environment_meta.len()
        || format!("{:x}", Sha256::digest(&helper_environment_bytes))
            != source.helper.environment_sha256
    {
        return Err(io::Error::other(
            "consumed H/K helper environment bytes changed",
        ));
    }
    let entries = selected["environment"]
        .as_array()
        .ok_or_else(|| io::Error::other("consumed H selected environment absent"))?;
    let mut expected = std::collections::BTreeSet::new();
    for entry in entries {
        let field = |name: &str| -> io::Result<Vec<u8>> {
            entry[name]
                .as_array()
                .ok_or_else(|| io::Error::other("consumed H environment field absent"))?
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| io::Error::other("consumed H environment byte invalid"))
                })
                .collect()
        };
        let key = field("key")?;
        let value = field("value")?;
        if key.is_empty()
            || key.contains(&0)
            || key.contains(&b'=')
            || value.contains(&0)
            || !expected.insert([key, b"=".to_vec(), value].concat())
        {
            return Err(io::Error::other("consumed H selected environment invalid"));
        }
    }
    let mut actual = Vec::new();
    host_proc_file(&format!("{}/environ", worker.host_pid))?.read_to_end(&mut actual)?;
    let actual_count = actual
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .count();
    let actual: std::collections::BTreeSet<Vec<u8>> = actual
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.to_vec())
        .collect();
    let mut worker_status = String::new();
    host_proc_file(&format!("{}/status", worker.host_pid))?.read_to_string(&mut worker_status)?;
    let worker_gid = worker_status
        .lines()
        .find_map(|line| {
            line.strip_prefix("Gid:")
                .and_then(|fields| fields.split_whitespace().next())
                .and_then(|gid| gid.parse::<u32>().ok())
        })
        .ok_or_else(|| io::Error::other("consumed H worker physical GID absent"))?;
    // Delegated H restores the selected environment in the Q worker after
    // exec. procfs environ retains the exec-time block, so it cannot attest
    // that restoration. The sealed intent and helper environment bytes above
    // carry the immutable selection; the exact worker/work/grant checks carry
    // its physical identity. Keep the legacy procfs check for ordinary H.
    if (grant.delegated_root_h.is_none() && (actual != expected || actual_count != expected.len()))
        || host_proc_uid(worker.host_pid)? != grant.owner_uid
        || worker_gid != peer.gid
        || host_proc_uid(work.init.host_pid)? != 0
    {
        return Err(io::Error::other(
            "consumed H selected physical environment/account changed",
        ));
    }
    if cfg!(feature = "age319-private-broker-fixture") && grant.delegated_root_h.is_none() {
        let selected_account =
            format!("AGE319_SELECTED_ACCOUNT={}:{}", grant.owner_uid, worker_gid);
        if !actual.contains(selected_account.as_bytes()) {
            return Err(io::Error::other("consumed H selected C account changed"));
        }
    }
    let mut intent_again = vec![0; bytes.len()];
    let mut helper_environment_again = vec![0; helper_environment_bytes.len()];
    intent.read_exact_at(&mut intent_again, 0)?;
    helper_environment.read_exact_at(&mut helper_environment_again, 0)?;
    if intent_again != bytes
        || helper_environment_again != helper_environment_bytes
        || fs::symlink_metadata(path)?.ino() != meta.ino()
        || intent.metadata()?.len() != meta.len()
        || fs::symlink_metadata(&helper_environment_path)?.ino() != helper_environment_meta.ino()
        || helper_environment.metadata()?.len() != helper_environment_meta.len()
    {
        return Err(io::Error::other(
            "consumed H intent/helper environment changed during check",
        ));
    }
    initiator.verify()?;
    worker.verify()?;
    work.init.verify()?;
    Ok(ProcessStamp::from(&worker))
}

#[expect(
    clippy::too_many_arguments,
    reason = "source decision joins independent live authorities"
)]
fn verify_exact_source_witness(
    probe: SourceWitnessProbe,
    socket: File,
    registration: File,
    peer: &PeerIdentity,
    runner_image: &File,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
    sidecar: &BrokerSidecar,
    check_original_state: bool,
) -> io::Result<source_decision_journal::Claims> {
    let mark = |stage: &str| {
        if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
            let _ = fs::write(Path::new(&gate).join("source-broker-stage"), stage);
        }
    };
    mark("received");
    let before = exact_registration_snapshot(&registration, &probe.registration_path)?;
    mark("fd-snapshot");
    if before.2.len() as u64 != probe.registration_len
        || format!("{:x}", Sha256::digest(&before.2)) != probe.registration_sha256
    {
        return Err(io::Error::other(
            "private registration asserted bytes mismatch",
        ));
    }
    verify_owner_socket(
        probe.owner.clone(),
        socket,
        peer,
        runner_image,
        host_namespace,
        roots,
        works,
        entries,
        grants,
    )?;
    mark("live-v");
    let source: oulipoly_state::completion_continuation::SourceRegistration =
        serde_json::from_slice(&before.2)?;
    source.validate().map_err(io::Error::other)?;
    let (issuer_kind, registration_worker) = if probe.owner.work_id.is_some() {
        (
            ExactSourceIssuerKind::ConsumedHSealedHelper,
            exact_consumed_h_worker(&probe, &source, peer, roots, works, grants)?,
        )
    } else {
        if probe.accepted_intent_path.is_some() {
            return Err(io::Error::other("original J supplied an H intent"));
        }
        (
            ExactSourceIssuerKind::OriginalJoinedChild,
            ProcessStamp::from(&peer.process),
        )
    };
    if Path::new(&source.handle_dir).join(&source.registration_relative) != probe.registration_path
        || source.domain_id != probe.owner.domain_id
        || source.owner_invocation_uuid != probe.owner_invocation_uuid
        || source.owner_session_id != probe.owner_session_id
        || source.registering_caller.pid != i64::from(registration_worker.host_pid)
        || source.registering_caller.boot_id != registration_worker.boot_id
        || source.registering_caller.starttime_ticks != registration_worker.starttime_ticks as i64
        || probe.owner.owner_generation.as_deref() != Some(&probe.owner_generation)
        || probe.owner.owner_invocation_uuid.as_deref() != Some(&probe.owner_invocation_uuid)
        || probe.owner.owner_session_id.as_deref() != Some(&probe.owner_session_id)
        || probe.owner.registration_authority_sha256.as_deref()
            != Some(format!("{:x}", Sha256::digest(probe.capability.as_bytes())).as_str())
    {
        return Err(io::Error::other("private source/peer witness mismatch"));
    }
    let release = sidecar
        .read_exact_release(
            &probe.source_generation,
            &probe.owner.root_id,
            &probe.owner_generation,
        )
        .map_err(io::Error::other)?;
    mark("held-release");
    let prepared = &release.prepared;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == probe.owner.root_id)
        .ok_or_else(|| io::Error::other("source decision root absent"))?;
    let guardian = PinnedProcess::open(prepared.guardian.host_pid)?;
    let driver = PinnedProcess::open(prepared.driver.host_pid)?;
    let matches_stamp = |prepared: &PreparedProcessStamp, observed: &ProcessStamp| {
        prepared.host_pid == observed.host_pid
            && prepared.boot_id == observed.boot_id
            && prepared.starttime_ticks == observed.starttime_ticks
            && prepared.pidns_dev == observed.pidns_dev
            && prepared.pidns_ino == observed.pidns_ino
    };
    let peer_stamp = ProcessStamp::from(&peer.process);
    let joined_stamp = if issuer_kind == ExactSourceIssuerKind::OriginalJoinedChild {
        &peer_stamp
    } else {
        entries
            .record(&probe.owner.root_id)
            .and_then(|entry| entry.joined_child.as_ref())
            .ok_or_else(|| io::Error::other("consumed H original joined child absent"))?
    };
    if prepared.root_id != probe.owner.root_id
        || prepared.source_generation != probe.source_generation
        || prepared.owner_generation != probe.owner_generation
        || prepared.owner_uid != peer.uid
        || !matches_stamp(&prepared.root_init, &ProcessStamp::from(&root.init))
        || !matches_stamp(&prepared.joined_child, joined_stamp)
        || !matches_stamp(&prepared.guardian, &ProcessStamp::from(&guardian))
        || !matches_stamp(&prepared.driver, &ProcessStamp::from(&driver))
        || prepared.domain_id != probe.owner.domain_id
        || prepared.supervisor_authority_id != probe.owner.supervisor_id
        || (issuer_kind == ExactSourceIssuerKind::OriginalJoinedChild
            && (prepared.joined_child.host_pid != peer.process.host_pid
                || prepared.joined_child.boot_id != peer.process.boot_id
                || prepared.joined_child.starttime_ticks != peer.process.starttime_ticks))
        || prepared.guardian.host_pid != probe.owner.guardian.host_pid
        || prepared.guardian.boot_id != probe.owner.guardian.boot_id
        || prepared.guardian.starttime_ticks != probe.owner.guardian.starttime_ticks
        || prepared.driver.host_pid != probe.owner.driver.host_pid
        || prepared.driver.boot_id != probe.owner.driver.boot_id
        || prepared.driver.starttime_ticks != probe.owner.driver.starttime_ticks
    {
        return Err(io::Error::other(
            "private source held-release actor mismatch",
        ));
    }
    root.init.verify()?;
    guardian.verify()?;
    driver.verify()?;
    let capability =
        oulipoly_state::CompletionRegistrationAuthority::from_process_environment_value(
            &probe.capability,
        )
        .map_err(io::Error::other)?;
    let original_state = if check_original_state {
        sidecar
            .verify_bound_invocation(
                &probe.owner_invocation_uuid,
                &probe.owner_session_id,
                &capability,
            )
            .map_err(io::Error::other)?
    } else {
        // Metadata validation is nonwaiting under an exclusive SQLite State
        // writer. The future consumer must compare this journal identity to
        // the database it has already opened in its own transaction.
        sidecar
            .bound_state_file_identity()
            .map_err(io::Error::other)?
    };
    if check_original_state {
        mark("bound-state");
    }
    if before != exact_registration_snapshot(&registration, &probe.registration_path)? {
        return Err(io::Error::other(
            "private registration FD/bytes changed across check",
        ));
    }
    peer.process.verify()?;
    if issuer_kind == ExactSourceIssuerKind::ConsumedHSealedHelper {
        let worker = PinnedProcess::open(registration_worker.host_pid)?;
        if ProcessStamp::from(&worker) != registration_worker {
            return Err(io::Error::other(
                "consumed H worker incarnation changed after check",
            ));
        }
        worker.verify()?;
    }
    mark("complete");
    let mut digest = Sha256::new();
    digest.update(b"oulipoly-completion-registration-authority-v1");
    digest.update(probe.capability.as_bytes());
    let caller_admission_id =
        oulipoly_state::completion_continuation::completion_obligation_admission_id(
            &source.handle,
            &source.owner_invocation_uuid,
        );
    Ok(source_decision_journal::Claims {
        root_id: probe.owner.root_id,
        root_init: prepared.root_init.clone(),
        source_generation: probe.source_generation,
        sidecar_generation: sidecar
            .retained_mailbox_generation()
            .map_err(io::Error::other)?,
        owner_generation: probe.owner_generation,
        owner_uid: prepared.owner_uid,
        domain_id: source.domain_id,
        supervisor_id: probe.owner.supervisor_id,
        guardian: probe.owner.guardian,
        driver: probe.owner.driver,
        peer: ProcessStamp::from(&peer.process),
        issuer_kind,
        registration_worker,
        owner_session_id: source.owner_session_id,
        owner_invocation_uuid: source.owner_invocation_uuid,
        capability_digest: format!("{:x}", digest.finalize()),
        caller_admission_id,
        handle: source.handle,
        registration_id: source.registration_id,
        registration_path: probe.registration_path,
        registration_device: before.0,
        registration_inode: before.1,
        registration_bytes: before.2,
        original_state_device: original_state.device,
        original_state_inode: original_state.inode,
        registration_sha256: probe.registration_sha256,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "postcommit H joins live guardian, helper, H/K, journal and bound State"
)]
fn challenge_postcommit_h(
    request: PostcommitHChallenge,
    socket: File,
    registration: File,
    retained_guardian: File,
    guardian: &PeerIdentity,
    runner_image: &File,
    host_namespace: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
    sidecar: &BrokerSidecar,
    journal: &source_decision_journal::Journal,
) -> io::Result<PostcommitHReadback> {
    let verification = request.verification;
    let release = sidecar
        .read_released_owner_for_root(&verification.witness.owner.root_id)
        .map_err(io::Error::other)?;
    if prepared_stamp(&ProcessStamp::from(&guardian.process)) != release.prepared.guardian
        || !guardian.process.same_executable_as(runner_image)?
        || release.owner.owner_generation != verification.witness.owner_generation
    {
        return Err(io::Error::other(
            "postcommit H caller is not original guardian",
        ));
    }
    let connector = socket_credentials(&retained_guardian)?;
    let other_end = socket_credentials(&socket)?;
    if connector.pid <= 0
        || connector.uid != guardian.uid
        || other_end.pid != guardian.process.host_pid
        || other_end.uid != guardian.uid
    {
        return Err(io::Error::other(
            "postcommit H retained connection changed peers",
        ));
    }
    let helper = PeerIdentity {
        uid: connector.uid,
        gid: connector.gid,
        process: PinnedProcess::open(connector.pid)?,
    };
    let claims = verify_exact_source_witness(
        verification.witness,
        socket,
        registration,
        &helper,
        runner_image,
        host_namespace,
        roots,
        works,
        entries,
        grants,
        sidecar,
        true,
    )?;
    if claims.issuer_kind != ExactSourceIssuerKind::ConsumedHSealedHelper {
        return Err(io::Error::other(
            "postcommit H challenge did not identify consumed H",
        ));
    }
    let decision = journal.verify_committed_retry(
        &verification.request_id,
        &verification.decision_id,
        &claims,
    )?;
    let authority_ordinal = sidecar
        .read_postcommit_exact_source(
            &claims.registration_id,
            &verification.request_id,
            &verification.decision_id,
            &serde_json::to_vec(&decision)?,
            &claims.root_id,
            &release.owner,
        )
        .map_err(io::Error::other)?;
    guardian.process.verify()?;
    helper.process.verify()?;
    Ok(PostcommitHReadback {
        decision,
        authority_ordinal,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "verify one source against all independent broker trust roots"
)]
fn verify_source_socket(
    witness: SourceSocketWitness,
    socket: File,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
    state_root: &Path,
) -> io::Result<String> {
    for id in [&witness.root_id, &witness.domain_id, &witness.supervisor_id] {
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid source witness ID"))?;
    }
    // The selected K is created by the separate fresh broker after this
    // original broker has opened its registry. Reopen only for a claimed
    // delegated source; all other source scopes keep their existing snapshot.
    #[cfg(feature = "age319-private-broker-fixture")]
    let refreshed_works = if witness.delegated_root_h_request_id.is_some() && private_fixture() {
        Some(WorkRegistry::open(state_root.join("works"), roots)?)
    } else {
        None
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let works = refreshed_works.as_ref().unwrap_or(works);
    if roots.has_debt()
        || works.has_debt()
        || entries.has_uncertain_write()
        || grants.has_debt()
        || !witness_matches(&witness.source, &peer.process)?
    {
        return Err(io::Error::other("uncertain source witness caller"));
    }
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == witness.root_id)
        .ok_or_else(|| io::Error::other("source witness root absent"))?;
    let entry = entries
        .record(&witness.root_id)
        .ok_or_else(|| io::Error::other("source witness entry absent"))?;
    if !entry.join_consumed
        || entry.joined_child.is_none()
        || entry.owner_uid != root.record.owner_uid
        || entry.domain_id.as_deref() != Some(witness.domain_id.as_str())
        || entry.supervisor_authority_id.as_deref() != Some(witness.supervisor_id.as_str())
    {
        return Err(io::Error::other("source witness root binding mismatch"));
    }
    let guardian_stamp = entry
        .guardian
        .as_ref()
        .ok_or_else(|| io::Error::other("source witness guardian absent"))?;
    let guardian = PinnedProcess::open(guardian_stamp.host_pid)?;
    if ProcessStamp::from(&guardian) != *guardian_stamp
        || !witness_matches(&witness.guardian, &guardian)?
        || !guardian.in_namespace(host_namespace)?
        || !guardian.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "source witness guardian incarnation mismatch",
        ));
    }
    let classified = classify_scope(peer, host_namespace, roots, works);
    // Fresh selected K has its own physical work ledger. The original
    // registry can classify that nested PID namespace as Root because it has
    // no WorkRegistry row for fresh K. Promote only after the fresh physical
    // K, consumed delegation and exact Bash incarnation prove Work scope.
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut delegated_proof_work_id = None;
    #[cfg(feature = "age319-private-broker-fixture")]
    let scope = match classified {
        Scope::Root(root_id)
            if private_fixture()
                && matches!(&witness.scope, SourceScope::Root)
                && witness.delegated_root_h_request_id.is_some()
                && root_id == witness.root_id =>
        {
            let work_id = verify_delegated_root_h_source(
                state_root,
                peer,
                &root_id,
                witness.delegated_root_h_request_id.as_deref().unwrap(),
            )?
            .0;
            delegated_proof_work_id = Some(work_id.clone());
            Scope::Work {
                root_id,
                work_id,
                work_incarnation: String::new(),
            }
        }
        other => other,
    };
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let scope = classified;
    // A legitimate in-root sudo descendant may become host UID 0. Its exact
    // PID namespace and source incarnation still bind it to this root/work;
    // the outside cancel route retains the original owner UID.
    if peer.uid != entry.owner_uid
        && !(peer.uid == 0 && matches!(&scope, Scope::Root(_) | Scope::Work { .. }))
    {
        return Err(io::Error::other("source UID is outside root policy"));
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    let scope_diagnostic = format!(
        "scope={scope:?} claim={:?} delegated={}",
        witness.scope,
        witness.delegated_root_h_request_id.is_some(),
    );
    match (&witness.scope, scope) {
        (SourceScope::Root, Scope::Root(root_id))
            if root_id == witness.root_id && witness.delegated_root_h_request_id.is_none() => {}
        #[cfg(feature = "age319-private-broker-fixture")]
        (
            SourceScope::Root,
            Scope::Work {
                root_id, work_id, ..
            },
        ) if root_id == witness.root_id && witness.delegated_root_h_request_id.is_some() => {
            let proved_work_id = match delegated_proof_work_id {
                Some(id) => id,
                None => {
                    verify_delegated_root_h_source(
                        state_root,
                        peer,
                        &root_id,
                        witness.delegated_root_h_request_id.as_deref().unwrap(),
                    )?
                    .0
                }
            };
            if proved_work_id != work_id {
                return Err(io::Error::other("delegated root H work scope changed"));
            }
        }
        (
            SourceScope::Nested { parent_work_id },
            Scope::Work {
                root_id,
                work_id,
                work_incarnation,
            },
        ) if root_id == witness.root_id && work_id == *parent_work_id => {
            let work = works
                .live_works()
                .find(|work| work.record.work_incarnation == work_incarnation)
                .ok_or_else(|| io::Error::other("source parent work absent"))?;
            let grant = grants
                .records()
                .iter()
                .find(|grant| {
                    grant.grant_id == work.record.accepted_grant_id.as_deref().unwrap_or("")
                        && grant.root_id == witness.root_id
                        && grant.work_id == *parent_work_id
                        && grant.consumed
                })
                .ok_or_else(|| io::Error::other("source causal parent grant absent"))?;
            if grant.root_init != ProcessStamp::from(&root.init)
                || grant.guardian != *guardian_stamp
                || grant.supervisor_authority_id != witness.supervisor_id
            {
                return Err(io::Error::other("source causal parent changed"));
            }
            work.init.verify()?;
        }
        (SourceScope::CancelOutside { work_id }, Scope::Outside) => {
            if !peer.process.in_namespace(host_namespace)? {
                return Err(io::Error::other(
                    "outside cancel caller is not in host PID namespace",
                ));
            }
            let grant = grants
                .records()
                .iter()
                .find(|grant| grant.root_id == witness.root_id && grant.work_id == *work_id)
                .ok_or_else(|| io::Error::other("source cancellation grant absent"))?;
            if grant.root_init != ProcessStamp::from(&root.init)
                || grant.guardian != *guardian_stamp
                || grant.supervisor_authority_id != witness.supervisor_id
                || grant.owner_uid != peer.uid
            {
                return Err(io::Error::other("source cancellation grant changed"));
            }
        }
        _ => {
            #[cfg(feature = "age319-private-broker-fixture")]
            if private_fixture() {
                return Err(io::Error::other(format!(
                    "source root/work scope mismatch: {scope_diagnostic}"
                )));
            }
            return Err(io::Error::other("source root/work scope mismatch"));
        }
    }
    let mut kind: libc::c_int = 0;
    let mut kind_len = std::mem::size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut libc::c_int).cast(),
            &mut kind_len,
        )
    } != 0
        || kind != libc::SOCK_STREAM
        || kind_len as usize != std::mem::size_of_val(&kind)
    {
        return Err(io::Error::other("source witness is not a stream socket"));
    }
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of_val(&credentials)
        || credentials.pid != guardian.host_pid
        || credentials.uid != entry.owner_uid
    {
        return Err(io::Error::other(
            "source socket peer is not pinned host guardian",
        ));
    }
    guardian.verify()?;
    root.init.verify()?;
    peer.process.verify()?;
    Ok(format!("verified-source {}\n", witness.root_id))
}

struct SourceTicket {
    witness: SourceSocketWitness,
    source_socket: File,
    source_uid: u32,
    source_gid: u32,
    created: Instant,
}

fn socket_credentials(socket: &File) -> io::Result<libc::ucred> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of_val(&credentials) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of_val(&credentials)
    {
        return Err(io::Error::other(
            "source ticket socket has no exact peer credentials",
        ));
    }
    Ok(credentials)
}

fn issue_source_ticket(
    witness: SourceSocketWitness,
    source_socket: File,
    peer: &PeerIdentity,
    tickets: &mut BTreeMap<String, SourceTicket>,
) -> io::Result<String> {
    tickets.retain(|_, ticket| ticket.created.elapsed() < SOURCE_TICKET_TTL);
    if tickets.len() >= 128 {
        return Err(io::Error::other("source ticket capacity exhausted"));
    }
    let ticket = uuid::Uuid::new_v4();
    let mut marker = [0u8; 17];
    marker[0] = b'@';
    marker[1..].copy_from_slice(ticket.as_bytes());
    // The broker writes the marker through the exact source-side open file
    // description supplied to S. Only its connected guardian endpoint can
    // read that ticket; it is never returned to the source caller.
    if unsafe {
        libc::send(
            source_socket.as_raw_fd(),
            marker.as_ptr().cast(),
            marker.len(),
            libc::MSG_NOSIGNAL,
        )
    } != marker.len() as isize
    {
        return Err(io::Error::other(
            "source marker send failed; outcome uncertain",
        ));
    }
    let root_id = witness.root_id.clone();
    tickets.insert(
        ticket.to_string(),
        SourceTicket {
            witness,
            source_socket,
            source_uid: peer.uid,
            source_gid: peer.gid,
            created: Instant::now(),
        },
    );
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture()
        && tickets
            .values()
            .any(|ticket| ticket.witness.delegated_root_h_request_id.is_some())
    {
        if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
            let _ = fs::write(
                Path::new(&gate).join("delegated-h-broker-stage"),
                b"ticket-issued",
            );
        }
    }
    Ok(format!("verified-source-v2 {root_id}\n"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "consume must recheck every broker trust root"
)]
fn consume_source_ticket(
    spec: SourceTicketUse,
    accepted_socket: File,
    guardian_peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    grants: &GrantRegistry,
    tickets: &mut BTreeMap<String, SourceTicket>,
    consumed_delegated: &mut BTreeMap<String, DelegatedRootHProof>,
    state_root: &Path,
) -> io::Result<String> {
    let ticket = tickets
        .remove(&spec.ticket)
        .ok_or_else(|| io::Error::other("source ticket absent or already consumed"))?;
    if ticket.created.elapsed() >= SOURCE_TICKET_TTL {
        return Err(io::Error::other("source ticket expired"));
    }
    let root_id = ticket.witness.root_id.clone();
    match (&ticket.witness.scope, &spec.request) {
        (
            SourceScope::Root,
            SourceControlUse::WorkRoot {
                root_id: actual,
                work_id,
            },
        ) if actual == &root_id && !work_id.is_empty() && work_id.len() <= 256 => {}
        (
            SourceScope::Nested { parent_work_id },
            SourceControlUse::WorkNested {
                root_id: actual,
                work_id,
                parent_work_id: actual_parent,
            },
        ) if actual == &root_id
            && actual_parent == parent_work_id
            && !work_id.is_empty()
            && work_id.len() <= 256 => {}
        (
            SourceScope::CancelOutside { work_id },
            SourceControlUse::Cancel {
                root_id: actual,
                work_id: actual_work,
            },
        ) if actual == &root_id && actual_work == work_id => {}
        (
            SourceScope::Root | SourceScope::Nested { .. },
            SourceControlUse::Cancel {
                root_id: actual,
                work_id,
            },
        ) if actual == &root_id && !work_id.is_empty() && work_id.len() <= 256 => {}
        _ => return Err(io::Error::other("source ticket command or scope mismatch")),
    }
    let entry = entries
        .record(&root_id)
        .ok_or_else(|| io::Error::other("source ticket root entry absent"))?;
    let guardian_stamp = entry
        .guardian
        .as_ref()
        .ok_or_else(|| io::Error::other("source ticket guardian absent"))?;
    if ProcessStamp::from(&guardian_peer.process) != *guardian_stamp
        || !guardian_peer.process.in_namespace(host_namespace)?
        || !guardian_peer.process.same_executable_as(runner_image)?
        || guardian_peer.uid != entry.owner_uid
    {
        return Err(io::Error::other(
            "source ticket caller is not bound guardian",
        ));
    }
    let mut kind: libc::c_int = 0;
    let mut kind_len = std::mem::size_of_val(&kind) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            accepted_socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut libc::c_int).cast(),
            &mut kind_len,
        )
    } != 0
        || kind != libc::SOCK_STREAM
        || kind_len as usize != std::mem::size_of_val(&kind)
    {
        return Err(io::Error::other(
            "source ticket accepted endpoint is not a stream",
        ));
    }
    let connector = socket_credentials(&accepted_socket)?;
    if (connector.pid, connector.uid, connector.gid)
        != (
            ticket.witness.source.host_pid,
            ticket.source_uid,
            ticket.source_gid,
        )
    {
        return Err(io::Error::other(
            "source ticket connector differs from S requester",
        ));
    }
    let source = PeerIdentity {
        uid: ticket.source_uid,
        gid: ticket.source_gid,
        process: PinnedProcess::open(connector.pid)?,
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let delegated = ticket.witness.delegated_root_h_request_id.is_some();
    let witness = ticket.witness.clone();
    verify_source_socket(
        ticket.witness,
        ticket.source_socket,
        &source,
        host_namespace,
        runner_image,
        roots,
        works,
        entries,
        grants,
        state_root,
    )?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture() && delegated {
        let SourceControlUse::WorkRoot { work_id, .. } = &spec.request else {
            return Err(io::Error::other("delegated H ticket is not a root work H"));
        };
        let request_id = witness.delegated_root_h_request_id.as_ref().unwrap();
        if consumed_delegated
            .values()
            .any(|proof| proof.witness.delegated_root_h_request_id.as_ref() == Some(request_id))
        {
            return Err(io::Error::other(
                "delegated H ticket already consumed for child",
            ));
        }
        let proof = DelegatedRootHProof {
            ticket_id: spec.ticket,
            work_id: work_id.clone(),
            witness,
        };
        consumed_delegated.insert(proof.ticket_id.clone(), proof.clone());
        if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
            let _ = fs::write(
                Path::new(&gate).join("delegated-h-broker-stage"),
                b"ticket-consumed",
            );
        }
        guardian_peer.process.verify()?;
        return Ok(format!(
            "verified-control {root_id} {}\n",
            serde_json::to_string(&proof)?
        ));
    }
    #[cfg(not(feature = "age319-private-broker-fixture"))]
    let _ = (consumed_delegated, witness);
    guardian_peer.process.verify()?;
    Ok(format!("verified-control {root_id}\n"))
}

fn verify_joined_child(
    witness: JoinedChildWitness,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    entries: &EntryRegistry,
) -> io::Result<String> {
    let entry = entries
        .record(&witness.root_id)
        .ok_or_else(|| io::Error::other("joined-child root entry absent"))?;
    let child_stamp = entry
        .joined_child
        .as_ref()
        .ok_or_else(|| io::Error::other("joined child was not durably pinned"))?;
    if !entry.join_consumed
        || entry.guardian.as_ref() != Some(&ProcessStamp::from(&peer.process))
        || entry.owner_uid != peer.uid
        || !peer.process.in_namespace(host_namespace)?
        || !peer.process.same_executable_as(runner_image)?
        || child_stamp.host_pid != witness.child.host_pid
        || child_stamp.boot_id != witness.child.boot_id
        || child_stamp.starttime_ticks != witness.child.starttime_ticks
    {
        return Err(io::Error::other(
            "joined-child guardian or identity mismatch",
        ));
    }
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == witness.root_id)
        .ok_or_else(|| io::Error::other("joined-child root namespace absent"))?;
    let child = PinnedProcess::open(witness.child.host_pid)?;
    if ProcessStamp::from(&child) != *child_stamp
        || !child.direct_child_of(&root.init)?
        || !child.in_namespace(root.init.namespace())?
    {
        return Err(io::Error::other("joined child no longer matches root PID1"));
    }
    Ok(format!("verified-joined-child {}\n", witness.root_id))
}

#[expect(
    clippy::too_many_arguments,
    reason = "inject broker trust roots and registries for the production dispatch fixture"
)]
fn dispatch_authenticated(
    operation: u8,
    payload: RequestPayload,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    registry: &RootRegistry,
    works: &WorkRegistry,
    entries: &mut EntryRegistry,
) -> io::Result<String> {
    let scope = classify_scope(peer, host_namespace, registry, works);
    let admitted = root_launch_admitted(peer, &scope, host_namespace)
        && matches!(peer.process.same_executable_as(runner_image), Ok(true));
    match (operation, scope) {
        (b'C', Scope::Root(root)) => Ok(format!("inside {root}\n")),
        (
            b'C',
            Scope::Work {
                root_id,
                work_incarnation,
                ..
            },
        ) => Ok(format!("inside-work {root_id} {work_incarnation}\n")),
        (b'C', Scope::Outside) => Ok("outside\n".to_owned()),
        (b'C', Scope::Uncertain) => Ok("uncertain\n".to_owned()),
        (b'E', Scope::Outside)
            if admitted && !entries.has_debt() && !entries.has_unsettled_join() =>
        {
            if !matches!(payload, RequestPayload::None) {
                return Err(io::Error::other("entry reservation has a payload"));
            }
            entries
                .reserve(peer.uid, &peer.process)
                .map(|id| format!("reserved {id}\n"))
        }
        (b'E', _) => Err(io::Error::other("entry reservation denied")),
        (b'P', Scope::Outside) if admitted => {
            let RequestPayload::Prepare {
                root_id,
                guardian_pid,
            } = payload
            else {
                return Err(io::Error::other("missing guardian prepare"));
            };
            let guardian = PinnedProcess::open(guardian_pid)?;
            if host_proc_uid(guardian_pid)? != peer.uid
                || !guardian.same_executable_as(runner_image)?
            {
                return Err(io::Error::other("guardian UID mismatch"));
            }
            entries.prepare_guardian(&root_id, peer.uid, &peer.process, &guardian)?;
            Ok(format!("prepared {root_id}\n"))
        }
        (b'P', _) => Err(io::Error::other("guardian prepare denied")),
        (b'G', Scope::Outside) if admitted => {
            let RequestPayload::Bind {
                root_id,
                domain_id,
                supervisor_id,
            } = payload
            else {
                return Err(io::Error::other("missing guardian binding"));
            };
            entries.bind_guardian(
                &root_id,
                &domain_id,
                &supervisor_id,
                peer.uid,
                &peer.process,
            )?;
            Ok(format!("bound {root_id} {domain_id} {supervisor_id}\n"))
        }
        (b'G', _) => Err(io::Error::other("guardian binding denied")),
        (b'A', Scope::Outside) if admitted => {
            let RequestPayload::Read { root_id } = payload else {
                return Err(io::Error::other("missing entry readback"));
            };
            let bound = entries.bound_entry(&root_id, peer.uid, &peer.process)?;
            Ok(format!(
                "bound-entry {root_id} {} {} {}\n",
                bound.domain_id.as_ref().unwrap(),
                bound.supervisor_authority_id.as_ref().unwrap(),
                bound.guardian.as_ref().unwrap().host_pid
            ))
        }
        (b'A', _) => Err(io::Error::other("entry readback denied")),
        (b'L', _) => Err(io::Error::other("ungated root launch disabled")),
        _ => Err(io::Error::other("unknown operation")),
    }
}

fn prepared_stamp(stamp: &ProcessStamp) -> PreparedProcessStamp {
    PreparedProcessStamp {
        host_pid: stamp.host_pid,
        boot_id: stamp.boot_id.clone(),
        starttime_ticks: stamp.starttime_ticks,
        pidns_dev: stamp.pidns_dev,
        pidns_ino: stamp.pidns_ino,
    }
}

/// Require the live, retained gate and all exact registry incarnations. A
/// persisted prepared row after broker restart is debt, never current owner.
fn held_prepared_actors(
    root_id: &str,
    peer: &PeerIdentity,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
) -> io::Result<[ProcessStamp; 4]> {
    if roots.has_debt() || entries.has_uncertain_write() {
        return Err(io::Error::other("prepared root registry uncertain"));
    }
    let handle = held
        .get(root_id)
        .ok_or_else(|| io::Error::other("prepared gate absent"))?;
    if handle.root_id() != root_id {
        return Err(io::Error::other("prepared gate root changed"));
    }
    let actors = handle.actors()?;
    let record = entries
        .record(root_id)
        .ok_or_else(|| io::Error::other("prepared entry absent"))?;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == root_id)
        .ok_or_else(|| io::Error::other("prepared root absent"))?;
    root.init.verify()?;
    if !record.join_consumed
        || record.owner_uid != peer.uid
        || root.record.owner_uid != peer.uid
        || record.entry != actors[0]
        || record.guardian.as_ref() != Some(&actors[1])
        || record.joined_child.as_ref() != Some(&actors[3])
        || ProcessStamp::from(&root.init) != actors[2]
    {
        return Err(io::Error::other("prepared actor registry changed"));
    }
    Ok(actors)
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent broker actor and State authorities"
)]
fn prepare_broker_owner(
    spec: StateWriteSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    entries: &mut EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &mut BrokerSidecar,
) -> io::Result<PreparedBrokerOwner> {
    if spec.protocol != "broker-prepared-write-v30"
        || spec.source_generation != sidecar.source_generation()
        || !peer.process.in_namespace(host_namespace)?
    {
        return Err(io::Error::other("prepared owner source or version changed"));
    }
    let StateWriteAction::Prepare {
        driver_pid,
        endpoint,
    } = spec.action
    else {
        return Err(io::Error::other("prepared owner action required"));
    };
    if !Path::new(&endpoint).is_absolute() || endpoint.len() > 1024 || endpoint.contains('\0') {
        return Err(io::Error::other("invalid pending endpoint"));
    }
    let actors = held_prepared_actors(&spec.root_id, peer, roots, entries, held)?;
    let entry_process = PinnedProcess::open(actors[0].host_pid)?;
    if ProcessStamp::from(&peer.process) != actors[1]
        || !peer.process.same_executable_as(runner_image)?
        || !entry_process.same_executable_as(runner_image)?
    {
        return Err(io::Error::other(
            "prepared publication requires original guardian",
        ));
    }
    let driver = PinnedProcess::open(driver_pid)?;
    if !driver.direct_child_of(&peer.process)?
        || !driver.same_executable_as(runner_image)?
        || !driver.in_namespace(host_namespace)?
        || host_proc_uid(driver_pid)? != peer.uid
    {
        return Err(io::Error::other(
            "prepared driver is not exact guardian child",
        ));
    }
    // The proposed PID is only a lookup hint. The broker first seals the
    // observed driver incarnation into its durable entry registry, then
    // constructs State evidence from that sealed stamp.
    let driver_stamp =
        entries.bind_prepared_driver(&spec.root_id, peer.uid, &peer.process, &driver)?;
    let entry = entries
        .record(&spec.root_id)
        .ok_or_else(|| io::Error::other("prepared entry absent"))?;
    let prepared = PreparedBrokerOwner {
        source_generation: spec.source_generation,
        root_id: spec.root_id,
        owner_uid: peer.uid,
        domain_id: entry
            .domain_id
            .clone()
            .ok_or_else(|| io::Error::other("prepared domain absent"))?,
        supervisor_authority_id: entry
            .supervisor_authority_id
            .clone()
            .ok_or_else(|| io::Error::other("prepared supervisor absent"))?,
        owner_generation: spec.owner_generation,
        endpoint,
        entry: prepared_stamp(&actors[0]),
        guardian: prepared_stamp(&actors[1]),
        driver: prepared_stamp(&driver_stamp),
        root_init: prepared_stamp(&actors[2]),
        joined_child: prepared_stamp(&actors[3]),
    };
    peer.process.verify()?;
    driver.verify()?;
    sidecar
        .prepare_exact_owner(&prepared)
        .map_err(io::Error::other)
}

fn read_prepared_broker_owner(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &BrokerSidecar,
) -> io::Result<PreparedBrokerOwner> {
    if spec.protocol != "broker-prepared-read-v30"
        || spec.attempt_id.is_some()
        || !peer.process.in_namespace(host_namespace)?
    {
        return Err(io::Error::other("invalid prepared read"));
    }
    let actors = held_prepared_actors(&spec.root_id, peer, roots, entries, held)?;
    let row = sidecar
        .read_exact_prepared_owner(
            &spec.source_generation,
            &spec.root_id,
            &spec.owner_generation,
        )
        .map_err(io::Error::other)?;
    let driver = PinnedProcess::open(row.driver.host_pid)?;
    let registry_driver = entries
        .record(&spec.root_id)
        .and_then(|entry| entry.prepared_driver.as_ref())
        .ok_or_else(|| io::Error::other("prepared driver registry absent"))?;
    let entry_process = PinnedProcess::open(actors[0].host_pid)?;
    let guardian = PinnedProcess::open(actors[1].host_pid)?;
    let caller = ProcessStamp::from(&peer.process);
    if row.owner_uid != peer.uid
        || row.entry != prepared_stamp(&actors[0])
        || row.guardian != prepared_stamp(&actors[1])
        || row.root_init != prepared_stamp(&actors[2])
        || row.joined_child != prepared_stamp(&actors[3])
        || row.driver != prepared_stamp(&ProcessStamp::from(&driver))
        || row.driver != prepared_stamp(registry_driver)
        || !driver.direct_child_of(&guardian)?
        || !entry_process.same_executable_as(runner_image)?
        || !guardian.same_executable_as(runner_image)?
        || !driver.same_executable_as(runner_image)?
        || !driver.in_namespace(host_namespace)?
        || host_proc_uid(driver.host_pid)? != peer.uid
        || (caller != actors[0] && caller != actors[1] && caller != ProcessStamp::from(&driver))
    {
        return Err(io::Error::other("prepared read actor changed"));
    }
    driver.verify()?;
    peer.process.verify()?;
    Ok(row)
}

fn encode_prepared_owner(owner: &PreparedBrokerOwner) -> io::Result<String> {
    let mut response = serde_json::to_string(owner)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other("prepared owner readback too large"));
    }
    Ok(response)
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent live actor and State authorities"
)]
fn release_prepared_broker_owner(
    spec: StateWriteSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    held: &mut BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &mut BrokerSidecar,
) -> io::Result<BrokerReleaseEvidence> {
    if spec.protocol != "broker-held-release-v30"
        || !matches!(spec.action, StateWriteAction::Release)
        || spec.source_generation != sidecar.source_generation()
    {
        return Err(io::Error::other("invalid held release request"));
    }
    let read = StateReadSpec {
        protocol: "broker-prepared-read-v30".into(),
        source_generation: spec.source_generation.clone(),
        root_id: spec.root_id.clone(),
        owner_generation: spec.owner_generation.clone(),
        attempt_id: None,
    };
    let prepared = read_prepared_broker_owner(
        read,
        peer,
        host_namespace,
        runner_image,
        roots,
        entries,
        held,
        sidecar,
    )?;
    if ProcessStamp::from(&peer.process).host_pid != prepared.guardian.host_pid
        || ProcessStamp::from(&peer.process).starttime_ticks != prepared.guardian.starttime_ticks
    {
        return Err(io::Error::other("held release requires original guardian"));
    }
    let gate = held
        .get_mut(&spec.root_id)
        .ok_or_else(|| io::Error::other("held gate absent"))?;
    gate.write_v30_gate(&spec.source_generation, &spec.owner_generation)?;
    let evidence = match sidecar.commit_exact_prepared_release(&prepared) {
        Ok(evidence) => evidence,
        Err(commit_or_read_error) => sidecar
            .read_exact_release(
                &prepared.source_generation,
                &prepared.root_id,
                &prepared.owner_generation,
            )
            .map_err(|_| io::Error::other(commit_or_read_error))?,
    };
    if evidence.prepared != prepared {
        return Err(io::Error::other("release commit/readback identity changed"));
    }
    gate.record_release(evidence.release_id.clone());
    Ok(evidence)
}

#[expect(
    clippy::too_many_arguments,
    reason = "independent live actor and State authorities"
)]
fn read_released_broker_owner(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    entries: &EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &BrokerSidecar,
) -> io::Result<BrokerReleaseEvidence> {
    if spec.protocol != "broker-release-readback-v30" {
        return Err(io::Error::other("invalid release readback"));
    }
    let prepared = read_prepared_broker_owner(
        StateReadSpec {
            protocol: "broker-prepared-read-v30".into(),
            ..spec
        },
        peer,
        host_namespace,
        runner_image,
        roots,
        entries,
        held,
        sidecar,
    )?;
    if ProcessStamp::from(&peer.process).host_pid != prepared.guardian.host_pid {
        return Err(io::Error::other(
            "release readback requires original guardian",
        ));
    }
    let gate = held
        .get(&prepared.root_id)
        .ok_or_else(|| io::Error::other("held gate absent"))?;
    let release_id = gate
        .release_id()
        .ok_or_else(|| io::Error::other("held release not committed"))?;
    let evidence = sidecar
        .read_exact_release(
            &prepared.source_generation,
            &prepared.root_id,
            &prepared.owner_generation,
        )
        .map_err(io::Error::other)?;
    if evidence.prepared != prepared || evidence.release_id != release_id {
        return Err(io::Error::other("held release readback changed"));
    }
    Ok(evidence)
}

/// A gate byte is never authority. Even after the durable release commit,
/// only the exact original child can obtain this post-gate observation, and
/// every actor is reopened and compared with its prepared incarnation. The
/// A broker restart loses the retained gate and therefore cannot attest an
/// old committed row as a current child release.
#[expect(
    clippy::too_many_arguments,
    reason = "independent broker actor and State authorities"
)]
fn attest_released_child(
    spec: StateReadSpec,
    peer: &PeerIdentity,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: &BrokerSidecar,
    retained_gate_required: bool,
    require_clean_work_registry: bool,
) -> io::Result<BrokerReleaseEvidence> {
    if spec.protocol != "broker-release-attest-v30"
        || spec.attempt_id.is_some()
        || roots.has_debt()
        || require_clean_work_registry && works.has_debt()
        || entries.has_uncertain_write()
        || !matches!(
            if require_clean_work_registry {
                classify_scope(peer, host_namespace, roots, works)
            } else {
                classify_scope_readback(peer, host_namespace, roots, works)
            },
            Scope::Root(ref root) if root == &spec.root_id
        )
    {
        return Err(io::Error::other("released child attestation denied"));
    }
    let evidence = sidecar
        .read_exact_release(
            &spec.source_generation,
            &spec.root_id,
            &spec.owner_generation,
        )
        .map_err(io::Error::other)?;
    if retained_gate_required
        && held
            .get(&spec.root_id)
            .and_then(root_join::HeldRootJoin::release_id)
            != Some(evidence.release_id.as_str())
    {
        return Err(io::Error::other(
            "release gate is not retained by this broker",
        ));
    }
    let prepared = &evidence.prepared;
    let entry = entries
        .record(&spec.root_id)
        .ok_or_else(|| io::Error::other("released entry absent"))?;
    let root = roots
        .live_roots()
        .find(|root| root.record.root_id == spec.root_id)
        .ok_or_else(|| io::Error::other("released PID1 absent"))?;
    let entry_process = PinnedProcess::open(prepared.entry.host_pid)?;
    let guardian = PinnedProcess::open(prepared.guardian.host_pid)?;
    let driver = PinnedProcess::open(prepared.driver.host_pid)?;
    let actor_matches = |process: &PinnedProcess, stamp: &PreparedProcessStamp| {
        prepared_stamp(&ProcessStamp::from(process)) == *stamp
    };
    if entry.owner_uid != prepared.owner_uid
        || root.record.owner_uid != prepared.owner_uid
        || peer.uid != prepared.owner_uid
        || !entry.join_consumed
        || entry.entry != ProcessStamp::from(&entry_process)
        || entry.guardian.as_ref() != Some(&ProcessStamp::from(&guardian))
        || entry.prepared_driver.as_ref() != Some(&ProcessStamp::from(&driver))
        || entry.joined_child.as_ref() != Some(&ProcessStamp::from(&peer.process))
        || entry.domain_id.as_deref() != Some(prepared.domain_id.as_str())
        || entry.supervisor_authority_id.as_deref()
            != Some(prepared.supervisor_authority_id.as_str())
        || !actor_matches(&entry_process, &prepared.entry)
        || !actor_matches(&guardian, &prepared.guardian)
        || !actor_matches(&driver, &prepared.driver)
        || !actor_matches(&root.init, &prepared.root_init)
        || !actor_matches(&peer.process, &prepared.joined_child)
        || !guardian.direct_child_of(&entry_process)?
        || !driver.direct_child_of(&guardian)?
        || !peer.process.direct_child_of(&root.init)?
        || !entry_process.in_namespace(host_namespace)?
        || !guardian.in_namespace(host_namespace)?
        || !driver.in_namespace(host_namespace)?
        || !peer.process.in_namespace(root.init.namespace())?
        || !entry_process.same_executable_as(runner_image)?
        || !guardian.same_executable_as(runner_image)?
        || !driver.same_executable_as(runner_image)?
        || !peer.process.same_executable_as(runner_image)?
        || host_proc_uid(entry_process.host_pid)? != prepared.owner_uid
        || host_proc_uid(guardian.host_pid)? != prepared.owner_uid
        || host_proc_uid(driver.host_pid)? != prepared.owner_uid
        || host_proc_uid(peer.process.host_pid)? != prepared.owner_uid
    {
        return Err(io::Error::other("released actor incarnation changed"));
    }
    root.init.verify()?;
    entry_process.verify()?;
    guardian.verify()?;
    driver.verify()?;
    peer.process.verify()?;
    Ok(evidence)
}

struct FreshHandoffBridgeRequest {
    spec: StateReadSpec,
    peer: PeerIdentity,
    lane: FreshV30LaneIdentity,
    read_only: bool,
    reply: SyncSender<io::Result<FreshReleasedHandoff>>,
}

struct FreshTerminalBridgeRequest {
    root_id: String,
    settlement: EntryTerminalSettlement,
    reply: SyncSender<io::Result<()>>,
}

struct FreshDrainBridgeRequest {
    root: RootRecord,
    reply: SyncSender<io::Result<root_drain::RootDrainInventory>>,
}

fn fence_root_from_terminal(
    bridge: &SyncSender<FreshDrainBridgeRequest>,
    root: &FreshReleasedHandoff,
    read: &FreshRootTerminalReadback,
) -> io::Result<root_drain::RootDrainInventory> {
    let execution = read
        .execution
        .as_ref()
        .ok_or_else(|| io::Error::other("root execution absent before drain fence"))?;
    if read.execution_state == "unknown"
        || !read.unresolved_child_request_ids.is_empty()
        || execution.handoff_id != root.handoff_id
        || execution.d_key != root.d_key
        || execution.invocation_uuid != root.invocation_uuid
        || execution.root_id != root.old_release.prepared.root_id
        || execution.owner_generation != root.old_release.prepared.owner_generation
        || execution.actor != read.actor
    {
        return Err(io::Error::other(
            "root terminal lineage unresolved before drain fence",
        ));
    }
    let prepared = &root.old_release.prepared;
    let init = &prepared.root_init;
    let (reply, answer) = mpsc::sync_channel(1);
    bridge
        .send(FreshDrainBridgeRequest {
            root: RootRecord {
                version: 1,
                boot_id: init.boot_id.clone(),
                root_id: prepared.root_id.clone(),
                owner_uid: prepared.owner_uid,
                init_host_pid: init.host_pid,
                init_starttime_ticks: init.starttime_ticks,
                pidns_dev: init.pidns_dev,
                pidns_ino: init.pidns_ino,
            },
            reply,
        })
        .map_err(|_| io::Error::other("root drain authority unavailable"))?;
    answer
        .recv_timeout(RELEASED_HANDOFF_REPLY_TIMEOUT)
        .map_err(|_| io::Error::other("root drain fence response uncertain"))?
}

fn exact_entry_terminal_settlement(
    read: &FreshRootTerminalReadback,
) -> io::Result<EntryTerminalSettlement> {
    let execution = read
        .execution
        .as_ref()
        .ok_or_else(|| io::Error::other("root terminal execution absent"))?;
    if read.execution_state == "unknown"
        || !read.unresolved_child_request_ids.is_empty()
        || read.publication_state != "settled"
        || read.publication_sha256.is_none()
        || execution.root_id != read.root_id
        || execution.handoff_id != read.handoff_id
        || execution.d_key != read.d_key
        || execution.invocation_uuid != read.invocation_uuid
        || execution.session_id != read.session_id
        || execution.owner_generation != read.owner_generation
        || execution.actor != read.actor
    {
        return Err(io::Error::other(
            "root terminal or publication remains unresolved",
        ));
    }
    Ok(EntryTerminalSettlement {
        d_key: read.d_key.clone(),
        handoff_id: read.handoff_id.clone(),
        invocation_uuid: read.invocation_uuid.clone(),
        session_id: read.session_id.clone(),
        actor: ProcessStamp {
            host_pid: read.actor.host_pid,
            boot_id: read.actor.boot_id.clone(),
            starttime_ticks: read.actor.starttime_ticks,
            pidns_dev: read.actor.pidns_dev,
            pidns_ino: read.actor.pidns_ino,
        },
        parent_grant_id: execution.parent.grant_id.clone(),
        publication_sha256: read.publication_sha256.clone().unwrap(),
    })
}

fn settle_entry_from_terminal(
    bridge: &SyncSender<FreshTerminalBridgeRequest>,
    read: &FreshRootTerminalReadback,
) -> io::Result<()> {
    let settlement = exact_entry_terminal_settlement(read)?;
    let (reply, answer) = mpsc::sync_channel(1);
    bridge
        .send(FreshTerminalBridgeRequest {
            root_id: read.root_id.clone(),
            settlement,
            reply,
        })
        .map_err(|_| io::Error::other("terminal entry authority unavailable"))?;
    answer
        .recv_timeout(RELEASED_HANDOFF_REPLY_TIMEOUT)
        .map_err(|_| io::Error::other("terminal entry settlement response uncertain"))?
}

/// Every older entry must have both the original caller's immutable settled
/// Q result and the broker's exact physical/State owner close certificate.
/// This runs on the old writer loop under the shared fresh admission mutex.
#[expect(
    clippy::too_many_arguments,
    reason = "entry reuse joins all retained authorities"
)]
fn require_prior_entries_closed(
    state_root: &Path,
    roots: &mut RootRegistry,
    entries: &EntryRegistry,
    works: &mut WorkRegistry,
    grants: &GrantRegistry,
    sources: &SourcePhysicalRegistry,
    sidecar: Option<&BrokerSidecar>,
) -> io::Result<()> {
    if entries.records().is_empty() {
        return Ok(());
    }
    // A fresh lane call can itself wait on this old writer loop for terminal
    // settlement. Refuse its known-absent prerequisite before opening State.
    if entries
        .records()
        .iter()
        .any(|entry| entry.terminal_settlement.is_none())
    {
        return Err(io::Error::other("prior entry caller result unsettled"));
    }
    let known: HashSet<&str> = entries
        .records()
        .iter()
        .map(|entry| entry.root_id.as_str())
        .collect();
    if roots.live_roots().count() + roots.debt_records().len() != known.len()
        || works
            .live_works()
            .any(|work| !known.contains(work.record.root_id.as_str()))
        || works
            .debt_records()
            .iter()
            .any(|work| !known.contains(work.root_id.as_str()))
        || grants
            .records()
            .iter()
            .any(|grant| !known.contains(grant.root_id.as_str()))
        || grants
            .native_records()
            .iter()
            .any(|grant| !known.contains(grant.root_id.as_str()))
        || sources
            .records()
            .iter()
            .any(|source| !known.contains(source.grant.root_id.as_str()))
    {
        return Err(io::Error::other("unaccounted prior root or work debt"));
    }
    let lane = FreshV30Lane::open_at(state_root).map_err(io::Error::other)?;
    let mut closed_cursors: Vec<oulipoly_state::mailbox::BrokerStateCloseCursor> = Vec::new();
    for entry in entries.records() {
        let root = roots
            .record(&entry.root_id)
            .ok_or_else(|| io::Error::other("prior entry has no exact root"))?;
        let stored = entry
            .terminal_settlement
            .as_ref()
            .ok_or_else(|| io::Error::other("prior entry caller result unsettled"))?;
        let (handoff, actor) = lane
            .released_handoff_for_root(&entry.root_id)
            .map_err(io::Error::other)?;
        let session = lane
            .read_session(&handoff.d_key)
            .map_err(io::Error::other)?
            .ok_or_else(|| io::Error::other("prior entry session absent"))?;
        let terminal = lane
            .read_private_root_terminal(&handoff, &actor, &session)
            .map_err(io::Error::other)?;
        if &exact_entry_terminal_settlement(&terminal)? != stored {
            return Err(io::Error::other("prior entry publication identity changed"));
        }
        let inventory =
            root_drain::readback(root, roots, entries, works, grants, sources, sidecar)?;
        let proof = inventory
            .owner_close_proof
            .ok_or_else(|| io::Error::other("prior entry owner not exactly closed"))?;
        if inventory.entry_unsettled
            || inventory.state_sidecar_outstanding_unknown
            || proof.root_id != entry.root_id
            || proof.owner_generation != terminal.owner_generation
        {
            return Err(io::Error::other(
                "prior entry close or caller result changed",
            ));
        }
        closed_cursors.push(proof.state_cursor);
    }
    // Directory enumeration after restart has no generation order. Compare
    // the immutable ordinals themselves and reject aliasing or a source swap.
    closed_cursors.sort_by_key(|cursor| cursor.authority_ordinal);
    if closed_cursors.windows(2).any(|pair| {
        pair[0].authority_ordinal >= pair[1].authority_ordinal
            || pair[0].file != pair[1].file
            || pair[0].sidecar_generation != pair[1].sidecar_generation
    }) {
        return Err(io::Error::other("prior entry close cursor order changed"));
    }
    let current = sidecar
        .ok_or_else(|| io::Error::other("prior entry sidecar absent"))?
        .read_current_close_cursor()
        .map_err(io::Error::other)?;
    if closed_cursors.last() != Some(&current) {
        return Err(io::Error::other("unclosed State continuity generation"));
    }
    for entry in entries.records() {
        roots.admit_closed_historical(&entry.root_id)?;
        works.admit_closed_historical(&entry.root_id);
    }
    Ok(())
}

/// Revisit one retained original source per idle tick. Only the serving
/// Broker owns these registries; neither workload ingress nor drain readback
/// calls this transition. The seals themselves re-read terminal State, both
/// native ACKs and the exact physical witnesses before writing or readback.
#[expect(
    clippy::too_many_arguments,
    reason = "the two existing seals retain separate authorities"
)]
fn reconcile_one_native_retirement(
    cursor: &mut usize,
    completed: &mut HashSet<String>,
    state_root: &Path,
    roots: &RootRegistry,
    sources: &mut SourcePhysicalRegistry,
    works: &mut WorkRegistry,
    grants: &GrantRegistry,
    sidecar: &mut BrokerSidecar,
    terminal_dir: &Path,
) {
    let count = sources.records().len();
    if count == 0 {
        return;
    }
    let index = *cursor % count;
    *cursor = index.wrapping_add(1);
    let source = &sources.records()[index];
    let source_id = source.grant.grant_id.clone();
    if completed.contains(&source_id) || !roots.admission_fenced(&source.grant.root_id) {
        return;
    }
    let Some(root) = roots
        .live_roots()
        .map(|live| &live.record)
        .chain(roots.debt_records())
        .find(|root| root.root_id == source.grant.root_id)
        .cloned()
    else {
        return;
    };
    let matching_grants: Vec<_> = grants
        .records()
        .iter()
        .filter(|grant| {
            grant.consumed
                && grant.root_id == root.root_id
                && grant.joined_child == source.joined_child
                && grant.guardian == source.guardian
        })
        .collect();
    if matching_grants.is_empty() {
        return;
    }
    let Ok(lane) = FreshV30Lane::open_at(state_root) else {
        return;
    };
    if sources
        .retire_after_native_acks(&root, roots, sidecar, &lane, &source_id)
        .is_err()
    {
        return;
    }
    let mut all_retired = true;
    for grant in matching_grants {
        if works
            .retire_after_native_acks(
                &root,
                roots,
                grants,
                sources,
                sidecar,
                &lane,
                grant,
                terminal_dir,
            )
            .is_err()
        {
            all_retired = false;
        }
    }
    if all_retired {
        completed.insert(source_id);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "old gate and fresh identity are separate authorities"
)]
fn released_child_handoff(
    request: FreshHandoffBridgeRequest,
    host_namespace: &File,
    runner_image: &File,
    roots: &RootRegistry,
    works: &WorkRegistry,
    entries: &EntryRegistry,
    held: &BTreeMap<String, root_join::HeldRootJoin>,
    sidecar: Option<&BrokerSidecar>,
    registry: &mut released_handoff::ReleasedHandoffRegistry,
    broker_incarnation: &str,
) {
    let result = (|| -> io::Result<FreshReleasedHandoff> {
        let sidecar = sidecar.ok_or_else(|| io::Error::other("old release sidecar absent"))?;
        if registry.is_uncertain() {
            return Err(io::Error::other("released handoff has unknown debt"));
        }
        let existing = registry.existing(&request.spec.root_id).cloned();
        let root_id = request.spec.root_id.clone();
        if request.read_only && existing.is_none() {
            return Err(io::Error::other("released handoff receipt absent"));
        }
        let evidence = attest_released_child(
            request.spec,
            &request.peer,
            host_namespace,
            runner_image,
            roots,
            works,
            entries,
            held,
            sidecar,
            existing.is_none(),
            !request.read_only,
        )?;
        if let Some(receipt) = existing {
            if receipt.old_release != evidence || receipt.fresh_lane != request.lane {
                return Err(io::Error::other(
                    "released handoff identity or lane changed",
                ));
            }
            return Ok(receipt);
        }
        let root_work_intent = held
            .get(&root_id)
            .ok_or_else(|| io::Error::other("released root has no held child"))?
            .root_work_intent()?;
        // Only the old loop can mint root authority. The private selected-K
        // route keeps the exact J capability for a later one-use delegation.
        let authority = oulipoly_state::CompletionRegistrationAuthority::generate()
            .map_err(io::Error::other)?;
        let image = runner_image.metadata()?;
        let delegate_h = private_fixture()
            && std::env::var_os("AGE319_PRIVATE_SELECTED_K_ROOT_H_V1").is_some()
            && matches!(
                root_work_intent,
                oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(_)
            );
        let root_work_authority = if delegate_h {
            Some(
                held.get(&root_id)
                    .ok_or_else(|| io::Error::other("delegated root J absent"))?
                    .root_work_authority()
                    .to_owned(),
            )
        } else {
            None
        };
        let receipt = FreshReleasedHandoff {
            handoff_id: uuid::Uuid::new_v4().to_string(),
            d_key: uuid::Uuid::new_v4().to_string(),
            invocation_uuid: uuid::Uuid::new_v4().to_string(),
            root_work_intent,
            broker_incarnation: broker_incarnation.into(),
            runner_image_device: image.dev(),
            runner_image_inode: image.ino(),
            old_release: evidence,
            fresh_lane: request.lane,
            registration_authority: authority.process_environment_value().into(),
            delegated_h_listener_policy: delegate_h.then(|| {
                if std::env::var_os("AGE319_PRIVATE_BASH_ORIGINAL_NOTIFY_V1").is_some() {
                    "notify"
                } else {
                    "response_only"
                }
                .into()
            }),
            delegated_root_work_authority: root_work_authority,
        };
        registry.persist(receipt)
    })();
    let _ = request.reply.send(result);
}

fn encode_release_evidence(evidence: &BrokerReleaseEvidence) -> io::Result<String> {
    let mut response = serde_json::to_string(evidence)?;
    response.push('\n');
    if response.len() > 4096 {
        return Err(io::Error::other("release attestation too large"));
    }
    Ok(response)
}

// Observe terminal receipts while this broker is serving as well as after a
// restart. A source can finish after its W reply with no further socket
// traffic, so capture cannot depend on a later caller request. An exact
// independently admitted capture may advance Broker evidence to accepted.
// The source acceptance and Broker release decision are separate commits.
// Broker publication atomically triggers the source and notifications before
// the root-owned receipt is delivered to the original Bash handle.
fn capture_terminal_sources(
    sidecar: &mut BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    pending: &mut BTreeMap<String, BrokerSourceEffectGrant>,
) {
    let grants: Vec<_> = pending.values().cloned().collect();
    for grant in grants {
        match sidecar.read_source_evidence(&grant) {
            Ok(Some(row)) => {
                if row.phase == "captured"
                    && let Err(error) = commit_v2_evidence(sidecar, physical, &grant.grant_id)
                {
                    eprintln!(
                        "source evidence acceptance debt {}: {error}",
                        grant.grant_id
                    );
                    // A captured legacy row without fresh H provenance is
                    // durable debt. A restart can reassess it; polling the
                    // same immutable refusal every tick cannot promote it.
                    pending.remove(&grant.grant_id);
                    continue;
                }
                match sidecar.read_source_evidence(&grant) {
                    Ok(Some(accepted)) if accepted.phase == "accepted" => {
                        let result =
                            accept_v2_completion_source(sidecar, physical, &grant.grant_id)
                                .and_then(|_| {
                                    decide_v2_source_retention_release(
                                        sidecar,
                                        physical,
                                        &grant.grant_id,
                                    )?;
                                    // Keep the original source/listener/row
                                    // relationship exact at the serving
                                    // boundary. This is not recipient grant,
                                    // submission or ACK authority.
                                    let binding = sidecar.read_consumed_source_candidate(&grant)?;
                                    if binding.registration()?.delivery_mode == "async"
                                        && sidecar
                                            .read_v2_recipient_grant(&grant.grant_id)?
                                            .is_none()
                                    {
                                        read_v2_recipient_custody(
                                            sidecar,
                                            physical,
                                            &grant.grant_id,
                                        )?;
                                    }
                                    deliver_v2_source_retention_release(
                                        sidecar,
                                        physical,
                                        &grant.grant_id,
                                    )
                                });
                        match result {
                            Ok(_) => {
                                pending.remove(&grant.grant_id);
                            }
                            Err(error) => {
                                eprintln!("source boundary debt {}: {error}", grant.grant_id)
                            }
                        }
                    }
                    Ok(Some(unknown)) if unknown.phase == "unknown" => {
                        pending.remove(&grant.grant_id);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("source boundary readback debt {}: {error}", grant.grant_id)
                    }
                }
            }
            Ok(None) => match physical.observe(&grant.grant_id) {
                Ok(SourceObservation::Drained { .. }) => {
                    if let Err(error) =
                        capture_and_stage_v2_evidence(sidecar, physical, &grant.grant_id)
                    {
                        eprintln!("source evidence debt {}: {error}", grant.grant_id);
                    }
                    if sidecar
                        .read_source_evidence(&grant)
                        .is_ok_and(|row| row.as_ref().is_some_and(|r| r.phase == "captured"))
                        && let Err(error) = commit_v2_evidence(sidecar, physical, &grant.grant_id)
                    {
                        eprintln!(
                            "source evidence acceptance debt {}: {error}",
                            grant.grant_id
                        );
                    }
                    // Keep the grant in the bounded pending set until the
                    // separate source and release decisions are durable.
                }
                Ok(SourceObservation::Unknown { .. }) | Err(_) => {
                    if let Err(error) = sidecar.retain_unknown_source_evidence(&grant) {
                        eprintln!(
                            "source evidence unknown-debt readback failed {}: {error}",
                            grant.grant_id
                        );
                    } else {
                        pending.remove(&grant.grant_id);
                    }
                }
                _ => {}
            },
            Err(error) => {
                eprintln!(
                    "source evidence readback failed {}: {error}",
                    grant.grant_id
                );
            }
        }
    }
}

fn serve() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other("host root required"));
    }
    // Every broker-created SQLite main/WAL/SHM artifact must be owner-only.
    unsafe { libc::umask(0o077) };
    let fixture = private_fixture();
    // Installation is mandatory for serving, including restart. Failure to
    // create an independent observer is an admission failure, not a reason to
    // fall back to the mutable mounted /proc pathname.
    install_detached_host_proc()?;
    let same_namespace = |left: &str, right: &str| -> io::Result<bool> {
        let left = host_proc_file(left)?.metadata()?;
        let right = host_proc_file(right)?.metadata()?;
        Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
    };
    if !fixture && !same_namespace("self/ns/user", "1/ns/user")? {
        return Err(io::Error::other("initial user namespace required"));
    }
    if !fixture && !same_namespace("self/ns/pid", "1/ns/pid")? {
        return Err(io::Error::other("host PID namespace required"));
    }
    if !fixture {
        checked_root_path(
            Path::new("/usr/local/libexec/oulipoly/oulipoly-kernel-broker"),
            false,
        )?;
    }
    let state = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1")
            .map_err(|_| io::Error::other("missing private state"))?
    } else {
        STATE.into()
    };
    let socket = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
            .map_err(|_| io::Error::other("missing private socket"))?
    } else {
        SOCKET.into()
    };
    let runner = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1")
            .map_err(|_| io::Error::other("missing private Runner"))?
    } else {
        RUNNER.into()
    };
    if !fixture {
        checked_root_path(Path::new(&state), true)?;
        checked_root_path(Path::new("/run/oulipoly-kernel-broker"), true)?;
        checked_root_path(Path::new(&runner), false)?;
    }
    let installed_pair = if fixture {
        None
    } else {
        let pair = InstalledPair::load(Path::new(installed_pair::MANIFEST), true)?;
        pair.verify_image_against(
            Path::new(installed_pair::BROKER),
            &pair.broker_sha256,
            true,
            &host_proc_file("self/exe")?,
        )?;
        Some(pair)
    };
    // The singleton lock and durable latch are established before the socket
    // accepts any new request. Normal startup never closes or reopens it.
    let mut entry_gate = EntryGate::open(Path::new(&state))?;
    let sidecar_directory = Path::new(&state).join("sidecar");
    // A staged cutover is all-or-nothing at broker restart. Retain the exact
    // v30 connection for future broker State operations, while legacy native
    // N/k remains nonlaunching until the writer protocol is migrated.
    let mut broker_sidecar = match fs::symlink_metadata(&sidecar_directory) {
        Ok(_) => {
            let path = sidecar_directory.join("pid-identity.db");
            Some(BrokerSidecar::open_existing(&path, Path::new(&state)).map_err(io::Error::other)?)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if let Some(sidecar) = broker_sidecar.as_mut() {
        sidecar
            .orphan_reserved_source_grants()
            .map_err(io::Error::other)?;
    }
    let mut source_decision_journal =
        match fs::symlink_metadata(Path::new(&state).join("source-decisions")) {
            Ok(_) => Some(source_decision_journal::Journal::open(Path::new(&state))?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
    let runner_image = File::open(&runner)?;
    if let Some(pair) = &installed_pair {
        pair.verify_file(Path::new(&runner), &pair.runner_sha256, true, &runner_image)?;
    }
    let launcher_image = if let Some(pair) = &installed_pair {
        let digest = pair
            .launcher_sha256
            .as_deref()
            .ok_or_else(|| io::Error::other("installed launcher missing from pair"))?;
        let path = Path::new(installed_pair::LAUNCHER);
        checked_root_path(path, false)?;
        let image = File::open(path)?;
        pair.verify_file(path, digest, true, &image)?;
        Some(image)
    } else {
        None
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_launcher_image = if fixture {
        std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_LAUNCHER_V1")
            .map(File::open)
            .transpose()?
    } else {
        None
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_generation = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GENERATION_V1").ok()
    } else {
        None
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let private_launches = if fixture {
        Some(private_installed_exec::LaunchLedger::open(Path::new(
            &state,
        ))?)
    } else {
        None
    };
    let works_path = Path::new(&state).join("works");
    if !works_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&works_path)?;
    }
    if !fixture {
        checked_root_path(&works_path, true)?;
    }
    let source_physical_path = Path::new(&state).join("source-physical");
    if !source_physical_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&source_physical_path)?;
    }
    if !fixture {
        checked_root_path(&source_physical_path, true)?;
    }
    // Startup reads physical records independently of the original driver.
    // No source effect is enabled here: only a later exact held-child binding
    // may insert a consumed grant and open its gate.
    let mut source_physical = SourcePhysicalRegistry::open(&source_physical_path)?;
    for grant_id in source_physical.orphaned_grants() {
        eprintln!("source physical debt {grant_id}: no durable held record");
    }
    for (grant_id, result) in source_physical.reconcile_cancellations() {
        if let Err(error) = result {
            eprintln!("source cancellation debt {grant_id}: {error}");
        }
    }
    for record in source_physical.records() {
        match source_physical.observe(&record.grant.grant_id) {
            Ok(SourceObservation::Unknown {
                reason, diagnostic, ..
            }) => {
                eprintln!(
                    "source physical debt {}: {reason}; diagnostic={diagnostic:?}",
                    record.grant.grant_id
                );
            }
            Err(error) => {
                eprintln!("source physical debt {}: {error}", record.grant.grant_id);
            }
            _ => {}
        }
    }
    // After a broker restart, a drained one-use source can be captured from
    // retained State and the prior physical record. Never mint another W or
    // infer source acceptance from zero exit. Failed capture becomes debt.
    let mut pending_source_evidence: BTreeMap<_, _> = source_physical
        .records()
        .iter()
        .map(|record| (record.grant.grant_id.clone(), record.grant.clone()))
        .collect();
    let mut tracked_source_records = source_physical.records().len();
    if let Some(sidecar) = broker_sidecar.as_mut() {
        capture_terminal_sources(sidecar, &source_physical, &mut pending_source_evidence);
    }
    let host_namespace = host_proc_file("self/ns/pid")?;
    let mut registry = RootRegistry::open(&state)?;
    let admission_fences = Arc::new(Mutex::new(
        registry
            .fenced_root_ids()
            .map(str::to_owned)
            .collect::<HashSet<_>>(),
    ));
    let mut works = WorkRegistry::open(&works_path, &registry)?;
    let entries_path = Path::new(&state).join("entries");
    if !entries_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&entries_path)?;
    }
    if !fixture {
        checked_root_path(&entries_path, true)?;
    }
    let mut entries = EntryRegistry::open(&entries_path)?;
    let handoffs_path = Path::new(&state).join("released-handoffs");
    if !handoffs_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&handoffs_path)?;
        File::open(Path::new(&state))?.sync_all()?;
    }
    let mut released_handoffs = released_handoff::ReleasedHandoffRegistry::open(&handoffs_path)?;
    // A prepared grant can only be consumed by the exact K launch. Its work
    // record and terminal receipt remain separate durable obligations.
    let grants_path = Path::new(&state).join("grants");
    if !grants_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&grants_path)?;
    }
    if !fixture {
        checked_root_path(&grants_path, true)?;
    }
    let mut grants = GrantRegistry::open(&grants_path)?;
    let broker_incarnation = uuid::Uuid::new_v4().to_string();
    let mut settled_native_q = HashSet::new();
    let mut retirement_cursor = 0usize;
    // Process-local only: restart retries and revalidates every durable seal.
    let mut completed_native_retirements = HashSet::<String>::new();
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut attempted_v2_wakes = HashSet::<String>::new();
    // Ephemeral by design: a broker restart invalidates every pre-wire source
    // decision. No work/cancel action can be authorized by a lost ticket.
    let mut source_tickets = BTreeMap::<String, SourceTicket>::new();
    let mut consumed_delegated_h_tickets = BTreeMap::<String, DelegatedRootHProof>::new();
    // Only this serving incarnation owns the pre-exec gate. A restart opens
    // durable J/prepared debt but cannot recreate or release a lost gate.
    let mut held_joins = BTreeMap::<String, root_join::HeldRootJoin>::new();
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut private_launches = private_launches;
    let terminal_path = Path::new(&state).join("terminals");
    if !terminal_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&terminal_path)?;
    }
    if !fixture {
        checked_root_path(&terminal_path, true)?;
    }
    if let Some(sidecar) = broker_sidecar.as_mut() {
        native_work::reconcile_after_restart(
            &grants,
            &works,
            sidecar,
            &broker_incarnation,
            &terminal_path,
            &mut settled_native_q,
        );
    }
    if let Ok(meta) = fs::symlink_metadata(&socket) {
        if !meta.file_type().is_socket() || meta.uid() != 0 {
            return Err(io::Error::other("unsafe existing socket"));
        }
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660))?;
    listener.set_nonblocking(true)?;
    let (handoff_tx, handoff_rx): (
        SyncSender<FreshHandoffBridgeRequest>,
        Receiver<FreshHandoffBridgeRequest>,
    ) = mpsc::sync_channel(FRESH_HANDOFF_QUEUE_CAPACITY);
    let (terminal_tx, terminal_rx): (
        SyncSender<FreshTerminalBridgeRequest>,
        Receiver<FreshTerminalBridgeRequest>,
    ) = mpsc::sync_channel(FRESH_HANDOFF_QUEUE_CAPACITY);
    let (drain_tx, drain_rx): (
        SyncSender<FreshDrainBridgeRequest>,
        Receiver<FreshDrainBridgeRequest>,
    ) = mpsc::sync_channel(FRESH_HANDOFF_QUEUE_CAPACITY);
    // The old loop alone owns the release gate and mutable kernel registries.
    // Fresh storage stays on another thread and is opened only at the fixed
    // broker-owned v30 directory. Neither handler can wait on the other's
    // socket or select the other's State connection.
    let fresh_root = Path::new(&state).join("v30");
    if fs::symlink_metadata(&fresh_root).is_ok() {
        let fresh_state_root = PathBuf::from(&state);
        let fresh_runner_image = runner_image.try_clone()?;
        let fresh_handoff_tx = handoff_tx.clone();
        let fresh_terminal_tx = terminal_tx.clone();
        let fresh_drain_tx = drain_tx.clone();
        let fresh_admission_fences = Arc::clone(&admission_fences);
        let fresh_socket = if fixture {
            Path::new(&socket).with_file_name("v30.sock")
        } else {
            PathBuf::from(FRESH_SOCKET)
        };
        std::thread::Builder::new()
            .name("fresh-v30-lane".into())
            .spawn(move || {
                if let Err(error) = serve_fresh_v30_at(
                    &fresh_state_root,
                    &fresh_socket,
                    fresh_runner_image,
                    Some(fresh_handoff_tx),
                    Some(fresh_terminal_tx),
                    Some(fresh_drain_tx),
                    fresh_admission_fences,
                ) {
                    eprintln!("fresh v30 lane closed: {error}");
                }
            })?;
    }
    loop {
        if let Ok(request) = drain_rx.try_recv() {
            let result = (|| {
                registry.exact_record(&request.root)?;
                let mut fences = admission_fences
                    .lock()
                    .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                let persisted = registry.fence_admission(&request.root);
                fences.insert(request.root.root_id.clone());
                persisted?;
                drop(fences);
                root_drain::readback(
                    &request.root,
                    &registry,
                    &entries,
                    &works,
                    &grants,
                    &source_physical,
                    broker_sidecar.as_ref(),
                )
            })();
            let _ = request.reply.send(result);
        }
        if let Ok(request) = terminal_rx.try_recv() {
            let result = entries.settle_join(&request.root_id, request.settlement);
            let _ = request.reply.send(result);
        }
        if let Ok(request) = handoff_rx.try_recv() {
            released_child_handoff(
                request,
                &host_namespace,
                &runner_image,
                &registry,
                &works,
                &entries,
                &held_joins,
                broker_sidecar.as_ref(),
                &mut released_handoffs,
                &broker_incarnation,
            );
        }
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if let Some(sidecar) = broker_sidecar.as_mut() {
                    capture_terminal_sources(
                        sidecar,
                        &source_physical,
                        &mut pending_source_evidence,
                    );
                    #[cfg(feature = "age319-private-broker-fixture")]
                    if fixture
                        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_V1").is_some()
                        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                            .is_some_and(|dir| Path::new(&dir).join("v2-wake-activate").exists())
                    {
                        for record in source_physical.records() {
                            if sidecar
                                .read_source_evidence(&record.grant)
                                .is_ok_and(|row| row.is_some_and(|row| row.phase == "accepted"))
                                && attempted_v2_wakes.insert(record.grant.grant_id.clone())
                                && let Err(error) = v2_wake::activate(
                                    sidecar,
                                    &source_physical,
                                    &registry,
                                    &record.grant.grant_id,
                                )
                            {
                                eprintln!(
                                    "v2 recipient wake debt {}: {error}",
                                    record.grant.grant_id
                                );
                            }
                        }
                    }
                    native_work::reconcile_after_restart(
                        &grants,
                        &works,
                        sidecar,
                        &broker_incarnation,
                        &terminal_path,
                        &mut settled_native_q,
                    );
                    reconcile_one_native_retirement(
                        &mut retirement_cursor,
                        &mut completed_native_retirements,
                        Path::new(&state),
                        &registry,
                        &mut source_physical,
                        &mut works,
                        &grants,
                        sidecar,
                        &terminal_path,
                    );
                }
                // The original serving Broker alone can retain a parent wait
                // for its own PID1 children. A crash between wait and journal
                // leaves only PID1's independent durable ECHILD receipt.
                for root in registry.live_roots() {
                    if root_pid1::read_terminal(&registry.pid1_directory(), &root.record)
                        .is_ok_and(|receipt| receipt.is_some())
                    {
                        let _ = registry.reap_terminal_pid1(&root.record);
                    }
                }
                std::thread::sleep(BROKER_ACCEPT_POLL_INTERVAL);
                continue;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        stream.set_read_timeout(Some(BROKER_INGRESS_IO_TIMEOUT))?;
        stream.set_write_timeout(Some(BROKER_INGRESS_IO_TIMEOUT))?;
        let result = peer_from_request(&mut stream).and_then(|(operation, payload, peer)| {
            // Request parsing is bounded. Root launch/recovery readiness is
            // governed by exact gates and process death, not a 5s cutoff.
            stream.set_read_timeout(None)?;
            stream.set_write_timeout(None)?;
            require_cutover_entry_route(
                operation,
                broker_sidecar.is_some(),
                entry_gate.is_closed(),
            )?;
            if matches!(operation, 0x80..=0x83) {
                if peer.uid != 0 || !peer.process.in_namespace(&host_namespace)? {
                    return Err(io::Error::other("host-root owner close intent required"));
                }
                let RequestPayload::OwnerCloseIntent { request } = payload else {
                    return Err(io::Error::other("owner close intent request absent"));
                };
                registry.exact_record(&request.expected)?;
                if operation == 0x82 || operation == 0x83 {
                    let _guard = admission_fences.lock()
                        .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                    let intent = registry.read_close_intent(&request.expected)?
                        .ok_or_else(|| io::Error::other("owner close intent absent"))?;
                    if intent.owner_generation != request.owner_generation {
                        return Err(io::Error::other("owner close intent generation changed"));
                    }
                    let inventory = root_drain::readback(
                        &request.expected, &registry, &entries, &works, &grants,
                        &source_physical, broker_sidecar.as_ref(),
                    )?;
                    let proof = if let Some(proof) = inventory.owner_close_proof {
                        proof
                    } else if operation == 0x82 {
                        root_drain::ready_for_owner_close_preflight(
                            &inventory, &request.expected.root_id, &request.owner_generation,
                        )?;
                        let owner = inventory.owner_close_inventory.as_ref()
                            .ok_or_else(|| io::Error::other("owner close inventory absent"))?;
                        if owner.source_generation != intent.source_generation
                            || owner.state_cursor.as_ref() != Some(&intent.state_cursor)
                        {
                            return Err(io::Error::other("owner close intent cursor changed"));
                        }
                        let physical = root_drain::physical_close_proof(&inventory)?;
                        let candidate = oulipoly_state::mailbox::BrokerClosedOwner {
                            source_generation: intent.source_generation.clone(),
                            root_id: request.expected.root_id.clone(),
                            owner_generation: intent.owner_generation.clone(),
                            state_cursor: intent.state_cursor.clone(),
                            root_record_json: serde_json::to_string(&request.expected)?,
                            physical_proof_json: serde_json::to_string(&physical)?,
                        };
                        let sidecar_path = broker_sidecar.as_ref()
                            .ok_or_else(|| io::Error::other("owner close retained sidecar absent"))?
                            .mailbox().path().to_path_buf();
                        broker_sidecar.as_mut()
                            .ok_or_else(|| io::Error::other("owner close retained sidecar absent"))?
                            .close_exact_root_owner(&candidate, || {
                                let observer = BrokerSidecar::open_existing(
                                    &sidecar_path, Path::new(&state),
                                )?;
                                let again = root_drain::readback(
                                    &request.expected, &registry, &entries, &works, &grants,
                                    &source_physical, Some(&observer),
                                ).map_err(|error| error.to_string())?;
                                root_drain::ready_for_owner_close_preflight(
                                    &again, &request.expected.root_id, &request.owner_generation,
                                ).map_err(|error| error.to_string())?;
                                if root_drain::physical_close_proof(&again)
                                    .map_err(|error| error.to_string())? != physical
                                    || again.owner_close_inventory.as_ref()
                                        != inventory.owner_close_inventory.as_ref()
                                {
                                    return Err("owner close physical/ACK evidence changed under writers".into());
                                }
                                Ok(())
                            })
                            .map_err(io::Error::other)?
                    } else {
                        return Err(io::Error::other("owner close committed proof absent"));
                    };
                    if proof.root_id != request.expected.root_id
                        || proof.owner_generation != request.owner_generation
                        || proof.source_generation != intent.source_generation
                        || proof.state_cursor != intent.state_cursor
                    {
                        return Err(io::Error::other("owner close committed proof changed"));
                    }
                    return Ok(format!("owner-close-v1 {}\n", serde_json::to_string(&proof)?));
                }
                let intent = if operation == 0x80 {
                    // Fresh admissions holding this mutex finish first. The
                    // old Broker serves this request on its single writer loop.
                    let _guard = admission_fences.lock()
                        .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                    let inventory = root_drain::readback(
                        &request.expected, &registry, &entries, &works, &grants,
                        &source_physical, broker_sidecar.as_ref(),
                    )?;
                    root_drain::ready_for_owner_close_preflight(
                        &inventory, &request.expected.root_id, &request.owner_generation,
                    )?;
                    let owner = inventory.owner_close_inventory.as_ref()
                        .ok_or_else(|| io::Error::other("owner close inventory absent"))?;
                    registry.issue_close_intent(&OwnerCloseIntent {
                        root: request.expected.clone(),
                        owner_generation: request.owner_generation.clone(),
                        source_generation: owner.source_generation.clone(),
                        state_cursor: owner.state_cursor.clone()
                            .ok_or_else(|| io::Error::other("owner close cursor absent"))?,
                    })?
                } else {
                    registry.read_close_intent(&request.expected)?
                        .ok_or_else(|| io::Error::other("owner close intent absent"))?
                };
                if intent.owner_generation != request.owner_generation {
                    return Err(io::Error::other("owner close intent generation changed"));
                }
                Ok(format!("owner-close-intent-v1 {}\n", serde_json::to_string(&intent)?))
            } else if operation == b'@' || operation == b'[' || operation == 0x7f {
                if peer.uid != 0 || !peer.process.in_namespace(&host_namespace)? {
                    return Err(io::Error::other("host-root drain readback required"));
                }
                let RequestPayload::RootDrain { expected } = payload else {
                    return Err(io::Error::other("root drain identity absent"));
                };
                registry.exact_record(&expected)?;
                if operation == b'@' {
                    let mut fences = admission_fences.lock()
                        .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                    let persisted = registry.fence_admission(&expected);
                    // A partial fence write must also stop the fresh lane in
                    // this broker incarnation; restart rejects pending files.
                    fences.insert(expected.root_id.clone());
                    persisted?;
                }
                let inventory = root_drain::readback(
                    &expected, &registry, &entries, &works, &grants, &source_physical,
                    broker_sidecar.as_ref(),
                )?;
                if operation == 0x7f {
                    root_drain::ready_for_pid1_request(&inventory)?;
                    root_pid1::publish_request(&registry.pid1_directory(), &expected)?;
                    return Ok("root-pid1-drain-v1 requested\n".into());
                }
                Ok(format!("root-drain-v1 {}\n", serde_json::to_string(&inventory)?))
            } else if operation == b'i' {
                let route = if entry_gate.is_closed() {
                    "draining"
                } else if broker_sidecar.is_some() {
                    "broker-v30-closed"
                } else {
                    "legacy-open"
                };
                Ok(format!("entry-gate-v1 {route}\n"))
            } else if operation == b'v' {
                let pair = installed_pair
                    .as_ref()
                    .ok_or_else(|| io::Error::other("installed pair unavailable"))?;
                if !peer.process.same_executable_as(&runner_image)? {
                    return Err(io::Error::other("installed pair Runner image mismatch"));
                }
                let route = if entry_gate.is_closed() {
                    "draining"
                } else if broker_sidecar.is_some() {
                    "broker-v30-closed"
                } else {
                    "legacy-open"
                };
                Ok(format!(
                    "installed-pair-v1 {} {} {route}\n",
                    pair.version, pair.generation
                ))
            } else if operation == b'L' {
                #[cfg(feature = "age319-private-broker-fixture")]
                if fixture {
                    let RequestPayload::InstalledLaunch { spec, descriptors } = payload else {
                        return Err(io::Error::other("invalid private installed launch request"));
                    };
                    let image = private_launcher_image
                        .as_ref()
                        .ok_or_else(|| io::Error::other("private launcher image missing"))?;
                    let generation = private_generation
                        .as_deref()
                        .ok_or_else(|| io::Error::other("private launch generation missing"))?;
                    if !peer.process.same_executable_as(image)? || spec.generation != generation {
                        return Err(io::Error::other(
                            "private launcher image/generation mismatch",
                        ));
                    }
                    installed_launch::validate(
                        &spec,
                        &installed_launch::files_as_raw(&descriptors),
                    )?;
                    let ledger = private_launches
                        .as_mut()
                        .ok_or_else(|| io::Error::other("private launch ledger missing"))?;
                    private_installed_exec::launch(
                        ledger,
                        spec,
                        descriptors,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        stream.try_clone()?,
                    )?;
                    return Ok(String::new());
                }
                let pair = installed_pair
                    .as_ref()
                    .ok_or_else(|| io::Error::other("installed pair unavailable"))?;
                let image = launcher_image
                    .as_ref()
                    .ok_or_else(|| io::Error::other("installed launcher unavailable"))?;
                if !peer.process.same_executable_as(image)? {
                    return Err(io::Error::other("installed launcher image mismatch"));
                }
                let RequestPayload::InstalledLaunch { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid installed launch request"));
                };
                if spec.generation != pair.generation {
                    return Err(io::Error::other("installed launcher generation mismatch"));
                }
                installed_launch::validate(&spec, &installed_launch::files_as_raw(&descriptors))?;
                // The current guardian/State routes cannot keep a GUI, PTY,
                // provider descendants and arbitrary sudo grandchildren under
                // one root. Accept no production launch until that is true.
                Err(io::Error::other(
                    "installed supervisor transport staged; workload admission closed",
                ))
            } else if operation == b'l' || operation == b'M' {
                #[cfg(feature = "age319-private-broker-fixture")]
                if fixture {
                    let RequestPayload::PrivateLaunchStatus {
                        request_id,
                        generation,
                        cancel,
                    } = payload
                    else {
                        return Err(io::Error::other("invalid private launch status request"));
                    };
                    let image = private_launcher_image
                        .as_ref()
                        .ok_or_else(|| io::Error::other("private launcher image missing"))?;
                    if !peer.process.same_executable_as(image)? {
                        return Err(io::Error::other("private launch status image mismatch"));
                    }
                    if private_generation.as_deref() != Some(generation.as_str()) {
                        return Err(io::Error::other(
                            "private launch status generation mismatch",
                        ));
                    }
                    return private_launches
                        .as_ref()
                        .ok_or_else(|| io::Error::other("private launch ledger missing"))?
                        .status(&request_id, &generation, peer.uid, cancel);
                }
                Err(io::Error::other("production launch status closed"))
            } else if operation == b'X' || operation == b'x' {
                if peer.uid != 0 || !peer.process.in_namespace(&host_namespace)? {
                    return Err(io::Error::other("host-root gate transition required"));
                }
                if operation == b'X' {
                    entry_gate.close()?;
                    Ok("entry-gate-v1 draining\n".into())
                } else {
                    entry_gate.abort_before_publication()?;
                    Ok("entry-gate-v1 legacy-open\n".into())
                }
            } else if operation == b'J' || operation == b'j' {
                if !root_launch_admitted(
                    &peer,
                    &classify_scope(&peer, &host_namespace, &registry, &works),
                    &host_namespace,
                ) {
                    return Err(io::Error::other("root join outside admission denied"));
                }
                let RequestPayload::Join { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid root join payload"));
                };
                if registry.admission_fenced(&spec.root_id) {
                    return Err(io::Error::other("exact root admission fenced"));
                }
                if operation == b'j' {
                    if held_joins.contains_key(&spec.root_id) {
                        return Err(io::Error::other("root join gate already held"));
                    }
                    let held = root_join::hold(
                        spec,
                        descriptors,
                        &peer,
                        &runner_image,
                        &mut registry,
                        &mut entries,
                        true,
                    )?;
                    let actors = held.actors()?;
                    let root_id = held.root_id().to_owned();
                    held_joins.insert(root_id.clone(), held);
                    Ok(format!("held-joined {root_id} {}\n", actors[3].host_pid))
                } else {
                    root_join::launch(
                        spec,
                        descriptors,
                        &peer,
                        &runner_image,
                        &mut registry,
                        &mut entries,
                    )
                }
            } else if operation == b'V' {
                let RequestPayload::VerifyOwner { witness, socket } = payload else {
                    return Err(io::Error::other("invalid owner witness payload"));
                };
                verify_owner_socket(
                    witness,
                    socket,
                    &peer,
                    &runner_image,
                    &host_namespace,
                    &registry,
                    &works,
                    &entries,
                    &grants,
                )
            } else if operation == b'=' {
                let RequestPayload::DiscoverOwner { request, socket } = payload else {
                    return Err(io::Error::other("invalid owner discovery payload"));
                };
                discover_owner(
                    request, socket, &peer, &runner_image, &host_namespace,
                    &registry, &works, &entries, &grants, &held_joins,
                    broker_sidecar.as_ref().ok_or_else(|| io::Error::other("v30 owner sidecar absent"))?,
                )
            } else if operation == b':' {
                let RequestPayload::ExactSourceDecision { request, socket, registration } = payload else {
                    return Err(io::Error::other("invalid exact source decision payload"));
                };
                let sidecar = broker_sidecar.as_ref()
                    .ok_or_else(|| io::Error::other("exact source decision requires v30 sidecar"))?;
                let claims = verify_exact_source_witness(
                    request.witness, socket, registration, &peer, &runner_image, &host_namespace,
                    &registry, &works, &entries, &grants, sidecar, true,
                )?;
                if registry.admission_fenced(&claims.root_id) {
                    return Err(io::Error::other("exact root admission fenced"));
                }
                if source_decision_journal.is_none() {
                    source_decision_journal = Some(source_decision_journal::Journal::open(Path::new(&state))?);
                }
                let readback = source_decision_journal.as_mut().unwrap().issue(&request.request_id, &claims)?;
                Ok(format!("{}\n", serde_json::to_string(&readback)?))
            } else if operation == b';' {
                let RequestPayload::VerifyExactSourceDecision { request, socket, registration } = payload else {
                    return Err(io::Error::other("invalid exact source verification payload"));
                };
                let sidecar = broker_sidecar.as_ref()
                    .ok_or_else(|| io::Error::other("exact source verification requires v30 sidecar"))?;
                let claims = verify_exact_source_witness(
                    request.witness, socket, registration, &peer, &runner_image, &host_namespace,
                    &registry, &works, &entries, &grants, sidecar, false,
                )?;
                let journal = source_decision_journal.as_ref()
                    .ok_or_else(|| io::Error::other("exact source decision absent"))?;
                let readback = if request.committed_retry {
                    journal.verify_committed_retry(&request.request_id, &request.decision_id, &claims)?
                } else {
                    journal.verify_decision(&request.request_id, &request.decision_id, &claims)?
                };
                Ok(format!("{}\n", serde_json::to_string(&readback)?))
            } else if operation == b'+' {
                let RequestPayload::PostcommitH { request, socket, registration, retained_guardian } = payload else {
                    return Err(io::Error::other("invalid postcommit H challenge payload"));
                };
                let sidecar = broker_sidecar.as_ref()
                    .ok_or_else(|| io::Error::other("postcommit H requires v30 sidecar"))?;
                let journal = source_decision_journal.as_ref()
                    .ok_or_else(|| io::Error::other("postcommit H decision journal absent"))?;
                let readback = challenge_postcommit_h(
                    request, socket, registration, retained_guardian, &peer,
                    &runner_image, &host_namespace, &registry, &works, &entries, &grants,
                    sidecar, journal,
                )?;
                Ok(format!("{}\n", serde_json::to_string(&readback)?))
            } else if cfg!(feature = "age319-private-broker-fixture") && operation == b'&' {
                #[cfg(feature = "age319-private-broker-fixture")]
                {
                    if !private_fixture() { return Err(io::Error::other("private source witness unavailable")); }
                    let RequestPayload::PrivateSourceWitness { probe, socket, registration } = payload else {
                        return Err(io::Error::other("invalid private source witness payload"));
                    };
                    let claims = verify_exact_source_witness(
                        probe, socket, registration, &peer, &runner_image, &host_namespace,
                        &registry, &works, &entries, &grants,
                        broker_sidecar.as_ref().ok_or_else(|| io::Error::other("private source witness requires v30 sidecar"))?, true,
                    )?;
                    Ok(format!("private-source-witness {}\n", claims.root_id))
                }
                #[cfg(not(feature = "age319-private-broker-fixture"))]
                unreachable!()
            } else if operation == b'S' || operation == b's' {
                let RequestPayload::VerifySourceSocket { witness, socket } = payload else {
                    return Err(io::Error::other("invalid source socket witness payload"));
                };
                verify_source_socket(
                    witness.clone(),
                    socket.try_clone()?,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &registry,
                    &works,
                    &entries,
                    &grants,
                    Path::new(&state),
                )?;
                if operation == b's' {
                    if registry.admission_fenced(&witness.root_id) {
                        return Err(io::Error::other("exact root admission fenced"));
                    }
                    issue_source_ticket(witness, socket, &peer, &mut source_tickets)
                } else {
                    Ok(format!("verified-source {}\n", witness.root_id))
                }
            } else if operation == b'T' {
                let RequestPayload::ConsumeSourceTicket { spec, socket } = payload else {
                    return Err(io::Error::other("invalid source ticket use payload"));
                };
                consume_source_ticket(
                    spec,
                    socket,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &registry,
                    &works,
                    &entries,
                    &grants,
                    &mut source_tickets,
                    &mut consumed_delegated_h_tickets,
                    Path::new(&state),
                )
            } else if operation == b'B' {
                let RequestPayload::VerifyJoinedChild { witness } = payload else {
                    return Err(io::Error::other("invalid joined-child witness payload"));
                };
                verify_joined_child(
                    witness,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &registry,
                    &entries,
                )
            } else if operation == b'H' {
                let RequestPayload::PrepareAcceptedWork { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid accepted-work payload"));
                };
                if registry.admission_fenced(&spec.root_id) {
                    return Err(io::Error::other("exact root admission fenced"));
                }
                let [executable, intent, cwd, state_dir, accepted] = descriptors;
                let receipt: Acceptance = serde_json::from_slice(
                    &oulipoly_kernel_broker::accepted_grant::read_bounded(&accepted)?,
                )?;
                let delegated_root_h = match receipt.delegated_root_h.as_ref() {
                    Some(proof) => {
                        #[cfg(feature = "age319-private-broker-fixture")]
                        {
                            if !witness_matches(&proof.witness.guardian, &peer.process)?
                                || proof.witness.domain_id != entries.record(&spec.root_id)
                                    .and_then(|entry| entry.domain_id.as_ref()).map(String::as_str).unwrap_or("")
                            {
                                return Err(io::Error::other("delegated H guardian or domain changed"));
                            }
                            Some(verify_delegated_root_h_grant(
                                proof, &receipt, &spec, &consumed_delegated_h_tickets,
                                Path::new(&state),
                            )?)
                        }
                        #[cfg(not(feature = "age319-private-broker-fixture"))]
                        {
                            let _ = proof;
                            return Err(io::Error::other("delegated H grant is private only"));
                        }
                    }
                    None => None,
                };
                let grant = grants.prepare(
                    &registry,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &executable,
                    &cwd,
                    &state_dir,
                    &accepted,
                    &intent,
                    &spec.root_id,
                    &spec.work_id,
                    &spec.request_sha256,
                    &spec.accepted_sha256,
                    &spec.owner_generation,
                    delegated_root_h,
                )?;
                Ok(format!("prepared-work {}\n", grant.grant_id))
            } else if operation == b'N' {
                let RequestPayload::PrepareNative { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid native prepare payload"));
                };
                if registry.admission_fenced(&spec.root_id) {
                    return Err(io::Error::other("exact root admission fenced"));
                }
                let [directory, request, receipt] = descriptors;
                if let Some(sidecar) = broker_sidecar.as_mut() {
                    if spec.protocol != "native-continuation-v30" {
                        return Err(io::Error::other("v30 native prepare protocol required"));
                    }
                    let exact = read_broker_state(
                        StateReadSpec {
                            protocol: "broker-state-read-v1".into(),
                            source_generation: sidecar.source_generation().into(),
                            root_id: spec.root_id.clone(),
                            owner_generation: spec.owner_generation.clone(),
                            attempt_id: Some(spec.attempt_id.clone()),
                        },
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    if !exact.broker_owned
                        || exact.owner.guardian_identity.pid != i64::from(peer.process.host_pid)
                        || exact.attempt.as_ref().map(|a| a.attempt_id.as_str())
                            != Some(spec.attempt_id.as_str())
                        || exact.phase.as_deref() != Some("accepted")
                        || exact.revision != Some(2)
                        || !exact.claim_present
                        || exact
                            .attempt
                            .as_ref()
                            .is_none_or(|attempt| attempt.operation != "activation")
                    {
                        return Err(io::Error::other(
                            "v30 native prepare lacks broker-owned acceptance",
                        ));
                    }
                    let guardian = ProcessStamp::from(&peer.process);
                    let bound = BoundNativeAuthority {
                        root_id: &spec.root_id,
                        domain_id: &exact.owner.domain_id,
                        supervisor_authority_id: &exact.owner.supervisor_authority_id,
                        owner_generation: &spec.owner_generation,
                        owner_uid: peer.uid,
                        guardian: &guardian,
                        host_namespace: &host_namespace,
                        runner_image: &runner_image,
                        receipt_sha256: &spec.receipt_sha256,
                    };
                    let verified =
                        verify_native_receipt(&peer, &bound, &directory, &request, &receipt)?;
                    if verified.attempt_id != spec.attempt_id
                        || exact.attempt.as_ref() != Some(&verified.accepted_snapshot.attempt)
                    {
                        return Err(io::Error::other(
                            "v30 native receipt differs from accepted attempt",
                        ));
                    }
                    let grant = grants.prepare_native_v30(
                        &registry,
                        &entries,
                        &works,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &directory,
                        &request,
                        &receipt,
                        &spec.root_id,
                        &spec.attempt_id,
                        &spec.owner_generation,
                        &spec.receipt_sha256,
                        sidecar.source_generation(),
                    )?;
                    let generation = sidecar.source_generation().to_owned();
                    sidecar
                        .bind_exact_native_grant_v30(
                            &generation,
                            &spec.root_id,
                            &exact.owner,
                            &verified.accepted_snapshot,
                            &grant.grant_id,
                            &verified.custodian_request_sha256,
                        )
                        .map_err(io::Error::other)?;
                    return Ok(format!("prepared-native-v30 {}\n", grant.grant_id));
                }
                if spec.protocol != "native-continuation-v1" {
                    return Err(io::Error::other("unsupported native prepare protocol"));
                }
                let grant = grants.prepare_native(
                    &registry,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &spec.root_id,
                    &spec.attempt_id,
                    &spec.owner_generation,
                    &spec.receipt_sha256,
                )?;
                Ok(format!("prepared-native {}\n", grant.grant_id))
            } else if operation == b'k' {
                if broker_sidecar.is_some() {
                    return Err(io::Error::other(
                        "legacy native K is closed after broker sidecar cutover",
                    ));
                }
                let RequestPayload::NativeK { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid native K payload"));
                };
                if spec.protocol != "native-continuation-v1" {
                    return Err(io::Error::other("unsupported native K protocol"));
                }
                let [directory, request, receipt, sidecar] = descriptors;
                grants.verify_native_k(
                    &spec,
                    &registry,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    &sidecar,
                )?;
                Err(io::Error::other(
                    "native K fixed Runner attach/release closed",
                ))
            } else if operation == b't' {
                let sidecar = broker_sidecar
                    .as_mut()
                    .ok_or_else(|| io::Error::other("v30 native K requires broker State"))?;
                let RequestPayload::NativeKV30 { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid v30 native K payload"));
                };
                let [directory, request, receipt] = descriptors;
                let verified = grants.inspect_native_k_v30(
                    &spec,
                    &registry,
                    &entries,
                    &works,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &directory,
                    &request,
                    &receipt,
                    sidecar,
                )?;
                // The physical route is exercised only by the private
                // broker fixture until the normal v30 invocation/session
                // admission and result-plus-Q integration share this lineage.
                if !fixture {
                    return Err(io::Error::other(
                        "production v30 native K admission closed pending normal invocation and Q settlement",
                    ));
                }
                native_work::launch(
                    &verified,
                    request,
                    &peer,
                    &runner_image,
                    &registry,
                    &mut works,
                    &mut grants,
                    sidecar,
                    &broker_incarnation,
                    &terminal_path,
                )
            } else if operation == b'q' || operation == b'z' {
                let sidecar = broker_sidecar.as_mut().ok_or_else(|| {
                    io::Error::other("v30 native observation requires broker State")
                })?;
                if operation == b'q' {
                    let RequestPayload::ObserveAcceptedWork { grant_id } = payload else {
                        return Err(io::Error::other("invalid v30 native observation"));
                    };
                    native_work::observe(
                        &grant_id,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &grants,
                        &works,
                        sidecar,
                        &broker_incarnation,
                        &terminal_path,
                    )
                } else {
                    let RequestPayload::CancelAcceptedWork { grant_id } = payload else {
                        return Err(io::Error::other("invalid v30 native cancellation"));
                    };
                    native_work::cancel(
                        &grant_id,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &grants,
                        &works,
                        sidecar,
                        &terminal_path,
                    )
                }
            } else if operation == b'K' {
                let RequestPayload::LaunchAcceptedWork { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid accepted launch payload"));
                };
                work_launch::launch(
                    &spec.grant_id,
                    descriptors,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &registry,
                    &mut works,
                    &entries,
                    &mut grants,
                    &terminal_path,
                )
            } else if operation == b'Q' {
                let RequestPayload::ObserveAcceptedWork { grant_id } = payload else {
                    return Err(io::Error::other("invalid accepted observation payload"));
                };
                work_launch::observe(
                    &grant_id,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &works,
                    &grants,
                    &terminal_path,
                )
            } else if operation == b'Z' {
                let RequestPayload::CancelAcceptedWork { grant_id } = payload else {
                    return Err(io::Error::other("invalid accepted cancellation payload"));
                };
                work_launch::cancel(
                    &grant_id,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &works,
                    &grants,
                )
            } else if operation == b'R' {
                let RequestPayload::StateRead { spec } = payload else {
                    return Err(io::Error::other("invalid broker State read payload"));
                };
                let sidecar = broker_sidecar
                    .as_ref()
                    .ok_or_else(|| io::Error::other("broker State cutover absent"))?;
                if spec.protocol == "broker-prepared-read-v30" {
                    let readback = read_prepared_broker_owner(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &entries,
                        &held_joins,
                        sidecar,
                    )?;
                    encode_prepared_owner(&readback)
                } else if spec.protocol == "broker-release-readback-v30" {
                    let evidence = read_released_broker_owner(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &entries,
                        &held_joins,
                        sidecar,
                    )?;
                    encode_release_evidence(&evidence)
                } else if spec.protocol == "broker-release-attest-v30" {
                    let evidence = attest_released_child(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        &held_joins,
                        sidecar,
                        true,
                        true,
                    )?;
                    encode_release_evidence(&evidence)
                } else if spec.protocol == "broker-repair-read-v30" {
                    let readback = read_bounded_repair(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_repair_readback(&readback)
                } else if spec.protocol == "broker-source-selection-v30" {
                    let selection = read_bounded_source_selection(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_source_selection(&selection)
                } else if spec.protocol == "broker-source-grant-read-v30" {
                    let grant = read_source_effect_grant(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_source_effect_grant(&grant)
                } else if spec.protocol == "broker-recipient-selection-v30" {
                    let sidecar = broker_sidecar
                        .as_mut()
                        .ok_or_else(|| io::Error::other("broker State cutover absent"))?;
                    let selection = read_bounded_recipient_selection(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_recipient_selection(&selection)
                } else {
                    let readback = read_broker_state(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_state_readback(&readback)
                }
            } else if operation == b'W' {
                let RequestPayload::StateWrite { spec } = payload else {
                    return Err(io::Error::other("invalid broker State write payload"));
                };
                if registry.admission_fenced(&spec.root_id)
                    && !matches!(&spec.action, StateWriteAction::Revoke { .. } | StateWriteAction::Repair { .. })
                {
                    return Err(io::Error::other("exact root admission fenced"));
                }
                let sidecar = broker_sidecar
                    .as_mut()
                    .ok_or_else(|| io::Error::other("broker State cutover absent"))?;
                if spec.protocol == "broker-prepared-write-v30" {
                    let readback = prepare_broker_owner(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &mut entries,
                        &held_joins,
                        sidecar,
                    )?;
                    encode_prepared_owner(&readback)
                } else if spec.protocol == "broker-held-release-v30" {
                    let evidence = release_prepared_broker_owner(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &entries,
                        &mut held_joins,
                        sidecar,
                    )?;
                    encode_release_evidence(&evidence)
                } else if spec.protocol == "broker-repair-write-v30" {
                    let readback = write_bounded_repair(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_repair_readback(&readback)
                } else if spec.protocol == "broker-source-grant-reserve-v30" {
                    if registry.admission_fenced(&spec.root_id) {
                        return Err(io::Error::other("exact root admission fenced"));
                    }
                    let grant = reserve_source_effect_grant(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_source_effect_grant(&Some(grant))
                } else if spec.protocol == "broker-source-effect-launch-v30"
                    && matches!(spec.action, StateWriteAction::LaunchSourceGrant)
                {
                    let exact = read_broker_state(
                        StateReadSpec {
                            protocol: "broker-state-read-v1".into(),
                            source_generation: spec.source_generation,
                            root_id: spec.root_id,
                            owner_generation: spec.owner_generation,
                            attempt_id: None,
                        },
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    if !exact.broker_owned
                        || exact.owner.driver_identity.pid != i64::from(peer.process.host_pid)
                    {
                        return Err(io::Error::other("source launch requires exact live driver"));
                    }
                    source_launch::launch(
                        &exact.root_id,
                        &exact.owner,
                        &peer,
                        &registry,
                        &entries,
                        sidecar,
                        &mut source_physical,
                        &source_physical_path,
                    )
                } else {
                    let readback = write_broker_state(
                        spec,
                        &peer,
                        &host_namespace,
                        &runner_image,
                        &registry,
                        &works,
                        &entries,
                        sidecar,
                    )?;
                    encode_state_readback(&readback)
                }
            } else if operation == b'Y' {
                let RequestPayload::StateGeneration { spec } = payload else {
                    return Err(io::Error::other("invalid broker State generation payload"));
                };
                let sidecar = broker_sidecar
                    .as_ref()
                    .ok_or_else(|| io::Error::other("broker State cutover absent"))?;
                broker_state_generation(
                    spec,
                    &peer,
                    &host_namespace,
                    &registry,
                    &works,
                    &entries,
                    sidecar,
                )
            } else if operation == b'I' {
                // The exact installed Runner may inspect the live storage
                // route before E/G/J. No pathname, copied row or environment
                // value can select the broker generation.
                if !matches!(payload, RequestPayload::None)
                    || !peer.process.in_namespace(&host_namespace)?
                    || !peer.process.same_executable_as(&runner_image)?
                {
                    return Err(io::Error::other("broker State route admission refused"));
                }
                let _entry_guard = admission_fences.lock().map_err(|_| {
                    io::Error::other("root admission fence poisoned")
                })?;
                // A guardian reads I after its own E. That active entry is
                // intentionally unsettled. E itself gates every reservation.
                if entries
                    .records()
                    .iter()
                    .all(|entry| entry.terminal_settlement.is_some())
                {
                    require_prior_entries_closed(
                        Path::new(&state),
                        &mut registry,
                        &entries,
                        &mut works,
                        &grants,
                        &source_physical,
                        broker_sidecar.as_ref(),
                    )?;
                }
                if !root_launch_admitted(
                    &peer,
                    &classify_scope(&peer, &host_namespace, &registry, &works),
                    &host_namespace,
                ) {
                    return Err(io::Error::other("broker State route admission refused"));
                }
                peer.process.verify()?;
                Ok(match broker_sidecar.as_ref() {
                    Some(sidecar) => {
                        format!(
                            "state-route broker-owned {} {}\n",
                            sidecar.source_generation(),
                            sidecar.domain_id().map_err(io::Error::other)?
                        )
                    }
                    None => "state-route legacy\n".into(),
                })
            } else {
                // E and exact owner close share this fence. A previous
                // unknown caller write, live root, pending debt or changed
                // continuity cursor must refuse before a second reservation.
                let _entry_guard = if matches!(operation, b'E' | b'e') {
                    Some(admission_fences.lock().map_err(|_| {
                        io::Error::other("root admission fence poisoned")
                    })?)
                } else {
                    None
                };
                if _entry_guard.is_some() {
                    require_prior_entries_closed(
                        Path::new(&state),
                        &mut registry,
                        &entries,
                        &mut works,
                        &grants,
                        &source_physical,
                        broker_sidecar.as_ref(),
                    )?;
                }
                dispatch_authenticated(
                    match operation {
                        b'e' => b'E',
                        b'p' => b'P',
                        b'g' => b'G',
                        b'a' => b'A',
                        other => other,
                    },
                    payload,
                    &peer,
                    &host_namespace,
                    &runner_image,
                    &registry,
                    &works,
                    &mut entries,
                )
            }
        });
        let response = result.unwrap_or_else(|error| format!("error {error}\n"));
        if !response.is_empty() {
            let _ = stream.write_all(response.as_bytes());
        }
        for record in &source_physical.records()[tracked_source_records..] {
            pending_source_evidence.insert(record.grant.grant_id.clone(), record.grant.clone());
        }
        tracked_source_records = source_physical.records().len();
        if let Some(sidecar) = broker_sidecar.as_mut() {
            capture_terminal_sources(sidecar, &source_physical, &mut pending_source_evidence);
        }
    }
}

fn fresh_payload_reply(
    kind: &str,
    submission: FreshDeliverySubmission,
) -> io::Result<serde_json::Value> {
    let mut grant = serde_json::to_value(submission.readback)?;
    grant["delivery_token"] = serde_json::Value::String(submission.delivery_token);
    Ok(serde_json::json!({ "kind": kind, "grant": grant,
        "payload_base64": base64::engine::general_purpose::STANDARD.encode(submission.payload) }))
}

fn bridge_released_handoff(
    bridge: &SyncSender<FreshHandoffBridgeRequest>,
    spec: StateReadSpec,
    peer: PeerIdentity,
    lane: &FreshV30LaneIdentity,
    read_only: bool,
    runner_image: &File,
) -> io::Result<FreshReleasedHandoff> {
    let expected = FreshRecipientIdentity {
        host_pid: peer.process.host_pid,
        boot_id: peer.process.boot_id.clone(),
        starttime_ticks: peer.process.starttime_ticks,
        pidns_dev: peer.process.pidns_dev,
        pidns_ino: peer.process.pidns_ino,
    };
    let (reply, answer) = mpsc::sync_channel(RELEASED_HANDOFF_REPLY_CAPACITY);
    bridge
        .send(FreshHandoffBridgeRequest {
            spec,
            peer,
            lane: lane.clone(),
            read_only,
            reply,
        })
        .map_err(|_| io::Error::other("old release authority unavailable"))?;
    let receipt = answer
        .recv_timeout(RELEASED_HANDOFF_REPLY_TIMEOUT)
        .map_err(|_| io::Error::other("old release authority response uncertain"))??;
    let current = PinnedProcess::open(expected.host_pid)?;
    current.verify()?;
    let image = runner_image.metadata()?;
    if current.boot_id != expected.boot_id
        || current.starttime_ticks != expected.starttime_ticks
        || current.pidns_dev != expected.pidns_dev
        || current.pidns_ino != expected.pidns_ino
        || !current.same_executable_as(runner_image)?
        || receipt.fresh_lane != *lane
        || receipt.runner_image_device != image.dev()
        || receipt.runner_image_inode != image.ino()
        || receipt.old_release.prepared.joined_child
            != prepared_stamp(&ProcessStamp::from(&current))
    {
        return Err(io::Error::other("fresh handoff peer or lane changed"));
    }
    Ok(receipt)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn serve_fresh_v30() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other("host root required"));
    }
    unsafe { libc::umask(0o077) };
    let fixture = private_fixture();
    install_detached_host_proc()?;
    if !fixture {
        for namespace in ["user", "pid"] {
            let self_ns = host_proc_file(&format!("self/ns/{namespace}"))?.metadata()?;
            let init_ns = host_proc_file(&format!("1/ns/{namespace}"))?.metadata()?;
            if (self_ns.dev(), self_ns.ino()) != (init_ns.dev(), init_ns.ino()) {
                return Err(io::Error::other("fresh broker requires host namespaces"));
            }
        }
    }
    let state_root = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1")
            .map_err(|_| io::Error::other("private fresh state root missing"))?
    } else {
        STATE.into()
    };
    let socket = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
            .map_err(|_| io::Error::other("private fresh socket missing"))?
    } else {
        FRESH_SOCKET.into()
    };
    let runner = if fixture {
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_RUNNER_V1")
            .map_err(|_| io::Error::other("private fresh Runner missing"))?
    } else {
        RUNNER.into()
    };
    if !fixture {
        checked_root_path(Path::new(&state_root), true)?;
        checked_root_path(Path::new("/run/oulipoly-kernel-broker"), true)?;
        checked_root_path(Path::new(&runner), false)?;
        checked_root_path(Path::new(installed_pair::BROKER), false)?;
        let pair = InstalledPair::load(Path::new(installed_pair::MANIFEST), true)?;
        pair.verify_image_against(
            Path::new(installed_pair::BROKER),
            &pair.broker_sha256,
            true,
            &host_proc_file("self/exe")?,
        )?;
        let image = File::open(&runner)?;
        pair.verify_file(Path::new(&runner), &pair.runner_sha256, true, &image)?;
    }
    let roots = RootRegistry::open(&state_root)?;
    let admission_fences = Arc::new(Mutex::new(
        roots
            .fenced_root_ids()
            .map(str::to_owned)
            .collect::<HashSet<_>>(),
    ));
    serve_fresh_v30_at(
        Path::new(&state_root),
        Path::new(&socket),
        File::open(&runner)?,
        None,
        None,
        None,
        admission_fences,
    )
}

#[cfg(feature = "age319-private-broker-fixture")]
fn fresh_bash_parent(
    state_root: &Path,
    lane: &FreshV30Lane,
    peer: &PeerIdentity,
    bash_image: Option<&File>,
) -> io::Result<(
    FreshReleasedHandoff,
    FreshRecipientIdentity,
    fresh_provider::ParentWork,
)> {
    let image = bash_image.ok_or_else(|| io::Error::other("installed Bash image absent"))?;
    if !peer.process.same_executable_as(image)? {
        return Err(io::Error::other("Bash child image changed"));
    }
    let roots = RootRegistry::open(state_root)?;
    let works = WorkRegistry::open(state_root.join("works"), &roots)?;
    let root_id = match classify_scope(peer, &host_proc_file("self/ns/pid")?, &roots, &works) {
        Scope::Root(id) | Scope::Work { root_id: id, .. } => id,
        Scope::Outside => return Err(io::Error::other("Bash child is outside a released root")),
        Scope::Uncertain => return Err(io::Error::other("Bash child scope uncertain")),
    };
    let (root, actor) = lane
        .released_handoff_for_root(&root_id)
        .map_err(io::Error::other)?;
    let init = roots
        .live_roots()
        .find(|live| live.record.root_id == root_id)
        .ok_or_else(|| io::Error::other("released root PID1 absent"))?;
    let expected = &root.old_release.prepared.root_init;
    if init.record.init_host_pid != expected.host_pid
        || init.record.init_starttime_ticks != expected.starttime_ticks
        || init.record.boot_id != expected.boot_id
        || init.record.pidns_dev != expected.pidns_dev
        || init.record.pidns_ino != expected.pidns_ino
    {
        return Err(io::Error::other("released root PID1 changed"));
    }
    let root_init = PinnedProcess::open(expected.host_pid)?;
    let root_session = lane
        .read_session(&root.d_key)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("released root D absent"))?;
    let parent = fresh_provider::parent_for_bash(
        &state_root.join("v30/fresh-provider"),
        &root,
        &root_session.session_id,
        &peer.process,
        &root_init,
    )?;
    peer.process.verify()?;
    Ok((root, actor, parent))
}

#[cfg(feature = "age319-private-broker-fixture")]
fn fixed_private_bash_child_plan() -> io::Result<fresh_provider::Plan> {
    // This is broker-owned fixture policy. No field of C, c or % supplies an
    // executable, argument, environment value or output assertion.
    let gate = PathBuf::from(
        std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1").map_err(io::Error::other)?,
    );
    let marker = gate.join("bash-physical-effect");
    let image = fs::canonicalize("/bin/sh")?;
    let fd = unsafe { libc::memfd_create(c"fresh-bash-empty-input".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let input = unsafe { File::from_raw_fd(fd) };
    let command = if std::env::var_os("AGE319_PRIVATE_BASH_SOURCE_SUCCESS_V1").is_some() {
        "printf 'broker-child-output\\n'; printf 'broker-ran\\n' > \"$1\""
    } else {
        "printf 'broker-child-output\\n'; printf 'broker-ran\\n' > \"$1\"; (setsid sh -c 'trap \"\" TERM; while :; do sleep 1; done' >/dev/null 2>&1 &)"
    };
    fresh_provider::plan(
        &image,
        &gate,
        &input,
        vec![
            "-c".into(),
            command.into(),
            "sh".into(),
            marker.display().to_string(),
        ],
        vec![("PATH".into(), "/usr/bin:/bin".into())],
    )
}

#[cfg(feature = "age319-private-broker-fixture")]
fn settle_ordinary_bash_q(state_root: &Path, request_id: &str) -> io::Result<bool> {
    let directory = state_root.join("v30/fresh-provider");
    let mut lane = FreshV30Lane::open_at(state_root).map_err(io::Error::other)?;
    let mut child = lane
        .read_bash_child(request_id)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("ordinary Bash C absent on repair"))?;
    child.session = lane
        .read_session(&child.d_key)
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("ordinary Bash D absent on repair"))?;
    let binding = fresh_provider::ordinary_selection_binding(&directory, &child)?;
    let Some(grant) = fresh_provider::grant_for_binding(&directory, &binding)? else {
        return Ok(false);
    };
    if !matches!(
        fresh_provider::observe(&directory, &grant)?,
        fresh_provider::Observation::Drained { .. }
    ) {
        return Ok(false);
    }
    let digest =
        FreshV30Lane::private_bash_source_registration_digest(&child).map_err(io::Error::other)?;
    let event = fresh_provider::select_bash_tree_event(
        &directory,
        &binding,
        &child,
        &lane.identity().lane_id,
        &lane.identity().source_generation,
        &digest,
    )?;
    lane.accept_private_bash_source(&event)
        .map_err(io::Error::other)?;
    lane.settle_private_bash_listener(request_id)
        .map_err(io::Error::other)?;
    Ok(true)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn ordinary_bash_completion_worker(
    state_root: PathBuf,
    receiver: Receiver<String>,
    startup_requests: Vec<String>,
) {
    let mut pending: HashSet<String> = startup_requests.into_iter().collect();
    loop {
        match receiver.recv_timeout(ORDINARY_BASH_COMPLETION_POLL_INTERVAL) {
            Ok(request_id) => {
                pending.insert(request_id);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        pending.retain(|request_id| {
            !matches!(settle_ordinary_bash_q(&state_root, request_id), Ok(true))
        });
    }
}

fn serve_fresh_v30_at(
    state_root: &Path,
    socket: &Path,
    runner_image: File,
    handoff_tx: Option<SyncSender<FreshHandoffBridgeRequest>>,
    terminal_tx: Option<SyncSender<FreshTerminalBridgeRequest>>,
    drain_tx: Option<SyncSender<FreshDrainBridgeRequest>>,
    admission_fences: Arc<Mutex<HashSet<String>>>,
) -> io::Result<()> {
    // A missing or incomplete publication cannot bind the new endpoint.
    let mut lane = FreshV30Lane::open_at(state_root).map_err(io::Error::other)?;
    oulipoly_kernel_broker::json_artifact::require_no_pending(
        &state_root.join("v30/fresh-provider"),
    )?;
    #[cfg(feature = "age319-private-broker-fixture")]
    if private_fixture() {
        if let Err(error) = lane.repair_captured_private_bash_sources() {
            eprintln!("fresh Bash source repair remains unknown: {error}");
        }
        if let Err(error) = lane.repair_private_bash_listener_notifications() {
            eprintln!("fresh Bash listener repair remains pending: {error}");
        }
        if let Err(error) = lane.repair_private_bash_notifications() {
            eprintln!("fresh Bash notification repair remains pending: {error}");
        }
    }
    // The lease spans all broker requests, including route, grant, provider K,
    // quota/auth/manual intent and K, and their readback paths. Offline index
    // rebuild takes the exclusive side before reading any retained evidence.
    #[cfg(feature = "age319-private-broker-fixture")]
    let admission = fresh_index::broker_admission_lease(&state_root.join("v30/fresh-provider"))
        .map_err(io::Error::other)?;
    #[cfg(feature = "age319-private-broker-fixture")]
    let provider_readback_v3 = {
        let root = state_root.join("v30/fresh-provider");
        match std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_V1") {
            Some(value) if value == "1" && private_fixture() => {
                let source = std::env::var_os(
                    "OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1",
                )
                .ok_or_else(|| io::Error::other("v3 admission config source absent"))?;
                let provider_writer_requested =
                    std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1")
                        .is_some_and(|value| value == "1")
                        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1")
                            .is_some_and(|value| value == "1");
                Some(
                    if provider_writer_requested {
                        fresh_index::KeyedGeneration::admit_provider_writer(
                            &root,
                            &admission,
                            Path::new(&source),
                        )
                    } else {
                        fresh_index::KeyedGeneration::admit_provider_readback(
                            &root,
                            &admission,
                            Path::new(&source),
                        )
                    }
                    .map_err(io::Error::other)?,
                )
            }
            Some(_) => return Err(io::Error::other("v3 provider readback switch invalid")),
            None => {
                if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_READBACK_V3_SOURCE_V1")
                    .is_some()
                {
                    return Err(io::Error::other("v3 source without admission switch"));
                }
                None
            }
        }
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let route_writer_v3 = match std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_V3_V1") {
        Some(value) if value == "1" && private_fixture() && provider_readback_v3.is_some() => true,
        None => false,
        Some(_) => return Err(io::Error::other("v3 route writer switch invalid")),
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let provider_writer_v3 =
        match std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_PROVIDER_K_V3_V1") {
            Some(value) if value == "1" && route_writer_v3 && private_fixture() => true,
            None => false,
            Some(_) => return Err(io::Error::other("v3 provider writer switch invalid")),
        };
    #[cfg(feature = "age319-private-broker-fixture")]
    let route_index = {
        let root = state_root.join("v30/fresh-provider");
        match std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_INDEX_V1") {
            Some(value) if value == "1" && private_fixture() && provider_readback_v3.is_none() => {
                Some(
                    fresh_index::Index::admit_live_routes(&root, &admission)
                        .map_err(io::Error::other)?,
                )
            }
            Some(_) => return Err(io::Error::other("route index fixture switch invalid")),
            None if provider_readback_v3.is_some() => None,
            None => {
                match fs::symlink_metadata(root.join("index-v1/manifest.json")) {
                    Ok(_) => {
                        return Err(io::Error::other(
                            "indexed route root requires indexed writer admission",
                        ));
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                None
            }
        }
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let route_index = match std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_ROUTE_READER_PROBE_V1")
    {
        Some(value) if value == "1" && private_fixture() => Some(
            route_index
                .ok_or_else(|| io::Error::other("route reader probe requires indexed writer"))?
                .enable_route_reader_probe(),
        ),
        Some(_) => return Err(io::Error::other("route reader probe switch invalid")),
        None => route_index,
    };
    let instance = EntryGate::open(&state_root.join("v30"))?;
    // An installed Bash child must match the package's pinned digest. Private
    // fixtures supply their built source binary only at broker startup.
    #[cfg(feature = "age319-private-broker-fixture")]
    let bash_image = if private_fixture() {
        std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_BASH_V1")
            .map(File::open)
            .transpose()?
    } else {
        let pair = InstalledPair::load(Path::new(installed_pair::MANIFEST), true)?;
        pair.bash_sha256
            .as_deref()
            .map(|digest| -> io::Result<File> {
                let path = Path::new(installed_pair::BASH);
                let image = File::open(path)?;
                pair.verify_file(path, digest, true, &image)?;
                Ok(image)
            })
            .transpose()?
    };
    match fs::symlink_metadata(socket) {
        Ok(meta) if meta.file_type().is_socket() && meta.uid() == 0 && meta.nlink() == 1 => {
            fs::remove_file(socket)?;
        }
        Ok(_) => return Err(io::Error::other("fresh v30 socket pathname is untrusted")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(socket)?;
    fs::set_permissions(socket, fs::Permissions::from_mode(0o660))?;
    #[cfg(feature = "age319-private-broker-fixture")]
    let ordinary_completion_tx = if private_fixture() {
        let (sender, receiver) = mpsc::channel();
        let directory = state_root.join("v30/fresh-provider");
        let startup_requests = if directory.exists() {
            fresh_provider::ordinary_selection_request_ids(&directory)?
        } else {
            Vec::new()
        };
        let state_root = state_root.to_path_buf();
        std::thread::Builder::new()
            .name("ordinary-bash-q-to-w".into())
            .spawn(move || {
                ordinary_bash_completion_worker(state_root, receiver, startup_requests)
            })?;
        Some(sender)
    } else {
        None
    };
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut native_codex_controls: HashMap<String, fresh_provider::NativeCodexControl> =
        HashMap::new();
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut native_bash_execs: HashMap<String, fresh_provider::NativeBashExec> = HashMap::new();
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut native_turn_execs: HashMap<String, fresh_provider::NativeTurnExec> = HashMap::new();
    #[cfg(feature = "age319-private-broker-fixture")]
    let mut native_f_execs: HashMap<String, fresh_provider::NativeFExec> = HashMap::new();
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        stream.set_read_timeout(Some(FRESH_V30_READ_TIMEOUT))?;
        stream.set_write_timeout(Some(FRESH_V30_WRITE_TIMEOUT))?;
        let mut submitted_grant = None;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_provider_k_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_native_bash_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_native_turn_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_native_f_turn_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_interactive_k_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_provider_q_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_account_effect_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_root_h_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut provider_output_files: Option<Vec<File>> = None;
        let mut drop_route_reply = false;
        let mut diagnostic_opcode = b'?';
        let mut diagnostic_stage = "request_decode";
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut diagnostic_key_hash = String::from("unavailable");
        let answer = (|| -> io::Result<String> {
            let (operation, payload, peer) = peer_from_request(&mut stream)?;
            diagnostic_opcode = operation;
            diagnostic_stage = "peer_authority";
            peer.process.verify()?;
            diagnostic_stage = "request_dispatch";
            if operation == b'i' {
                if !matches!(payload, RequestPayload::None) {
                    return Err(io::Error::other("fresh gate read has a payload"));
                }
                return Ok(if instance.is_closed() {
                    "entry-gate-v1 draining\n"
                } else {
                    "entry-gate-v1 fresh-v30-closed\n"
                }
                .into());
            }
            let local_lookup = matches!(
                &payload,
                RequestPayload::FreshRecipientRequest {
                    request: FreshRecipientRequest::Lookup { .. }
                } | RequestPayload::FreshBashChildRequest { .. }
                    | RequestPayload::FreshBashPrivateResult { .. }
            );
            // The shared front door has its own pinned image. It may observe
            // the live lane identity, but it cannot acquire Runner authority.
            // Every effect-bearing operation still requires the fresh Runner
            // image and its physical release/State checks below.
            let route_observation = operation == b'I' && matches!(payload, RequestPayload::None);
            if !local_lookup
                && !route_observation
                && !peer.process.same_executable_as(&runner_image)?
            {
                return Err(io::Error::other(
                    "fresh lane requires installed Runner image",
                ));
            }
            let recipient = FreshRecipientIdentity {
                host_pid: peer.process.host_pid,
                boot_id: peer.process.boot_id.clone(),
                starttime_ticks: peer.process.starttime_ticks,
                pidns_dev: peer.process.pidns_dev,
                pidns_ino: peer.process.pidns_ino,
            };
            match operation {
                #[cfg(feature = "age319-private-broker-fixture")]
                b'w' | b'r' => {
                    if !private_fixture() {
                        return Err(io::Error::other("manual quota fixture route closed"));
                    }
                    let directory = state_root.join("v30/fresh-provider");
                    let _route_lock = if provider_readback_v3.is_some() {
                        Some(fresh_provider::route_selection_lock(&directory)?)
                    } else {
                        None
                    };
                    let result = if operation == b'w' {
                        let RequestPayload::ManualQuotaRequest {
                            request,
                            descriptors,
                        } = payload
                        else {
                            return Err(io::Error::other("manual quota begin request absent"));
                        };
                        if instance.is_closed() {
                            return Err(io::Error::other("manual quota entry gate closed"));
                        }
                        let [source]: [File; 1] = descriptors
                            .try_into()
                            .map_err(|_| io::Error::other("manual quota config source absent"))?;
                        if let Some(generation) = provider_readback_v3.as_ref() {
                            manual_quota::begin_v3(
                                &directory, &source, &request, peer.uid, peer.gid, generation,
                            )?
                        } else {
                            manual_quota::begin_indexed(
                                &directory,
                                &source,
                                &request,
                                peer.uid,
                                peer.gid,
                                route_index.as_ref(),
                            )?
                        }
                    } else {
                        let RequestPayload::ManualQuotaObserve { operation_id } = payload else {
                            return Err(io::Error::other("manual quota observation absent"));
                        };
                        if let Some(generation) = provider_readback_v3.as_ref() {
                            manual_quota::readback_id_v3(
                                &directory,
                                &operation_id,
                                peer.uid,
                                peer.gid,
                                generation,
                            )?
                        } else {
                            manual_quota::readback_id_indexed(
                                &directory,
                                &operation_id,
                                peer.uid,
                                peer.gid,
                                route_index.as_ref(),
                            )?
                        }
                    };
                    if operation == b'w'
                        && provider_readback_v3.is_some()
                        && std::env::var_os(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_MANUAL_BEGIN_REPLY_V3_V1",
                        )
                        .is_some()
                    {
                        return Ok(String::new());
                    }
                    return Ok(format!(
                        "manual-quota {}\n",
                        serde_json::to_string(&result)?
                    ));
                }
                b'I' if matches!(payload, RequestPayload::None) => Ok(format!(
                    "fresh-v30-route {} {} {}\n",
                    lane.identity().lane_id,
                    lane.identity().source_generation,
                    lane.identity().domain_id,
                )),
                b'U' => {
                    let RequestPayload::FreshChildRequest { request } = payload else {
                        return Err(io::Error::other("fresh child request payload absent"));
                    };
                    if let Some(spec) = request.release {
                        if !request.request_id.is_empty() || !request.invocation_uuid.is_empty() {
                            return Err(io::Error::other(
                                "caller-proposed fresh authority refused",
                            ));
                        }
                        let bridge = handoff_tx.as_ref().ok_or_else(|| {
                            io::Error::other("in-process release authority unavailable")
                        })?;
                        let receipt = bridge_released_handoff(
                            bridge,
                            spec,
                            peer,
                            lane.identity(),
                            false,
                            &runner_image,
                        )?;
                        if instance.is_closed() {
                            lane.require_released_handoff(&receipt.d_key, &receipt, &recipient)
                                .map_err(io::Error::other)?;
                        } else {
                            let _guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if _guard.contains(&receipt.old_release.prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            lane.bind_released_handoff(&receipt, &recipient)
                                .map_err(io::Error::other)?;
                        }
                        return Ok(format!(
                            "fresh-child-handoff {}\n",
                            serde_json::to_string(&receipt)?
                        ));
                    }
                    if !private_fixture() {
                        return Err(io::Error::other(
                            "fresh U requires live old-gate handoff evidence",
                        ));
                    }
                    if !peer.process.in_namespace(&host_proc_file("self/ns/pid")?)? {
                        return Err(io::Error::other(
                            "released child cannot use synthetic private U",
                        ));
                    }
                    if instance.is_closed() {
                        lane.require_child_request(
                            &request.request_id,
                            &request.invocation_uuid,
                            &recipient,
                        )
                        .map_err(io::Error::other)?;
                    } else {
                        lane.reserve_child_request(
                            &request.request_id,
                            &request.invocation_uuid,
                            &recipient,
                        )
                        .map_err(io::Error::other)?;
                    }
                    Ok(format!(
                        "fresh-child-request {} {}\n",
                        request.request_id, request.invocation_uuid
                    ))
                }
                #[cfg(not(feature = "age319-private-broker-fixture"))]
                b'C' | b'X' | b'c' | b'E' | b'O' | b'^' | b'v' | b'u' | b'<' => {
                    Err(io::Error::other(
                        "fresh Bash child/work/result closed until normal root grant and physical result custody",
                    ))
                }
                #[cfg(feature = "age319-private-broker-fixture")]
                b'C' | b'X' | b'c' | b'E' | b'O' | b'%' | b'!' | b'8' | b'9' | b'^' | b'v'
                | b'u' | b'<'
                    if !matches!(operation, b'8' | b'9')
                        || matches!(&payload, RequestPayload::FreshBashChildRequest { .. }) =>
                {
                    diagnostic_stage = "bash_child_parent_readback";
                    if !private_fixture() {
                        return Err(io::Error::other(
                            "fresh Bash child/work/result closed until normal root grant and physical result custody",
                        ));
                    }
                    if matches!(operation, b'E' | b'8' | b'^') && instance.is_closed() {
                        return Err(io::Error::other("fresh Bash private work gate closed"));
                    }
                    let (request_id, listener_policy, ordinary_command, ordinary_k_digest) =
                        match &payload {
                            RequestPayload::FreshBashChildRequest {
                                request_id,
                                listener_policy,
                                ordinary_command,
                                ordinary_k_digest,
                            } => (
                                request_id.clone(),
                                *listener_policy,
                                ordinary_command.clone(),
                                *ordinary_k_digest,
                            ),
                            RequestPayload::FreshBashPrivateResult { result } => {
                                (result.request_id.clone(), None, None, None)
                            }
                            _ => return Err(io::Error::other("Bash child request absent")),
                        };
                    diagnostic_key_hash = format!("{:x}", Sha256::digest(request_id.as_bytes()));
                    let (root, root_actor, parent_work) =
                        fresh_bash_parent(state_root, &lane, &peer, bash_image.as_ref())?;
                    let directory = state_root.join("v30/fresh-provider");
                    let _admission_guard =
                        if matches!(operation, b'C' | b'X' | b'E' | b'%' | b'8' | b'^') {
                            let guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if guard.contains(&root.old_release.prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            Some(guard)
                        } else {
                            None
                        };
                    let ordinary_plan = if operation == b'X' {
                        ordinary_command
                            .as_ref()
                            .map(|command| {
                                let plan = fresh_provider::ordinary_bash_plan(command, None)?;
                                fresh_provider::bind_ordinary_bash_intent(
                                    &directory,
                                    &request_id,
                                    &peer.process,
                                    command,
                                )?;
                                Ok::<_, io::Error>(plan)
                            })
                            .transpose()?
                    } else {
                        None
                    };
                    let child = if matches!(operation, b'C' | b'X') && !instance.is_closed() {
                        diagnostic_stage = "bash_child_admission";
                        lane.admit_bash_child(
                            &request_id,
                            &root,
                            &root_actor,
                            &recipient,
                            parent_work.grant_id(),
                            parent_work.work_id(),
                            listener_policy
                                .ok_or_else(|| io::Error::other("fresh Bash C policy absent"))?,
                        )
                        .map_err(io::Error::other)?
                    } else {
                        diagnostic_stage = "bash_child_exact_readback";
                        let mut child = lane
                            .read_bash_child(&request_id)
                            .map_err(io::Error::other)?
                            .ok_or_else(|| io::Error::other("Bash child absent"))?;
                        child.session = lane
                            .read_session(&child.d_key)
                            .map_err(io::Error::other)?
                            .ok_or_else(|| io::Error::other("Bash child D incomplete"))?;
                        lane.require_bash_child(&child, &root, &root_actor, &recipient)
                            .map_err(io::Error::other)?;
                        if child.parent_work_grant_id != parent_work.grant_id()
                            || child.parent_work_id != parent_work.work_id()
                        {
                            return Err(io::Error::other("Bash causal parent work changed"));
                        }
                        child
                    };
                    peer.process.verify()?;
                    if operation == b'<' {
                        diagnostic_stage = "root_h_delegation_consume";
                        let selected =
                            fresh_provider::selected_root_h_k(&directory, &root, &parent_work)?;
                        match lane
                            .read_root_h_delegation(&root.handoff_id)
                            .map_err(io::Error::other)?
                        {
                            Some(existing)
                                if existing.child_request_id == child.request_id
                                    && existing.selected_k == selected => {}
                            Some(_) => {
                                return Err(io::Error::other("root H delegation already assigned"));
                            }
                            None => {
                                lane.issue_root_h_delegation(
                                    &root,
                                    &root_actor,
                                    &child,
                                    selected.clone(),
                                )
                                .map_err(io::Error::other)?;
                            }
                        }
                        let consumed = lane
                            .consume_root_h_delegation(&root, &root_actor, &child, &selected)
                            .map_err(io::Error::other)?;
                        peer.process.verify()?;
                        if std::env::var_os("AGE319_PRIVATE_DROP_ROOT_H_REPLY_V1").is_some() {
                            drop_root_h_reply = true;
                        }
                        return Ok(format!(
                            "root-h-consumed {}\n",
                            serde_json::to_string(&consumed)?
                        ));
                    }
                    if matches!(operation, b'C' | b'X' | b'c') {
                        lane.register_private_bash_source(&child)
                            .map_err(io::Error::other)?;
                        if matches!(operation, b'C' | b'X') {
                            lane.register_private_bash_listener(
                                &child,
                                listener_policy.ok_or_else(|| {
                                    io::Error::other("fresh Bash C policy absent")
                                })?,
                            )
                            .map_err(io::Error::other)?;
                        } else {
                            lane.require_private_bash_listener(&child, None)
                                .map_err(io::Error::other)?;
                        }
                    }
                    match operation {
                        b'v' | b'u' => {
                            diagnostic_stage = "bash_sync_publication";
                            let (receipt, first) = if operation == b'v' {
                                lane.begin_private_bash_sync_publication(&request_id, &recipient)
                                    .map_err(io::Error::other)?
                            } else {
                                (
                                    lane.read_private_bash_sync_publication(
                                        &request_id,
                                        &recipient,
                                    )
                                    .map_err(io::Error::other)?
                                    .ok_or_else(|| {
                                        io::Error::other("sync publication reservation absent")
                                    })?,
                                    false,
                                )
                            };
                            if first {
                                let stdout = open_exact_sync_stream(
                                    &directory,
                                    &receipt.event.physical_grant_id,
                                    "stdout",
                                    receipt.event.stdout_len,
                                    &receipt.event.stdout_sha256,
                                )?;
                                let stderr = open_exact_sync_stream(
                                    &directory,
                                    &receipt.event.physical_grant_id,
                                    "stderr",
                                    receipt.event.stderr_len,
                                    &receipt.event.stderr_sha256,
                                )?;
                                provider_output_files = Some(vec![stdout, stderr]);
                            }
                            Ok(format!(
                                "fresh-bash-sync-{} {}\n",
                                if first { "begin" } else { "unknown" },
                                serde_json::to_string(&receipt)?
                            ))
                        }
                        b'C' | b'X' | b'c' => {
                            if matches!(operation, b'C' | b'X') {
                                diagnostic_stage = "bash_child_selection";
                                let root_init = PinnedProcess::open(
                                    root.old_release.prepared.root_init.host_pid,
                                )?;
                                let binding = fresh_provider::binding_from_bash_child(
                                    &root,
                                    &child,
                                    &peer.process,
                                    &parent_work,
                                    &root_init,
                                )?;
                                if let Some(plan) = ordinary_plan.as_ref() {
                                    fresh_provider::select_ordinary_child_work(
                                        &directory,
                                        &child,
                                        &binding,
                                        &peer.process,
                                        ordinary_command.as_ref().unwrap(),
                                        plan,
                                    )?;
                                } else {
                                    let plan = fixed_private_bash_child_plan()?;
                                    fresh_provider::select_private_child_work(
                                        &directory, &child, &binding, &plan,
                                    )?;
                                }
                            } else {
                                let root_init = PinnedProcess::open(
                                    root.old_release.prepared.root_init.host_pid,
                                )?;
                                let binding = fresh_provider::binding_from_bash_child(
                                    &root,
                                    &child,
                                    &peer.process,
                                    &parent_work,
                                    &root_init,
                                )?;
                                fresh_provider::child_selection_is_ordinary(
                                    &directory, &child, &binding,
                                )?;
                            }
                            Ok(format!(
                                "fresh-bash-child {}\n",
                                serde_json::to_string(&child)?
                            ))
                        }
                        b'E' => {
                            diagnostic_stage = "bash_child_work_grant";
                            let grant = lane
                                .admit_private_bash_work(&child)
                                .map_err(io::Error::other)?;
                            Ok(format!("fresh-bash-work {grant}\n"))
                        }
                        b'O' => {
                            diagnostic_stage = "bash_child_result";
                            let RequestPayload::FreshBashPrivateResult { result } = payload else {
                                unreachable!()
                            };
                            lane.record_private_bash_result(&result)
                                .map_err(io::Error::other)?;
                            Ok(format!(
                                "fresh-bash-result {}\n",
                                serde_json::to_string(&result)?
                            ))
                        }
                        #[cfg(feature = "age319-private-broker-fixture")]
                        b'8' | b'9' | b'!' | b'%' | b'^' => {
                            diagnostic_stage = match operation {
                                b'8' => "bash_child_physical_k",
                                b'^' => "bash_child_ordinary_physical_k",
                                b'9' => "bash_child_physical_q",
                                b'!' => "bash_child_physical_cancel",
                                b'%' => "bash_child_source_w",
                                _ => unreachable!(),
                            };
                            let root_pid = root.old_release.prepared.root_init.host_pid;
                            let root_init = PinnedProcess::open(root_pid)?;
                            let actor = PinnedProcess::open(peer.process.host_pid)?;
                            let binding = fresh_provider::binding_from_bash_child(
                                &root,
                                &child,
                                &actor,
                                &parent_work,
                                &root_init,
                            )?;
                            if operation == b'%' {
                                let registration_digest =
                                    FreshV30Lane::private_bash_source_registration_digest(&child)
                                        .map_err(io::Error::other)?;
                                let event = fresh_provider::select_bash_tree_event(
                                    &directory,
                                    &binding,
                                    &child,
                                    &lane.identity().lane_id,
                                    &lane.identity().source_generation,
                                    &registration_digest,
                                )
                                .map_err(|error| {
                                    io::Error::other(format!(
                                        "source W unknown for C {}; physical artifact {}; {error}",
                                        child.request_id,
                                        directory.display()
                                    ))
                                })?;
                                if std::env::var_os("AGE319_PRIVATE_SOURCE_W_CAPTURE_ONLY_V1")
                                    .is_some()
                                {
                                    return Err(io::Error::other(format!(
                                        "source W captured before State; exact repair debt at {}",
                                        directory
                                            .join(format!(
                                                "{}.source-event.json",
                                                event.physical_grant_id
                                            ))
                                            .display()
                                    )));
                                }
                                lane.accept_private_bash_source(&event).map_err(|error| {
                                    io::Error::other(format!(
                                        "source W unknown; captured artifact {}: {error}",
                                        directory
                                            .join(format!(
                                                "{}.source-event.json",
                                                event.physical_grant_id
                                            ))
                                            .display()
                                    ))
                                })?;
                                if let Err(error) = lane.settle_private_bash_listener(&request_id) {
                                    eprintln!(
                                        "source W accepted; listener settlement pending for C {request_id}: {error}"
                                    );
                                }
                                return Ok(format!(
                                    "fresh-bash-source-accepted {}\n",
                                    serde_json::to_string(&event)?
                                ));
                            }
                            if matches!(operation, b'8' | b'^') {
                                let ordinary = fresh_provider::child_selection_is_ordinary(
                                    &directory, &child, &binding,
                                )?;
                                if ordinary != (operation == b'^') {
                                    return Err(io::Error::other(
                                        "fresh Bash child K opcode differs from selected role",
                                    ));
                                }
                                let plan = if ordinary {
                                    let command = ordinary_command.as_ref().ok_or_else(|| {
                                        io::Error::other(
                                            "ordinary Bash K command descriptor absent",
                                        )
                                    })?;
                                    fresh_provider::require_ordinary_bash_command(
                                        &directory,
                                        &child.request_id,
                                        &actor,
                                        command,
                                    )?;
                                    fresh_provider::bind_ordinary_bash_intent(
                                        &directory,
                                        &child.request_id,
                                        &actor,
                                        command,
                                    )?;
                                    let expected =
                                        sha2::Sha256::digest(serde_json::to_vec(&command)?);
                                    if ordinary_k_digest.as_ref().is_none_or(|digest| {
                                        digest.as_slice() != expected.as_slice()
                                    }) {
                                        return Err(io::Error::other(
                                            "ordinary Bash argv/cwd/environment changed after C before K",
                                        ));
                                    }
                                    let source = fresh_provider::ordinary_selected_source(
                                        &directory,
                                        &child.request_id,
                                    )?;
                                    fresh_provider::ordinary_bash_plan(command, Some(source))?
                                } else {
                                    fixed_private_bash_child_plan()?
                                };
                                fresh_provider::require_admitted_child_work_plan(
                                    &directory, &child, &binding, &plan,
                                )?;
                                let prepared = fresh_provider::prepare(&directory, binding, plan)?;
                                let grant = fresh_provider::launch(
                                    prepared,
                                    &root_init,
                                    &actor,
                                    peer.uid,
                                    peer.gid,
                                    route_index.as_ref(),
                                )?;
                                if ordinary {
                                    ordinary_completion_tx
                                        .as_ref()
                                        .ok_or_else(|| {
                                            io::Error::other(
                                                "ordinary Bash completion worker absent after K",
                                            )
                                        })?
                                        .send(child.request_id.clone())
                                        .map_err(|_| {
                                            io::Error::other(
                                                "ordinary Bash completion worker lost after K",
                                            )
                                        })?;
                                }
                                return Ok(format!("fresh-bash-physical-k {grant}\n"));
                            }
                            let grant = fresh_provider::grant_for_binding(&directory, &binding)?
                                .ok_or_else(|| io::Error::other("fresh Bash physical K absent"))?;
                            if operation == b'!' {
                                fresh_provider::cancel(&directory, &grant)?;
                                return Ok(format!("fresh-bash-physical-cancel {grant}\n"));
                            }
                            match fresh_provider::observe(&directory, &grant)? {
                                fresh_provider::Observation::Unknown => {
                                    Ok(format!("fresh-bash-physical-unknown {grant}\n"))
                                }
                                fresh_provider::Observation::Pending => {
                                    Ok(format!("fresh-bash-physical-pending {grant}\n"))
                                }
                                fresh_provider::Observation::ProviderExited(status) => {
                                    Ok(format!("fresh-bash-physical-exited {grant} {status}\n"))
                                }
                                fresh_provider::Observation::Drained {
                                    status,
                                    stdout: _,
                                    stderr: _,
                                    stdout_len,
                                    stderr_len,
                                    stdout_sha256,
                                    stderr_sha256,
                                    cancelled,
                                } => Ok(format!(
                                    "fresh-bash-physical-drained {grant} {status} {stdout_len} {stderr_len} {cancelled} {} {}\n",
                                    stdout_sha256, stderr_sha256,
                                )),
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                b'D' | b'd' => {
                    let RequestPayload::FreshSessionRequest { request_id } = payload else {
                        return Err(io::Error::other("fresh session request identity absent"));
                    };
                    let released = match lane.released_handoff_for_child(&request_id, &recipient) {
                        Ok(receipt) => {
                            let spec = StateReadSpec {
                                protocol: "broker-release-attest-v30".into(),
                                source_generation: receipt
                                    .old_release
                                    .prepared
                                    .source_generation
                                    .clone(),
                                root_id: receipt.old_release.prepared.root_id.clone(),
                                owner_generation: receipt
                                    .old_release
                                    .prepared
                                    .owner_generation
                                    .clone(),
                                attempt_id: None,
                            };
                            let bridge = handoff_tx.as_ref().ok_or_else(|| {
                                io::Error::other("in-process release authority unavailable")
                            })?;
                            let current = bridge_released_handoff(
                                bridge,
                                spec,
                                peer,
                                lane.identity(),
                                true,
                                &runner_image,
                            )?;
                            if current != receipt {
                                return Err(io::Error::other(
                                    "old and fresh handoff receipts differ",
                                ));
                            }
                            Some(receipt)
                        }
                        Err(error)
                            if private_fixture()
                                && error == "fresh released handoff absent before D" =>
                        {
                            if !peer.process.in_namespace(&host_proc_file("self/ns/pid")?)? {
                                return Err(io::Error::other(
                                    "released child cannot use synthetic private D",
                                ));
                            }
                            lane.require_child_actor(&request_id, &recipient, false)
                                .map_err(io::Error::other)?;
                            None
                        }
                        Err(error) => return Err(io::Error::other(error)),
                    };
                    // d never repairs a half-written pair; only a retry of
                    // the same D key may finish its State-first admission.
                    let _admission_guard = if operation == b'D' && !instance.is_closed() {
                        if let Some(receipt) = released.as_ref() {
                            let guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if guard.contains(&receipt.old_release.prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            Some(guard)
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let session = if operation == b'd' {
                        match lane.read_session(&request_id).map_err(io::Error::other)? {
                            Some(session) => session,
                            None => return Ok("fresh-session absent\n".into()),
                        }
                    } else {
                        if instance.is_closed() {
                            match lane.read_session(&request_id).map_err(io::Error::other)? {
                                Some(session) => session,
                                None => {
                                    return Err(io::Error::other(
                                        "fresh v30 session allocation gate closed",
                                    ));
                                }
                            }
                        } else {
                            lane.allocate_session(&request_id)
                                .map_err(io::Error::other)?
                        }
                    };
                    if let Some(receipt) = released {
                        if operation == b'D' && !instance.is_closed() {
                            lane.ensure_released_invocation(&receipt, &recipient, &session)
                                .map_err(io::Error::other)?;
                        } else {
                            lane.require_released_invocation(&receipt, &recipient, &session)
                                .map_err(io::Error::other)?;
                        }
                    }
                    Ok(format!(
                        "fresh-session {}\n",
                        serde_json::to_string(&session)?
                    ))
                }
                b'0' | b'1' | b'2' => {
                    let RequestPayload::FreshRootEffectRequest { request } = payload else {
                        return Err(io::Error::other("fresh root effect request absent"));
                    };
                    if (operation == b'2') != request.success.is_some() {
                        return Err(io::Error::other("fresh root effect result shape invalid"));
                    }
                    let receipt = lane
                        .released_handoff_for_child(&request.d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    // Reading or returning an already prepared root effect
                    // may follow a child PID1 exit. Preparing a new effect
                    // still requires a clean work registry.
                    let read_only = operation != b'0';
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        read_only,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other("root effect release readback changed"));
                    }
                    let session = lane
                        .read_session(&request.d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("root effect D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    let effect = match operation {
                        b'0' => {
                            if instance.is_closed() {
                                return Err(io::Error::other(
                                    "fresh root effect entry gate closed",
                                ));
                            }
                            let guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if guard.contains(&receipt.old_release.prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            Some(
                                lane.begin_root_effect(&receipt, &recipient, &session)
                                    .map_err(io::Error::other)?,
                            )
                        }
                        b'1' => lane
                            .read_root_effect(&receipt, &recipient, &session)
                            .map_err(io::Error::other)?,
                        b'2' => Some(
                            lane.return_root_effect(
                                &receipt,
                                &recipient,
                                &session,
                                request.success.unwrap(),
                            )
                            .map_err(io::Error::other)?,
                        ),
                        _ => unreachable!(),
                    };
                    match effect {
                        Some(effect) => Ok(format!(
                            "fresh-root-effect {}\n",
                            serde_json::to_string(&effect)?
                        )),
                        None => Ok("fresh-root-effect absent\n".into()),
                    }
                }
                b'3' | b'4' => {
                    let RequestPayload::FreshRootEffectRequest { request } = payload else {
                        return Err(io::Error::other("normal work request absent"));
                    };
                    if request.success.is_some() {
                        return Err(io::Error::other(
                            "normal work request cannot return a result",
                        ));
                    }
                    let receipt = lane
                        .released_handoff_for_child(&request.d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other("normal work release readback changed"));
                    }
                    let session = lane
                        .read_session(&request.d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("normal work D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    let preparation = if operation == b'3' {
                        if instance.is_closed() {
                            return Err(io::Error::other("normal work preparation gate closed"));
                        }
                        let guard = admission_fences
                            .lock()
                            .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                        if guard.contains(&receipt.old_release.prepared.root_id) {
                            return Err(io::Error::other("exact root admission fenced"));
                        }
                        Some(
                            lane.prepare_normal_work(&receipt, &recipient, &session)
                                .map_err(io::Error::other)?,
                        )
                    } else {
                        lane.read_normal_work(&receipt, &recipient, &session)
                            .map_err(io::Error::other)?
                    };
                    match preparation {
                        Some(preparation) => Ok(format!(
                            "fresh-normal-work {}\n",
                            serde_json::to_string(&preparation)?
                        )),
                        None => Ok("fresh-normal-work absent\n".into()),
                    }
                }
                0x84 | 0x85 => {
                    let RequestPayload::FreshNormalModelRequest {
                        request,
                        config_dir,
                    } = payload
                    else {
                        return Err(io::Error::other("normal model request absent"));
                    };
                    if request.success.is_some() {
                        return Err(io::Error::other(
                            "normal model request cannot return an effect",
                        ));
                    }
                    if operation == 0x84 && instance.is_closed() {
                        return Err(io::Error::other("normal model selection gate closed"));
                    }
                    let receipt = lane
                        .released_handoff_for_child(&request.d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other("normal model release readback changed"));
                    }
                    let session = lane
                        .read_session(&request.d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("normal model D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    let selection = if operation == 0x84 {
                        let guard = admission_fences
                            .lock()
                            .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                        if guard.contains(&receipt.old_release.prepared.root_id) {
                            return Err(io::Error::other("exact root admission fenced"));
                        }
                        oulipoly_kernel_broker::normal_model_selection::select(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &config_dir,
                        )?
                    } else {
                        match oulipoly_kernel_broker::normal_model_selection::observe(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &config_dir,
                        )? {
                            Some(selection) => selection,
                            None => return Ok("fresh-normal-model absent\n".into()),
                        }
                    };
                    if oulipoly_kernel_broker::normal_model_selection::observe(
                        &lane,
                        &receipt,
                        &recipient,
                        &session,
                        &config_dir,
                    )? != Some(selection.clone())
                    {
                        return Err(io::Error::other("normal model selection readback changed"));
                    }
                    let live = PinnedProcess::open(recipient.host_pid)?;
                    if live.boot_id != recipient.boot_id
                        || live.starttime_ticks != recipient.starttime_ticks
                        || live.pidns_dev != recipient.pidns_dev
                        || live.pidns_ino != recipient.pidns_ino
                    {
                        return Err(io::Error::other(
                            "normal model actor changed during selection",
                        ));
                    }
                    live.verify()?;
                    Ok(format!(
                        "fresh-normal-model {}\n",
                        serde_json::to_string(&selection)?
                    ))
                }
                0x8c | 0x8d => {
                    let RequestPayload::FreshNormalPublicationRequest {
                        request,
                        mut descriptors,
                    } = payload
                    else {
                        return Err(io::Error::other("normal publication request absent"));
                    };
                    if request.success.is_some() {
                        return Err(io::Error::other(
                            "normal publication cannot return root effect",
                        ));
                    }
                    let receipt = lane
                        .released_handoff_for_child(&request.d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other("normal publication release changed"));
                    }
                    let session = lane
                        .read_session(&request.d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("normal publication D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    let live = PinnedProcess::open(recipient.host_pid)?;
                    if live.boot_id != recipient.boot_id
                        || live.starttime_ticks != recipient.starttime_ticks
                        || live.pidns_dev != recipient.pidns_dev
                        || live.pidns_ino != recipient.pidns_ino
                    {
                        return Err(io::Error::other("normal publication actor changed"));
                    }
                    live.verify()?;
                    let publication = if operation == 0x8c {
                        let (stdout, stderr) = descriptors.split_at_mut(1);
                        normal_physical::publish(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &state_root,
                            &mut stdout[0],
                            &mut stderr[0],
                        )?
                    } else {
                        normal_physical::observe_publication(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &state_root,
                        )?
                    };
                    live.verify()?;
                    Ok(format!(
                        "fresh-normal-publication {}\n",
                        serde_json::to_string(&publication)?
                    ))
                }
                0x86 | 0x87 | 0x88 | 0x89 | 0x8a | 0x8b => {
                    let RequestPayload::FreshNormalPlanRequest {
                        request,
                        config_dir,
                        cwd,
                        environment,
                    } = payload
                    else {
                        return Err(io::Error::other("normal plan request absent"));
                    };
                    if request.success.is_some() {
                        return Err(io::Error::other(
                            "normal plan request cannot return an effect",
                        ));
                    }
                    if matches!(operation, 0x86 | 0x88 | 0x8a) && instance.is_closed() {
                        return Err(io::Error::other("normal plan gate closed"));
                    }
                    let peer_uid = peer.uid;
                    let peer_gid = peer.gid;
                    let receipt = lane
                        .released_handoff_for_child(&request.d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other("normal plan release readback changed"));
                    }
                    let session = lane
                        .read_session(&request.d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("normal plan D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    // Once K is spent, readback must survive later source
                    // changes. State still binds the exact actor, session,
                    // admission and plan; observing Q cannot launch again.
                    if operation == 0x8b {
                        let live = PinnedProcess::open(recipient.host_pid)?;
                        if live.boot_id != recipient.boot_id
                            || live.starttime_ticks != recipient.starttime_ticks
                            || live.pidns_dev != recipient.pidns_dev
                            || live.pidns_ino != recipient.pidns_ino
                        {
                            return Err(io::Error::other("normal physical actor changed"));
                        }
                        live.verify()?;
                        return match normal_physical::observe(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &state_root,
                        )? {
                            Some(readback) => Ok(format!(
                                "fresh-normal-physical {}\n",
                                serde_json::to_string(&readback)?
                            )),
                            None => Ok("fresh-normal-physical absent\n".into()),
                        };
                    }
                    let plan = if operation == 0x86 {
                        let guard = admission_fences
                            .lock()
                            .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                        if guard.contains(&receipt.old_release.prepared.root_id) {
                            return Err(io::Error::other("exact root admission fenced"));
                        }
                        oulipoly_kernel_broker::normal_plan_custody::select(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &config_dir,
                            &cwd,
                            &environment,
                        )?
                    } else {
                        match oulipoly_kernel_broker::normal_plan_custody::observe(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &config_dir,
                            &cwd,
                            &environment,
                        )? {
                            Some(plan) => plan,
                            None if operation == 0x89 => {
                                return Ok("fresh-normal-admission absent\n".into());
                            }
                            None if matches!(operation, 0x88 | 0x8a | 0x8b) => {
                                return Err(io::Error::other("normal admission plan absent"));
                            }
                            None => return Ok("fresh-normal-plan absent\n".into()),
                        }
                    };
                    if oulipoly_kernel_broker::normal_plan_custody::observe(
                        &lane,
                        &receipt,
                        &recipient,
                        &session,
                        &config_dir,
                        &cwd,
                        &environment,
                    )? != Some(plan.clone())
                    {
                        return Err(io::Error::other("normal plan readback changed"));
                    }
                    let live = PinnedProcess::open(recipient.host_pid)?;
                    if live.boot_id != recipient.boot_id
                        || live.starttime_ticks != recipient.starttime_ticks
                        || live.pidns_dev != recipient.pidns_dev
                        || live.pidns_ino != recipient.pidns_ino
                    {
                        return Err(io::Error::other("normal plan actor changed"));
                    }
                    live.verify()?;
                    if matches!(operation, 0x88 | 0x89 | 0x8a | 0x8b) {
                        let _admission_guard = if matches!(operation, 0x88 | 0x8a) {
                            let guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if guard.contains(&receipt.old_release.prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            Some(guard)
                        } else {
                            None
                        };
                        let admission = if operation == 0x88 {
                            lane.admit_normal_provider_plan(&receipt, &recipient, &session, &plan)
                                .map(Some)
                                .map_err(io::Error::other)?
                        } else {
                            lane.read_normal_provider_admission(&receipt, &recipient, &session)
                                .map_err(io::Error::other)?
                        };
                        if oulipoly_kernel_broker::normal_plan_custody::observe(
                            &lane,
                            &receipt,
                            &recipient,
                            &session,
                            &config_dir,
                            &cwd,
                            &environment,
                        )? != Some(plan.clone())
                        {
                            return Err(io::Error::other("normal admission plan changed"));
                        }
                        live.verify()?;
                        if matches!(operation, 0x8a | 0x8b) {
                            let admission = admission.ok_or_else(|| {
                                io::Error::other("normal physical admission absent")
                            })?;
                            let readback = if operation == 0x8a {
                                let materialized =
                                    oulipoly_kernel_broker::normal_plan_custody::reopen_recipe(
                                        &plan,
                                        &config_dir,
                                        &cwd,
                                        &environment,
                                    )?;
                                normal_physical::launch(
                                    &lane,
                                    &receipt,
                                    &recipient,
                                    &session,
                                    &admission,
                                    &plan,
                                    materialized,
                                    &config_dir,
                                    &state_root,
                                    peer_uid,
                                    peer_gid,
                                )?
                            } else {
                                match normal_physical::observe(
                                    &lane,
                                    &receipt,
                                    &recipient,
                                    &session,
                                    &state_root,
                                )? {
                                    Some(value) => value,
                                    None => return Ok("fresh-normal-physical absent\n".into()),
                                }
                            };
                            return Ok(format!(
                                "fresh-normal-physical {}\n",
                                serde_json::to_string(&readback)?
                            ));
                        }
                        return match admission {
                            Some(admission) => Ok(format!(
                                "fresh-normal-admission {}\n",
                                serde_json::to_string(&admission)?
                            )),
                            None => Ok("fresh-normal-admission absent\n".into()),
                        };
                    }
                    Ok(format!(
                        "fresh-normal-plan {}\n",
                        serde_json::to_string(&plan)?
                    ))
                }
                #[cfg(feature = "age319-private-broker-fixture")]
                b'5' | b'6' | b'7' | b'8' | b'9' | b'b' | b'y' | b'x' | b'$' | b'*' | b'/'
                | b'>' | b'_' | b'h' | b'f' | b'(' | b')' | b'm' | b'n' | b'o' | b'#' | b'{'
                | b'}' | b']' | b'|' | b'~' | b'?' => {
                    if !private_fixture() {
                        return Err(io::Error::other("fresh provider fixture route closed"));
                    }
                    let native_bash_request = match &payload {
                        RequestPayload::PrivateNativeBashExec { request } => Some(request.clone()),
                        _ => None,
                    };
                    let (d_key, route_request, effect_request, pty_request, descriptors) =
                        match payload {
                            RequestPayload::FreshProviderRequest {
                                request,
                                descriptors,
                            } if request.success.is_none() => {
                                (request.d_key, None, None, None, descriptors)
                            }
                            RequestPayload::PrivateNativeBashExec { request } => {
                                (request.d_key, None, None, None, Vec::new())
                            }
                            RequestPayload::FreshRouteRequest {
                                request,
                                descriptors,
                            } => (
                                request.d_key.clone(),
                                Some(request),
                                None,
                                None,
                                descriptors,
                            ),
                            RequestPayload::FreshAccountEffectRequest { request } => {
                                (request.d_key.clone(), None, Some(request), None, Vec::new())
                            }
                            RequestPayload::FreshInteractivePtyHandoff {
                                request,
                                descriptors,
                            } => (
                                request.d_key.clone(),
                                None,
                                None,
                                Some(request),
                                descriptors,
                            ),
                            _ => {
                                return Err(io::Error::other(
                                    "fresh provider/route request absent",
                                ));
                            }
                        };
                    let receipt = lane
                        .released_handoff_for_child(&d_key, &recipient)
                        .map_err(io::Error::other)?;
                    let actor_pid = peer.process.host_pid;
                    let actor_uid = peer.uid;
                    let actor_gid = peer.gid;
                    let spec = StateReadSpec {
                        protocol: "broker-release-attest-v30".into(),
                        source_generation: receipt.old_release.prepared.source_generation.clone(),
                        root_id: receipt.old_release.prepared.root_id.clone(),
                        owner_generation: receipt.old_release.prepared.owner_generation.clone(),
                        attempt_id: None,
                    };
                    let bridge = handoff_tx.as_ref().ok_or_else(|| {
                        io::Error::other("in-process release authority unavailable")
                    })?;
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
                        &runner_image,
                    )? != receipt
                    {
                        return Err(io::Error::other(
                            "fresh provider old release readback changed",
                        ));
                    }
                    let session = lane
                        .read_session(&d_key)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("fresh provider D absent"))?;
                    lane.require_released_invocation(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?;
                    let held = lane
                        .read_normal_work(&receipt, &recipient, &session)
                        .map_err(io::Error::other)?
                        .ok_or_else(|| io::Error::other("fresh provider held J absent"))?;
                    let root =
                        PinnedProcess::open(receipt.old_release.prepared.root_init.host_pid)?;
                    let actor = PinnedProcess::open(actor_pid)?;
                    if actor.boot_id != recipient.boot_id
                        || actor.starttime_ticks != recipient.starttime_ticks
                        || (actor.pidns_dev, actor.pidns_ino)
                            != (recipient.pidns_dev, recipient.pidns_ino)
                    {
                        return Err(io::Error::other("fresh provider peer incarnation changed"));
                    }
                    let binding =
                        fresh_provider::binding_from_held(&receipt, &held, &actor, &root)?;
                    let directory = state_root.join("v30/fresh-provider");
                    // The release-attestation bridge above is served by the
                    // old loop. Acquire the admission lock only after that
                    // bridge reply, before any provider admission mutation.
                    let _admission_guard = if matches!(
                        operation,
                        b'5' | b'7'
                            | b'h'
                            | b'('
                            | b'f'
                            | b')'
                            | b'm'
                            | b'#'
                            | b'{'
                            | b'}'
                            | b'~'
                            | b'?'
                    ) {
                        let guard = admission_fences
                            .lock()
                            .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                        if guard.contains(&receipt.old_release.prepared.root_id) {
                            return Err(io::Error::other("exact root admission fenced"));
                        }
                        Some(guard)
                    } else {
                        None
                    };
                    if provider_readback_v3.is_some()
                        && pty_request.is_none()
                        && !(matches!(operation, b'6' | b'8' | b'9' | b'h' | b'm' | b'n' | b'o')
                            || (route_writer_v3 && operation == b'f'))
                        && !(provider_writer_v3 && matches!(operation, b'5' | b'7'))
                    {
                        return Err(io::Error::other(
                            "v3 route, cancellation and provider K writers are closed",
                        ));
                    }
                    if let Some(pty_request) = pty_request {
                        if !matches!(operation, b'#' | b'{' | b'}' | b']' | b'|' | b'~' | b'?')
                            || (!matches!(operation, b']' | b'|' | b'~' | b'?')
                                && instance.is_closed())
                        {
                            return Err(io::Error::other("fresh PTY handoff gate closed"));
                        }
                        if operation == b']' {
                            if !descriptors.is_empty() {
                                let [master, transcript]: [File; 2] =
                                    descriptors.try_into().map_err(|_| {
                                        io::Error::other("interactive finalizer descriptors absent")
                                    })?;
                                fresh_provider::finalize_interactive_output(
                                    &directory,
                                    &binding,
                                    &pty_request,
                                    &actor,
                                    master,
                                    transcript,
                                )?;
                            }
                            return fresh_provider::observe_interactive(
                                &directory,
                                &binding,
                                &pty_request,
                            );
                        }
                        if operation == b'|' {
                            let (reply, file) = fresh_provider::interactive_output(
                                &directory,
                                &binding,
                                &pty_request,
                            )?;
                            provider_output_files = Some(vec![file]);
                            return Ok(reply);
                        }
                        if matches!(operation, b'~' | b'?') {
                            let [master]: [File; 1] = descriptors.try_into().map_err(|_| {
                                io::Error::other("resident original PTY master absent")
                            })?;
                            let resident = fresh_provider::observe_interactive_resident(
                                &directory,
                                &binding,
                                &pty_request,
                                &actor,
                                &master,
                            )?;
                            if resident.registration.session_id != session.session_id
                                || resident.registration.invocation_uuid != receipt.invocation_uuid
                                || resident.registration.creator.os_pid
                                    != i64::from(recipient.host_pid)
                                || resident.registration.creator.os_boot_id != recipient.boot_id
                                || resident.registration.creator.os_pid_starttime_ticks
                                    != recipient.starttime_ticks as i64
                            {
                                return Err(io::Error::other(
                                    "resident original root or D changed",
                                ));
                            }
                            let generation = lane
                                .register_interactive_resident(&resident.registration)
                                .map_err(io::Error::other)?;
                            let socket_identity = if operation == b'?' {
                                let statement = oulipoly_runtime::executor::cli::pty_broker::query_pty_generation_identity(
                                    &pty_request.control_path,
                                    resident.control_device,
                                    resident.control_inode,
                                ).map_err(io::Error::other)?;
                                if statement.generation_id != resident.registration.grant_id
                                    || statement.spawn_invocation_uuid != receipt.invocation_uuid
                                    || statement.creator_process != resident.registration.creator
                                    || statement.provider_process != resident.registration.provider
                                    || statement.provider_account != resident.registration.account
                                    || statement.provider_session_id != session.session_id
                                {
                                    return Err(io::Error::other(
                                        "resident broker socket challenge changed",
                                    ));
                                }
                                Some(statement)
                            } else {
                                None
                            };
                            return Ok(format!(
                                "fresh-interactive-resident {}\n",
                                serde_json::to_string(&serde_json::json!({
                                    "resident": resident,
                                    "generation": generation,
                                    "lane_id": session.lane_id,
                                    "source_generation": session.source_generation,
                                    "socket": socket_identity,
                                }))?
                            ));
                        }
                        if operation == b'#' {
                            let [master, slave]: [File; 2] =
                                descriptors.try_into().map_err(|_| {
                                    io::Error::other("fresh PTY handoff descriptors absent")
                                })?;
                            fresh_provider::attest_pre_k_interactive_pty(
                                &directory,
                                &binding,
                                &actor,
                                &pty_request,
                                &master,
                                &slave,
                            )?;
                            return Ok("fresh-pty-handoff-pre-k\n".into());
                        }
                        let mut descriptors = descriptors;
                        let relay = if operation == b'}' {
                            Some(
                                descriptors
                                    .pop()
                                    .ok_or_else(|| io::Error::other("interactive relay absent"))?,
                            )
                        } else {
                            None
                        };
                        let [image, cwd, input, recipe, source, master, slave]: [File; 7] =
                            descriptors.try_into().map_err(|_| {
                                io::Error::other("fresh interactive preparation descriptors absent")
                            })?;
                        let plan = fresh_provider::prepare_interactive_k(
                            &directory,
                            &binding,
                            &actor,
                            &pty_request,
                            image,
                            cwd,
                            input,
                            recipe,
                            source,
                            master.try_clone()?,
                            slave.try_clone()?,
                        )?;
                        if operation == b'{' {
                            return Ok("fresh-interactive-k-preparation-pre-k\n".into());
                        }
                        let relay =
                            relay.ok_or_else(|| io::Error::other("interactive relay absent"))?;
                        if !relay.metadata()?.file_type().is_socket() {
                            return Err(io::Error::other("interactive relay is not a socket"));
                        }
                        let relay = unsafe { UnixStream::from_raw_fd(relay.into_raw_fd()) };
                        let mut relay_peer: libc::ucred = unsafe { std::mem::zeroed() };
                        let mut relay_peer_len =
                            std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                        if unsafe {
                            libc::getsockopt(
                                relay.as_raw_fd(),
                                libc::SOL_SOCKET,
                                libc::SO_PEERCRED,
                                (&mut relay_peer as *mut libc::ucred).cast(),
                                &mut relay_peer_len,
                            )
                        } != 0
                            || relay_peer_len as usize != std::mem::size_of::<libc::ucred>()
                            || (relay_peer.pid, relay_peer.uid, relay_peer.gid)
                                != (actor_pid, actor_uid, actor_gid)
                        {
                            return Err(io::Error::other(
                                "interactive relay peer differs from original actor",
                            ));
                        }
                        let grant = fresh_provider::launch_interactive(
                            &directory, binding, plan, &root, &actor, actor_uid, actor_gid, master,
                            slave, relay,
                        )?;
                        if std::env::var_os(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_INTERACTIVE_K_REPLY_V1",
                        )
                        .is_some()
                        {
                            drop_interactive_k_reply = true;
                        }
                        return Ok(format!("fresh-interactive-k {grant}\n"));
                    }
                    if let Some(route_request) = route_request {
                        let expected_pin = match &held.intent {
                            oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                                if args.len() == 3
                                    && args[0] == "--model"
                                    && args[1] == route_request.model =>
                            {
                                None
                            }
                            oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                                if args.len() == 5
                                    && args[0] == "--model"
                                    && args[1] == route_request.model
                                    && args[2] == "--pin-provider" =>
                            {
                                Some(args[3].as_str())
                            }
                            _ => {
                                return Err(io::Error::other(
                                    "fresh route model differs from held root intent",
                                ));
                            }
                        };
                        if route_request.pin.as_deref() != expected_pin {
                            return Err(io::Error::other(
                                "fresh route pin differs from held root intent",
                            ));
                        }
                        if matches!(operation, b'h' | b'(') {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh route entry gate closed"));
                            }
                            let _route_lock = if provider_readback_v3.is_some() {
                                Some(fresh_provider::route_selection_lock(&directory)?)
                            } else {
                                None
                            };
                            let [image_fd, cwd, input, recipe, config_dir]: [File; 5] = descriptors
                                .try_into()
                                .map_err(|_| io::Error::other("fresh route descriptors absent"))?;
                            fresh_provider::validate_route_source(&config_dir, &route_request)?;
                            fresh_provider::bind_route_source(
                                &directory,
                                &binding,
                                &route_request,
                                &config_dir,
                                true,
                            )?;
                            let image =
                                fs::read_link(format!("/proc/self/fd/{}", image_fd.as_raw_fd()))?;
                            let plan = fresh_provider::plan_from_descriptors(
                                &image, image_fd, cwd, input, recipe,
                            )?;
                            if operation == b'h' {
                                fresh_provider::register_route_candidate(
                                    &directory,
                                    &binding,
                                    &route_request,
                                    plan,
                                    fresh_provider::terminal_recognizer_from_source(
                                        &config_dir,
                                        &route_request,
                                    )?,
                                )?;
                                if let Some(generation) = provider_readback_v3.as_ref() {
                                    generation
                                        .record_route_model(
                                            &route_request.model,
                                            &route_request.config_sha256,
                                        )
                                        .map_err(io::Error::other)?;
                                }
                            } else {
                                fresh_provider::validate_interactive_plan_source(
                                    &config_dir,
                                    &route_request,
                                    &plan,
                                )?;
                                fresh_provider::register_interactive_candidate(
                                    &directory,
                                    &binding,
                                    &route_request,
                                    plan,
                                )?;
                            }
                            return Ok("fresh-route-registered\n".into());
                        }
                        let [config_dir]: [File; 1] = descriptors
                            .try_into()
                            .map_err(|_| io::Error::other("fresh route source absent"))?;
                        fresh_provider::validate_route_source(&config_dir, &route_request)?;
                        fresh_provider::bind_route_source(
                            &directory,
                            &binding,
                            &route_request,
                            &config_dir,
                            false,
                        )?;
                        let selection = if operation == b')' {
                            serde_json::to_string(&fresh_provider::select_interactive_plan(
                                &directory,
                                &binding,
                                &route_request,
                            )?)?
                        } else if route_writer_v3 {
                            let generation = provider_readback_v3
                                .as_ref()
                                .ok_or_else(|| io::Error::other("v3 route admission absent"))?;
                            serde_json::to_string(&fresh_provider::select_route_v3(
                                &directory,
                                &binding,
                                &route_request,
                                generation,
                            )?)?
                        } else {
                            serde_json::to_string(&fresh_provider::select_route_with_index(
                                &directory,
                                &binding,
                                &route_request,
                                route_index.as_ref(),
                            )?)?
                        };
                        if route_writer_v3
                            && std::env::var_os(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ROUTE_V3_REPLY_V1",
                            )
                            .is_some()
                            && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                                .is_some_and(|gate| {
                                    !Path::new(&gate).join("route-reply-dropped").exists()
                                })
                        {
                            drop_route_reply = true;
                        }
                        return Ok(format!("fresh-route-selected {selection}\n"));
                    }
                    if let Some(effect_request) = effect_request {
                        if operation == b'm' {
                            if let Some(index) = route_index.as_ref() {
                                let receipt = directory
                                    .join(format!("{}.route-selection.json", binding.handoff_id));
                                let receipt_present = match fs::symlink_metadata(receipt) {
                                    Ok(_) => true,
                                    Err(error) if error.kind() == io::ErrorKind::NotFound => false,
                                    Err(error) => return Err(error),
                                };
                                let decision_present = index
                                    .decision(&binding.handoff_id)
                                    .map_err(io::Error::other)?
                                    .is_some();
                                if receipt_present || decision_present {
                                    index
                                        .require_live_route(&binding.handoff_id)
                                        .map_err(io::Error::other)?;
                                }
                            }
                        }
                        if instance.is_closed() && operation == b'm' {
                            return Err(io::Error::other("fresh account effect entry gate closed"));
                        }
                        let expected_pin = match &held.intent {
                            oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                                if args.len() == 3
                                    && args[0] == "--model"
                                    && args[1] == effect_request.model =>
                            {
                                None
                            }
                            oulipoly_state::mailbox::FreshRootWorkIntent::NormalCli(args)
                                if args.len() == 5
                                    && args[0] == "--model"
                                    && args[1] == effect_request.model
                                    && args[2] == "--pin-provider" =>
                            {
                                Some(args[3].as_str())
                            }
                            _ => {
                                return Err(io::Error::other(
                                    "fresh effect model differs from held root intent",
                                ));
                            }
                        };
                        if expected_pin.is_some_and(|pin| pin != effect_request.account) {
                            return Err(io::Error::other(
                                "fresh effect account differs from held pin",
                            ));
                        }
                        if operation == b'o' {
                            let generation = provider_readback_v3.as_ref().ok_or_else(|| {
                                io::Error::other("shared quota requires keyed v3 admission")
                            })?;
                            let _route_lock = fresh_provider::route_selection_lock(&directory)?;
                            let (shared, keyed_io) = fresh_index::measure_keyed_io(|| {
                                let _physical_io =
                                    fresh_index::ReaderIoGuard::start("v3-shared-quota");
                                fresh_provider::shared_quota_effect_v3(
                                    &directory,
                                    generation,
                                    &binding,
                                    &effect_request,
                                )
                            });
                            eprintln!("age319 v3 shared quota keyed I/O: {keyed_io:?}");
                            if matches!(&shared, Ok(Some(_)))
                                && std::env::var_os(
                                    "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_SHARED_Q_REPLY_V3_V1",
                                )
                                .is_some()
                                && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                                    .is_some_and(|gate| {
                                        !Path::new(&gate)
                                            .join("account-effect-reply-dropped")
                                            .exists()
                                    })
                            {
                                drop_account_effect_reply = true;
                            }
                            return Ok(format!(
                                "fresh-shared-quota {}\n",
                                serde_json::to_string(&shared?)?
                            ));
                        }
                        let effect = if let Some(generation) = provider_readback_v3.as_ref() {
                            // The route reader holds this lock through its
                            // receipt/cursor publication. A quota/auth K or Q
                            // cannot change any selected account revision in
                            // that interval.
                            let _route_lock = fresh_provider::route_selection_lock(&directory)?;
                            let boundary = if operation == b'm' {
                                "v3-quota-begin"
                            } else {
                                "v3-quota-observe"
                            };
                            let (effect, keyed_io) = fresh_index::measure_keyed_io(|| {
                                let _physical_io = fresh_index::ReaderIoGuard::start(boundary);
                                if operation == b'm' {
                                    fresh_provider::begin_quota_effect_v3(
                                        &directory,
                                        generation,
                                        &binding,
                                        &effect_request,
                                        &root,
                                        &actor,
                                        actor_uid,
                                        actor_gid,
                                    )
                                } else {
                                    fresh_provider::observe_quota_effect_v3(
                                        &directory,
                                        generation,
                                        &binding,
                                        &effect_request,
                                    )
                                }
                            });
                            eprintln!("age319 v3 quota {boundary} keyed I/O: {keyed_io:?}");
                            effect?
                        } else if operation == b'm' {
                            fresh_provider::begin_account_effect_indexed(
                                &directory,
                                &binding,
                                &effect_request,
                                &root,
                                &actor,
                                actor_uid,
                                actor_gid,
                                route_index.as_ref(),
                            )?
                        } else {
                            fresh_provider::observe_account_effect_indexed(
                                &directory,
                                &binding,
                                &effect_request,
                                route_index.as_ref(),
                            )?
                        };
                        if operation == b'm'
                            && (std::env::var_os(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                            ).is_some()
                                || (effect_request.kind == oulipoly_kernel_broker::protocol::FreshAccountEffectKind::AuthRefresh
                                    && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_AUTH_EFFECT_REPLY_V3_V1").is_some()))
                        {
                            drop_account_effect_reply = true;
                        }
                        return Ok(format!(
                            "fresh-account-effect {}\n",
                            serde_json::to_string(&effect)?
                        ));
                    }
                    if operation == b'5' || operation == b'b' {
                        if instance.is_closed() {
                            return Err(io::Error::other("fresh provider K entry gate closed"));
                        }
                        let _v3_route_lock = if provider_writer_v3 {
                            Some(fresh_provider::route_selection_lock(&directory)?)
                        } else {
                            None
                        };
                        let [image_fd, cwd, input, recipe]: [File; 4] = descriptors
                            .try_into()
                            .map_err(|_| io::Error::other("fresh provider descriptors absent"))?;
                        let image =
                            fs::read_link(format!("/proc/self/fd/{}", image_fd.as_raw_fd()))?;
                        let plan = fresh_provider::plan_from_descriptors(
                            &image, image_fd, cwd, input, recipe,
                        )?;
                        let v3_selected = if provider_writer_v3 {
                            let generation = provider_readback_v3
                                .as_ref()
                                .ok_or_else(|| io::Error::other("v3 provider generation absent"))?;
                            if fresh_provider::grant_for_binding(&directory, &binding)?.is_some() {
                                return Err(io::Error::other(
                                    "v3 provider grant already announced; observe exact D",
                                ));
                            }
                            Some(fresh_provider::require_selected_plan_v3(
                                &directory, &binding, &plan, generation,
                            )?)
                        } else {
                            fresh_provider::require_selected_plan_indexed(
                                &directory,
                                &binding,
                                &plan,
                                route_index.as_ref(),
                            )?;
                            None
                        };
                        if let Some(index) = route_index.as_ref() {
                            index
                                .require_live_route(&binding.handoff_id)
                                .map_err(io::Error::other)?;
                        }
                        let mut prepared =
                            fresh_provider::prepare(&directory, binding.clone(), plan)?;
                        let v3_revision = if let Some((account, revision)) = v3_selected.as_ref() {
                            Some(fresh_provider::announce_provider_v3(
                                &prepared,
                                provider_readback_v3.as_ref().unwrap(),
                                account,
                                *revision,
                            )?)
                        } else {
                            None
                        };
                        if let Some(index) = route_index.as_ref() {
                            prepared.announce_indexed_grant(index)?;
                        }
                        let grant = if operation == b'b' {
                            if v3_selected.is_some() {
                                return Err(io::Error::other(
                                    "private native Codex v3 writer not joined",
                                ));
                            }
                            let expected = std::env::var(
                                "OULIPOLY_KERNEL_BROKER_PRIVATE_NATIVE_CODEX_SHA256_V1",
                            )
                            .map_err(|_| {
                                io::Error::other("private native Codex image SHA pin absent")
                            })?;
                            let control = fresh_provider::launch_native_codex(
                                prepared,
                                &root,
                                &actor,
                                actor_uid,
                                actor_gid,
                                route_index.as_ref(),
                                &expected,
                            )?;
                            let grant = control.grant_id().to_owned();
                            native_codex_controls.insert(grant.clone(), control);
                            grant
                        } else if let Some((account, _)) = v3_selected {
                            fresh_provider::launch_v3_provider(
                                prepared,
                                &root,
                                &actor,
                                actor_uid,
                                actor_gid,
                                provider_readback_v3.as_ref().unwrap(),
                                &account,
                                v3_revision.unwrap(),
                            )?
                        } else {
                            fresh_provider::launch(
                                prepared,
                                &root,
                                &actor,
                                actor_uid,
                                actor_gid,
                                route_index.as_ref(),
                            )?
                        };
                        if std::env::var_os(
                            "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_PROVIDER_K_REPLY_V1",
                        )
                        .is_some()
                        {
                            drop_provider_k_reply = true;
                        }
                        return Ok(format!("fresh-provider-k {grant}\n"));
                    }
                    let grant = if operation == b'9' {
                        let [image_fd, cwd, input, recipe]: [File; 4] =
                            descriptors.try_into().map_err(|_| {
                                io::Error::other("fresh provider readback descriptors absent")
                            })?;
                        let image =
                            fs::read_link(format!("/proc/self/fd/{}", image_fd.as_raw_fd()))?;
                        let plan = fresh_provider::plan_from_descriptors(
                            &image, image_fd, cwd, input, recipe,
                        )?;
                        fresh_provider::grant_for_matching_plan(&directory, &binding, &plan)?
                    } else {
                        fresh_provider::grant_for_binding(&directory, &binding)?
                            .ok_or_else(|| io::Error::other("fresh provider grant absent"))?
                    };
                    if operation == b'y' {
                        let control = native_codex_controls.get_mut(&grant).ok_or_else(|| {
                            io::Error::other("native Codex control absent for selected K")
                        })?;
                        let readback = control.readback(&directory, &binding)?;
                        return Ok(format!(
                            "fresh-native-codex {}\n",
                            serde_json::to_string(&readback)?
                        ));
                    }
                    if operation == b'$' {
                        let request = native_bash_request
                            .ok_or_else(|| io::Error::other("native Bash request absent"))?;
                        if native_bash_execs.contains_key(&grant) {
                            return Err(io::Error::other("native Bash command already submitted"));
                        }
                        let control = native_codex_controls.remove(&grant).ok_or_else(|| {
                            io::Error::other(
                                "native Codex control absent; command unknown, no replay",
                            )
                        })?;
                        let image = bash_image
                            .as_ref()
                            .ok_or_else(|| io::Error::other("native Bash pinned image absent"))?;
                        let gate = PathBuf::from(
                            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                                .map_err(io::Error::other)?,
                        );
                        let execution = control.begin_bash(
                            &directory,
                            &binding,
                            image,
                            &gate,
                            &request.request_id,
                            request.notify,
                        )?;
                        native_bash_execs.insert(grant.clone(), execution);
                        if std::env::var_os("AGE319_PRIVATE_NATIVE_BASH_DROP_REPLY_V1").is_some() {
                            drop_native_bash_reply = true;
                        }
                        return Ok(format!("native-bash-submitted {}\n", request.request_id));
                    }
                    if operation == b'x' {
                        let execution = native_bash_execs.get_mut(&grant).ok_or_else(|| {
                            io::Error::other(
                                "native Bash command control absent; unknown, no replay",
                            )
                        })?;
                        return execution.observe(&binding);
                    }
                    if operation == b'*' {
                        if native_turn_execs.contains_key(&grant) {
                            return Err(io::Error::other(
                                "native turn already reserved; no replay",
                            ));
                        }
                        let execution = native_bash_execs.get_mut(&grant).ok_or_else(|| {
                            io::Error::other("native Bash result absent; native turn refused")
                        })?;
                        let control = execution.take_control(&binding)?;
                        let turn = control.begin_turn(
                            &directory,
                            state_root,
                            &binding,
                            std::env::var_os("AGE319_PRIVATE_NATIVE_RECIPIENT_ACK_V1").is_some(),
                        )?;
                        native_turn_execs.insert(grant.clone(), turn);
                        if std::env::var_os("AGE319_PRIVATE_NATIVE_TURN_DROP_REPLY_V1").is_some() {
                            drop_native_turn_reply = true;
                        }
                        return Ok(format!("native-turn-submitted {grant}\n"));
                    }
                    if operation == b'/' {
                        let execution = native_turn_execs.get_mut(&grant).ok_or_else(|| {
                            io::Error::other("native turn control absent; unknown, no replay")
                        })?;
                        return execution.observe(&binding);
                    }
                    if operation == b'>' {
                        if native_f_execs.contains_key(&grant) {
                            return Err(io::Error::other(
                                "native F turn already reserved; no replay",
                            ));
                        }
                        let gate = PathBuf::from(
                            std::env::var("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                                .map_err(io::Error::other)?,
                        );
                        let report: serde_json::Value =
                            serde_json::from_slice(&fs::read(gate.join("bash-recipient-output"))?)?;
                        if report["mode"] != "native_k_pending" {
                            return Err(io::Error::other("native F source report mode changed"));
                        }
                        let delivery_request = report["delivery_request_id"]
                            .as_str()
                            .ok_or_else(|| io::Error::other("native F request absent"))?;
                        let token = report["grant"]["delivery_token"]
                            .as_str()
                            .ok_or_else(|| io::Error::other("native F token absent"))?;
                        let bash_request = report["bash_request_id"]
                            .as_str()
                            .ok_or_else(|| io::Error::other("native F Bash W request absent"))?;
                        let (_, recipient) = lane
                            .released_handoff_for_root(&receipt.old_release.prepared.root_id)
                            .map_err(io::Error::other)?;
                        let execution = native_turn_execs.get_mut(&grant).ok_or_else(|| {
                            io::Error::other("original native ACK control absent; F refused")
                        })?;
                        let control = execution.take_control(&binding)?;
                        let turn = control.begin_f_turn(
                            &directory,
                            state_root,
                            &binding,
                            bash_request,
                            delivery_request,
                            token,
                            &recipient,
                        )?;
                        native_f_execs.insert(grant.clone(), turn);
                        if std::env::var_os("AGE319_PRIVATE_NATIVE_F_TURN_DROP_REPLY_V1").is_some()
                        {
                            drop_native_f_turn_reply = true;
                        }
                        return Ok(format!("native-f-submitted {grant}\n"));
                    }
                    if operation == b'_' {
                        let execution = native_f_execs.get_mut(&grant).ok_or_else(|| {
                            io::Error::other("native F control absent; unknown, no replay")
                        })?;
                        return execution.observe(&binding);
                    }
                    if let Some(v3) = provider_readback_v3.as_ref() {
                        if provider_writer_v3 {
                            fresh_provider::settle_v3_provider(v3, &directory, &binding, &grant)?;
                            // A physical drain file precedes the PID1 wait and
                            // keyed Q/terminal CAS. This is still pending work,
                            // never a certified Q or permission to replay K.
                            if operation == b'6'
                                && directory.join(format!("{grant}.drain.json")).exists()
                                && matches!(
                                    fresh_provider::observe(&directory, &grant)?,
                                    fresh_provider::Observation::Pending
                                        | fresh_provider::Observation::ProviderExited(_)
                                )
                            {
                                return Ok(format!("fresh-provider-pending {grant}\n"));
                            }
                        }
                        let indexed =
                            fresh_provider::require_v3_provider_binding(v3, &directory, &binding)?;
                        if indexed != grant {
                            return Err(io::Error::other("v3 provider plan/grant differs"));
                        }
                    }
                    if operation == b'7' {
                        fresh_provider::cancel(&directory, &grant)?;
                        return Ok(format!("fresh-provider-cancel {grant}\n"));
                    }
                    if let Some(index) = route_index.as_ref() {
                        fresh_provider::reconcile_indexed_provider_binding(
                            index, &directory, &binding,
                        )?;
                    }
                    let result = match fresh_provider::observe(&directory, &grant)? {
                        fresh_provider::Observation::Unknown => {
                            format!("fresh-provider-unknown {grant}\n")
                        }
                        fresh_provider::Observation::Pending => {
                            format!("fresh-provider-pending {grant}\n")
                        }
                        fresh_provider::Observation::ProviderExited(status) => {
                            format!("fresh-provider-exited {grant} {status}\n")
                        }
                        fresh_provider::Observation::Drained {
                            status,
                            stdout,
                            stderr,
                            stdout_len,
                            stderr_len,
                            stdout_sha256,
                            stderr_sha256,
                            cancelled,
                        } => {
                            if operation == b'6'
                                && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1")
                                    .is_some_and(|path| {
                                        Path::new(&path).join("provider-drop-q-reply").exists()
                                    })
                            {
                                drop_provider_q_reply = true;
                            }
                            if operation == b'8' {
                                provider_output_files = Some(vec![stdout, stderr]);
                                format!(
                                    "fresh-provider-output {grant} {status} {stdout_len} {stdout_sha256} {stderr_len} {stderr_sha256} {cancelled}\n"
                                )
                            } else {
                                format!(
                                    "fresh-provider-drained {grant} {status} {stdout_len} {stderr_len} {cancelled}\n"
                                )
                            }
                        }
                    };
                    Ok(result)
                }
                b'F' => {
                    // No production delivery can be authorized by a private
                    // prefix D/session or by a copied old mailbox row.
                    if !private_fixture() {
                        return Err(io::Error::other(
                            "fresh recipient effect closed until released-child handoff, real invocation, registration, W, result and ACK",
                        ));
                    }
                    let RequestPayload::FreshRecipientRequest { request } = payload else {
                        return Err(io::Error::other("fresh recipient request absent"));
                    };
                    let reply = match request {
                        FreshRecipientRequest::FenceRootTerminal { ref d_key } => {
                            let root = lane
                                .released_handoff_for_child(d_key, &recipient)
                                .map_err(io::Error::other)?;
                            let session = lane
                                .read_session(d_key)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| io::Error::other("root drain D session absent"))?;
                            let read = lane
                                .read_private_root_terminal(&root, &recipient, &session)
                                .map_err(io::Error::other)?;
                            let bridge = drain_tx
                                .as_ref()
                                .ok_or_else(|| io::Error::other("root drain bridge absent"))?;
                            let inventory = fence_root_from_terminal(bridge, &root, &read)?;
                            serde_json::json!({"kind":"root_drain_readback", "inventory":inventory})
                        }
                        FreshRecipientRequest::ReadRootTerminal { ref d_key }
                        | FreshRecipientRequest::SettleRootTerminal { ref d_key }
                        | FreshRecipientRequest::RepairRootTerminal { ref d_key }
                        | FreshRecipientRequest::BeginRootPublication { ref d_key, .. }
                        | FreshRecipientRequest::BeginRootCallerResult { ref d_key, .. }
                        | FreshRecipientRequest::SettleRootCallerResult { ref d_key, .. } => {
                            let root = lane
                                .released_handoff_for_child(&d_key, &recipient)
                                .map_err(io::Error::other)?;
                            let session = lane
                                .read_session(&d_key)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| {
                                    io::Error::other("root terminal D session absent")
                                })?;
                            let read = match &request {
                                FreshRecipientRequest::ReadRootTerminal { .. } => {
                                    lane.read_private_root_terminal(&root, &recipient, &session)
                                }
                                FreshRecipientRequest::SettleRootTerminal { .. } => {
                                    lane.settle_private_root_terminal(&root, &recipient, &session)
                                }
                                FreshRecipientRequest::RepairRootTerminal { .. } => {
                                    lane.repair_private_root_terminal(&root, &recipient, &session)
                                }
                                FreshRecipientRequest::BeginRootPublication {
                                    artifact_base64,
                                    ..
                                } => {
                                    let bytes = base64::engine::general_purpose::STANDARD
                                        .decode(artifact_base64)
                                        .map_err(io::Error::other)?;
                                    lane.begin_private_root_publication(
                                        &root, &recipient, &session, &bytes,
                                    )
                                }
                                FreshRecipientRequest::BeginRootCallerResult { result, .. } => lane
                                    .begin_private_root_caller_result(
                                        &root, &recipient, &session, result,
                                    ),
                                FreshRecipientRequest::SettleRootCallerResult {
                                    result, ..
                                } => lane.settle_private_root_caller_result(
                                    &root, &recipient, &session, result,
                                ),
                                _ => unreachable!(),
                            }
                            .map_err(io::Error::other)?;
                            if matches!(
                                &request,
                                FreshRecipientRequest::SettleRootCallerResult { .. }
                            ) {
                                #[cfg(feature = "age319-private-broker-fixture")]
                                if provider_writer_v3 {
                                    fresh_provider::require_v3_terminal_publication(
                                        provider_readback_v3.as_ref().ok_or_else(|| {
                                            io::Error::other("v3 terminal generation absent")
                                        })?,
                                        &state_root.join("v30/fresh-provider"),
                                        &read,
                                    )?;
                                }
                                if let Some(bridge) = terminal_tx.as_ref() {
                                    settle_entry_from_terminal(bridge, &read)?;
                                }
                            }
                            serde_json::json!({"kind":"root_terminal_readback", "terminal":read})
                        }
                        FreshRecipientRequest::ActivateBashSource { request_id } => {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh recipient entry gate closed"));
                            }
                            let child = lane
                                .read_bash_child(&request_id)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| {
                                    io::Error::other("fresh Bash notification child absent")
                                })?;
                            let _guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if _guard.contains(&child.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            let seq = lane
                                .request_private_bash_notification(&request_id, &recipient)
                                .map_err(io::Error::other)?;
                            serde_json::json!({"kind":"notification_requested", "request_id":request_id,
                                "seq":seq, "evidence":"explicit_original_listener_request"})
                        }
                        FreshRecipientRequest::Submit {
                            allocation_request_id,
                            delivery_request_id,
                        } => {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh recipient entry gate closed"));
                            }
                            let session = lane
                                .read_session(&allocation_request_id)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| {
                                    io::Error::other("fresh session allocation absent")
                                })?;
                            let (root_id, _) = lane
                                .recipient_binding(&session, &recipient)
                                .map_err(io::Error::other)?;
                            let _guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if _guard.contains(&root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            let submitted = lane
                                .submit_recipient_delivery(
                                    &delivery_request_id,
                                    &session,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?;
                            submitted_grant = Some(submitted.readback.grant_id.clone());
                            fresh_payload_reply("delivery", submitted)?
                        }
                        FreshRecipientRequest::Read {
                            delivery_request_id,
                        } => {
                            let grant = lane
                                .read_recipient_delivery_by_request(
                                    &delivery_request_id,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?;
                            serde_json::json!({ "kind": "readback", "grant": grant })
                        }
                        FreshRecipientRequest::Recover {
                            delivery_request_id,
                        } => {
                            let recovered = lane
                                .recover_recipient_delivery_by_request(
                                    &delivery_request_id,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?;
                            fresh_payload_reply("recovered_delivery", recovered)?
                        }
                        FreshRecipientRequest::PrepareNativeF { preparation } => {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh recipient entry gate closed"));
                            }
                            let delivery = lane
                                .read_recipient_delivery_by_request(
                                    &preparation.delivery_request_id,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?
                                .ok_or_else(|| io::Error::other("native F delivery absent"))?;
                            let _guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if _guard.contains(&delivery.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            if let Some(existing) = lane
                                .read_native_f_preparation(
                                    &preparation.preparation_request_id,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?
                            {
                                verify_native_f_readback(&lane, &existing)
                                    .map_err(io::Error::other)?;
                            }
                            let prepared = lane
                                .prepare_native_f_input(
                                    &preparation,
                                    &recipient,
                                    |generation, path, device, inode| {
                                        verify_native_f_resident(
                                            generation,
                                            path,
                                            device,
                                            inode,
                                            &preparation.provider_instance_id,
                                            &preparation.settings_id,
                                            generation
                                                .session_id
                                                .as_deref()
                                                .ok_or("native F provider session absent")?,
                                        )
                                    },
                                )
                                .map_err(io::Error::other)?;
                            verify_native_f_readback(&lane, &prepared).map_err(io::Error::other)?;
                            serde_json::json!({"kind":"native_f_preparation", "preparation":prepared})
                        }
                        FreshRecipientRequest::ReadNativeFPreparation {
                            preparation_request_id,
                        } => {
                            let prepared = lane
                                .read_native_f_preparation(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?;
                            if let Some(record) = prepared.as_ref() {
                                verify_native_f_readback(&lane, record)
                                    .map_err(io::Error::other)?;
                            }
                            serde_json::json!({"kind":"native_f_preparation_readback", "preparation":prepared})
                        }
                        FreshRecipientRequest::BeginNativeFSubmission {
                            preparation_request_id,
                        } => {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh recipient entry gate closed"));
                            }
                            let prepared = lane
                                .read_native_f_preparation(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| io::Error::other("native F preparation absent"))?;
                            let _guard = admission_fences
                                .lock()
                                .map_err(|_| io::Error::other("root admission fence poisoned"))?;
                            if _guard.contains(&prepared.root_id) {
                                return Err(io::Error::other("exact root admission fenced"));
                            }
                            let fence = lane
                                .begin_native_f_submission(
                                    &preparation_request_id,
                                    &recipient,
                                    |generation, path, device, inode| {
                                        verify_native_f_resident(
                                            generation,
                                            path,
                                            device,
                                            inode,
                                            &prepared.provider_instance_id,
                                            &prepared.settings_id,
                                            &prepared.provider_session_id,
                                        )
                                    },
                                )
                                .map_err(io::Error::other)?;
                            serde_json::json!({"kind":"native_f_submission_fence", "fence":fence})
                        }
                        FreshRecipientRequest::ReadNativeFSubmission {
                            preparation_request_id,
                        } => {
                            let fence = lane
                                .read_native_f_submission(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?;
                            serde_json::json!({"kind":"native_f_submission_readback", "fence":fence})
                        }
                        FreshRecipientRequest::RecordNativeFTransport {
                            preparation_request_id,
                        } => {
                            let prepared = lane
                                .read_native_f_preparation(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?
                                .ok_or_else(|| {
                                    io::Error::other("native F transport preparation absent")
                                })?;
                            verify_native_f_readback(&lane, &prepared).map_err(io::Error::other)?;
                            let transport = lane
                                .record_native_f_transport(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?;
                            #[cfg(feature = "age319-private-broker-fixture")]
                            private_native_f_drop_reply("transport");
                            serde_json::json!({"kind":"native_f_transport", "transport":transport})
                        }
                        FreshRecipientRequest::ReadNativeFTransport {
                            preparation_request_id,
                        } => {
                            let transport = lane
                                .read_native_f_transport(&preparation_request_id, &recipient)
                                .map_err(io::Error::other)?;
                            if transport.is_some() {
                                let prepared = lane
                                    .read_native_f_preparation(&preparation_request_id, &recipient)
                                    .map_err(io::Error::other)?
                                    .ok_or_else(|| {
                                        io::Error::other("native F transport preparation absent")
                                    })?;
                                verify_native_f_readback(&lane, &prepared)
                                    .map_err(io::Error::other)?;
                            }
                            serde_json::json!({"kind":"native_f_transport_readback", "transport":transport})
                        }
                        FreshRecipientRequest::CertifyNativeFReceipt {
                            preparation_request_id,
                            observed,
                        } => {
                            let receipt = certify_native_f_with_source(
                                &mut lane,
                                &preparation_request_id,
                                &recipient,
                                &observed,
                            )
                            .map_err(io::Error::other)?;
                            #[cfg(feature = "age319-private-broker-fixture")]
                            private_native_f_drop_reply("receipt");
                            serde_json::json!({"kind":"native_f_receipt", "receipt":receipt})
                        }
                        FreshRecipientRequest::ReadNativeFReceipt {
                            preparation_request_id,
                        } => {
                            let receipt = read_native_f_receipt_with_source(
                                &lane,
                                &preparation_request_id,
                                &recipient,
                            )
                            .map_err(io::Error::other)?;
                            serde_json::json!({"kind":"native_f_receipt_readback", "receipt":receipt})
                        }
                        FreshRecipientRequest::AcknowledgeNativeFReceipt {
                            preparation_request_id,
                            delivery_token,
                        } => {
                            let grant = acknowledge_native_f_receipt_with_source(
                                &mut lane,
                                &preparation_request_id,
                                &delivery_token,
                                &recipient,
                            )
                            .map_err(io::Error::other)?;
                            #[cfg(feature = "age319-private-broker-fixture")]
                            private_native_f_drop_reply("ack");
                            serde_json::json!({"kind":"native_f_auto_ack", "grant":grant})
                        }
                        FreshRecipientRequest::ReadNativeFAutoAck {
                            preparation_request_id,
                        } => {
                            let ack = read_native_f_auto_ack_with_source(
                                &lane,
                                &preparation_request_id,
                                &recipient,
                            )
                            .map_err(io::Error::other)?;
                            serde_json::json!({"kind":"native_f_auto_ack_readback", "ack":ack})
                        }
                        FreshRecipientRequest::Acknowledge {
                            grant_id,
                            delivery_token,
                        } => {
                            let grant = lane
                                .acknowledge_recipient_delivery(
                                    &grant_id,
                                    &delivery_token,
                                    &recipient,
                                )
                                .map_err(io::Error::other)?;
                            serde_json::json!({ "kind": "ack", "grant": grant })
                        }
                        FreshRecipientRequest::Delegate {
                            grant_ids,
                            delegate,
                        } => {
                            let pinned = PinnedProcess::open(delegate.host_pid)?;
                            pinned.verify()?;
                            let observed = FreshRecipientIdentity {
                                host_pid: pinned.host_pid,
                                boot_id: pinned.boot_id.clone(),
                                starttime_ticks: pinned.starttime_ticks,
                                pidns_dev: pinned.pidns_dev,
                                pidns_ino: pinned.pidns_ino,
                            };
                            if observed != delegate || !pinned.same_executable_as(&runner_image)? {
                                return Err(io::Error::other(
                                    "delegate is not a live exact Runner process",
                                ));
                            }
                            let batch = lane
                                .delegate_ack_batch(&grant_ids, &recipient, &delegate)
                                .map_err(io::Error::other)?;
                            serde_json::json!({ "kind": "delegation", "batch": batch })
                        }
                        FreshRecipientRequest::AcknowledgeDelegated { delegation_id } => {
                            let batch = lane
                                .acknowledge_delegated_batch(&delegation_id, &recipient)
                                .map_err(io::Error::other)?;
                            serde_json::json!({ "kind": "delegated_ack", "batch": batch })
                        }
                        FreshRecipientRequest::Lookup {
                            lane_id,
                            session_id,
                            seq,
                        } => {
                            let bytes = lane
                                .lookup_payload(&lane_id, &session_id, seq)
                                .map_err(io::Error::other)?;
                            serde_json::json!({ "kind": "payload", "byte_len": bytes.len(),
                                "payload_base64": base64::engine::general_purpose::STANDARD.encode(bytes) })
                        }
                    };
                    serde_json::to_string(&reply).map_err(io::Error::other)
                }
                _ => Err(io::Error::other(
                    "fresh v30 effects closed pending source/recipient/K/Q/Runner-result/ACK lineage",
                )),
            }
        })();
        #[cfg(feature = "age319-private-broker-fixture")]
        if drop_provider_k_reply
            || drop_native_bash_reply
            || drop_native_turn_reply
            || drop_native_f_turn_reply
            || drop_provider_q_reply
            || drop_account_effect_reply
            || drop_route_reply
            || drop_interactive_k_reply
            || drop_root_h_reply
        {
            if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
                let marker = if drop_native_f_turn_reply {
                    "native-f-turn-reply-dropped"
                } else if drop_native_turn_reply {
                    "native-turn-reply-dropped"
                } else if drop_native_bash_reply {
                    "native-bash-reply-dropped"
                } else if drop_root_h_reply {
                    "root-h-reply-dropped"
                } else if drop_route_reply {
                    "route-reply-dropped"
                } else if drop_provider_k_reply {
                    "provider-k-reply-dropped"
                } else if drop_interactive_k_reply {
                    "interactive-k-reply-dropped"
                } else if drop_provider_q_reply {
                    "provider-q-reply-dropped"
                } else {
                    "account-effect-reply-dropped"
                };
                fs::write(Path::new(&gate).join(marker), b"yes")?;
            }
            continue;
        }
        let response = answer.unwrap_or_else(|error| {
            eprintln!(
                "oulipoly broker request error: opcode={} stage={} key_sha256={} kind={:?} os_error={:?}",
                diagnostic_opcode as char,
                diagnostic_stage,
                {
                    #[cfg(feature = "age319-private-broker-fixture")]
                    { diagnostic_key_hash.as_str() }
                    #[cfg(not(feature = "age319-private-broker-fixture"))]
                    { "unavailable" }
                },
                error.kind(),
                error.raw_os_error()
            );
            let wire: String = error
                .to_string()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(240)
                .collect();
            format!("error {wire}\n")
        });
        #[cfg(feature = "age319-private-broker-fixture")]
        if diagnostic_opcode == b'v'
            && provider_output_files.is_some()
            && std::env::var_os("AGE319_PRIVATE_SYNC_PARTIAL_SOCKET_REPLY_V1").is_some()
        {
            let _ = stream.write_all(&response.as_bytes()[..response.len().min(16)]);
            continue;
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        if let Some(files) = provider_output_files {
            let fds: Vec<_> = files.iter().map(AsRawFd::as_raw_fd).collect();
            let mut iov = libc::iovec {
                iov_base: response.as_ptr().cast_mut().cast(),
                iov_len: response.len(),
            };
            let mut control = [0u8; 64];
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds.as_slice()) as _) } as usize;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&msg);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len =
                    libc::CMSG_LEN(std::mem::size_of_val(fds.as_slice()) as _) as usize;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr(),
                    libc::CMSG_DATA(header).cast(),
                    fds.len(),
                );
                let sent = libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL);
                if sent != response.len() as isize {
                    eprintln!(
                        "oulipoly broker descriptor reply incomplete: opcode={} sent={sent}",
                        diagnostic_opcode as char
                    );
                }
            }
            continue;
        }
        let write = stream.write_all(response.as_bytes());
        if matches!(
            diagnostic_opcode,
            b'C' | b'c' | b'E' | b'O' | b'%' | b'&' | b'!'
        ) {
            eprintln!(
                "oulipoly broker reply: opcode={} bytes={} newline={} prefix={} write={}",
                diagnostic_opcode as char,
                response.len(),
                response.ends_with('\n'),
                if response.starts_with("error ") {
                    "error"
                } else {
                    "success"
                },
                if write.is_ok() { "ok" } else { "error" }
            );
            if let Err(error) = &write {
                eprintln!(
                    "oulipoly broker reply write error: opcode={} kind={:?} os_error={:?}",
                    diagnostic_opcode as char,
                    error.kind(),
                    error.raw_os_error()
                );
            }
        }
        if write.is_ok() {
            if let Some(grant_id) = submitted_grant {
                let _ = lane.mark_recipient_submitted(&grant_id);
            }
        }
    }
    Ok(())
}

pub fn run() {
    let args: Vec<_> = std::env::args_os().collect();
    let result = match args.as_slice() {
        [_, mode, state_root] if mode == "--normal-provider-supervisor" => {
            normal_physical::supervisor(Path::new(state_root))
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        [_, mode, namespace_fd, socket_fd] if mode == "--age319-v2-wake-launcher" => {
            if !private_fixture() {
                Err(io::Error::other("private v2 wake launcher unavailable"))
            } else {
                let namespace_fd = namespace_fd.to_string_lossy().parse::<i32>();
                let socket_fd = socket_fd.to_string_lossy().parse::<i32>();
                match (namespace_fd, socket_fd) {
                    (Ok(namespace_fd), Ok(socket_fd)) => v2_wake::launcher(namespace_fd, socket_fd),
                    _ => Err(io::Error::other("invalid v2 wake inherited descriptors")),
                }
            }
        }
        [_] => serve(),
        [_, mode] if mode == "--initialize-fresh-v30" => {
            FreshV30Lane::initialize_at(Path::new(STATE))
                .map(|identity| {
                    println!("{}", serde_json::to_string(&identity).unwrap());
                })
                .map_err(io::Error::other)
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        [_, mode, source]
            if mode == "--offline-rebuild-fresh-index"
                || mode == "--offline-rebuild-fresh-index-v3" =>
        {
            let state = if private_fixture() {
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1")
                    .ok_or_else(|| io::Error::other("offline fixture state absent"))
                    .map(PathBuf::from)
            } else {
                Ok(PathBuf::from(STATE))
            };
            let socket = if private_fixture() {
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
                    .ok_or_else(|| io::Error::other("offline fixture socket absent"))
                    .map(PathBuf::from)
            } else {
                Ok(PathBuf::from(FRESH_SOCKET))
            };
            state.and_then(|state| {
                socket.and_then(|socket| {
                    if mode == "--offline-rebuild-fresh-index-v3" {
                        fresh_index::rebuild_keyed_offline(
                            &state.join("v30/fresh-provider"),
                            &socket,
                            Path::new(source),
                        )
                    } else {
                        fresh_index::Index::rebuild_offline(
                            &state.join("v30/fresh-provider"),
                            &socket,
                            Path::new(source),
                        )
                        .map(|_| ())
                    }
                    .map_err(io::Error::other)
                })
            })
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        [_, mode, source, account] if mode == "--offline-reconcile-fresh-index" => {
            let state = if private_fixture() {
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_STATE_V1")
                    .ok_or_else(|| io::Error::other("offline fixture state absent"))
                    .map(PathBuf::from)
            } else {
                Ok(PathBuf::from(STATE))
            };
            let socket = if private_fixture() {
                std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
                    .ok_or_else(|| io::Error::other("offline fixture socket absent"))
                    .map(PathBuf::from)
            } else {
                Ok(PathBuf::from(FRESH_SOCKET))
            };
            state.and_then(|state| {
                socket.and_then(|socket| {
                    let index = fresh_index::Index::open(&state.join("v30/fresh-provider"))
                        .map_err(io::Error::other)?;
                    index
                        .reconcile_offline_account_frozen(
                            &socket,
                            &account.to_string_lossy(),
                            Path::new(source),
                        )
                        .map(|_| ())
                        .map_err(io::Error::other)
                })
            })
        }
        [_, mode] if mode == "--serve-fresh-v30" => {
            #[cfg(feature = "age319-private-broker-fixture")]
            if private_fixture() {
                return if let Err(error) = serve_fresh_v30() {
                    eprintln!("kernel broker: {error}");
                    std::process::exit(1);
                };
            }
            Err(io::Error::other(
                "separate fresh broker authority retired; use the single broker service",
            ))
        }
        #[cfg(feature = "age319-private-broker-fixture")]
        [_, mode, path] if mode == "--manual-quota-worker" && private_fixture() => {
            manual_quota::worker(Path::new(path))
        }
        _ => {
            eprintln!("unknown broker mode");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("kernel broker: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oulipoly_kernel_broker::installed_launch::capture_from;
    use std::ffi::OsString;

    #[cfg(not(feature = "age319-private-broker-fixture"))]
    #[test]
    #[ignore = "requires a retained State receipt exported by the private Broker image"]
    fn imported_native_f_receipt_cannot_become_default_ack_after_restart() {
        let marker = std::env::var_os("AGE319_NATIVE_F_EXPORT_MARKER")
            .expect("private Broker receipt export marker required");
        let exported: serde_json::Value =
            serde_json::from_slice(&fs::read(marker).unwrap()).unwrap();
        let state_root = Path::new(exported["state_root"].as_str().unwrap());
        let request = exported["request"].as_str().unwrap();
        let token = exported["token"].as_str().unwrap();
        let delivery_request = exported["delivery_request"].as_str().unwrap();
        let recipient: FreshRecipientIdentity =
            serde_json::from_value(exported["recipient"].clone()).unwrap();
        let unavailable = "native F provider source verifier unavailable; pending";
        for _ in 0..2 {
            // The second open models a default Broker restart after an
            // uncertain read/ACK reply against the same retained State.
            let mut lane = FreshV30Lane::open_at(state_root).unwrap();
            let retained = lane
                .read_native_f_receipt(request, &recipient)
                .unwrap()
                .expect("private image exported a committed receipt");
            assert_eq!(
                certify_native_f_with_source(
                    &mut lane,
                    request,
                    &recipient,
                    &retained.observation,
                )
                .unwrap_err(),
                unavailable
            );
            assert_eq!(
                read_native_f_receipt_with_source(&lane, request, &recipient).unwrap_err(),
                unavailable
            );
            assert_eq!(
                acknowledge_native_f_receipt_with_source(&mut lane, request, token, &recipient,)
                    .unwrap_err(),
                unavailable
            );
            assert_eq!(
                read_native_f_auto_ack_with_source(&lane, request, &recipient).unwrap_err(),
                unavailable
            );
            assert!(
                lane.read_native_f_auto_ack(request, &recipient)
                    .unwrap()
                    .is_none()
            );
            assert!(
                lane.read_native_f_receipt(request, &recipient)
                    .unwrap()
                    .is_some()
            );
            let grant = lane
                .read_recipient_delivery_by_request(delivery_request, &recipient)
                .unwrap()
                .unwrap();
            assert_ne!(grant.phase, "acked");
        }
        let lane = FreshV30Lane::open_at(state_root).unwrap();
        assert!(read_native_f_receipt_with_source(
            &lane,
            &uuid::Uuid::new_v4().to_string(),
            &recipient,
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn v30_service_refuses_every_legacy_entry_and_work_operation() {
        for operation in [
            b'E', b'P', b'G', b'A', b'J', b'B', b'V', b'S', b's', b'T', b'H', b'k', b'K', b'Q',
            b'Z', b'C', b'L',
        ] {
            assert!(
                require_cutover_entry_route(operation, true, false).is_err(),
                "legacy opcode {} admitted",
                operation as char
            );
            assert!(require_cutover_entry_route(operation, false, false).is_ok());
        }
        for operation in [b'Y', b'R', b'W', b'I', b'N', b't'] {
            assert!(require_cutover_entry_route(operation, true, false).is_ok());
            assert!(require_cutover_entry_route(operation, false, true).is_err());
        }
        for operation in [b'e', b'p', b'g', b'a', b'j'] {
            assert!(require_cutover_entry_route(operation, true, false).is_ok());
            assert!(require_cutover_entry_route(operation, false, false).is_err());
            assert!(require_cutover_entry_route(operation, true, true).is_err());
        }
        assert!(require_cutover_entry_route(b':', true, false).is_ok());
        assert!(require_cutover_entry_route(b':', false, false).is_err());
        assert!(require_cutover_entry_route(b':', true, true).is_err());
        assert!(require_cutover_entry_route(b';', true, false).is_ok());
        assert!(require_cutover_entry_route(b';', false, false).is_err());
        assert!(require_cutover_entry_route(b';', true, true).is_err());
        for operation in [b'i', b'X', b'x'] {
            assert!(require_cutover_entry_route(operation, true, true).is_ok());
        }
        for operation in [
            b'E', b'P', b'G', b'A', b'J', b'B', b'V', b'S', b's', b'T', b'H', b'N', b'k', b't',
            b'K', b'Q', b'Z', b'C', b'L', b'Y', b'R', b'W', b'I',
        ] {
            assert!(
                require_cutover_entry_route(operation, false, true).is_err(),
                "closed gate admitted opcode {}",
                operation as char
            );
        }
    }
    use std::io::Read;
    use std::process::Command;
    use std::thread;

    #[test]
    fn private_installed_launch_frame_preserves_gui_cwd_and_challenged_peer() {
        let captured = capture_from(
            &uuid::Uuid::new_v4().to_string(),
            vec![OsString::from("oulipoly-plane")],
            vec![(OsString::from("DISPLAY"), OsString::from(":9"))],
            [-1; 3],
        )
        .unwrap();
        let (mut client, mut server) = UnixStream::pair().unwrap();
        let sender = thread::spawn(move || {
            let mut challenge = [0u8; 16];
            use std::io::Read;
            client.read_exact(&mut challenge).unwrap();
            let mut bytes = vec![b'L'];
            bytes.extend_from_slice(&challenge);
            bytes.extend_from_slice(&serde_json::to_vec(&captured.spec).unwrap());
            let mut iov = libc::iovec {
                iov_base: bytes.as_mut_ptr().cast(),
                iov_len: bytes.len(),
            };
            let descriptors: Vec<_> = captured
                .descriptors
                .iter()
                .map(AsRawFd::as_raw_fd)
                .collect();
            let mut control = [0u8; 64];
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(descriptors.as_slice()) as _) }
                    as usize;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len =
                    libc::CMSG_LEN(std::mem::size_of_val(descriptors.as_slice()) as _) as usize;
                std::ptr::copy_nonoverlapping(
                    descriptors.as_ptr(),
                    libc::CMSG_DATA(header).cast(),
                    descriptors.len(),
                );
                assert_eq!(
                    libc::sendmsg(client.as_raw_fd(), &message, 0),
                    bytes.len() as isize
                );
            }
        });
        let (operation, payload, credentials, process) = recv_request(&mut server).unwrap();
        sender.join().unwrap();
        assert_eq!(operation, b'L');
        assert_eq!(credentials.pid, std::process::id() as i32);
        process.verify().unwrap();
        let RequestPayload::InstalledLaunch { spec, descriptors } = payload else {
            panic!("expected installed launch");
        };
        assert_eq!(spec.kind, installed_launch::EntryKind::Gui);
        assert_eq!(spec.environment[0].1, b":9");
        installed_launch::validate(&spec, &installed_launch::files_as_raw(&descriptors)).unwrap();
        assert_eq!(descriptors.len(), 1);
    }

    #[test]
    fn state_read_requires_exact_live_guardian_not_same_uid_sibling_or_copied_owner() {
        let current = PinnedProcess::open(std::process::id() as i32).unwrap();
        let peer = PeerIdentity {
            uid: unsafe { libc::geteuid() },
            gid: unsafe { libc::getegid() },
            process: PinnedProcess::open(std::process::id() as i32).unwrap(),
        };
        let identity = oulipoly_state::completion_continuation::SourceProcessIdentity {
            pid: i64::from(current.host_pid),
            boot_id: current.boot_id.clone(),
            starttime_ticks: current.starttime_ticks as i64,
        };
        let mut owner = oulipoly_state::mailbox::CompletionDomainOwner {
            protocol: oulipoly_state::completion_continuation::PROTOCOL.into(),
            domain_id: uuid::Uuid::new_v4().to_string(),
            supervisor_authority_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            guardian_identity: identity.clone(),
            driver_identity: identity,
            endpoint: "/fixture".into(),
        };
        let image = File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(
            state_actor_matches(
                &peer,
                &current,
                &ProcessStamp::from(&current),
                &owner,
                &image
            )
            .unwrap()
        );
        owner.guardian_identity.pid += 1;
        assert!(
            !state_actor_matches(
                &peer,
                &current,
                &ProcessStamp::from(&current),
                &owner,
                &image
            )
            .unwrap()
        );
        owner.guardian_identity.pid -= 1;
        let mut sibling = Command::new("sleep").arg("5").spawn().unwrap();
        let sibling_process = PinnedProcess::open(sibling.id() as i32).unwrap();
        assert!(
            !state_actor_matches(
                &peer,
                &sibling_process,
                &ProcessStamp::from(&sibling_process),
                &owner,
                &image
            )
            .unwrap()
        );
        sibling.kill().unwrap();
        sibling.wait().unwrap();
    }

    #[test]
    fn state_read_replayed_challenge_is_rejected_before_dispatch() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let receiver = thread::spawn(move || recv_request(&mut server));
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let spec = StateReadSpec {
            protocol: "broker-state-read-v1".into(),
            source_generation: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            attempt_id: None,
        };
        let mut frame = vec![b'R'];
        frame.extend_from_slice(&[0u8; 16]);
        frame.extend_from_slice(&serde_json::to_vec(&spec).unwrap());
        client.write_all(&frame).unwrap();
        assert!(receiver.join().unwrap().is_err());
    }

    #[test]
    fn legacy_and_fresh_e_frames_keep_their_exact_lengths() {
        for (body_len, accepted) in [(0, true), (1, false), (15, false), (16, true), (17, false)] {
            let (mut server, mut client) = UnixStream::pair().unwrap();
            let receiver = thread::spawn(move || recv_request(&mut server));
            let mut challenge = [0u8; 16];
            client.read_exact(&mut challenge).unwrap();
            let mut frame = vec![b'E'];
            frame.extend_from_slice(&challenge);
            frame.extend_from_slice(&vec![0x5a; body_len]);
            client.write_all(&frame).unwrap();
            let result = receiver.join().unwrap();
            assert_eq!(result.is_ok(), accepted, "E frame length {}", frame.len());
            if let Ok((operation, payload, _, _)) = result {
                assert_eq!(operation, b'E');
                if body_len == 0 {
                    assert!(matches!(payload, RequestPayload::None));
                } else {
                    assert!(matches!(
                        payload,
                        RequestPayload::FreshBashChildRequest { .. }
                    ));
                }
            }
        }
    }

    #[test]
    fn state_write_frame_is_challenged_and_carries_no_path_or_row_authority() {
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let receiver = thread::spawn(move || recv_request(&mut server));
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let spec = StateWriteSpec {
            protocol: "broker-state-write-v1".into(),
            source_generation: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            action: StateWriteAction::Accept {
                attempt_id: uuid::Uuid::new_v4().to_string(),
            },
        };
        let mut frame = vec![b'W'];
        frame.extend_from_slice(&challenge);
        frame.extend_from_slice(&serde_json::to_vec(&spec).unwrap());
        client.write_all(&frame).unwrap();
        let (operation, payload, _, pinned) = receiver.join().unwrap().unwrap();
        assert_eq!(operation, b'W');
        assert_eq!(pinned.host_pid, std::process::id() as i32);
        assert!(matches!(
            payload,
            RequestPayload::StateWrite {
                spec: StateWriteSpec {
                    action: StateWriteAction::Accept { .. },
                    ..
                }
            }
        ));
    }

    #[test]
    fn production_dispatch_reserves_pre_fork_and_binds_only_prepared_guardian() {
        let uid = unsafe { libc::getuid() };
        if uid < 1000 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let works_path = temp.path().join("works");
        let entries_path = temp.path().join("entries");
        fs::create_dir(&works_path).unwrap();
        fs::create_dir(&entries_path).unwrap();
        let registry = RootRegistry::open(temp.path()).unwrap();
        let works = WorkRegistry::open(&works_path, &registry).unwrap();
        let mut entries = EntryRegistry::open(&entries_path).unwrap();
        let host = File::open("/proc/self/ns/pid").unwrap();
        let image = File::open(std::env::current_exe().unwrap()).unwrap();
        let entry = PeerIdentity {
            uid,
            gid: unsafe { libc::getgid() },
            process: PinnedProcess::open(std::process::id() as i32).unwrap(),
        };
        assert!(
            dispatch_authenticated(
                b'E',
                RequestPayload::FreshBashChildRequest {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    listener_policy: None,
                    #[cfg(feature = "age319-private-broker-fixture")]
                    ordinary_command: None,
                    #[cfg(feature = "age319-private-broker-fixture")]
                    ordinary_k_digest: None,
                },
                &entry,
                &host,
                &image,
                &registry,
                &works,
                &mut entries,
            )
            .is_err()
        );
        assert!(!entries.has_debt());
        let reserved = dispatch_authenticated(
            b'E',
            RequestPayload::None,
            &entry,
            &host,
            &image,
            &registry,
            &works,
            &mut entries,
        )
        .unwrap();
        let root = reserved
            .trim()
            .strip_prefix("reserved ")
            .unwrap()
            .to_owned();
        assert!(entries.record(&root).unwrap().prepared_guardian.is_none());
        let domain = uuid::Uuid::new_v4().to_string();
        let supervisor = uuid::Uuid::new_v4().to_string();
        assert!(
            dispatch_authenticated(
                b'A',
                RequestPayload::Read {
                    root_id: root.clone()
                },
                &entry,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        assert!(
            dispatch_authenticated(
                b'G',
                RequestPayload::Bind {
                    root_id: root.clone(),
                    domain_id: domain.clone(),
                    supervisor_id: supervisor.clone(),
                },
                &entry,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        let (mut parent_gate, mut child_gate) = UnixStream::pair().unwrap();
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            drop(parent_gate);
            let mut release = [0u8; 1];
            let ok = child_gate.read_exact(&mut release).is_ok() && release == [1];
            unsafe { libc::_exit(if ok { 0 } else { 1 }) }
        }
        drop(child_gate);
        let prepared = dispatch_authenticated(
            b'P',
            RequestPayload::Prepare {
                root_id: root.clone(),
                guardian_pid: child,
            },
            &entry,
            &host,
            &image,
            &registry,
            &works,
            &mut entries,
        )
        .unwrap();
        assert_eq!(prepared, format!("prepared {root}\n"));
        assert!(
            dispatch_authenticated(
                b'G',
                RequestPayload::Bind {
                    root_id: root.clone(),
                    domain_id: domain.clone(),
                    supervisor_id: supervisor.clone(),
                },
                &entry,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        let guardian = PeerIdentity {
            uid,
            gid: entry.gid,
            process: PinnedProcess::open(child).unwrap(),
        };
        let bound = dispatch_authenticated(
            b'G',
            RequestPayload::Bind {
                root_id: root.clone(),
                domain_id: domain.clone(),
                supervisor_id: supervisor.clone(),
            },
            &guardian,
            &host,
            &image,
            &registry,
            &works,
            &mut entries,
        )
        .unwrap();
        assert_eq!(bound, format!("bound {root} {domain} {supervisor}\n"));
        let readback = dispatch_authenticated(
            b'A',
            RequestPayload::Read {
                root_id: root.clone(),
            },
            &entry,
            &host,
            &image,
            &registry,
            &works,
            &mut entries,
        )
        .unwrap();
        assert_eq!(
            readback,
            format!("bound-entry {root} {domain} {supervisor} {child}\n")
        );
        assert!(
            dispatch_authenticated(
                b'A',
                RequestPayload::Read {
                    root_id: root.clone()
                },
                &guardian,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        assert!(
            dispatch_authenticated(
                b'G',
                RequestPayload::Bind {
                    root_id: root.clone(),
                    domain_id: domain.clone(),
                    supervisor_id: supervisor.clone(),
                },
                &guardian,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        assert!(
            dispatch_authenticated(
                b'L',
                RequestPayload::None,
                &entry,
                &host,
                &image,
                &registry,
                &works,
                &mut entries
            )
            .is_err()
        );
        parent_gate.write_all(&[1]).unwrap();
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    }

    #[test]
    fn root_launch_requires_host_namespace_outside_scope_and_user_uid() {
        let host_namespace = File::open("/proc/self/ns/pid").unwrap();
        let process = PinnedProcess::open(std::process::id() as i32).unwrap();
        let mut peer = PeerIdentity {
            uid: 1000,
            gid: 1000,
            process,
        };
        assert!(root_launch_admitted(
            &peer,
            &Scope::Outside,
            &host_namespace
        ));
        assert!(!root_launch_admitted(
            &peer,
            &Scope::Root(uuid::Uuid::new_v4().to_string()),
            &host_namespace
        ));
        assert!(!root_launch_admitted(
            &peer,
            &Scope::Uncertain,
            &host_namespace
        ));
        peer.uid = 0;
        assert!(!root_launch_admitted(
            &peer,
            &Scope::Outside,
            &host_namespace
        ));
    }

    #[test]
    fn challenged_per_request_credentials_accept_exact_sender() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (op, payload, peer) = peer_from_request(&mut stream).unwrap();
            assert_eq!(op, b'C');
            assert!(matches!(
                payload,
                RequestPayload::FreshBashChildRequest { .. }
            ));
            assert_eq!(peer.process.host_pid, std::process::id() as i32);
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut message = [0u8; 33];
        message[0] = b'C';
        message[1..17].copy_from_slice(&challenge);
        message[17..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 33, 0) },
            33
        );
        server.join().unwrap();
    }

    #[test]
    fn challenged_request_rejects_wrong_nonce() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(peer_from_request(&mut stream).is_err());
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut message = [0u8; 33];
        message[0] = b'C';
        message[1..17].copy_from_slice(&challenge);
        message[1] ^= 0xff;
        message[17..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 33, 0) },
            33
        );
        server.join().unwrap();
    }

    #[test]
    fn inherited_connected_fd_cannot_speak_for_connector() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let error = peer_from_request(&mut stream).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("transferred/inherited socket sender"),
                "unexpected denial: {error}"
            );
        });
        let script = "import os,socket,sys\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect(sys.argv[1]);c=s.recv(16);p=os.fork()\nif p==0:\n s.sendall(b'C'+c+bytes.fromhex('11111111111141118111111111111111'));os._exit(0)\nos.waitpid(p,0)";
        let output = Command::new("python3")
            .args(["-c", script, socket.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.join().unwrap();
    }

    #[test]
    fn privileged_child_pid_namespace_cannot_claim_outside_connector() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let outside = UnixStream::connect(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let error = peer_from_request(&mut stream).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("transferred/inherited socket sender"),
                "unexpected denial: {error}"
            );
        });
        let child_script = r#"
import errno, os, socket, struct, sys
assert os.getpid() == 1
caps = next(line.split()[1] for line in open('/proc/self/status') if line.startswith('CapEff:'))
assert int(caps, 16) & (1 << 21), caps  # CAP_SYS_ADMIN in the child user namespace
s = socket.socket(fileno=3)
challenge = s.recv(16)
assert len(challenge) == 16
message = b'C' + challenge + bytes.fromhex('11111111111141118111111111111111')
claimed = struct.pack('3i', int(sys.argv[1]), 0, 0)
try:
    s.sendmsg([message], [(socket.SOL_SOCKET, socket.SCM_CREDENTIALS, claimed)])
except OSError as error:
    assert error.errno == errno.ESRCH, error
else:
    sys.exit(42)
# A real send from this PID namespace is translated to its host PID and must
# still differ from the outside connector's pinned SO_PEERCRED identity.
assert s.send(message) == len(message)
"#;
        let source_fd = outside.as_raw_fd();
        let mut child = Command::new("unshare");
        child.args([
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount",
            "--mount-proc",
            "python3",
            "-c",
            child_script,
            &std::process::id().to_string(),
        ]);
        use std::os::unix::process::CommandExt;
        unsafe {
            child.pre_exec(move || {
                if libc::dup2(source_fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        server.join().unwrap();
    }

    #[test]
    fn challenged_request_rejects_passed_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            assert!(peer_from_request(&mut stream).is_err());
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut request = [0u8; 33];
        request[0] = b'C';
        request[1..17].copy_from_slice(&challenge);
        request[17..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let payload = File::open("/dev/null").unwrap();
        let mut iov = libc::iovec {
            iov_base: request.as_mut_ptr().cast(),
            iov_len: request.len(),
        };
        let mut control = [0u8; 64];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as _) } as _;
        let cmsg = unsafe { libc::CMSG_FIRSTHDR(&message) };
        assert!(!cmsg.is_null());
        unsafe {
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as _) as _;
            *(libc::CMSG_DATA(cmsg) as *mut i32) = payload.as_raw_fd();
            assert_eq!(libc::sendmsg(client.as_raw_fd(), &message, 0), 33);
        }
        server.join().unwrap();
    }

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn fresh_interactive_pty_handoff_frame_preserves_peer_and_exact_pair() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let mut master_fd = -1;
        let mut slave_fd = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master_fd,
                    &mut slave_fd,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master_fd) };
        let slave = unsafe { File::from_raw_fd(slave_fd) };
        let request = oulipoly_kernel_broker::protocol::PrivateFreshPtyHandoff {
            d_key: uuid::Uuid::new_v4().to_string(),
            session_id: format!("v30:{}:{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
            role: oulipoly_kernel_broker::protocol::FreshPlanRole::Interactive,
            account: "selected".into(),
            plan_sha256: "a".repeat(64),
            control_path: temp.path().join("control.sock"),
        };
        let expected_request = request.clone();
        let client_master = master.try_clone().unwrap();
        let client_slave = slave.try_clone().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (operation, payload, peer) = peer_from_request(&mut stream).unwrap();
            assert_eq!(operation, b'#');
            assert_eq!(peer.process.host_pid, std::process::id() as i32);
            let RequestPayload::FreshInteractivePtyHandoff {
                request,
                descriptors,
            } = payload
            else {
                panic!("PTY handoff decoded as another operation");
            };
            assert_eq!(request.d_key, expected_request.d_key);
            assert_eq!(request.session_id, expected_request.session_id);
            assert_eq!(request.account, expected_request.account);
            assert_eq!(request.plan_sha256, expected_request.plan_sha256);
            assert_eq!(request.control_path, expected_request.control_path);
            assert_eq!(descriptors.len(), 2);
            assert_eq!(
                descriptors[0].metadata().unwrap().ino(),
                master.metadata().unwrap().ino()
            );
            assert_eq!(
                descriptors[1].metadata().unwrap().ino(),
                slave.metadata().unwrap().ino()
            );
            stream.write_all(b"fresh-pty-handoff-pre-k\n").unwrap();
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut frame = vec![b'#'];
        frame.extend_from_slice(&challenge);
        frame.extend_from_slice(&serde_json::to_vec(&request).unwrap());
        let descriptors = [client_master.as_raw_fd(), client_slave.as_raw_fd()];
        let mut iov = libc::iovec {
            iov_base: frame.as_mut_ptr().cast(),
            iov_len: frame.len(),
        };
        let mut control = [0u8; 64];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as _;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&message);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as _;
            std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(cmsg).cast(), 2);
            assert_eq!(
                libc::sendmsg(client.as_raw_fd(), &message, 0),
                frame.len() as isize
            );
        }
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert_eq!(response, "fresh-pty-handoff-pre-k\n");
        server.join().unwrap();
    }

    #[cfg(feature = "age319-private-broker-fixture")]
    #[test]
    fn fresh_bash_policy_and_physical_frames_are_distinct() {
        for (operation, policy) in [
            (b'C', Some(1u8)),
            (b'c', None),
            (b'8', None),
            (b'9', None),
            (b'%', None),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let socket = temp.path().join("socket");
            let listener = UnixListener::bind(&socket).unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let (parsed_operation, payload, _) = peer_from_request(&mut stream).unwrap();
                assert_eq!(parsed_operation, operation);
                assert!(
                    matches!(payload, RequestPayload::FreshBashChildRequest { listener_policy, .. }
                    if if operation == b'C' {
                        matches!(listener_policy, Some(FreshBashListenerPolicy::Notify))
                    } else { listener_policy.is_none() })
                );
            });
            let mut client = UnixStream::connect(&socket).unwrap();
            let mut challenge = [0u8; 16];
            client.read_exact(&mut challenge).unwrap();
            let mut frame = vec![operation];
            frame.extend_from_slice(&challenge);
            frame.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
            if let Some(policy) = policy {
                frame.push(policy);
            }
            client.write_all(&frame).unwrap();
            server.join().unwrap();
        }
    }

    #[test]
    fn native_prepare_frame_is_challenged_and_has_exact_three_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (operation, payload, peer) = peer_from_request(&mut stream).unwrap();
            assert_eq!(operation, b'N');
            assert_eq!(peer.process.host_pid, std::process::id() as i32);
            let RequestPayload::PrepareNative { spec, descriptors } = payload else {
                panic!("native frame decoded as another operation");
            };
            assert_eq!(spec.receipt_sha256, "a".repeat(64));
            assert_eq!(descriptors.len(), 3);
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let spec = NativePrepareSpec {
            protocol: "native-continuation-v1".into(),
            root_id: uuid::Uuid::new_v4().to_string(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            receipt_sha256: "a".repeat(64),
        };
        let mut request = vec![b'N'];
        request.extend_from_slice(&challenge);
        request.extend_from_slice(&serde_json::to_vec(&spec).unwrap());
        let files = [
            File::open("/dev/null").unwrap(),
            File::open("/dev/null").unwrap(),
            File::open("/dev/null").unwrap(),
        ];
        let fds = files.each_ref().map(|file| file.as_raw_fd());
        let mut iov = libc::iovec {
            iov_base: request.as_mut_ptr().cast(),
            iov_len: request.len(),
        };
        let mut control = [0u8; 128];
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&fds) as _) } as _;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&message);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&fds) as _) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg).cast(), 3);
            assert_eq!(
                libc::sendmsg(client.as_raw_fd(), &message, 0),
                request.len() as isize
            );
        }
        server.join().unwrap();
    }

    #[test]
    fn native_k_frame_is_distinct_and_requires_exact_four_descriptors() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let spec = NativeKSpec {
            protocol: "native-continuation-v1".into(),
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            receipt_sha256: "a".repeat(64),
        };
        let expected_grant = spec.grant_id.clone();
        let server = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                match index {
                    0 => {
                        let (operation, payload, _) = peer_from_request(&mut stream).unwrap();
                        assert_eq!(operation, b'k');
                        let RequestPayload::NativeK { spec, descriptors } = payload else {
                            panic!("native K decoded as original-work K");
                        };
                        assert_eq!(spec.grant_id, expected_grant);
                        assert_eq!(descriptors.len(), 4);
                    }
                    _ => assert!(peer_from_request(&mut stream).is_err()),
                }
            }
        });
        for index in 0..3 {
            let mut client = UnixStream::connect(&socket).unwrap();
            let mut challenge = [0u8; 16];
            client.read_exact(&mut challenge).unwrap();
            let mut body = serde_json::to_value(&spec).unwrap();
            if index == 2 {
                body["argv"] = serde_json::json!(["/bin/sh"]);
            }
            let mut request = vec![b'k'];
            request.extend_from_slice(&challenge);
            request.extend_from_slice(&serde_json::to_vec(&body).unwrap());
            let files: Vec<_> = (0..if index == 1 { 3 } else { 4 })
                .map(|_| File::open("/dev/null").unwrap())
                .collect();
            let fds: Vec<_> = files.iter().map(AsRawFd::as_raw_fd).collect();
            let mut iov = libc::iovec {
                iov_base: request.as_mut_ptr().cast(),
                iov_len: request.len(),
            };
            let mut control = [0u8; 128];
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds.as_slice()) as _) } as _;
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&message);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds.as_slice()) as _) as _;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr(),
                    libc::CMSG_DATA(cmsg).cast(),
                    fds.len(),
                );
                assert_eq!(
                    libc::sendmsg(client.as_raw_fd(), &message, 0),
                    request.len() as isize
                );
            }
        }
        server.join().unwrap();
    }

    #[test]
    fn v30_native_k_frame_has_no_sidecar_descriptor_or_argv() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("socket");
        let listener = UnixListener::bind(&socket).unwrap();
        let spec = NativeKSpec {
            protocol: "native-continuation-v30".into(),
            grant_id: uuid::Uuid::new_v4().to_string(),
            root_id: uuid::Uuid::new_v4().to_string(),
            attempt_id: uuid::Uuid::new_v4().to_string(),
            owner_generation: uuid::Uuid::new_v4().to_string(),
            receipt_sha256: "a".repeat(64),
        };
        let server = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                if index == 0 {
                    let (operation, payload, _) = peer_from_request(&mut stream).unwrap();
                    assert_eq!(operation, b't');
                    let RequestPayload::NativeKV30 { spec, descriptors } = payload else {
                        panic!("v30 native K decoded as another operation");
                    };
                    assert_eq!(spec.protocol, "native-continuation-v30");
                    assert_eq!(descriptors.len(), 3);
                } else {
                    assert!(peer_from_request(&mut stream).is_err());
                }
            }
        });
        for index in 0..3 {
            let mut client = UnixStream::connect(&socket).unwrap();
            let mut challenge = [0u8; 16];
            client.read_exact(&mut challenge).unwrap();
            let mut body = serde_json::to_value(&spec).unwrap();
            if index == 2 {
                body["argv"] = serde_json::json!(["/bin/sh"]);
            }
            let mut frame = vec![b't'];
            frame.extend_from_slice(&challenge);
            frame.extend_from_slice(&serde_json::to_vec(&body).unwrap());
            let files: Vec<_> = (0..if index == 1 { 4 } else { 3 })
                .map(|_| File::open("/dev/null").unwrap())
                .collect();
            let fds: Vec<_> = files.iter().map(AsRawFd::as_raw_fd).collect();
            let mut iov = libc::iovec {
                iov_base: frame.as_mut_ptr().cast(),
                iov_len: frame.len(),
            };
            let mut control = [0u8; 128];
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(fds.as_slice()) as _) } as _;
            unsafe {
                let cmsg = libc::CMSG_FIRSTHDR(&message);
                (*cmsg).cmsg_level = libc::SOL_SOCKET;
                (*cmsg).cmsg_type = libc::SCM_RIGHTS;
                (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(fds.as_slice()) as _) as _;
                std::ptr::copy_nonoverlapping(
                    fds.as_ptr(),
                    libc::CMSG_DATA(cmsg).cast(),
                    fds.len(),
                );
                assert_eq!(
                    libc::sendmsg(client.as_raw_fd(), &message, 0),
                    frame.len() as isize
                );
            }
        }
        server.join().unwrap();
    }
}
