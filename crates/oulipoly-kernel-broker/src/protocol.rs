//! One connection, one challenged request. Entry operations cannot select an
//! executable, UID, namespace, or mount. The accepted-work guardian operation
//! carries the initiator's already pinned executable and accepted descriptors.
use crate::installed_launch::InstalledLaunchSpec;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const INSTALLED_SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";

/// Entry routing is observed from the live broker before any user-side
/// sidecar read. This is a storage version observation, not a grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateRoute {
    Legacy,
    BrokerOwned {
        source_generation: String,
        domain_id: String,
    },
}

pub fn state_route_at(path: &Path) -> io::Result<StateRoute> {
    let response = request_frame_at(path, Operation::ReadStateRoute, Payload::None)?;
    if response == "state-route legacy\n" {
        return Ok(StateRoute::Legacy);
    }
    let fields = response
        .strip_prefix("state-route broker-owned ")
        .and_then(|value| value.strip_suffix('\n'))
        .ok_or_else(|| io::Error::other("invalid broker State route"))?;
    let (generation, domain) = fields
        .split_once(' ')
        .ok_or_else(|| io::Error::other("missing broker domain"))?;
    let parsed = uuid::Uuid::parse_str(generation)
        .map_err(|_| io::Error::other("invalid broker State generation"))?;
    let parsed_domain =
        uuid::Uuid::parse_str(domain).map_err(|_| io::Error::other("invalid broker domain"))?;
    if parsed.to_string() != generation || parsed_domain.to_string() != domain {
        return Err(io::Error::other("noncanonical broker State generation"));
    }
    Ok(StateRoute::BrokerOwned {
        source_generation: generation.into(),
        domain_id: domain.into(),
    })
}

/// Opt-in v1 readback. The broker derives the domain and supervisor from its
/// durable entry registry; these IDs never authorize a request by themselves.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateReadSpec {
    pub protocol: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub attempt_id: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateGenerationSpec {
    pub protocol: String,
    pub root_id: String,
}

/// Discover the active source generation only for an already bound guardian.
/// The returned value must accompany every later State read or write.
pub fn state_generation_at(path: &Path, root_id: &str) -> io::Result<String> {
    state_generation_versioned_at(path, root_id, "broker-state-generation-v1")
}

pub fn prepared_generation_at(path: &Path, root_id: &str) -> io::Result<String> {
    state_generation_versioned_at(path, root_id, "broker-prepared-generation-v30")
}

fn state_generation_versioned_at(path: &Path, root_id: &str, protocol: &str) -> io::Result<String> {
    let spec = StateGenerationSpec {
        protocol: protocol.into(),
        root_id: root_id.into(),
    };
    let body = serde_json::to_vec(&spec)?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut frame = Vec::with_capacity(17 + body.len());
    frame.push(b'Y');
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(&body);
    if unsafe {
        libc::send(
            stream.as_raw_fd(),
            frame.as_ptr().cast(),
            frame.len(),
            libc::MSG_NOSIGNAL,
        )
    } != frame.len() as isize
    {
        return Err(io::Error::other("short State generation request"));
    }
    let response = read_response(stream)?;
    let generation = response
        .trim_end()
        .strip_prefix("state-generation ")
        .ok_or_else(|| io::Error::other("State generation refused"))?;
    let parsed =
        uuid::Uuid::parse_str(generation).map_err(|_| io::Error::other("bad State generation"))?;
    if parsed.to_string() != generation {
        return Err(io::Error::other("noncanonical State generation"));
    }
    Ok(generation.into())
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateWriteSpec {
    pub protocol: String,
    pub source_generation: String,
    pub root_id: String,
    pub owner_generation: String,
    pub action: StateWriteAction,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateWriteAction {
    Prepare {
        driver_pid: i32,
        endpoint: String,
    },
    Publish {
        driver_pid: i32,
        endpoint: String,
    },
    Reserve {
        attempt: oulipoly_state::mailbox::ContinuationAttempt,
    },
    Revoke {
        attempt: oulipoly_state::mailbox::ContinuationAttempt,
    },
    Accept {
        attempt_id: String,
    },
    Repair {
        expected_ordinal: i64,
    },
    ReserveSourceGrant,
    Release,
}

/// Prepared evidence is inert: it does not elect an owner or open the child gate.
pub fn prepare_owner_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<oulipoly_state::mailbox::PreparedBrokerOwner> {
    if spec.protocol != "broker-prepared-write-v30"
        || !matches!(spec.action, StateWriteAction::Prepare { .. })
    {
        return Err(io::Error::other("invalid prepared owner request"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'W', spec)?).map_err(io::Error::other)
}

pub fn read_prepared_owner_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::PreparedBrokerOwner> {
    if spec.protocol != "broker-prepared-read-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid prepared owner read"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

/// Guardian-only exact readback after a release reply was lost. The broker
/// still requires its original retained gate and current actor incarnations.
pub fn read_released_owner_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerReleaseEvidence> {
    if spec.protocol != "broker-release-readback-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid release readback request"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

pub fn release_prepared_owner_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerReleaseEvidence> {
    if spec.protocol != "broker-held-release-v30"
        || !matches!(spec.action, StateWriteAction::Release)
    {
        return Err(io::Error::other("invalid held release request"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'W', spec)?).map_err(io::Error::other)
}

/// Private fault fixture: send the real challenged release frame, then lose
/// only its reply. The caller must reconcile using exact live readback.
#[cfg(feature = "age319-private-broker-fixture")]
pub fn release_prepared_owner_drop_reply_at(path: &Path, spec: &StateWriteSpec) -> io::Result<()> {
    if spec.protocol != "broker-held-release-v30"
        || !matches!(spec.action, StateWriteAction::Release)
    {
        return Err(io::Error::other("invalid held release request"));
    }
    state_write_drop_reply_at(path, spec)
}

#[cfg(feature = "age319-private-broker-fixture")]
pub fn prepare_owner_drop_reply_at(path: &Path, spec: &StateWriteSpec) -> io::Result<()> {
    if spec.protocol != "broker-prepared-write-v30"
        || !matches!(spec.action, StateWriteAction::Prepare { .. })
    {
        return Err(io::Error::other("invalid lost-reply preparation"));
    }
    state_write_drop_reply_at(path, spec)
}

#[cfg(feature = "age319-private-broker-fixture")]
pub fn repair_bounded_drop_reply_at(path: &Path, spec: &StateWriteSpec) -> io::Result<()> {
    if spec.protocol != "broker-repair-write-v30"
        || !matches!(spec.action, StateWriteAction::Repair { .. })
    {
        return Err(io::Error::other("invalid lost-reply bounded repair"));
    }
    state_write_drop_reply_at(path, spec)
}

#[cfg(feature = "age319-private-broker-fixture")]
fn state_write_drop_reply_at(path: &Path, spec: &StateWriteSpec) -> io::Result<()> {
    let body = serde_json::to_vec(spec)?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'W');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    if unsafe {
        libc::send(
            stream.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            libc::MSG_NOSIGNAL,
        )
    } != request.len() as isize
    {
        return Err(io::Error::other("short lost-reply State write request"));
    }
    Ok(())
}

/// Child-only broker attestation of a committed release and all live pinned
/// actors. A prepared row or physical gate byte cannot satisfy this read.
pub fn attest_released_child_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerReleaseEvidence> {
    if spec.protocol != "broker-release-attest-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid release attestation request"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

/// A committed write reply can be lost. Reconcile by `read_state_at` using the
/// same source/root/owner/attempt identity before any retry or launch decision.
pub fn write_state_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerContinuationReadback> {
    send_state_request_at(path, b'W', spec)
}

pub fn read_state_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerContinuationReadback> {
    send_state_request_at(path, b'R', spec)
}

pub fn read_bounded_repair_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerRepairReadback> {
    if spec.protocol != "broker-repair-read-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid bounded repair read"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

/// Read the next broker-selected source without receiving a path or an effect
/// grant. The broker authenticates the exact live driver for every request.
pub fn read_bounded_source_selection_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerSourceSelection> {
    if spec.protocol != "broker-source-selection-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid bounded source selection read"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

/// Read one pending recipient chosen from the broker-retained sidecar. This
/// readback is neither a session authentication nor a one-use work grant.
pub fn read_bounded_recipient_selection_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerRecipientSelection> {
    if spec.protocol != "broker-recipient-selection-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid bounded recipient selection read"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

/// Read the grant for the broker-selected pending source. No registration ID,
/// listener, pathname or grant ID is accepted from the caller.
pub fn read_source_effect_grant_at(
    path: &Path,
    spec: &StateReadSpec,
) -> io::Result<Option<oulipoly_state::mailbox::BrokerSourceEffectGrant>> {
    if spec.protocol != "broker-source-grant-read-v30" || spec.attempt_id.is_some() {
        return Err(io::Error::other("invalid source grant read"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'R', spec)?).map_err(io::Error::other)
}

pub fn reserve_source_effect_grant_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerSourceEffectGrant> {
    if spec.protocol != "broker-source-grant-reserve-v30"
        || !matches!(spec.action, StateWriteAction::ReserveSourceGrant)
    {
        return Err(io::Error::other("invalid source grant reservation"));
    }
    let granted: Option<oulipoly_state::mailbox::BrokerSourceEffectGrant> =
        serde_json::from_slice(&send_state_frame_at(path, b'W', spec)?)
            .map_err(io::Error::other)?;
    granted.ok_or_else(|| io::Error::other("broker source grant reservation absent"))
}

#[cfg(feature = "age319-private-broker-fixture")]
pub fn reserve_source_effect_grant_drop_reply_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<()> {
    if spec.protocol != "broker-source-grant-reserve-v30"
        || !matches!(spec.action, StateWriteAction::ReserveSourceGrant)
    {
        return Err(io::Error::other("invalid lost-reply source grant"));
    }
    state_write_drop_reply_at(path, spec)
}

pub fn write_bounded_repair_at(
    path: &Path,
    spec: &StateWriteSpec,
) -> io::Result<oulipoly_state::mailbox::BrokerRepairReadback> {
    if spec.protocol != "broker-repair-write-v30"
        || !matches!(spec.action, StateWriteAction::Repair { .. })
    {
        return Err(io::Error::other("invalid bounded repair write"));
    }
    serde_json::from_slice(&send_state_frame_at(path, b'W', spec)?).map_err(io::Error::other)
}

fn send_state_request_at<T: serde::Serialize>(
    path: &Path,
    opcode: u8,
    spec: &T,
) -> io::Result<oulipoly_state::mailbox::BrokerContinuationReadback> {
    serde_json::from_slice(&send_state_frame_at(path, opcode, spec)?).map_err(io::Error::other)
}

fn send_state_frame_at<T: serde::Serialize>(
    path: &Path,
    opcode: u8,
    spec: &T,
) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(spec)?;
    if body.len() > 2048 {
        return Err(io::Error::other("State read request too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(opcode);
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    if unsafe {
        libc::send(
            stream.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            libc::MSG_NOSIGNAL,
        )
    } != request.len() as isize
    {
        return Err(io::Error::other("short State read request"));
    }
    let mut response = Vec::new();
    stream.take(4097).read_to_end(&mut response)?;
    if response.len() > 4096 || !response.ends_with(b"\n") {
        return Err(io::Error::other("invalid State read response"));
    }
    if response.starts_with(b"error ") {
        return Err(io::Error::other(
            String::from_utf8_lossy(&response).into_owned(),
        ));
    }
    Ok(response)
}

#[derive(Clone, Copy)]
pub enum Operation {
    Classify,
    ObserveEntryGate,
    ObserveInstalledPair,
    CloseEntryGate,
    AbortEntryGate,
    ReserveEntry,
    ReadEntry,
    ReadStateRoute,
    ReserveV30Entry,
    ReadV30Entry,
    PrepareV30Guardian,
    BindV30Guardian,
    LaunchFixedRunner,
}

/// The original entry's invocation, never an executable selection. The broker
/// always executes its installed Runner image and supplies argv[0] itself.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinSpec {
    pub root_id: String,
    pub domain_id: String,
    pub supervisor_id: String,
    pub guardian_pid: i32,
    /// Exact root capability read back from the pinned guardian by the host
    /// entry. The broker transports it only with the consumed one-use join;
    /// the child must still prove it to the guardian before work admission.
    pub root_authority: String,
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
}

/// Host PID identities from the durable native owner. A root-namespace client
/// cannot interpret SO_PEERCRED's PID for its outside guardian; the host broker
/// checks these against pinned host processes and the connected owner socket.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerWitness {
    pub root_id: String,
    pub domain_id: String,
    pub supervisor_id: String,
    pub guardian: ProcessWitness,
    pub driver: ProcessWitness,
    /// Used only by the consumed-work sealed-helper branch of V. The joined
    /// child branch remains compatible with its original witness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_generation: Option<String>,
    /// Exact accepted work selected by a sealed completion helper. Joined
    /// root children have no work ID and retain their original V shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_invocation_uuid: Option<String>,
    /// Digest of the native one-use authority carried by the accepted intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_authority_sha256: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessWitness {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinedChildWitness {
    pub root_id: String,
    pub child: ProcessWitness,
}

/// A source presents its live host incarnation and the exact connected
/// guardian socket. The broker observes the caller and socket in the host PID
/// domain; these fields are assertions to compare, not authority by themselves.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSocketWitness {
    pub root_id: String,
    pub domain_id: String,
    pub supervisor_id: String,
    pub guardian: ProcessWitness,
    pub source: ProcessWitness,
    pub scope: SourceScope,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceScope {
    Root,
    Nested {
        parent_work_id: String,
    },
    /// A separate controller outside all broker roots can authenticate the
    /// endpoint for an already accepted work. The guardian still authorizes
    /// cancellation using the work's independent cancel capability.
    CancelOutside {
        work_id: String,
    },
}

/// Version 2 binds the broker's source decision to the exact connected
/// guardian socket. The broker writes `@` followed by a fresh 16-byte ticket
/// on that socket before returning success. The guardian consumes the ticket
/// only after checking the sender of every byte of a complete control frame.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceControlUse {
    WorkRoot {
        root_id: String,
        work_id: String,
    },
    WorkNested {
        root_id: String,
        work_id: String,
        parent_work_id: String,
    },
    Cancel {
        root_id: String,
        work_id: String,
    },
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTicketUse {
    pub ticket: String,
    pub request: SourceControlUse,
}

pub fn verify_source_socket_v2_at(
    path: &Path,
    witness: &SourceSocketWitness,
    guardian_socket_fd: RawFd,
) -> io::Result<()> {
    verify_source_socket_operation_at(
        path,
        witness,
        guardian_socket_fd,
        b's',
        "verified-source-v2",
    )
}

/// This read-only attestation is valid only for this challenged request and
/// this connected socket. It does not prepare or consume an H/K work grant.
pub fn verify_source_socket_at(
    path: &Path,
    witness: &SourceSocketWitness,
    guardian_socket_fd: RawFd,
) -> io::Result<()> {
    verify_source_socket_operation_at(path, witness, guardian_socket_fd, b'S', "verified-source")
}

fn verify_source_socket_operation_at(
    path: &Path,
    witness: &SourceSocketWitness,
    guardian_socket_fd: RawFd,
    operation: u8,
    accepted: &str,
) -> io::Result<()> {
    let body = serde_json::to_vec(witness)?;
    if body.len() > 2048 {
        return Err(io::Error::other("source socket witness too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(operation);
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as usize;
        *libc::CMSG_DATA(header).cast::<RawFd>() = guardian_socket_fd;
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other("short source socket verification request"));
    }
    let response = read_response(stream)?;
    if response != format!("{accepted} {}\n", witness.root_id) {
        return Err(io::Error::other(format!(
            "host source socket verification refused: {}",
            response.trim()
        )));
    }
    Ok(())
}

/// Only the outside guardian calls this after the complete frame and its
/// ancillary data have passed sender checks. Broker restart loses tickets and
/// therefore refuses instead of reusing an old attestation.
pub fn consume_source_ticket_at(
    path: &Path,
    ticket: [u8; 16],
    request: SourceControlUse,
    accepted_socket_fd: RawFd,
) -> io::Result<()> {
    let root_id = match &request {
        SourceControlUse::WorkRoot { root_id, .. }
        | SourceControlUse::WorkNested { root_id, .. }
        | SourceControlUse::Cancel { root_id, .. } => root_id.clone(),
    };
    let spec = SourceTicketUse {
        ticket: uuid::Uuid::from_bytes(ticket).to_string(),
        request,
    };
    let body = serde_json::to_vec(&spec)?;
    if body.len() > 2048 {
        return Err(io::Error::other("source ticket use too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut frame = Vec::with_capacity(17 + body.len());
    frame.push(b'T');
    frame.extend_from_slice(&challenge);
    frame.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: frame.as_mut_ptr().cast(),
        iov_len: frame.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as usize;
        *libc::CMSG_DATA(header).cast::<RawFd>() = accepted_socket_fd;
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != frame.len() as isize
    {
        return Err(io::Error::other(
            "short source ticket use; outcome uncertain",
        ));
    }
    let response = read_response(stream)?;
    if response != format!("verified-control {root_id}\n") {
        return Err(io::Error::other(format!(
            "source ticket refused: {}",
            response.trim()
        )));
    }
    Ok(())
}

/// The outside guardian asks the broker to attest the exact one-use joined
/// Runner child. PID ancestry cannot cross the broker-created namespace fork.
pub fn verify_joined_child_at(path: &Path, witness: &JoinedChildWitness) -> io::Result<()> {
    let body = serde_json::to_vec(witness)?;
    if body.len() > 2048 {
        return Err(io::Error::other("joined-child witness too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'B');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    stream.write_all(&request)?;
    let response = read_response(stream)?;
    if response != format!("verified-joined-child {}\n", witness.root_id) {
        return Err(io::Error::other(format!(
            "broker joined-child attestation refused: {}",
            response.trim()
        )));
    }
    Ok(())
}

/// A host guardian's positive acceptance. The broker reads the receipt and
/// intent from the accompanying descriptors; these fields bind the guardian's
/// in-memory decision to those exact bytes. This operation only prepares debt.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedWorkSpec {
    pub root_id: String,
    pub work_id: String,
    pub request_sha256: String,
    pub accepted_sha256: String,
    pub owner_generation: String,
}

/// Native prepare is a separate challenged wire from Bash H. The digest must
/// come from the original guardian's retained fsynced receipt decision.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePrepareSpec {
    pub protocol: String,
    pub root_id: String,
    pub attempt_id: String,
    pub owner_generation: String,
    pub receipt_sha256: String,
}

/// The lowercase k frame is reserved for native continuation. It cannot be
/// decoded as Bash original-work K and supplies no command or argv. The only
/// eventual executable/entry is the broker's pinned Runner image at
/// `__completion-root-worker-v1`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeKSpec {
    pub protocol: String,
    pub grant_id: String,
    pub root_id: String,
    pub attempt_id: String,
    pub owner_generation: String,
    pub receipt_sha256: String,
}

/// Descriptor order: accepted-work directory, immutable request, accepted
/// receipt, and the actual sidecar named by that request. This preflight wire
/// is deliberately closed before grant consumption or worker launch.
pub fn native_k_at(path: &Path, spec: &NativeKSpec, descriptors: [RawFd; 4]) -> io::Result<()> {
    let response = send_native_descriptors(path, b'k', spec, descriptors)?;
    // Until an authenticated attach/release protocol exists, even an
    // unexpected positive broker response cannot be treated as launch.
    Err(io::Error::other(format!(
        "native K closed: {}",
        response.trim_end()
    )))
}

/// v30 K carries only the accepted directory/request/receipt. The broker
/// chooses the fixed image, nested namespace and held worker; a lost reply
/// must be observed as spent/unknown and never retried by another launch.
pub fn native_k_v30_at(
    path: &Path,
    spec: &NativeKSpec,
    descriptors: [RawFd; 3],
) -> io::Result<String> {
    if spec.protocol != "native-continuation-v30" {
        return Err(io::Error::other("v30 native K protocol required"));
    }
    send_native_descriptors(path, b't', spec, descriptors)
}

/// Descriptor order: exact accepted-work directory, immutable
/// custodian-request.json, and native-continuation-accepted-v1.json.
pub fn prepare_native_at(
    path: &Path,
    spec: &NativePrepareSpec,
    descriptors: [RawFd; 3],
) -> io::Result<String> {
    send_native_descriptors(path, b'N', spec, descriptors)
}

fn send_native_descriptors<T: serde::Serialize, const N: usize>(
    path: &Path,
    operation: u8,
    spec: &T,
    descriptors: [RawFd; N],
) -> io::Result<String> {
    read_response(send_native_descriptors_frame(
        path,
        operation,
        spec,
        descriptors,
    )?)
}

/// Exercise an uncertain t response without changing the broker's challenged
/// request path. The sent frame can have spent K even though this caller gets
/// no launch result; readback must use the retained grant and Q.
#[cfg(feature = "age319-private-broker-fixture")]
pub fn native_k_v30_drop_reply_at(
    path: &Path,
    spec: &NativeKSpec,
    descriptors: [RawFd; 3],
) -> io::Result<()> {
    if spec.protocol != "native-continuation-v30" {
        return Err(io::Error::other("v30 native K protocol required"));
    }
    drop(send_native_descriptors_frame(
        path,
        b't',
        spec,
        descriptors,
    )?);
    Ok(())
}

fn send_native_descriptors_frame<T: serde::Serialize, const N: usize>(
    path: &Path,
    operation: u8,
    spec: &T,
    descriptors: [RawFd; N],
) -> io::Result<UnixStream> {
    let body = serde_json::to_vec(spec)?;
    if body.len() > 2048 {
        return Err(io::Error::other("native prepare request too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(operation);
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), N);
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other("short native prepare; outcome uncertain"));
    }
    Ok(stream)
}

/// A prepared H grant is launched once by its original guardian. The seven
/// descriptors are the five H artifacts, the worker control socket, and the
/// read end of the worker capability pipe. The broker chooses the namespace,
/// executable argument vector, and process credentials from the grant.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchAcceptedWorkSpec {
    pub grant_id: String,
}

/// Read the exact broker work state. A response is only a drain certificate
/// when the recorded PID1 is dead and its terminal receipt is valid.
pub fn observe_accepted_work_at(path: &Path, grant_id: &str) -> io::Result<String> {
    let id = uuid::Uuid::parse_str(grant_id).map_err(|_| io::Error::other("bad grant ID"))?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(33);
    request.push(b'Q');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(id.as_bytes());
    stream.write_all(&request)?;
    read_response(stream)
}

/// Request cancellation of the exact consumed work grant. This only wakes its
/// PID1; Q remains the separate terminal and physical drain certificate.
pub fn cancel_accepted_work_at(path: &Path, grant_id: &str) -> io::Result<String> {
    let id = uuid::Uuid::parse_str(grant_id).map_err(|_| io::Error::other("bad grant ID"))?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(33);
    request.push(b'Z');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(id.as_bytes());
    stream.write_all(&request)?;
    read_response(stream)
}

/// Observe native PID1/terminal evidence and settle exact State Q when both
/// the terminal and parent wait receipts are present. Q does not integrate a
/// Runner result or release any source/recipient obligation.
pub fn observe_native_work_v30_at(path: &Path, grant_id: &str) -> io::Result<String> {
    native_work_control_at(path, b'q', grant_id)
}

/// Persist exact native cancellation intent before signalling its PID1.
pub fn cancel_native_work_v30_at(path: &Path, grant_id: &str) -> io::Result<String> {
    native_work_control_at(path, b'z', grant_id)
}

fn native_work_control_at(path: &Path, operation: u8, grant_id: &str) -> io::Result<String> {
    let id =
        uuid::Uuid::parse_str(grant_id).map_err(|_| io::Error::other("bad native grant ID"))?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(33);
    request.push(operation);
    request.extend_from_slice(&challenge);
    request.extend_from_slice(id.as_bytes());
    stream.write_all(&request)?;
    read_response(stream)
}

pub fn launch_accepted_work_at(
    path: &Path,
    spec: &LaunchAcceptedWorkSpec,
    descriptors: [RawFd; 7],
) -> io::Result<String> {
    let body = serde_json::to_vec(spec)?;
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'K');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 128];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), 7);
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other("short launch request; outcome uncertain"));
    }
    read_response(stream)
}

/// Descriptor order: accepted initiator executable, intent, cwd, state directory,
/// and the exclusively created acceptance receipt. No arbitrary argv is sent.
pub fn prepare_accepted_work_at(
    path: &Path,
    spec: &AcceptedWorkSpec,
    descriptors: [RawFd; 5],
) -> io::Result<String> {
    let body = serde_json::to_vec(spec)?;
    if body.len() > 2048 {
        return Err(io::Error::other("accepted-work request too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'H');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), 5);
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other(
            "short accepted-work request; outcome uncertain",
        ));
    }
    read_response(stream)
}

/// Only CLI surfaces that dispatch without native service bootstrap may enter
/// the root child. Observer-aware PID reads do not supply broker-owned nested
/// work launch, control, and physical drain, or authenticate the host procfs
/// observer against a privileged workload that can replace `/proc`.
pub fn supported_entry_args(args: &[String]) -> bool {
    #[cfg(feature = "age319-private-broker-fixture")]
    if matches!(args, [first, second] if first == "__age319-private-installed-probe-v1" && (second == "tty" || second == "setuid" || second == "sleep"))
        || matches!(args, [first, second, marker] if first == "__age319-private-installed-probe-v1" && second == "ambient" && marker.starts_with('/'))
    {
        return (unsafe { libc::geteuid() }) == 0
            && std::fs::read_to_string("/proc/self/uid_map")
                .ok()
                .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"));
    }
    #[cfg(feature = "age319-private-broker-fixture")]
    if matches!(args, [only] if only == "__age319-private-join-only-v1" || only == "__age319-private-bash-work-v1" || only == "__age319-private-normal-v30")
        && unsafe { libc::geteuid() } == 0
        && std::fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
    {
        return true;
    }
    matches!(args, [only] if only == "--help" || only == "-h")
        || args.first().is_some_and(|first| first == "diagnostics")
}

/// Transfer stdin, stdout, stderr, the working directory, and a one-way
/// completion receipt socket held by the entry until the child exits.
/// A single challenged sendmsg keeps credentials, data and descriptors together.
pub fn join_at(path: &Path, spec: &JoinSpec, descriptors: [RawFd; 5]) -> io::Result<String> {
    join_versioned_at(path, spec, descriptors, b'J')
}

/// Submit an exact installed CLI/GUI entry. The response is a receipt or an
/// explicit refusal; a lost reply is uncertain and must never be retried as a
/// new request ID. The production broker currently refuses before execution.
pub fn submit_installed_launch_at(
    path: &Path,
    spec: &InstalledLaunchSpec,
    descriptors: &[RawFd],
) -> io::Result<String> {
    let stream = checked_connection(path)?;
    submit_installed_launch_on(stream, spec, descriptors)
}

/// Private feature-only readback/cancel. The request ID selects an existing
/// journal record; it never grants another execution.
#[cfg(feature = "age319-private-broker-fixture")]
pub fn private_installed_control_at(
    path: &Path,
    request_id: &str,
    generation: &str,
    cancel: bool,
) -> io::Result<String> {
    let id = uuid::Uuid::parse_str(request_id)
        .map_err(|_| io::Error::other("invalid private launch request ID"))?;
    if id.to_string() != request_id {
        return Err(io::Error::other("noncanonical private launch request ID"));
    }
    let generation_id = uuid::Uuid::parse_str(generation)
        .map_err(|_| io::Error::other("invalid private launch generation"))?;
    if generation_id.to_string() != generation {
        return Err(io::Error::other("noncanonical private launch generation"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut frame = [0u8; 49];
    frame[0] = if cancel { b'M' } else { b'l' };
    frame[1..17].copy_from_slice(&challenge);
    frame[17..33].copy_from_slice(id.as_bytes());
    frame[33..49].copy_from_slice(generation_id.as_bytes());
    if stream.write(&frame)? != frame.len() {
        return Err(io::Error::other("private launch status request uncertain"));
    }
    read_response(stream)
}

fn submit_installed_launch_on(
    mut stream: UnixStream,
    spec: &InstalledLaunchSpec,
    descriptors: &[RawFd],
) -> io::Result<String> {
    crate::installed_launch::validate(spec, descriptors)?;
    let body = serde_json::to_vec(spec)?;
    if body.len() > 48 * 1024 {
        return Err(io::Error::other("installed launch request too large"));
    }
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'L');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(
            descriptors.as_ptr(),
            libc::CMSG_DATA(header).cast(),
            descriptors.len(),
        );
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other("installed launch submission uncertain"));
    }
    read_response(stream)
}

#[cfg(test)]
mod installed_launch_wire_tests {
    use super::*;
    use crate::installed_launch::{EntryKind, capture_from};
    use std::ffi::OsString;
    use std::fs::File;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::thread;

    fn exchange(kind: EntryKind, reply: Option<&'static str>) -> (io::Result<String>, String) {
        let name = if kind == EntryKind::Gui {
            "oulipoly-plane"
        } else {
            "agents"
        };
        let (_master, slave) = if kind == EntryKind::Cli {
            let mut master = -1;
            let mut slave = -1;
            assert_eq!(
                unsafe {
                    libc::openpty(
                        &mut master,
                        &mut slave,
                        std::ptr::null_mut(),
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                },
                0
            );
            (
                Some(unsafe { OwnedFd::from_raw_fd(master) }),
                Some(unsafe { OwnedFd::from_raw_fd(slave) }),
            )
        } else {
            (None, None)
        };
        let stdio = slave.as_ref().map_or([-1; 3], |fd| [fd.as_raw_fd(); 3]);
        let captured = capture_from(
            &uuid::Uuid::new_v4().to_string(),
            vec![OsString::from(name), OsString::from("a b")],
            vec![],
            stdio,
        )
        .unwrap();
        let request_id = captured.spec.request_id.clone();
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let challenge = [7u8; 16];
            server.write_all(&challenge).unwrap();
            let mut bytes = [0u8; 48 * 1024 + 17];
            let mut iov = libc::iovec {
                iov_base: bytes.as_mut_ptr().cast(),
                iov_len: bytes.len(),
            };
            let mut control = [0u8; 128];
            let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = control.len();
            let size =
                unsafe { libc::recvmsg(server.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
            assert!(size > 17);
            assert_eq!(bytes[0], b'L');
            assert_eq!(&bytes[1..17], &challenge);
            let body: InstalledLaunchSpec =
                serde_json::from_slice(&bytes[17..size as usize]).unwrap();
            assert_eq!(body.kind, kind);
            assert_eq!(body.args, [b"a b".to_vec()]);
            let mut received = Vec::new();
            let mut header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
            while !header.is_null() {
                let current = unsafe { &*header };
                if current.cmsg_level == libc::SOL_SOCKET && current.cmsg_type == libc::SCM_RIGHTS {
                    let count = (current.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) } as usize)
                        / std::mem::size_of::<RawFd>();
                    for index in 0..count {
                        let fd = unsafe { *libc::CMSG_DATA(header).cast::<RawFd>().add(index) };
                        received.push(unsafe { File::from_raw_fd(fd) });
                    }
                }
                header = unsafe { libc::CMSG_NXTHDR(&msg, header) };
            }
            assert_eq!(received.len(), if kind == EntryKind::Cli { 4 } else { 1 });
            for fd in received.iter().take(received.len() - 1) {
                assert_eq!(unsafe { libc::isatty(fd.as_raw_fd()) }, 1);
            }
            assert!(received.last().unwrap().metadata().unwrap().is_dir());
            if let Some(reply) = reply {
                server.write_all(reply.as_bytes()).unwrap();
            }
            body.request_id
        });
        let result = submit_installed_launch_on(
            client,
            &captured.spec,
            &captured
                .descriptors
                .iter()
                .map(AsRawFd::as_raw_fd)
                .collect::<Vec<_>>(),
        );
        assert_eq!(worker.join().unwrap(), request_id);
        (result, request_id)
    }

    #[test]
    fn private_cli_gui_second_entrant_and_lost_reply_have_no_retry() {
        assert_eq!(
            exchange(EntryKind::Cli, Some("error staged\n")).0.unwrap(),
            "error staged\n"
        );
        let lost = exchange(EntryKind::Gui, None);
        assert!(lost.0.is_err());
        let next = exchange(EntryKind::Cli, Some("error staged\n"));
        assert_ne!(lost.1, next.1);
        assert_eq!(next.0.unwrap(), "error staged\n");
        assert_eq!(
            exchange(EntryKind::Cli, Some("error ungated root launch disabled\n"))
                .0
                .unwrap(),
            "error ungated root launch disabled\n"
        );
    }

    #[test]
    fn missing_installed_socket_refuses_before_handoff() {
        let captured = capture_from(
            &uuid::Uuid::new_v4().to_string(),
            vec![OsString::from("agents")],
            vec![],
            [-1; 3],
        )
        .unwrap();
        let missing = std::path::Path::new("/no-such-oulipoly-kernel-broker/control.sock");
        assert!(
            submit_installed_launch_at(
                missing,
                &captured.spec,
                &captured
                    .descriptors
                    .iter()
                    .map(AsRawFd::as_raw_fd)
                    .collect::<Vec<_>>()
            )
            .is_err()
        );
    }
}

pub fn join_held_v30_at(
    path: &Path,
    spec: &JoinSpec,
    descriptors: [RawFd; 5],
) -> io::Result<String> {
    join_versioned_at(path, spec, descriptors, b'j')
}

fn join_versioned_at(
    path: &Path,
    spec: &JoinSpec,
    descriptors: [RawFd; 5],
    opcode: u8,
) -> io::Result<String> {
    let body = serde_json::to_vec(spec)?;
    if body.len() > 48 * 1024 {
        return Err(io::Error::other("join environment too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(opcode);
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen =
        unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&descriptors) as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&descriptors) as _) as usize;
        std::ptr::copy_nonoverlapping(descriptors.as_ptr(), libc::CMSG_DATA(header).cast(), 5);
    }
    let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) };
    if sent != request.len() as isize {
        return Err(io::Error::other("short join request; outcome uncertain"));
    }
    read_response(stream)
}

/// The descriptor is the already-connected client end of the native owner
/// socket. It is inspected in the broker's host PID namespace, not trusted as
/// an authority merely because the caller supplied it.
pub fn verify_owner_at(path: &Path, witness: &OwnerWitness, owner_fd: RawFd) -> io::Result<()> {
    let body = serde_json::to_vec(witness)?;
    if body.len() > 2048 {
        return Err(io::Error::other("owner witness too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'V');
    request.extend_from_slice(&challenge);
    request.extend_from_slice(&body);
    let mut iov = libc::iovec {
        iov_base: request.as_mut_ptr().cast(),
        iov_len: request.len(),
    };
    let mut control = [0u8; 64];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as _) } as usize;
    unsafe {
        let header = libc::CMSG_FIRSTHDR(&msg);
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as _) as usize;
        *libc::CMSG_DATA(header).cast::<RawFd>() = owner_fd;
    }
    if unsafe { libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) }
        != request.len() as isize
    {
        return Err(io::Error::other("short owner verification request"));
    }
    let response = read_response(stream)?;
    if response != format!("verified-owner {}\n", witness.root_id) {
        return Err(io::Error::other(format!(
            "host owner verification refused: {}",
            response.trim()
        )));
    }
    Ok(())
}

fn checked_connection(path: &Path) -> io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut length,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>()
        || unsafe { credentials.assume_init().uid } != 0
    {
        return Err(io::Error::other("broker peer is not host root"));
    }
    Ok(stream)
}

fn read_response(stream: UnixStream) -> io::Result<String> {
    let mut response = Vec::new();
    stream.take(257).read_to_end(&mut response)?;
    if response.len() > 256 || !response.ends_with(b"\n") {
        return Err(io::Error::other("invalid broker response"));
    }
    String::from_utf8(response).map_err(|_| io::Error::other("non-UTF8 broker response"))
}

pub fn request_at(path: &Path, operation: Operation) -> io::Result<String> {
    request_frame_at(path, operation, Payload::None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryRoute {
    LegacyOpen,
    Draining,
    BrokerV30Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub struct InstalledPairObservation {
    pub version: String,
    pub generation: String,
    pub route: EntryRoute,
}

pub fn observe_installed_pair_at(path: &Path) -> io::Result<InstalledPairObservation> {
    let response = request_at(path, Operation::ObserveInstalledPair)?;
    parse_installed_pair_response(&response)
}

fn parse_installed_pair_response(response: &str) -> io::Result<InstalledPairObservation> {
    if !response.ends_with('\n') {
        return Err(io::Error::other("invalid installed pair response"));
    }
    let fields: Vec<_> = response.trim_end_matches('\n').split(' ').collect();
    if fields.len() != 4
        || fields[0] != "installed-pair-v1"
        || uuid::Uuid::parse_str(fields[2])
            .ok()
            .is_none_or(|id| id.to_string() != fields[2])
    {
        return Err(io::Error::other("invalid installed pair response"));
    }
    let route = match fields[3] {
        "legacy-open" => EntryRoute::LegacyOpen,
        "draining" => EntryRoute::Draining,
        "broker-v30-closed" => EntryRoute::BrokerV30Closed,
        _ => return Err(io::Error::other("invalid installed pair route")),
    };
    Ok(InstalledPairObservation {
        version: fields[1].into(),
        generation: fields[2].into(),
        route,
    })
}

#[cfg(test)]
mod installed_pair_response_tests {
    use super::*;

    #[test]
    fn old_missing_and_malformed_broker_responses_refuse() {
        let absent = tempfile::tempdir().unwrap().path().join("missing.sock");
        assert!(observe_installed_pair_at(&absent).is_err());
        assert!(parse_installed_pair_response("entry-gate-v1 legacy-open\n").is_err());
        assert!(
            parse_installed_pair_response("installed-pair-v1 0.1.0 bad legacy-open\n").is_err()
        );
        assert!(parse_installed_pair_response("installed-pair-v1 0.1.0 bad legacy-open").is_err());
    }
}

/// Reads only broker-owned service state. A retired user sidecar is never a
/// source for this result; absence of a reachable broker is an error.
pub fn observe_entry_gate_at(path: &Path) -> io::Result<EntryRoute> {
    match request_at(path, Operation::ObserveEntryGate)?.as_str() {
        "entry-gate-v1 legacy-open\n" => Ok(EntryRoute::LegacyOpen),
        "entry-gate-v1 draining\n" => Ok(EntryRoute::Draining),
        "entry-gate-v1 broker-v30-closed\n" => Ok(EntryRoute::BrokerV30Closed),
        _ => Err(io::Error::other("unrecognized broker entry gate response")),
    }
}

/// Administrative prerequisite only. This prevents new broker admission and
/// survives restart; it does not stop already-running direct sidecar writers.
pub fn close_entry_gate_at(path: &Path) -> io::Result<()> {
    if request_at(path, Operation::CloseEntryGate)? == "entry-gate-v1 draining\n" {
        Ok(())
    } else {
        Err(io::Error::other("broker entry gate closure refused"))
    }
}

/// Reopens v29 admission after a failed cutover prerequisite only while no
/// broker sidecar has been published. A published copy needs separate recovery.
pub fn abort_entry_gate_at(path: &Path) -> io::Result<()> {
    if request_at(path, Operation::AbortEntryGate)? == "entry-gate-v1 legacy-open\n" {
        Ok(())
    } else {
        Err(io::Error::other("broker entry gate abort refused"))
    }
}

#[derive(Clone, Copy)]
enum Payload {
    None,
    Prepare(uuid::Uuid, i32),
    Bind(uuid::Uuid, uuid::Uuid, uuid::Uuid),
    Read(uuid::Uuid),
}

pub fn prepare_guardian_at(path: &Path, root_id: &str, guardian_pid: i32) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    if guardian_pid <= 0 {
        return Err(io::Error::other("bad guardian PID"));
    }
    request_frame_at(
        path,
        Operation::ReserveEntry,
        Payload::Prepare(root, guardian_pid),
    )
}

pub fn prepare_v30_guardian_at(
    path: &Path,
    root_id: &str,
    guardian_pid: i32,
) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    request_frame_at(
        path,
        Operation::PrepareV30Guardian,
        Payload::Prepare(root, guardian_pid),
    )
}

pub fn bind_v30_guardian_at(
    path: &Path,
    root_id: &str,
    domain_id: &str,
    supervisor_id: &str,
) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    let domain = uuid::Uuid::parse_str(domain_id).map_err(|_| io::Error::other("bad domain ID"))?;
    let supervisor =
        uuid::Uuid::parse_str(supervisor_id).map_err(|_| io::Error::other("bad supervisor ID"))?;
    request_frame_at(
        path,
        Operation::BindV30Guardian,
        Payload::Bind(root, domain, supervisor),
    )
}

pub fn read_v30_entry_at(path: &Path, root_id: &str) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    request_frame_at(path, Operation::ReadV30Entry, Payload::Read(root))
}

pub fn prepare_guardian(root_id: &str, guardian_pid: i32) -> io::Result<String> {
    prepare_guardian_at(Path::new(INSTALLED_SOCKET), root_id, guardian_pid)
}

pub fn bind_guardian_at(
    path: &Path,
    root_id: &str,
    domain_id: &str,
    supervisor_id: &str,
) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    let domain = uuid::Uuid::parse_str(domain_id).map_err(|_| io::Error::other("bad domain ID"))?;
    let supervisor =
        uuid::Uuid::parse_str(supervisor_id).map_err(|_| io::Error::other("bad supervisor ID"))?;
    request_frame_at(
        path,
        Operation::ReserveEntry,
        Payload::Bind(root, domain, supervisor),
    )
}

pub fn bind_guardian(root_id: &str, domain_id: &str, supervisor_id: &str) -> io::Result<String> {
    bind_guardian_at(
        Path::new(INSTALLED_SOCKET),
        root_id,
        domain_id,
        supervisor_id,
    )
}

pub fn read_entry_at(path: &Path, root_id: &str) -> io::Result<String> {
    let root = uuid::Uuid::parse_str(root_id).map_err(|_| io::Error::other("bad root ID"))?;
    request_frame_at(path, Operation::ReadEntry, Payload::Read(root))
}

pub fn read_entry(root_id: &str) -> io::Result<String> {
    read_entry_at(Path::new(INSTALLED_SOCKET), root_id)
}

fn request_frame_at(path: &Path, operation: Operation, payload: Payload) -> io::Result<String> {
    let mut stream = checked_connection(path)?;
    if matches!(
        operation,
        Operation::ObserveEntryGate
            | Operation::ObserveInstalledPair
            | Operation::CloseEntryGate
            | Operation::AbortEntryGate
    ) {
        let timeout = Some(std::time::Duration::from_secs(5));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
    }
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = [0u8; 65];
    request[0] = match operation {
        Operation::Classify => b'C',
        Operation::ObserveEntryGate => b'i',
        Operation::ObserveInstalledPair => b'v',
        Operation::CloseEntryGate => b'X',
        Operation::AbortEntryGate => b'x',
        Operation::ReserveEntry if matches!(payload, Payload::Prepare(..)) => b'P',
        Operation::ReserveEntry if matches!(payload, Payload::Bind(..)) => b'G',
        Operation::ReserveEntry => b'E',
        Operation::ReadEntry => b'A',
        Operation::ReserveV30Entry => b'e',
        Operation::PrepareV30Guardian => b'p',
        Operation::BindV30Guardian => b'g',
        Operation::ReadV30Entry => b'a',
        Operation::ReadStateRoute => b'I',
        Operation::LaunchFixedRunner => b'L',
    };
    request[1..17].copy_from_slice(&challenge);
    let length = match payload {
        Payload::None => 17,
        Payload::Prepare(root, pid) => {
            request[17..33].copy_from_slice(root.as_bytes());
            request[33..37].copy_from_slice(&pid.to_ne_bytes());
            37
        }
        Payload::Bind(root, domain, supervisor) => {
            request[17..33].copy_from_slice(root.as_bytes());
            request[33..49].copy_from_slice(domain.as_bytes());
            request[49..65].copy_from_slice(supervisor.as_bytes());
            65
        }
        Payload::Read(root) => {
            request[17..33].copy_from_slice(root.as_bytes());
            33
        }
    };
    // One send yields one SCM_CREDENTIALS-bearing request message.
    let written = unsafe {
        libc::send(
            stream.as_raw_fd(),
            request.as_ptr().cast(),
            length,
            libc::MSG_NOSIGNAL,
        )
    };
    if written != length as isize {
        return Err(io::Error::other("short broker request"));
    }
    read_response(stream)
}

pub fn request(operation: Operation) -> io::Result<String> {
    request_at(Path::new(INSTALLED_SOCKET), operation)
}
