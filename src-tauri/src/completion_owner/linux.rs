use super::*;
use oulipoly_kernel_broker::protocol::StateRoute;
use oulipoly_kernel_broker::protocol::{self, JoinedChildWitness, OwnerWitness, ProcessWitness};
use oulipoly_state::completion_continuation::{PROTOCOL, SourceProcessIdentity};
use oulipoly_state::diagnostic_recorder::{
    DiagnosticPhase, PhaseObservation, SpanStart, process_recorder,
};
use oulipoly_state::mailbox::{CompletionDomainOwner, MailboxDb};
use oulipoly_state::pid_identity::{
    PidIdentityDb, ProcessIdentity, read_current_process_identity,
    read_direct_child_process_identity, read_live_process_identity,
    read_retained_direct_child_process_identity,
};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const GUARDIAN_POLL_INTERVAL: Duration = Duration::from_millis(50);
const OWNER_HELLO_MAX_BYTES: u64 = 8_192;
#[path = "control.rs"]
mod control;
use control::{ControlRequest, ControlService, JoinRefusal, JoinRequest, RefusalReason};
#[path = "context_leases.rs"]
mod context_leases;
use context_leases::ContextLeases;
#[cfg(feature = "age360-fault-fixtures")]
#[path = "j01_private_trace.rs"]
mod j01_private_trace;

pub(super) fn identity(pid: i64) -> Result<SourceProcessIdentity, String> {
    // `pid` is a key in this process's procfs observer, never getpid(),
    // Child::id(), or a PID received from a different namespace.
    let identity = read_live_process_identity(pid)?.ok_or("process identity disappeared")?;
    Ok(source_identity(identity))
}

fn source_identity(identity: ProcessIdentity) -> SourceProcessIdentity {
    SourceProcessIdentity {
        pid: identity.os_pid,
        boot_id: identity.os_boot_id,
        starttime_ticks: identity.os_pid_starttime_ticks,
    }
}

pub(super) fn current_identity() -> Result<SourceProcessIdentity, String> {
    read_current_process_identity().map(source_identity)
}

pub(super) fn direct_child_identity(local_pid: u32) -> Result<SourceProcessIdentity, String> {
    read_direct_child_process_identity(local_pid).map(source_identity)
}

pub(super) fn retained_direct_child_identity(
    local_pid: u32,
) -> Result<SourceProcessIdentity, String> {
    read_retained_direct_child_process_identity(local_pid).map(source_identity)
}

pub(super) fn require_owner(domain_id: &str) -> Result<CompletionDomainOwner, String> {
    if let Some(discovered) = read_v30_owner_if_present(None)? {
        if discovered.owner.domain_id != domain_id {
            return Err("completion owner domain/protocol conflict".into());
        }
        return Ok(discovered.owner);
    }
    let endpoint =
        std::env::var_os(ENDPOINT_ENV).ok_or("native continuation endpoint was not inherited")?;
    let owner = hello(Path::new(&endpoint))?;
    if owner.protocol != PROTOCOL || owner.domain_id != domain_id {
        return Err("completion owner domain/protocol conflict".into());
    }
    let mailbox = MailboxDb::open_existing_native_authority(&MailboxDb::default_path()?)?;
    let current = mailbox
        .completion_continuation_owner()?
        .ok_or("completion owner is no longer running")?;
    // Discovery/handshake only: source admission rechecks live ownership under
    // the State-then-sidecar fence; this result is not an admission capability.
    if current.domain_id != owner.domain_id
        || current.supervisor_authority_id != owner.supervisor_authority_id
        || current.owner_generation != owner.owner_generation
        || current.endpoint != owner.endpoint
        || current.guardian_identity != owner.guardian_identity
        || current.driver_identity != owner.driver_identity
    {
        return Err("completion owner handshake does not match current authority".into());
    }
    Ok(owner)
}

pub(super) fn read_v30_owner_if_present(
    query_pid: Option<i32>,
) -> Result<Option<protocol::OwnerDiscoveryReadback>, String> {
    if std::env::var_os(super::EXPECTED_KERNEL_ROOT_ENV).is_none() {
        return Ok(None);
    }
    let v30_endpoint = std::env::var_os(super::V30_OWNER_ENDPOINT_ENV).is_some();
    if !v30_endpoint && std::env::var_os(ENDPOINT_ENV).is_none() {
        return Err("pinned completion endpoint was not inherited".into());
    }
    match discover_v30_owner(query_pid) {
        Ok(readback) => Ok(Some(readback)),
        // A legacy broker has no retained sidecar. This exact challenged
        // reply preserves its historical owner path; any other refusal is
        // fail-closed and cannot turn v30 into a copied-sidecar fallback.
        Err(error)
            if !v30_endpoint
                && error == "owner discovery refused: error v30 owner sidecar absent" =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

pub(super) fn discover_v30_owner(
    query_pid: Option<i32>,
) -> Result<protocol::OwnerDiscoveryReadback, String> {
    let root = std::env::var(super::EXPECTED_KERNEL_ROOT_ENV)
        .map_err(|_| "v30 root was not inherited".to_string())?;
    let endpoint = std::env::var_os(super::V30_OWNER_ENDPOINT_ENV)
        .or_else(|| std::env::var_os(ENDPOINT_ENV))
        .ok_or("v30 completion endpoint was not inherited")?;
    let socket = UnixStream::connect(Path::new(&endpoint)).map_err(|e| e.to_string())?;
    // v30's guardian keeps the listener bound during held D but does not
    // serve the historical hello. Broker V checks this connected FD.
    let request = protocol::OwnerDiscoveryRequest {
        root_id: root.clone(),
        query_pid,
        expected_owner_generation: None,
    };
    let readback =
        protocol::discover_owner_at(&owner_broker_socket(), &request, socket.as_raw_fd())
            .map_err(|e| e.to_string())?;
    if readback.root_id != root
        || readback.owner.endpoint != endpoint.to_string_lossy()
        || readback.owner.protocol != PROTOCOL
    {
        return Err("v30 owner discovery readback conflict".into());
    }
    Ok(readback)
}

#[cfg(feature = "age319-private-broker-fixture")]
pub(super) fn private_discovery_probe(
    release: &oulipoly_state::mailbox::BrokerReleaseEvidence,
    gate: &Path,
) -> Result<(), String> {
    let exact = discover_v30_owner(Some(
        i32::try_from(std::process::id()).map_err(|e| e.to_string())?,
    ))?;
    if exact.root_id != release.prepared.root_id
        || exact.source_generation != release.prepared.source_generation
        || exact.release_id != release.release_id
        || exact.owner != release.owner
        || exact.pid_binding.is_some()
    {
        return Err("private owner discovery changed held D or invented PID session".into());
    }
    if require_owner(&release.owner.domain_id)? != release.owner {
        return Err("Runner owner lookup changed held D owner".into());
    }
    if !super::bootstrap_service()
        .unwrap_err()
        .contains("v30 completion service join is unavailable")
    {
        return Err("v30 service entry passed the closed source route".into());
    }
    let socket = UnixStream::connect(&release.owner.endpoint).map_err(|e| e.to_string())?;
    let mut request = protocol::OwnerDiscoveryRequest {
        root_id: release.prepared.root_id.clone(),
        query_pid: None,
        expected_owner_generation: Some(uuid::Uuid::new_v4().to_string()),
    };
    if protocol::discover_owner_at(&owner_broker_socket(), &request, socket.as_raw_fd()).is_ok() {
        return Err("stale owner generation returned discovery".into());
    }
    request.expected_owner_generation = Some(release.owner.owner_generation.clone());
    request.root_id = uuid::Uuid::new_v4().to_string();
    if protocol::discover_owner_at(&owner_broker_socket(), &request, socket.as_raw_fd()).is_ok() {
        return Err("other root returned discovery".into());
    }
    request.root_id = release.prepared.root_id.clone();
    let (wrong, _other) = UnixStream::pair().map_err(|e| e.to_string())?;
    if protocol::discover_owner_at(&owner_broker_socket(), &request, wrong.as_raw_fd()).is_ok() {
        return Err("unconnected guardian FD returned discovery".into());
    }
    let outsider = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["notify", "agent-bash-capability", "--json"])
        .env_remove("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1")
        .env_remove("OULIPOLY_KERNEL_CHILD_JOIN_FD_V1")
        .env_remove("OULIPOLY_KERNEL_V30_PRIVATE_CHILD_V1")
        .output()
        .map_err(|e| e.to_string())?;
    let outsider_error = String::from_utf8_lossy(&outsider.stderr);
    if outsider.status.success() || !outsider_error.contains("outside consumed work") {
        return Err(format!(
            "helper outside consumed H/K work did not receive broker V refusal: {outsider_error}"
        ));
    }
    let stripped = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["notify", "agent-bash-capability", "--json"])
        .env_remove("OULIPOLY_KERNEL_HOST_ENTRY_REQUIRED_V1")
        .env_remove("OULIPOLY_KERNEL_CHILD_JOIN_FD_V1")
        .env_remove(super::V30_OWNER_ENDPOINT_ENV)
        .env_remove(ENDPOINT_ENV)
        .output()
        .map_err(|e| e.to_string())?;
    if stripped.status.success()
        || !String::from_utf8_lossy(&stripped.stderr)
            .contains("pinned completion endpoint was not inherited")
    {
        return Err("stripped v30 endpoint fell back to retired owner copy".into());
    }
    std::fs::write(
        gate.join("owner-discovery"),
        serde_json::to_vec(&exact).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn hello(endpoint: &Path) -> Result<CompletionDomainOwner, String> {
    let mut socket = UnixStream::connect(endpoint).map_err(|e| e.to_string())?;
    socket.write_all(b"hello\n").map_err(|e| e.to_string())?;
    let mut response = Vec::new();
    (&mut socket)
        .take(OWNER_HELLO_MAX_BYTES + 1)
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    if response.len() as u64 > OWNER_HELLO_MAX_BYTES {
        return Err("oversized completion owner hello".into());
    }
    let owner: CompletionDomainOwner =
        serde_json::from_slice(&response).map_err(|e| e.to_string())?;
    verify_owner_peer(&socket, &owner)
        .map_err(|error| format!("completion hello process identity mismatch: {error}"))?;
    Ok(owner)
}

fn verify_owner_peer(socket: &UnixStream, owner: &CompletionDomainOwner) -> Result<(), String> {
    if let Some(root) = std::env::var_os(super::EXPECTED_KERNEL_ROOT_ENV) {
        let root = root.to_str().ok_or("invalid expected kernel root")?;
        return verify_kernel_owner_socket(root, owner, socket);
    }
    let peer = peer_pid(socket)?;
    if owner.guardian_identity != identity(peer)?
        || owner.driver_identity != identity(owner.driver_identity.pid)?
    {
        return Err("completion owner process identity mismatch".into());
    }
    Ok(())
}

pub(super) fn verify_kernel_owner_socket(
    root_id: &str,
    owner: &CompletionDomainOwner,
    socket: &UnixStream,
) -> Result<(), String> {
    let witness = kernel_owner_witness(root_id, owner)?;
    protocol::verify_owner_at(&owner_broker_socket(), &witness, socket.as_raw_fd())
        .map_err(|error| error.to_string())
}

fn kernel_owner_witness(
    root_id: &str,
    owner: &CompletionDomainOwner,
) -> Result<OwnerWitness, String> {
    let process = |identity: &SourceProcessIdentity| -> Result<ProcessWitness, String> {
        Ok(ProcessWitness {
            host_pid: i32::try_from(identity.pid).map_err(|_| "invalid host PID")?,
            boot_id: identity.boot_id.clone(),
            starttime_ticks: u64::try_from(identity.starttime_ticks)
                .map_err(|_| "invalid host starttime")?,
        })
    };
    let witness = OwnerWitness {
        root_id: root_id.to_owned(),
        domain_id: owner.domain_id.clone(),
        supervisor_id: owner.supervisor_authority_id.clone(),
        guardian: process(&owner.guardian_identity)?,
        driver: process(&owner.driver_identity)?,
        owner_generation: Some(owner.owner_generation.clone()),
        work_id: std::env::var("AGENT_BASH_OWNER_WORK_ID_V1").ok(),
        owner_session_id: std::env::var("AGENT_BASH_OWNER_SESSION_ID").ok(),
        owner_invocation_uuid: std::env::var("AGENT_BASH_OWNER_INVOCATION_UUID").ok(),
        registration_authority_sha256: std::env::var_os(
            oulipoly_state::COMPLETION_REGISTRATION_AUTHORITY_ENV,
        )
        .map(|value| format!("{:x}", Sha256::digest(value.as_bytes()))),
    };
    Ok(witness)
}

pub(super) fn owner_broker_socket() -> PathBuf {
    #[cfg(feature = "age319-private-broker-fixture")]
    if unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && let Some(path) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1")
    {
        return PathBuf::from(path);
    }
    PathBuf::from(protocol::INSTALLED_SOCKET)
}

// A live native context prevents idle retirement before its provider can admit
// a source. CLOEXEC keeps this lease out of provider/workload descendants.
static NATIVE_CONTEXT: std::sync::OnceLock<UnixStream> = std::sync::OnceLock::new();
fn retain_context(socket: UnixStream) -> Result<(), String> {
    NATIVE_CONTEXT
        .set(socket)
        .map_err(|_| "native completion context already joined".into())
}
fn join(endpoint: &Path, expected: &CompletionDomainOwner) -> Result<(), String> {
    let (socket, grant) = connect_context(endpoint, expected)?;
    unsafe {
        std::env::set_var(
            ROOT_AUTHORITY_ENV,
            serde_json::to_string(&grant).map_err(|error| error.to_string())?,
        );
        std::env::set_var(super::ORIGINAL_WORK_REQUIRED_ENV, "1");
    }
    retain_context(socket)
}

fn connect_context(
    endpoint: &Path,
    expected: &CompletionDomainOwner,
) -> Result<(UnixStream, super::original_work::RootAuthorityGrant), String> {
    let mut socket = UnixStream::connect(endpoint).map_err(|e| e.to_string())?;
    if std::env::var_os(super::EXPECTED_KERNEL_ROOT_ENV).is_some() {
        // In the namespace path, authenticate this newly connected socket
        // before sending the inherited root grant to it. The ordinary host
        // path retains its existing connect-time UID check and post-reply
        // incarnation comparison.
        verify_owner_peer(&socket, expected)
            .map_err(|_| "completion join peer conflict".to_owned())?;
    } else {
        peer_pid(&socket)?;
    }
    let mode = match std::env::var(ROOT_AUTHORITY_ENV) {
        Ok(value) => super::original_work::RootJoinMode::Inherit {
            grant: serde_json::from_str(&value)
                .map_err(|_| "invalid inherited root authority capability")?,
        },
        Err(std::env::VarError::NotPresent) => super::original_work::RootJoinMode::Fresh,
        Err(error) => return Err(error.to_string()),
    };
    let request = super::original_work::RootJoinRequest {
        protocol: super::original_work::ROOT_PROTOCOL.into(),
        mode,
    };
    socket.write_all(b"join!\n").map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut socket, &request).map_err(|error| error.to_string())?;
    socket.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    for _ in 0..8193 {
        let mut byte = [0];
        socket.read_exact(&mut byte).map_err(|e| e.to_string())?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    if bytes.len() > 8192 {
        return Err("oversized completion join".into());
    }
    // Authenticate the peer before interpreting a negative. A refusal is not
    // rollback/no-side-effect proof and never authorizes replay or fresh launch.
    verify_owner_peer(&socket, expected).map_err(|_| "completion join peer conflict".to_owned())?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if value.get("completion_join_refusal").is_some() {
        let refusal: JoinRefusal =
            serde_json::from_value(value).map_err(|_| "invalid completion join refusal")?;
        if refusal.guardian_identity != expected.guardian_identity
            || refusal.protocol != PROTOCOL
            || refusal.protocol != expected.protocol
            || refusal.domain_id != expected.domain_id
            || refusal.owner_generation != expected.owner_generation
            || expected.endpoint != endpoint.to_string_lossy()
        {
            return Err("completion join refusal identity conflict".into());
        }
        let category = match refusal.completion_join_refusal {
            RefusalReason::QueueFull => "queue full",
            RefusalReason::Retiring => "retirement pause",
            RefusalReason::Persistence => "persistence failure",
            RefusalReason::Identity => "process or protocol identity mismatch",
        };
        let diagnostic = refusal.diagnostic.as_deref().unwrap_or("unavailable");
        return Err(format!(
            "completion join refused: {category}; diagnostic={diagnostic}; admission outcome uncertain; no replay authorized"
        ));
    }
    let response: super::original_work::RootJoinResponse =
        serde_json::from_value(value).map_err(|e| e.to_string())?;
    let owner = &response.owner;
    if owner != expected || owner.endpoint != endpoint.to_string_lossy() {
        return Err("completion join peer conflict".into());
    }
    if response.root_authority.guardian_identity != owner.guardian_identity
        || response.root_authority.domain_id != owner.domain_id
        || response.root_authority.supervisor_authority_id != owner.supervisor_authority_id
        || response.root_authority.protocol != super::original_work::ROOT_PROTOCOL
    {
        return Err("root authority join response conflict".into());
    }
    if let Some(root) = std::env::var_os(super::EXPECTED_KERNEL_ROOT_ENV)
        && response.root_authority.root_id != root.to_string_lossy()
    {
        return Err("broker root and completion grant conflict".into());
    }
    Ok((socket, response.root_authority))
}

pub(super) fn bootstrap() -> Result<(), super::BootstrapError> {
    if std::env::var_os(ENDPOINT_ENV).is_some()
        || std::env::var_os(super::V30_OWNER_ENDPOINT_ENV).is_some()
    {
        // Wake-producing entry joins existing authority; read/ACK never bootstrap.
        if read_v30_owner_if_present(None)?.is_some() {
            return Err(
                "v30 completion service join is unavailable before source registration route"
                    .into(),
            );
        }
        let mailbox = MailboxDb::open_existing_native_authority(&MailboxDb::default_path()?)?;
        let domain = mailbox
            .completion_continuation_domain()?
            .ok_or("inherited endpoint has no native domain")?;
        let owner = require_owner(&domain)?;
        join(Path::new(&owner.endpoint), &owner)?;
        return Ok(());
    }
    validate_independent_entry()?;
    let path = MailboxDb::default_path()?;
    // State first: interruption before the additive sidecar upgrade leaves a
    // resumable pair, not a running recovery owner against a partial transition.
    // Both opens must succeed before election/join or any recovery fork.
    // The domain-keyed owner election cannot exist until the second database
    // has an identity. Serialize independent bootstrap at its stable path first;
    // this does not replace deployment's all-writers-stopped prerequisite.
    let directory = path.parent().ok_or("sidecar parent absent")?;
    std::fs::create_dir_all(directory).map_err(|error| {
        format!(
            "Failed to create state directory {}: {error}",
            directory.display()
        )
    })?;
    let transition = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.with_extension("completion-bootstrap.lock"))
        .map_err(|e| e.to_string())?;
    <std::fs::File as fs4::FileExt>::lock(&transition).map_err(|e| e.to_string())?;
    let state =
        oulipoly_state::StateDb::open_default_with_error().map_err(super::BootstrapError::State)?;
    let mailbox = MailboxDb::open_completion_continuation_domain(&path)?;
    let domain = mailbox
        .completion_continuation_domain()?
        .ok_or("missing native domain")?;
    if state.has_legacy_completion_admissions()? {
        eprintln!(
            "unsupported_legacy_source_recovery: legacy admissions have no v2 recovery binding; retained legacy notify/repair and mailbox delivery remain available. This is not a pending-work count or permission to clear debt."
        );
    }
    drop(state);
    drop(mailbox);
    drop(transition);
    let directory = PathBuf::from("/tmp")
        .join(format!("oulipoly-completion-{}-{domain}", unsafe {
            libc::geteuid()
        }));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string().into()),
    }
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("completion election directory is not private".into());
    }
    let endpoint = directory.join("owner.sock");
    // Admission and retirement share this gate. Hold it through startup or
    // hello + join + authority validation, not through provider execution.
    let admission = admission_gate(&endpoint)?;
    <std::fs::File as fs4::FileExt>::lock(&admission).map_err(|e| e.to_string())?;
    let election = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join("election.lock"))
        .map_err(|e| e.to_string())?;
    match <std::fs::File as fs4::FileExt>::try_lock(&election) {
        Ok(()) => {}
        Err(fs4::TryLockError::WouldBlock) => {
            let owner = hello(&endpoint)?;
            if owner.domain_id != domain {
                return Err("completion election domain conflict".into());
            }
            join(&endpoint, &owner)?;
            unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
            require_owner(&domain)?;
            return Ok(());
        }
        Err(e) => return Err(e.to_string().into()),
    }
    let (mut ready, announce) = UnixStream::pair().map_err(|e| e.to_string())?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string().into());
    }
    if pid == 0 {
        drop(ready);
        let code = guardian(&path, &endpoint, &domain, election, Some(announce), None)
            .map(|()| 0)
            .unwrap_or(70);
        unsafe { libc::_exit(code) }
    }
    drop(announce);
    drop(election); // close only; never LOCK_UN the child's open description.
    // Readiness is a correctness barrier, not a latency policy. Migration and
    // bounded recovery can legitimately exceed five seconds on a large retained
    // store. A hard socket timeout previously surfaced Linux EAGAIN as apparent
    // process exhaustion while the healthy guardian continued in the background.
    // Wait for exact readiness or channel closure; individual database and IPC
    // operations retain their own typed contention/failure bounds.
    let grant = await_guardian_ready(&mut ready, pid)?;
    unsafe { std::env::set_var(ENDPOINT_ENV, &endpoint) };
    unsafe {
        std::env::set_var(
            ROOT_AUTHORITY_ENV,
            serde_json::to_string(&grant).map_err(|error| error.to_string())?,
        );
        std::env::set_var(super::ORIGINAL_WORK_REQUIRED_ENV, "1");
    };
    require_owner(&domain)?;
    retain_context(ready)?;
    Ok(())
}

/// Continue in the broker-pinned host child. The entry has already verified G/A;
/// the driver remains behind its owner-frame gate until a second A and a
/// durable owner comparison after publication. Losing election is a refusal:
/// this child cannot join another guardian and still satisfy the broker pin.
pub(crate) fn run_pinned_guardian(
    pin: &super::PinnedGuardian,
    announce: UnixStream,
) -> Result<(), String> {
    if matches!(
        protocol::state_route_at(&owner_broker_socket()).map_err(|e| e.to_string())?,
        StateRoute::BrokerOwned { .. }
    ) {
        return Err("v30 guardian requires broker-held J before owner startup".into());
    }
    let path = MailboxDb::default_path()?;
    path.parent().ok_or("sidecar parent absent")?;
    let transition = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path.with_extension("completion-bootstrap.lock"))
        .map_err(|e| e.to_string())?;
    <std::fs::File as fs4::FileExt>::lock(&transition).map_err(|e| e.to_string())?;
    let state = oulipoly_state::schema_probe::run_schema_probe()
        .map_err(|e| format!("State changed after broker binding: {e:?}"))?
        .state_db;
    if !state.exists || state.user_version != state.current_schema_version || !state.compatible {
        return Err("State domain changed after broker binding".into());
    }
    let mailbox = MailboxDb::open_read_only(&path)?;
    let domain = mailbox
        .completion_continuation_domain()?
        .ok_or("missing native domain after broker grant")?;
    if domain != pin.domain_id {
        return Err("native domain changed after broker guardian binding".into());
    }
    drop(mailbox);
    drop(transition);

    let directory = PathBuf::from("/tmp")
        .join(format!("oulipoly-completion-{}-{domain}", unsafe {
            libc::geteuid()
        }));
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.to_string()),
    }
    let metadata = std::fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("completion election directory is not private".into());
    }
    let endpoint = directory.join("owner.sock");
    let admission = admission_gate(&endpoint)?;
    <std::fs::File as fs4::FileExt>::lock(&admission).map_err(|e| e.to_string())?;
    let election = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory.join("election.lock"))
        .map_err(|e| e.to_string())?;
    <std::fs::File as fs4::FileExt>::try_lock(&election)
        .map_err(|_| "pinned guardian did not win completion owner election")?;
    drop(admission);
    guardian(
        &path,
        &endpoint,
        &domain,
        election,
        Some(announce),
        Some(pin),
    )
}

/// The v30 bootstrap owns no retired sidecar handle.  The child and driver may
/// exist before publication, but both remain behind independent broker/owner
/// gates until the exact held J has been consumed.
pub(crate) fn run_pinned_guardian_v30(
    pin: &super::PinnedGuardian,
    mut entry: UnixStream,
) -> Result<(), String> {
    let broker = owner_broker_socket();
    let StateRoute::BrokerOwned { domain_id, .. } =
        protocol::state_route_at(&broker).map_err(|e| e.to_string())?
    else {
        return Err("v30 guardian lost broker-owned source".into());
    };
    if domain_id != pin.domain_id {
        return Err("v30 guardian domain changed".into());
    }
    let directory = PathBuf::from("/tmp").join(format!(
        "oulipoly-completion-v30-{}-{}",
        unsafe { libc::geteuid() },
        pin.root_id
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .map_err(|e| e.to_string())?;
    let endpoint = directory.join("owner.sock");
    let listener = UnixListener::bind(&endpoint).map_err(|e| e.to_string())?;
    let (owner, mut driver_gate) = start_v30_driver(&endpoint, pin, listener.as_raw_fd())?;
    let context = identity(peer_pid(&entry)?)?;
    let grant = super::original_work::RootAuthorities::default().fresh_with_root_id(
        &owner,
        context,
        pin.root_id.clone(),
    )?;
    let proposal = serde_json::json!({
        "driver_pid": owner.driver_identity.pid,
        "owner_generation": owner.owner_generation,
        "endpoint": owner.endpoint,
        "root_authority": serde_json::to_string(&grant).map_err(|e| e.to_string())?,
    });
    serde_json::to_writer(&mut entry, &proposal).map_err(|e| e.to_string())?;
    entry.write_all(b"\n").map_err(|e| e.to_string())?;
    let mut receipt = [0];
    entry.read_exact(&mut receipt).map_err(|e| e.to_string())?;
    if receipt != [b'J'] {
        return Err("v30 guardian did not receive exact held J receipt".into());
    }
    let route = super::broker_route::V30OwnerRoute::guardian(
        &broker,
        &pin.root_id,
        &pin.domain_id,
        &pin.supervisor_authority_id,
        &owner.owner_generation,
    )?;
    let prepared = route.prepare(owner.driver_identity.pid as i32, &endpoint)?;
    serde_json::to_writer(&mut entry, &prepared).map_err(|e| e.to_string())?;
    entry.write_all(b"\n").map_err(|e| e.to_string())?;
    entry.read_exact(&mut receipt).map_err(|e| e.to_string())?;
    if receipt != [b'A'] {
        return Err("v30 entry did not attest prepared owner".into());
    }
    let released = route.release(&prepared)?;
    route.read_running(&released.owner, None)?;
    serde_json::to_writer(&mut entry, &released).map_err(|e| e.to_string())?;
    entry.write_all(b"\n").map_err(|e| e.to_string())?;
    publish_driver_owner(&mut driver_gate, &released.owner)?;
    // Keep the original guardian alive while the released child performs its
    // post-gate broker attestation. EOF or death is refusal, never succession.
    entry.read_exact(&mut receipt).map_err(|e| e.to_string())?;
    if receipt != [b'D'] {
        return Err("v30 child completion was not reported".into());
    }
    driver_gate.write_all(b"D").map_err(|e| e.to_string())?;
    Ok(())
}

fn start_v30_driver(
    endpoint: &Path,
    pin: &super::PinnedGuardian,
    listener: RawFd,
) -> Result<(CompletionDomainOwner, UnixStream), String> {
    let (release, gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    let image = std::ffi::CString::new("/proc/self/exe").unwrap();
    let arg0 = std::ffi::CString::new("oulipoly-agent-runner").unwrap();
    let mode = std::ffi::CString::new(super::driver::DRIVER_ARG).unwrap();
    // The selected v30 branch never opens this legacy positional argument.
    // Its root selector and owner frame are checked against broker R.
    let retired = std::ffi::CString::new(MailboxDb::default_path()?.as_os_str().as_bytes())
        .map_err(|_| "legacy driver path contains NUL")?;
    let fd_arg = std::ffi::CString::new(gate.as_raw_fd().to_string()).unwrap();
    let root =
        std::ffi::CString::new(pin.root_id.as_str()).map_err(|_| "broker root contains NUL")?;
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(release);
        unsafe { libc::close(listener) };
        let fd = gate.as_raw_fd();
        close_except(&[fd]);
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
            unsafe { libc::_exit(70) }
        }
        let argv = [
            arg0.as_ptr(),
            mode.as_ptr(),
            retired.as_ptr(),
            fd_arg.as_ptr(),
            root.as_ptr(),
            std::ptr::null(),
        ];
        unsafe {
            libc::execv(image.as_ptr(), argv.as_ptr());
            libc::_exit(70)
        }
    }
    drop(gate);
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: pin.domain_id.clone(),
        supervisor_authority_id: pin.supervisor_authority_id.clone(),
        owner_generation: uuid::Uuid::new_v4().to_string(),
        guardian_identity: current_identity()?,
        driver_identity: direct_child_identity(pid as u32)?,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    Ok((owner, release))
}

pub(crate) fn verify_pinned_owner_ready(
    announce: &mut UnixStream,
    pin: &super::PinnedGuardian,
    guardian_pid: i32,
) -> Result<String, String> {
    if matches!(
        protocol::state_route_at(&owner_broker_socket()).map_err(|e| e.to_string())?,
        StateRoute::BrokerOwned { .. }
    ) {
        return Err("v30 guardian owner readback requires held-J broker route".into());
    }
    let grant = await_guardian_ready(announce, guardian_pid)?;
    let expected_guardian = identity(i64::from(guardian_pid))?;
    let expected_entry = current_identity()?;
    if grant.protocol != super::original_work::ROOT_PROTOCOL
        || grant.completion_protocol != PROTOCOL
        || grant.root_id != pin.root_id
        || grant.domain_id != pin.domain_id
        || grant.supervisor_authority_id != pin.supervisor_authority_id
        || grant.guardian_identity != expected_guardian
        || grant.root_identity != expected_entry
    {
        return Err("pinned root authority grant does not match broker identities".into());
    }
    let mailbox = MailboxDb::open_read_only(&MailboxDb::default_path()?)?;
    let owner = mailbox
        .completion_continuation_owner()?
        .ok_or("pinned completion owner was not published")?;
    if owner.protocol != PROTOCOL
        || owner.domain_id != pin.domain_id
        || owner.supervisor_authority_id != pin.supervisor_authority_id
        || owner.guardian_identity != expected_guardian
        || owner.driver_identity != identity(owner.driver_identity.pid)?
        || mailbox.completion_owner_kernel_root_id(&owner.owner_generation)?
            != Some(pin.root_id.clone())
    {
        return Err("durable completion owner does not match broker identities".into());
    }
    serde_json::to_string(&grant).map_err(|error| error.to_string())
}

fn await_guardian_ready(
    ready: &mut UnixStream,
    pid: i32,
) -> Result<super::original_work::RootAuthorityGrant, String> {
    let mut ready_byte = [0];
    ready.read_exact(&mut ready_byte).map_err(|error| {
        format!(
            "completion_guardian_startup_failed: guardian pid {pid} closed its ready channel before publishing owner/context readiness: {error}"
        )
    })?;
    if ready_byte != [1] {
        return Err(format!(
            "completion_guardian_startup_failed: guardian pid {pid} published an invalid readiness marker"
        ));
    }
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 16 * 1024 {
            return Err("completion guardian root authority frame is too large".into());
        }
        let mut byte = [0];
        ready.read_exact(&mut byte).map_err(|error| {
            format!("completion_guardian_startup_failed: root authority frame missing: {error}")
        })?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

/// Lock ordering: admission -> lifetime election -> State -> sidecar.
/// Retirement takes admission nonblocking while holding election, so an entry
/// joining the running owner cannot deadlock its guardian. Waiting entrants do
/// not hold State/sidecar locks. Kernel lock release, not a DB row or a delay,
/// signals that the retiring custodian has relinquished the election.
fn admission_gate(endpoint: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(endpoint.with_file_name("admission.lock"))
        .map_err(|e| e.to_string())
}

fn try_begin_retirement(
    admission: &std::fs::File,
    close_idle: impl FnOnce() -> bool,
) -> Result<bool, String> {
    match <std::fs::File as fs4::FileExt>::try_lock(admission) {
        Ok(()) => {
            let closing = close_idle();
            if !closing {
                <std::fs::File as fs4::FileExt>::unlock(admission).map_err(|e| e.to_string())?;
            }
            // A successful close retains the gate through physical drain.
            Ok(closing)
        }
        Err(fs4::TryLockError::WouldBlock) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

fn guardian(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    election: std::fs::File,
    mut announce: Option<UnixStream>,
    pinned: Option<&super::PinnedGuardian>,
) -> Result<(), String> {
    if unsafe { libc::setsid() } < 0
        || unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } < 0
    {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let _ = std::fs::remove_file(endpoint); // lifetime election is held; no live peer is replaced.
    let listener = UnixListener::bind(endpoint).map_err(|e| e.to_string())?;
    if pinned.is_some() {
        // Accepted sockets inherit PASSCRED. Enable it before the endpoint is
        // announced so even bytes queued before accept retain real senders.
        let enabled: libc::c_int = 1;
        if unsafe {
            libc::setsockopt(
                listener.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                (&enabled as *const libc::c_int).cast(),
                std::mem::size_of_val(&enabled) as _,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    redirect_stdio()?;
    let mut retained = vec![listener.as_raw_fd(), election.as_raw_fd()];
    if let Some(socket) = &announce {
        retained.push(socket.as_raw_fd());
    }
    retained.extend(super::custody::pending_birth_fds());
    close_except(&retained);
    let (mut owner, mut driver_channel) = start_driver(
        path,
        endpoint,
        domain,
        listener.as_raw_fd(),
        pinned,
        pinned.is_some(),
    )?;
    let mut contexts = ContextLeases::inherit(path)?;
    let mut root_authorities = super::original_work::RootAuthorities::default();
    if let Some(mut socket) = announce.take() {
        let context = identity(peer_pid(&socket)?)?;
        MailboxDb::open(path)?.retain_completion_context(&context)?;
        let grant = if let Some(pinned) = pinned {
            root_authorities.fresh_with_root_id(&owner, context.clone(), pinned.root_id.clone())?
        } else {
            root_authorities.fresh(&owner, context.clone())?
        };
        socket.write_all(&[1]).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut socket, &grant).map_err(|error| error.to_string())?;
        socket.write_all(b"\n").map_err(|error| error.to_string())?;
        if pinned.is_some() {
            let mut release = [0];
            socket
                .read_exact(&mut release)
                .map_err(|error| error.to_string())?;
            if release != [b'R'] {
                return Err("pinned guardian owner verification refused".into());
            }
        }
        socket.set_nonblocking(true).map_err(|e| e.to_string())?;
        contexts.retain_local(context, socket);
    }
    if pinned.is_some() {
        let mailbox = MailboxDb::open(path)?;
        if mailbox.completion_continuation_owner()?.as_ref() != Some(&owner)
            || mailbox.completion_owner_kernel_root_id(&owner.owner_generation)?
                != pinned.map(|pin| pin.root_id.clone())
        {
            return Err("pinned completion owner changed before driver release".into());
        }
        publish_driver_owner(&mut driver_channel, &owner)?;
    }
    let mut root_supervisor = super::root_supervisor::RootSupervisor::new(path, driver_channel)?;
    root_supervisor.set_kernel_pinned(pinned.map(|pin| pin.root_id.as_str()));
    // Open a distinct description after close_except: never reuse the
    // bootstrap parent's inherited flock description.
    let admission = admission_gate(endpoint)?;
    let mut closing = false;
    let mut control = if let Some(pin) = pinned {
        ControlService::start_pinned(&listener, &owner, &pin.root_id)?
    } else {
        ControlService::start(&listener, &owner)?
    };
    let mut pending = Vec::new();
    loop {
        // This guardian is the only process-tree authority.  It accepts launch
        // proposals, owns worker children, observes cancellation, reaps exact
        // terminal identities, and integrates retained results before generic
        // descendant cleanup can consume a wait status.
        if !closing {
            append_pending(
                &mut pending,
                control.recover_finished(&listener, &owner)?,
                &owner,
            );
        }
        loop {
            let mut status = 0;
            // This child has independent, exact custody from start_driver; an
            // unrelated unread birth must not hide its terminal wait.
            let driver_pid =
                if identity(owner.driver_identity.pid).as_ref() == Ok(&owner.driver_identity) {
                    unsafe {
                        libc::waitpid(owner.driver_identity.pid as i32, &mut status, libc::WNOHANG)
                    }
                } else {
                    0
                };
            let pid = if driver_pid > 0 {
                driver_pid
            } else {
                let protected = root_supervisor.original_worker_pids();
                super::custody::reap_unprotected(&mut status, &protected)
            };
            if pid <= 0 {
                if closing
                    && pid < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
                {
                    let _ = std::fs::remove_file(endpoint);
                    // Release the lifetime election only after actual ECHILD,
                    // and before waking entrants blocked on the admission gate.
                    control.stop()?;
                    drop(listener);
                    drop(election);
                    drop(admission);
                    return Ok(());
                }
                break;
            }
            // A waited descendant is not automatically attributable drain for
            // an activation whose custodian was lost. Its durable row remains.
            if !closing && i64::from(pid) == owner.driver_identity.pid {
                // No control thread (or copied thread locks) may cross fork.
                append_pending(&mut pending, control.stop()?, &owner);
                let (replacement, driver_channel) =
                    start_driver(path, endpoint, domain, listener.as_raw_fd(), pinned, false)?;
                owner = replacement;
                root_supervisor.replace_driver(driver_channel)?;
                control = if let Some(pin) = pinned {
                    ControlService::start_pinned(&listener, &owner, &pin.root_id)?
                } else {
                    ControlService::start(&listener, &owner)?
                };
            }
        }
        if pending.is_empty() {
            append_pending(&mut pending, control.pending(), &owner);
        }
        retain_pending_requests(
            path,
            &owner,
            &mut contexts,
            &mut root_authorities,
            &mut root_supervisor,
            &mut pending,
            pinned.is_some(),
        );
        // Requests, especially cancellation, are admitted before this pass may
        // issue an execution grant. A cancel already visible to the guardian
        // therefore cannot race behind a same-pass grant.
        // A setsid descendant can be adopted directly by this subreaper after
        // the original worker exits. The old session alone cannot certify its
        // drain. Unknown direct children conservatively hold original results.
        let adopted_child_live = guardian_unattributed_child_live(
            &owner.driver_identity,
            &root_supervisor.known_direct_child_pids(),
        );
        root_supervisor.tick(&owner, adopted_child_live)?;
        // Errors leave the group owned for the next pass; never a release ACK.
        let _ = contexts.release_disconnected(path);
        root_authorities.retain_live(
            &contexts.identities(),
            &root_supervisor.original_active_root_ids(),
        );
        #[cfg(feature = "age360-fault-fixtures")]
        j01_private_trace::after_context_release(path);
        if !closing
            && contexts.is_empty()
            && pending.is_empty()
            && root_supervisor.is_empty()
            && !retained_native_context(path).unwrap_or(true)
        {
            // Hold admission before pausing joins: an entrant already holding
            // this gate must finish hello/join, not be rejected by retirement.
            closing = try_begin_retirement(&admission, || {
                close_idle_owner(&owner, &control, &mut pending)
            })?;
            if closing {
                control.close()?;
            } else {
                control.resume()?;
            }
        }
        std::thread::sleep(GUARDIAN_POLL_INTERVAL);
    }
}

fn guardian_unattributed_child_live(driver: &SourceProcessIdentity, workers: &[i64]) -> bool {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else {
        return true;
    };
    for task in tasks {
        let Ok(task) = task else {
            return true;
        };
        let Ok(children) = std::fs::read_to_string(task.path().join("children")) else {
            return true;
        };
        for value in children.split_whitespace() {
            let Ok(pid) = value.parse::<i64>() else {
                return true;
            };
            if !(pid == driver.pid && identity(pid).as_ref() == Ok(driver))
                && !workers.contains(&pid)
            {
                return true;
            }
        }
    }
    false
}

fn close_idle_owner(
    owner: &CompletionDomainOwner,
    control: &ControlService,
    pending: &mut Vec<ControlRequest>,
) -> bool {
    // The barrier exposes every earlier join without obstructing hello during
    // the State close. A failed reader cannot supply permission to retire.
    if control.pause().is_err() {
        return false;
    }
    append_pending(pending, control.pending(), owner);
    if !pending.is_empty() {
        return false;
    }
    oulipoly_state::StateDb::open_default()
        .and_then(|state| state.close_idle_completion_continuation_owner(owner))
        .unwrap_or(false)
}

// Both the channel and the writer backlog are bounded. A refused request has
// not been persisted or acknowledged. Preserve earlier accepted ordering when
// a driver replacement transfers the old reader's queued sockets.
fn append_pending(
    pending: &mut Vec<ControlRequest>,
    requests: Vec<ControlRequest>,
    owner: &CompletionDomainOwner,
) {
    for request in requests {
        if pending.len() < control::PENDING_LIMIT {
            pending.push(request);
        } else {
            refuse_control_request(owner, request);
        }
    }
}

fn refuse_control_request(owner: &CompletionDomainOwner, request: ControlRequest) {
    match request {
        ControlRequest::Join(mut request) => {
            JoinRefusal::new(owner, RefusalReason::QueueFull).send(&mut request.socket)
        }
        ControlRequest::Work(request) => reject_work(
            owner,
            request,
            "root control queue is full before acceptance".into(),
        ),
        ControlRequest::Cancel(request) => reject_cancel(
            owner,
            request,
            "root control queue is full before cancellation".into(),
        ),
    }
}

fn retain_pending_requests(
    path: &Path,
    owner: &CompletionDomainOwner,
    contexts: &mut ContextLeases,
    root_authorities: &mut super::original_work::RootAuthorities,
    root_supervisor: &mut super::root_supervisor::RootSupervisor,
    pending: &mut Vec<ControlRequest>,
    kernel_pinned: bool,
) {
    for request in pending.drain(..pending.len().min(8)) {
        match request {
            ControlRequest::Join(request) => retain_pending_context(
                path,
                owner,
                contexts,
                root_authorities,
                root_supervisor,
                request,
            ),
            ControlRequest::Work(request) => {
                let authorized = match &request.submission.registration {
                    super::original_work::WorkRegistration::Root => {
                        if kernel_pinned {
                            // Pinned control consumed a one-use T ticket for
                            // exact Root scope before queueing this request.
                            root_authorities
                                .authorize_capability(owner, &request.submission.root_authority)
                        } else if let Some(parent_work_id) = root_supervisor
                            .original_parent_for_peer(
                                &request.submission.root_authority.root_id,
                                &request.peer,
                            )
                        {
                            Err(format!(
                                "paired peer is inside active parent {parent_work_id}; exact nested authority is required"
                            ))
                        } else {
                            root_authorities.authorize_root(
                                owner,
                                &request.peer,
                                &request.submission.root_authority,
                            )
                        }
                    }
                    super::original_work::WorkRegistration::Nested {
                        parent_work_id,
                        parent_capability,
                    } => root_authorities
                        .authorize_capability(owner, &request.submission.root_authority)
                        .and_then(|()| {
                            root_supervisor
                                .original_accepts_nested(
                                    &request.submission.root_authority.root_id,
                                    parent_work_id,
                                    parent_capability,
                                    &request.peer,
                                )
                                .then_some(())
                                .ok_or_else(|| {
                                    "nested original work is not in its exact active parent tree"
                                        .to_owned()
                                })
                        }),
                };
                if let Err(error) = authorized {
                    reject_work(owner, request, error);
                } else {
                    root_supervisor.submit_original(owner, request);
                }
            }
            ControlRequest::Cancel(request) => {
                // Public control eligibility is bound to a private per-handle
                // capability, not the caller's ambient root-wide grant. The
                // supervisor validates that exact capability before accepting
                // and persisting the actual peer principal.
                root_supervisor.cancel_original(owner, request);
            }
        }
    }
}

fn retain_pending_context(
    path: &Path,
    owner: &CompletionDomainOwner,
    contexts: &mut ContextLeases,
    root_authorities: &mut super::original_work::RootAuthorities,
    root_supervisor: &super::root_supervisor::RootSupervisor,
    mut request: JoinRequest,
) {
    let started = Instant::now();
    #[cfg(feature = "age360-fault-fixtures")]
    if path
        .parent()
        .and_then(Path::parent)
        .is_some_and(|root| root.join("force-join-identity-refusal").exists())
        && std::fs::read(format!("/proc/{}/cmdline", request.context.pid))
            .is_ok_and(|cmdline| cmdline.split(|byte| *byte == 0).any(|arg| arg == b"resume"))
    {
        refuse_join_identity(
            owner,
            &mut request,
            started,
            "J90_FIXTURE_REFUSAL",
            "forced_before_scope_admission",
        );
        return;
    }
    if matches!(
        &request.request.mode,
        super::original_work::RootJoinMode::Fresh
    ) {
        #[cfg(feature = "age360-fault-fixtures")]
        j01_private_trace::at_fresh_join(
            path,
            owner,
            &request.context,
            contexts,
            root_authorities,
            root_supervisor,
        );
        let existing_context_roots = root_authorities.roots_for_peer(&request.context);
        let active_root = root_supervisor.original_root_for_peer(&request.context);
        if active_root.is_some()
            || (!existing_context_roots.is_empty()
                && !root_supervisor.accepted_native_activation_for_peer(&request.context))
        {
            refuse_join_identity(
                owner,
                &mut request,
                started,
                "J01_FRESH_INSIDE_ROOT",
                &format!(
                    "scopes={} active={}",
                    existing_context_roots.len(),
                    active_root.is_some()
                ),
            );
            return;
        }
        let peer_image = process_image_identity(request.context.pid);
        let owner_image = process_image_identity(i64::from(std::process::id()));
        if peer_image.as_ref().ok() != owner_image.as_ref().ok()
            || peer_image.is_err()
            || owner_image.is_err()
        {
            refuse_join_identity(
                owner,
                &mut request,
                started,
                "J02_FRESH_IMAGE",
                &format!(
                    "peer_image={} owner_image={}",
                    image_observation(&peer_image),
                    image_observation(&owner_image)
                ),
            );
            return;
        }
    }
    if request.request.protocol != super::original_work::ROOT_PROTOCOL {
        refuse_join_identity(
            owner,
            &mut request,
            started,
            "J03_PROTOCOL",
            "root_protocol_mismatch",
        );
        return;
    }
    let inherit_from_active = match &request.request.mode {
        super::original_work::RootJoinMode::Fresh => false,
        super::original_work::RootJoinMode::Inherit { grant } => {
            let direct_started = Instant::now();
            let direct = root_authorities.validate_inherit(owner, &request.context, grant);
            let direct_us = direct_started.elapsed().as_micros();
            if direct.is_ok() {
                false
            } else {
                let capability_started = Instant::now();
                let capability = root_authorities.authorize_capability(owner, grant);
                let capability_us = capability_started.elapsed().as_micros();
                let active = capability.is_ok()
                    && root_supervisor.original_peer_in_root(&grant.root_id, &request.context);
                if active {
                    true
                } else {
                    let broker_started = Instant::now();
                    let broker = if capability.is_ok()
                        && i32::try_from(request.context.pid).is_ok_and(|pid| pid > 0)
                        && u64::try_from(request.context.starttime_ticks).is_ok()
                    {
                        protocol::verify_joined_child_at(
                            &owner_broker_socket(),
                            &JoinedChildWitness {
                                root_id: grant.root_id.clone(),
                                child: ProcessWitness {
                                    host_pid: request.context.pid as i32,
                                    boot_id: request.context.boot_id.clone(),
                                    starttime_ticks: request.context.starttime_ticks as u64,
                                },
                            },
                        )
                    } else {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "precondition",
                        ))
                    };
                    if broker.is_ok() {
                        true
                    } else {
                        let code = if capability.is_err() {
                            "J04_INHERIT_CAPABILITY"
                        } else {
                            "J05_INHERIT_LINEAGE"
                        };
                        let detail = format!(
                            "direct={} direct_us={} capability={} capability_us={} active={} broker_kind={:?} broker_errno={:?} broker_us={} root={}:{} root_live={} peer_live={} peer_parent={} peer_image={} root_image={}",
                            inherit_error_code(direct.as_ref().err()),
                            direct_us,
                            inherit_error_code(capability.as_ref().err()),
                            capability_us,
                            active,
                            broker.as_ref().err().map(std::io::Error::kind),
                            broker.as_ref().err().and_then(std::io::Error::raw_os_error),
                            broker_started.elapsed().as_micros(),
                            grant.root_identity.pid,
                            grant.root_identity.starttime_ticks,
                            live_identity_observation(&grant.root_identity),
                            live_identity_observation(&request.context),
                            parent_observation(request.context.pid),
                            image_observation(&process_image_identity(request.context.pid)),
                            image_observation(&process_image_identity(grant.root_identity.pid)),
                        );
                        refuse_join_identity(owner, &mut request, started, code, &detail);
                        return;
                    }
                }
            }
        }
    };
    if contexts.admit(path, &request.context).is_err() {
        JoinRefusal::new(owner, RefusalReason::Persistence).send(&mut request.socket);
        return;
    }
    let grant = match &request.request.mode {
        super::original_work::RootJoinMode::Fresh => {
            let fresh = if root_authorities.roots_for_peer(&request.context).is_empty() {
                root_authorities.fresh(owner, request.context.clone())
            } else {
                root_authorities.fresh_from_accepted_native(
                    owner,
                    request.context.clone(),
                    root_supervisor,
                )
            };
            match fresh {
                Ok(grant) => grant,
                Err(error) => {
                    refuse_join_identity(
                        owner,
                        &mut request,
                        started,
                        "J06_FRESH_SCOPE",
                        inherit_error_code(Some(&error)),
                    );
                    return;
                }
            }
        }
        super::original_work::RootJoinMode::Inherit { grant } if inherit_from_active => {
            match root_authorities.inherit_from_active_tree(owner, request.context.clone(), grant) {
                Ok(grant) => grant,
                Err(error) => {
                    refuse_join_identity(
                        owner,
                        &mut request,
                        started,
                        "J07_ACTIVE_SCOPE",
                        inherit_error_code(Some(&error)),
                    );
                    return;
                }
            }
        }
        super::original_work::RootJoinMode::Inherit { grant } => {
            match root_authorities.inherit(owner, request.context.clone(), grant) {
                Ok(grant) => grant,
                Err(error) => {
                    refuse_join_identity(
                        owner,
                        &mut request,
                        started,
                        "J08_INHERIT_SCOPE",
                        inherit_error_code(Some(&error)),
                    );
                    return;
                }
            }
        }
    };
    // Persistence preceded the reply. Even a lost reply is owned locally;
    // grouping sockets by incarnation prevents one release deleting another.
    let response = super::original_work::RootJoinResponse {
        owner: owner.clone(),
        root_authority: grant,
    };
    let _ = serde_json::to_writer(&mut request.socket, &response)
        .map_err(|e| e.to_string())
        .and_then(|()| request.socket.write_all(b"\n").map_err(|e| e.to_string()));
    let _ = request.socket.set_nonblocking(true);
    contexts.retain_local(request.context, request.socket);
}

fn process_image_identity(pid: i64) -> std::io::Result<(u64, u64)> {
    std::fs::metadata(format!("/proc/{pid}/exe")).map(|image| (image.dev(), image.ino()))
}

fn image_observation(image: &std::io::Result<(u64, u64)>) -> String {
    match image {
        Ok((dev, ino)) => format!("{dev}:{ino}"),
        Err(error) => format!("errno={:?} kind={:?}", error.raw_os_error(), error.kind()),
    }
}

fn live_identity_observation(expected: &SourceProcessIdentity) -> &'static str {
    match identity(expected.pid) {
        Ok(actual) if actual == *expected => "exact",
        Ok(_) => "different",
        Err(_) => "unreadable",
    }
}

fn parent_observation(pid: i64) -> String {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) => return format!("errno={:?}", error.raw_os_error()),
    };
    let parent = stat
        .rsplit_once(')')
        .and_then(|(_, tail)| tail.split_whitespace().nth(1))
        .and_then(|value| value.parse::<i64>().ok());
    match parent {
        Some(parent) => match identity(parent) {
            Ok(parent_identity) => format!(
                "{}:{}",
                parent_identity.pid, parent_identity.starttime_ticks
            ),
            Err(_) => format!("{parent}:unreadable"),
        },
        None => "unparseable".into(),
    }
}

fn inherit_error_code(error: Option<&String>) -> &'static str {
    match error.map(String::as_str) {
        None => "ok",
        Some("root authority inheritance is outside the exact process tree") => {
            "outside_exact_tree"
        }
        Some("root capability protocol or authority mismatch") => "protocol_or_authority",
        Some("unknown root capability") => "unknown_scope",
        Some("root capability mismatch") => "capability_mismatch",
        Some("live context is already inside a root authority") => "existing_scope",
        Some("duplicate root authority ID") => "duplicate_scope",
        Some("invalid broker root ID") => "invalid_root_id",
        Some(_) => "other",
    }
}

fn refuse_join_identity(
    owner: &CompletionDomainOwner,
    request: &mut JoinRequest,
    started: Instant,
    code: &'static str,
    detail: &str,
) {
    record_original_rejection(owner, &request.context, None, None, code);
    let (mode, root_identity) = match &request.request.mode {
        super::original_work::RootJoinMode::Fresh => ("fresh", "none".to_owned()),
        super::original_work::RootJoinMode::Inherit { grant } => (
            "inherit",
            format!(
                "{}:{}",
                grant.root_identity.pid, grant.root_identity.starttime_ticks
            ),
        ),
    };
    let diagnostic = format!(
        "{code} mode={mode} peer={}:{}:{} guardian={}:{}:{} root={} elapsed_us={} {detail}",
        request.context.pid,
        request.context.boot_id,
        request.context.starttime_ticks,
        owner.guardian_identity.pid,
        owner.guardian_identity.boot_id,
        owner.guardian_identity.starttime_ticks,
        root_identity,
        started.elapsed().as_micros(),
    );
    JoinRefusal::new(owner, RefusalReason::Identity)
        .with_diagnostic(diagnostic)
        .send(&mut request.socket);
}

fn reject_work(
    owner: &CompletionDomainOwner,
    mut request: super::original_work::InboundWork,
    error: String,
) {
    record_original_rejection(
        owner,
        &request.peer,
        Some(&request.submission.root_authority.root_id),
        Some(&request.submission.work_id),
        "original_work_rejected_preaccept",
    );
    let response = super::original_work::WorkResponse {
        protocol: super::original_work::PROTOCOL.into(),
        work_id: request.submission.work_id.clone(),
        status: "rejected_preaccept".into(),
        root_id: request.submission.root_authority.root_id.clone(),
        supervisor_authority_id: owner.supervisor_authority_id.clone(),
        worker_identity: None,
        detail: Some(error),
    };
    let _ = serde_json::to_writer(&mut request.socket, &response);
    let _ = request.socket.write_all(b"\n");
}

fn reject_cancel(
    owner: &CompletionDomainOwner,
    mut request: super::original_work::InboundCancel,
    error: String,
) {
    record_original_rejection(
        owner,
        &request.peer,
        Some(&request.submission.root_id),
        Some(&request.submission.work_id),
        "original_work_cancellation_rejected",
    );
    let response = super::original_work::WorkResponse {
        protocol: super::original_work::PROTOCOL.into(),
        work_id: request.submission.work_id.clone(),
        status: "rejected".into(),
        root_id: request.submission.root_id.clone(),
        supervisor_authority_id: owner.supervisor_authority_id.clone(),
        worker_identity: None,
        detail: Some(error),
    };
    let _ = serde_json::to_writer(&mut request.socket, &response);
    let _ = request.socket.write_all(b"\n");
}

pub(super) fn record_control_gap(
    owner: &CompletionDomainOwner,
    peer_pid: i64,
    cause: &'static str,
) {
    let peer = SourceProcessIdentity {
        pid: peer_pid,
        boot_id: String::new(),
        starttime_ticks: 0,
    };
    record_original_rejection(owner, &peer, None, None, cause);
}

fn record_original_rejection(
    owner: &CompletionDomainOwner,
    peer: &SourceProcessIdentity,
    root_id: Option<&str>,
    work_id: Option<&str>,
    cause: &'static str,
) {
    let mut start = SpanStart::new("root_original_work_rejection", "process_tree")
        .with_lifecycle_phase("original_work_preaccept")
        .with_identifier("supervisor_authority_id", &owner.supervisor_authority_id)
        .with_identifier("authority_pid", &owner.guardian_identity.pid.to_string())
        .with_identifier("authority_boot_id", &owner.guardian_identity.boot_id)
        .with_identifier(
            "authority_starttime_ticks",
            &owner.guardian_identity.starttime_ticks.to_string(),
        )
        .with_identifier("initiator_pid", &peer.pid.to_string())
        .with_identifier(
            "initiator_boot_id",
            if peer.boot_id.is_empty() {
                "unavailable"
            } else {
                &peer.boot_id
            },
        )
        .with_identifier(
            "initiator_starttime_ticks",
            &peer.starttime_ticks.to_string(),
        );
    if let Some(root_id) = root_id {
        start = start.with_identifier("root_id", root_id);
    }
    if let Some(work_id) = work_id {
        start = start.with_hashed_correlation("work_id", work_id);
    }
    process_recorder().with_requested_span(start, |span| {
        let _ = span.record(
            DiagnosticPhase::Failed,
            PhaseObservation::not_started().with_cause(cause),
        );
    });
}

/// A lost socket owner does not erase a live native entry's ability to admit
/// later work. Process identity only settles this *admission lease*, never an
/// activation or original-workload tree obligation.
fn retained_native_context(path: &Path) -> Result<bool, String> {
    let db = MailboxDb::open(path)?;
    let mut live = false;
    for context in db.completion_contexts()? {
        // The residual sweep must use the same positive expiry evidence as
        // inherited groups; optional absence is not permission to retire.
        if !context_leases::incarnation_expired(&context) {
            live = true;
        } else {
            db.release_completion_context(&context)?;
        }
    }
    Ok(live)
}

fn start_driver(
    path: &Path,
    endpoint: &Path,
    domain: &str,
    listener: RawFd,
    pinned: Option<&super::PinnedGuardian>,
    hold_owner_frame: bool,
) -> Result<(CompletionDomainOwner, UnixStream), String> {
    let (mut release, gate) = UnixStream::pair().map_err(|e| e.to_string())?;
    let executable = std::ffi::CString::new("/proc/self/exe").expect("static path has no NUL");
    let arg0 = std::ffi::CString::new("oulipoly-agent-runner").expect("static argv has no NUL");
    let entry = std::ffi::CString::new(super::driver::DRIVER_ARG).expect("static argv has no NUL");
    let path_arg = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| "completion driver path contains NUL")?;
    let fd_arg = std::ffi::CString::new(gate.as_raw_fd().to_string())
        .expect("numeric descriptor has no NUL");
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if pid == 0 {
        drop(release);
        unsafe { libc::close(listener) };
        let _ = super::custody::pending_birth_fds();
        close_except(&[gate.as_raw_fd()]);
        let flags = unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_GETFD) };
        if flags < 0
            || unsafe { libc::fcntl(gate.as_raw_fd(), libc::F_SETFD, flags & !libc::FD_CLOEXEC) }
                < 0
        {
            unsafe { libc::_exit(70) }
        }
        let argv = [
            arg0.as_ptr(),
            entry.as_ptr(),
            path_arg.as_ptr(),
            fd_arg.as_ptr(),
            std::ptr::null(),
        ];
        unsafe {
            libc::execv(executable.as_ptr(), argv.as_ptr());
            libc::_exit(70)
        }
    }
    drop(gate);
    let guardian_identity = current_identity()?;
    // Driver replacement beneath the same living guardian stays in the same
    // process-tree authority. A later independent guardian always mints a new
    // root and publication explicitly adopts only unresolved predecessor debt.
    let supervisor_authority_id = if let Some(pin) = pinned {
        pin.supervisor_authority_id.clone()
    } else {
        MailboxDb::open(path)?
            .completion_continuation_owner()?
            .filter(|current| current.guardian_identity == guardian_identity)
            .map(|current| current.supervisor_authority_id)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    };
    let owner = CompletionDomainOwner {
        protocol: PROTOCOL.into(),
        domain_id: domain.into(),
        supervisor_authority_id,
        owner_generation: uuid::Uuid::new_v4().to_string(),
        guardian_identity,
        driver_identity: direct_child_identity(pid as u32)?,
        endpoint: endpoint.to_string_lossy().into_owned(),
    };
    MailboxDb::open(path)?.publish_completion_owner_with_kernel_root(
        &owner,
        pinned.map(|pin| pin.root_id.as_str()),
    )?;
    // On the first pinned publication the driver cannot read its owner frame
    // until the entry has compared the durable row with broker A. Replacement
    // drivers remain beneath the already verified guardian and authority.
    if !hold_owner_frame {
        publish_driver_owner(&mut release, &owner)?;
    }
    Ok((owner, release))
}

fn publish_driver_owner(
    release: &mut UnixStream,
    owner: &CompletionDomainOwner,
) -> Result<(), String> {
    serde_json::to_writer(&mut *release, owner).map_err(|e| e.to_string())?;
    release.write_all(b"\n").map_err(|e| e.to_string())
}

pub(super) fn read_driver_owner(socket: &mut UnixStream) -> Result<CompletionDomainOwner, String> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= 8192 {
            return Err("oversized root supervisor driver owner frame".into());
        }
        let mut byte = [0];
        socket
            .read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        if byte == [b'\n'] {
            break;
        }
        bytes.push(byte[0]);
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn validate_independent_entry() -> Result<(), String> {
    let current_ns = std::fs::read_link("/proc/self/ns/net").map_err(|e| e.to_string())?;
    let sidecar = PidIdentityDb::default_path()?;
    let identities = if sidecar.exists() {
        Some(PidIdentityDb::open_read_only(&sidecar)?)
    } else {
        None
    };
    let mut pid = unsafe { libc::getppid() };
    while pid > 1 {
        let stat =
            std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(|e| e.to_string())?;
        let tail = stat.rsplit_once(')').ok_or("invalid ancestor stat")?.1;
        let parent: i32 = tail
            .split_whitespace()
            .nth(1)
            .ok_or("missing ancestor ppid")?
            .parse()
            .map_err(|_| "invalid ancestor ppid")?;
        use std::os::unix::fs::MetadataExt;
        let same_uid = std::fs::metadata(format!("/proc/{pid}"))
            .map(|m| m.uid() == unsafe { libc::geteuid() })
            .unwrap_or(false);
        if same_uid
            && std::fs::read_link(format!("/proc/{pid}/ns/net"))
                .ok()
                .as_ref()
                == Some(&current_ns)
        {
            let actual = read_live_process_identity(i64::from(pid))?
                .ok_or("ancestor changed during admission")?;
            if let Some(db) = identities.as_ref()
                && db.lookup_by_identity(&actual)?.is_some()
            {
                return Err("managed native ancestor has no inherited completion owner".into());
            }
            let image =
                std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|e| e.to_string())?;
            let image = image.to_string_lossy();
            let environment =
                std::fs::read(format!("/proc/{pid}/environ")).map_err(|e| e.to_string())?;
            let marked = environment.split(|b| *b == 0).any(|entry| {
                entry.starts_with(b"OULIPOLY_COMPLETION_REGISTRATION_AUTHORITY=")
                    || entry.starts_with(b"AGENT_BASH_OWNER_INVOCATION_UUID=")
            });
            // This broad veto also catches a markerless paired Bash descendant
            // that invokes Runner directly. Independent Bash registration needs
            // a narrower lineage witness before this can be relaxed.
            if image.contains("agent-bash") || marked {
                return Err(
                    "managed Bash/provider ancestor has no inherited completion owner".into(),
                );
            }
            if read_live_process_identity(i64::from(pid))?.as_ref() != Some(&actual) {
                return Err("ancestor changed during admission".into());
            }
        }
        if parent == pid {
            return Err("cyclic ancestor identity".into());
        }
        pid = parent;
    }
    Ok(())
}

fn peer_pid(socket: &UnixStream) -> Result<i64, String> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut credentials as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if result != 0 || credentials.uid != unsafe { libc::geteuid() } {
        return Err("completion peer is not same UID".into());
    }
    Ok(i64::from(credentials.pid))
}

pub(super) fn close_except(keep: &[RawFd]) {
    if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
        let descriptors: Vec<i32> = entries
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        for fd in descriptors {
            if fd > 2 && !keep.contains(&fd) {
                unsafe { libc::close(fd) };
            }
        }
    }
}
fn redirect_stdio() -> Result<(), String> {
    let null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .map_err(|e| e.to_string())?;
    for fd in 0..3 {
        if unsafe { libc::dup2(null.as_raw_fd(), fd) } < 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "linux_admission_tests.rs"]
mod admission_tests;

#[cfg(test)]
#[path = "linux_identity_tests.rs"]
mod identity_tests;
