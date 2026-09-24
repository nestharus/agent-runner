//! Opt-in host-root broker for the pinned guardian and one-use root child join.

const SOURCE_TICKET_TTL: std::time::Duration = std::time::Duration::from_secs(30);
const BROKER_ACCEPT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);
const BROKER_INGRESS_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const RELEASED_HANDOFF_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const FRESH_V30_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const FRESH_V30_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const FRESH_HANDOFF_QUEUE_CAPACITY: usize = 8;
const RELEASED_HANDOFF_REPLY_CAPACITY: usize = 1;

const REQUEST_RECEIVE_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(feature = "age319-private-broker-fixture")]
#[path = "fresh_provider.rs"]
mod fresh_provider;
#[path = "native_work.rs"]
mod native_work;
#[cfg(feature = "age319-private-broker-fixture")]
#[path = "private_installed_exec.rs"]
mod private_installed_exec;
#[path = "released_handoff.rs"]
mod released_handoff;
#[path = "root_join.rs"]
mod root_join;
#[path = "source_launch.rs"]
mod source_launch;
#[path = "work_launch.rs"]
mod work_launch;
use base64::Engine as _;
use oulipoly_kernel_broker::accepted_grant::GrantRegistry;
use oulipoly_kernel_broker::cutover_gate::EntryGate;
use oulipoly_kernel_broker::entry_registry::{EntryRegistry, ProcessStamp};
use oulipoly_kernel_broker::identity::{
    PeerIdentity, PinnedProcess, host_proc_file, host_proc_uid, install_detached_host_proc,
};
use oulipoly_kernel_broker::installed_launch::{self, InstalledLaunchSpec};
use oulipoly_kernel_broker::installed_pair::{self, InstalledPair};
use oulipoly_kernel_broker::native_receipt::{
    BoundNativeAuthority, verify as verify_native_receipt,
};
use oulipoly_kernel_broker::protocol::{
    AcceptedWorkSpec, FreshChildRequest, FreshRecipientRequest, FreshRootEffectRequest, JoinSpec,
    JoinedChildWitness, LaunchAcceptedWorkSpec, NativeKSpec, NativePrepareSpec, OwnerWitness,
    ProcessWitness, SourceControlUse, SourceScope, SourceSocketWitness, SourceTicketUse,
    StateGenerationSpec, StateReadSpec, StateWriteAction, StateWriteSpec,
};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::source_acceptance::capture_and_stage_v2_evidence;
use oulipoly_kernel_broker::source_physical::{SourceObservation, SourcePhysicalRegistry};
use oulipoly_kernel_broker::work_registry::{Scope, WorkRegistry, classify_scope};
use oulipoly_state::mailbox::{
    BrokerReleaseEvidence, BrokerSidecar, BrokerSourceEffectGrant, FreshDeliverySubmission,
    FreshRecipientIdentity, FreshReleasedHandoff, FreshV30Lane, FreshV30LaneIdentity,
    PreparedBrokerOwner, PreparedProcessStamp,
};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::Instant;

const SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";
const STATE: &str = "/var/lib/oulipoly-kernel-broker";
const FRESH_SOCKET: &str = "/run/oulipoly-kernel-broker/v30.sock";
const RUNNER: &str = "/usr/local/libexec/oulipoly/oulipoly-agent-runner";

#[cfg(feature = "age319-private-broker-fixture")]
fn private_fixture() -> bool {
    (unsafe { libc::geteuid() }) == 0
        && fs::read_to_string("/proc/self/uid_map")
            .ok()
            .is_some_and(|map| map.split_ascii_whitespace().nth(2) == Some("1"))
        && std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_SOCKET_V1").is_some()
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
    if matches!(operation, b'i' | b'v' | b'X' | b'x') {
        return Ok(());
    }
    if gate_closed {
        return Err(io::Error::other("broker entry gate is durably closed"));
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
            b'Y' | b'R'
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
        )
    {
        #[cfg(feature = "age319-private-broker-fixture")]
        if private_fixture() && matches!(operation, b'L' | b'l' | b'M') {
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
    },
    #[allow(
        dead_code,
        reason = "fresh Bash lane is closed without the private fixture"
    )]
    FreshBashPrivateResult {
        result: oulipoly_state::mailbox::FreshBashPrivateResult,
    },
    #[cfg(feature = "age319-private-broker-fixture")]
    FreshProviderRequest {
        request: FreshRootEffectRequest,
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
        b'Q' | b'Z' | b'q' | b'z' | b'D' | b'd' | b'C' | b'c' => read == 33,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'%' | b'&' | b'!' => read == 33,
        // Legacy E has no body; fresh Bash E carries a request UUID on its
        // separate socket. Preserve both exact wire shapes for pinned images.
        b'E' => read == 17 || read == 33,
        #[cfg(feature = "age319-private-broker-fixture")]
        b'l' | b'M' => read == 49,
        b'A' | b'a' => read == 33,
        b'J' | b'j' => (18..=48 * 1024 + 17).contains(&read),
        b'L' => (18..=48 * 1024 + 17).contains(&read),
        b'V' | b'S' | b's' | b'T' | b'H' | b'K' | b'B' | b'N' | b'k' | b't' | b'R' | b'W'
        | b'Y' | b'0' | b'1' | b'2' | b'3' | b'4' => (18..=2048 + 17).contains(&read),
        #[cfg(feature = "age319-private-broker-fixture")]
        b'5' | b'6' | b'7' | b'8' | b'9' => (18..=2048 + 17).contains(&read),
        #[cfg(feature = "age319-private-broker-fixture")]
        b'h' | b'f' | b'm' | b'n' => (18..=48 * 1024 + 17).contains(&read),
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
            #[cfg(feature = "age319-private-broker-fixture")]
            b'5' | b'9' => descriptors.len() != 4,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'h' => descriptors.len() != 5,
            #[cfg(feature = "age319-private-broker-fixture")]
            b'f' => descriptors.len() != 1,
            b'L' => !(1..=4).contains(&descriptors.len()),
            b'V' | b'S' | b's' | b'T' => descriptors.len() != 1,
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
        b'U' => RequestPayload::FreshChildRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'F' => RequestPayload::FreshRecipientRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'0' | b'1' | b'2' | b'3' | b'4' => RequestPayload::FreshRootEffectRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'5' | b'6' | b'7' | b'8' | b'9' => RequestPayload::FreshProviderRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'h' | b'f' => RequestPayload::FreshRouteRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
            descriptors,
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'm' | b'n' => RequestPayload::FreshAccountEffectRequest {
            request: serde_json::from_slice(&request[17..read as usize])?,
        },
        b'D' | b'd' => RequestPayload::FreshSessionRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'C' | b'c' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        #[cfg(feature = "age319-private-broker-fixture")]
        b'%' | b'&' | b'!' => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'E' if read == 33 => RequestPayload::FreshBashChildRequest {
            request_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
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
) -> io::Result<String> {
    for id in [&witness.root_id, &witness.domain_id, &witness.supervisor_id] {
        uuid::Uuid::parse_str(id).map_err(|_| io::Error::other("invalid source witness ID"))?;
    }
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
    let scope = classify_scope(peer, host_namespace, roots, works);
    // A legitimate in-root sudo descendant may become host UID 0. Its exact
    // PID namespace and source incarnation still bind it to this root/work;
    // the outside cancel route retains the original owner UID.
    if peer.uid != entry.owner_uid
        && !(peer.uid == 0 && matches!(&scope, Scope::Root(_) | Scope::Work { .. }))
    {
        return Err(io::Error::other("source UID is outside root policy"));
    }
    match (&witness.scope, scope) {
        (SourceScope::Root, Scope::Root(root_id)) if root_id == witness.root_id => {}
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
        _ => return Err(io::Error::other("source root/work scope mismatch")),
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
    )?;
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
) -> io::Result<BrokerReleaseEvidence> {
    if spec.protocol != "broker-release-attest-v30"
        || spec.attempt_id.is_some()
        || roots.has_debt()
        || works.has_debt()
        || entries.has_uncertain_write()
        || !matches!(
            classify_scope(peer, host_namespace, roots, works),
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
        // Only the old loop can mint root authority. A later Bash descendant
        // must use a separate registration and grant; this root has no handle.
        let authority = oulipoly_state::CompletionRegistrationAuthority::generate()
            .map_err(io::Error::other)?;
        let image = runner_image.metadata()?;
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
// traffic, so capture cannot depend on a later caller request. This only
// stages evidence; it never accepts, releases, or notifies a source.
fn capture_terminal_sources(
    sidecar: &mut BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    pending: &mut BTreeMap<String, BrokerSourceEffectGrant>,
) {
    let grants: Vec<_> = pending.values().cloned().collect();
    for grant in grants {
        match sidecar.read_source_evidence(&grant) {
            Ok(Some(_)) => {
                pending.remove(&grant.grant_id);
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
                        .is_ok_and(|row| row.is_some())
                    {
                        pending.remove(&grant.grant_id);
                    }
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
    // Ephemeral by design: a broker restart invalidates every pre-wire source
    // decision. No work/cancel action can be authorized by a lost ticket.
    let mut source_tickets = BTreeMap::<String, SourceTicket>::new();
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
    // The old loop alone owns the release gate and mutable kernel registries.
    // Fresh storage stays on another thread and is opened only at the fixed
    // broker-owned v30 directory. Neither handler can wait on the other's
    // socket or select the other's State connection.
    let fresh_root = Path::new(&state).join("v30");
    if fs::symlink_metadata(&fresh_root).is_ok() {
        let fresh_state_root = PathBuf::from(&state);
        let fresh_runner_image = runner_image.try_clone()?;
        let fresh_handoff_tx = handoff_tx.clone();
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
                ) {
                    eprintln!("fresh v30 lane closed: {error}");
                }
            })?;
    }
    loop {
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
                    native_work::reconcile_after_restart(
                        &grants,
                        &works,
                        sidecar,
                        &broker_incarnation,
                        &terminal_path,
                        &mut settled_native_q,
                    );
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
            if operation == b'i' {
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
                )?;
                if operation == b's' {
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
                let [executable, intent, cwd, state_dir, accepted] = descriptors;
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
                )?;
                Ok(format!("prepared-work {}\n", grant.grant_id))
            } else if operation == b'N' {
                let RequestPayload::PrepareNative { spec, descriptors } = payload else {
                    return Err(io::Error::other("invalid native prepare payload"));
                };
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
                    || !root_launch_admitted(
                        &peer,
                        &classify_scope(&peer, &host_namespace, &registry, &works),
                        &host_namespace,
                    )
                    || !peer.process.same_executable_as(&runner_image)?
                {
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
    serve_fresh_v30_at(
        Path::new(&state_root),
        Path::new(&socket),
        File::open(&runner)?,
        None,
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
    fresh_provider::plan(
        &image,
        &gate,
        &input,
        vec![
            "-c".into(),
            "printf 'broker-child-output\\n'; printf 'broker-ran\\n' > \"$1\"; (setsid sh -c 'trap \"\" TERM; while :; do sleep 1; done' >/dev/null 2>&1 &)".into(),
            "sh".into(),
            marker.display().to_string(),
        ],
        vec![("PATH".into(), "/usr/bin:/bin".into())],
    )
}

fn serve_fresh_v30_at(
    state_root: &Path,
    socket: &Path,
    runner_image: File,
    handoff_tx: Option<SyncSender<FreshHandoffBridgeRequest>>,
) -> io::Result<()> {
    // A missing or incomplete publication cannot bind the new endpoint.
    let mut lane = FreshV30Lane::open_at(state_root).map_err(io::Error::other)?;
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
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        stream.set_read_timeout(Some(FRESH_V30_READ_TIMEOUT))?;
        stream.set_write_timeout(Some(FRESH_V30_WRITE_TIMEOUT))?;
        let mut submitted_grant = None;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_provider_k_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_provider_q_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut drop_account_effect_reply = false;
        #[cfg(feature = "age319-private-broker-fixture")]
        let mut provider_output_files: Option<[File; 2]> = None;
        let answer = (|| -> io::Result<String> {
            let (operation, payload, peer) = peer_from_request(&mut stream)?;
            peer.process.verify()?;
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
            let local_lookup =
                matches!(
                    &payload,
                    RequestPayload::FreshRecipientRequest {
                        request: FreshRecipientRequest::Lookup { .. }
                    }
                ) || matches!(operation, b'C' | b'c' | b'E' | b'O' | b'%' | b'&' | b'!');
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
                b'C' | b'c' | b'E' | b'O' => Err(io::Error::other(
                    "fresh Bash child/work/result closed until normal root grant and physical result custody",
                )),
                #[cfg(feature = "age319-private-broker-fixture")]
                b'C' | b'c' | b'E' | b'O' | b'%' | b'&' | b'!' => {
                    if !private_fixture() {
                        return Err(io::Error::other(
                            "fresh Bash child/work/result closed until normal root grant and physical result custody",
                        ));
                    }
                    if operation == b'E' && instance.is_closed() {
                        return Err(io::Error::other("fresh Bash private work gate closed"));
                    }
                    let request_id = match &payload {
                        RequestPayload::FreshBashChildRequest { request_id } => request_id.clone(),
                        RequestPayload::FreshBashPrivateResult { result } => {
                            result.request_id.clone()
                        }
                        _ => return Err(io::Error::other("Bash child request absent")),
                    };
                    let (root, root_actor, parent_work) =
                        fresh_bash_parent(state_root, &lane, &peer, bash_image.as_ref())?;
                    let child = if operation == b'C' && !instance.is_closed() {
                        lane.admit_bash_child(
                            &request_id,
                            &root,
                            &root_actor,
                            &recipient,
                            parent_work.grant_id(),
                            parent_work.work_id(),
                        )
                        .map_err(io::Error::other)?
                    } else {
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
                    match operation {
                        b'C' | b'c' => {
                            if operation == b'C' {
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
                                let plan = fixed_private_bash_child_plan()?;
                                fresh_provider::select_private_child_work(
                                    &state_root.join("v30/fresh-provider"),
                                    &child,
                                    &binding,
                                    &plan,
                                )?;
                            }
                            Ok(format!(
                                "fresh-bash-child {}\n",
                                serde_json::to_string(&child)?
                            ))
                        }
                        b'E' => {
                            let grant = lane
                                .admit_private_bash_work(&child)
                                .map_err(io::Error::other)?;
                            Ok(format!("fresh-bash-work {grant}\n"))
                        }
                        b'O' => {
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
                        b'%' | b'&' | b'!' => {
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
                            let directory = state_root.join("v30/fresh-provider");
                            if operation == b'%' {
                                let plan = fixed_private_bash_child_plan()?;
                                fresh_provider::require_admitted_child_work_plan(
                                    &directory, &child, &binding, &plan,
                                )?;
                                let prepared = fresh_provider::prepare(&directory, binding, plan)?;
                                let grant = fresh_provider::launch(
                                    prepared, &root_init, &actor, peer.uid, peer.gid,
                                )?;
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
                    if bridge_released_handoff(
                        bridge,
                        spec,
                        peer,
                        lane.identity(),
                        true,
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
                #[cfg(feature = "age319-private-broker-fixture")]
                b'5' | b'6' | b'7' | b'8' | b'9' | b'h' | b'f' | b'm' | b'n' => {
                    if !private_fixture() {
                        return Err(io::Error::other("fresh provider fixture route closed"));
                    }
                    let (d_key, route_request, effect_request, descriptors) = match payload {
                        RequestPayload::FreshProviderRequest {
                            request,
                            descriptors,
                        } if request.success.is_none() => (request.d_key, None, None, descriptors),
                        RequestPayload::FreshRouteRequest {
                            request,
                            descriptors,
                        } => (request.d_key.clone(), Some(request), None, descriptors),
                        RequestPayload::FreshAccountEffectRequest { request } => {
                            (request.d_key.clone(), None, Some(request), Vec::new())
                        }
                        _ => return Err(io::Error::other("fresh provider/route request absent")),
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
                        if operation == b'h' {
                            if instance.is_closed() {
                                return Err(io::Error::other("fresh route entry gate closed"));
                            }
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
                        let selection =
                            fresh_provider::select_route(&directory, &binding, &route_request)?;
                        return Ok(format!(
                            "fresh-route-selected {}\n",
                            serde_json::to_string(&selection)?
                        ));
                    }
                    if let Some(effect_request) = effect_request {
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
                        let effect = if operation == b'm' {
                            fresh_provider::begin_account_effect(
                                &directory,
                                &binding,
                                &effect_request,
                                &root,
                                &actor,
                                actor_uid,
                                actor_gid,
                            )?
                        } else {
                            fresh_provider::observe_account_effect(
                                &directory,
                                &binding,
                                &effect_request,
                            )?
                        };
                        if operation == b'm'
                            && std::env::var_os(
                                "OULIPOLY_KERNEL_BROKER_FIXTURE_DROP_ACCOUNT_EFFECT_REPLY_V1",
                            )
                            .is_some()
                        {
                            drop_account_effect_reply = true;
                        }
                        return Ok(format!(
                            "fresh-account-effect {}\n",
                            serde_json::to_string(&effect)?
                        ));
                    }
                    if operation == b'5' {
                        if instance.is_closed() {
                            return Err(io::Error::other("fresh provider K entry gate closed"));
                        }
                        let [image_fd, cwd, input, recipe]: [File; 4] = descriptors
                            .try_into()
                            .map_err(|_| io::Error::other("fresh provider descriptors absent"))?;
                        let image =
                            fs::read_link(format!("/proc/self/fd/{}", image_fd.as_raw_fd()))?;
                        let plan = fresh_provider::plan_from_descriptors(
                            &image, image_fd, cwd, input, recipe,
                        )?;
                        fresh_provider::require_selected_plan(&directory, &binding, &plan)?;
                        let prepared = fresh_provider::prepare(&directory, binding, plan)?;
                        let grant =
                            fresh_provider::launch(prepared, &root, &actor, actor_uid, actor_gid)?;
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
                    if operation == b'7' {
                        fresh_provider::cancel(&directory, &grant)?;
                        return Ok(format!("fresh-provider-cancel {grant}\n"));
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
                                provider_output_files = Some([stdout, stderr]);
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
                            submitted_grant = Some(recovered.readback.grant_id.clone());
                            fresh_payload_reply("recovered_delivery", recovered)?
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
        if drop_provider_k_reply || drop_provider_q_reply || drop_account_effect_reply {
            if let Some(gate) = std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_GATE_DIR_V1") {
                let marker = if drop_provider_k_reply {
                    "provider-k-reply-dropped"
                } else if drop_provider_q_reply {
                    "provider-q-reply-dropped"
                } else {
                    "account-effect-reply-dropped"
                };
                fs::write(Path::new(&gate).join(marker), b"yes")?;
            }
            continue;
        }
        let response = answer.unwrap_or_else(|error| format!("error {error}\n"));
        #[cfg(feature = "age319-private-broker-fixture")]
        if let Some(files) = provider_output_files {
            let fds = [files[0].as_raw_fd(), files[1].as_raw_fd()];
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
                unsafe { libc::CMSG_SPACE(std::mem::size_of_val(&fds) as _) } as usize;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&msg);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&fds) as _) as usize;
                std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(header).cast(), 2);
                libc::sendmsg(stream.as_raw_fd(), &msg, libc::MSG_NOSIGNAL);
            }
            continue;
        }
        if stream.write_all(response.as_bytes()).is_ok() {
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
        [_] => serve(),
        [_, mode] if mode == "--initialize-fresh-v30" => {
            FreshV30Lane::initialize_at(Path::new(STATE))
                .map(|identity| {
                    println!("{}", serde_json::to_string(&identity).unwrap());
                })
                .map_err(io::Error::other)
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
