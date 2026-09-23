//! Opt-in host-root broker for the pinned guardian and one-use root child join.
#[path = "root_join.rs"]
mod root_join;
#[path = "work_launch.rs"]
mod work_launch;
use oulipoly_kernel_broker::accepted_grant::GrantRegistry;
use oulipoly_kernel_broker::entry_registry::{EntryRegistry, ProcessStamp};
use oulipoly_kernel_broker::identity::{
    PeerIdentity, PinnedProcess, host_proc_file, host_proc_uid, install_detached_host_proc,
};
use oulipoly_kernel_broker::protocol::{
    AcceptedWorkSpec, JoinSpec, JoinedChildWitness, LaunchAcceptedWorkSpec, NativePrepareSpec,
    OwnerWitness, ProcessWitness, SourceControlUse, SourceScope, SourceSocketWitness,
    SourceTicketUse,
};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::work_registry::{Scope, WorkRegistry, classify_scope};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

const SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";
const STATE: &str = "/var/lib/oulipoly-kernel-broker";
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

#[derive(Debug)]
enum RequestPayload {
    None,
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
    let mut request = [0u8; 64 * 1024];
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
        b'G' => read == 65,
        b'P' => read == 37,
        b'Q' | b'Z' => read == 33,
        b'A' => read == 33,
        b'J' => (18..=48 * 1024 + 17).contains(&read),
        b'V' | b'S' | b's' | b'T' | b'H' | b'K' | b'B' | b'N' => (18..=2048 + 17).contains(&read),
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
            b'J' | b'H' => descriptors.len() != 5,
            b'N' => descriptors.len() != 3,
            b'K' => descriptors.len() != 7,
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
        b'P' => RequestPayload::Prepare {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            guardian_pid: i32::from_ne_bytes(request[33..37].try_into().unwrap()),
        },
        b'G' => RequestPayload::Bind {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
            domain_id: uuid::Uuid::from_bytes(request[33..49].try_into().unwrap()).to_string(),
            supervisor_id: uuid::Uuid::from_bytes(request[49..65].try_into().unwrap()).to_string(),
        },
        b'A' => RequestPayload::Read {
            root_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'Q' => RequestPayload::ObserveAcceptedWork {
            grant_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'Z' => RequestPayload::CancelAcceptedWork {
            grant_id: uuid::Uuid::from_bytes(request[17..33].try_into().unwrap()).to_string(),
        },
        b'J' => RequestPayload::Join {
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
        b'K' => RequestPayload::LaunchAcceptedWork {
            spec: serde_json::from_slice(&request[17..read as usize])?,
            descriptors: descriptors
                .try_into()
                .map_err(|_| io::Error::other("accepted launch descriptors"))?,
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
    if roots.has_debt() || entries.has_debt() {
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
        || entry.domain_id.as_deref() != Some(&witness.domain_id)
        || entry.supervisor_authority_id.as_deref() != Some(&witness.supervisor_id)
    {
        return Err(io::Error::other("owner witness root binding changed"));
    }
    let joined_child = entry
        .joined_child
        .as_ref()
        .ok_or_else(|| io::Error::other("joined child absent"))?;
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
            || grant.owner_uid != peer.uid
            || grant.root_init != ProcessStamp::from(&root.init)
            || grant.joined_child != *joined_child
            || grant.supervisor_authority_id != witness.supervisor_id
            || witness.owner_generation.as_deref() != Some(&grant.owner_generation)
            || witness.registration_authority_sha256.as_deref()
                != Some(&helper.registration_authority_sha256)
            || witness.owner_session_id.as_deref() != Some(helper.owner_session_id.as_str())
            || witness.owner_invocation_uuid.as_deref()
                != Some(helper.owner_invocation_uuid.as_str())
            || !helper.matches_live_executable(&peer.process)?
        {
            return Err(io::Error::other(
                "owner helper grant, session, or image mismatch",
            ));
        }
        work.init.verify()?;
    }
    let guardian_stamp = entry
        .guardian
        .as_ref()
        .ok_or_else(|| io::Error::other("owner witness guardian absent"))?;
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
    tickets.retain(|_, ticket| ticket.created.elapsed() < Duration::from_secs(30));
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
    if ticket.created.elapsed() >= Duration::from_secs(30) {
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

fn serve() -> io::Result<()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::other("host root required"));
    }
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
    let runner_image = File::open(&runner)?;
    let works_path = Path::new(&state).join("works");
    if !works_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&works_path)?;
    }
    if !fixture {
        checked_root_path(&works_path, true)?;
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
    // Ephemeral by design: a broker restart invalidates every pre-wire source
    // decision. No work/cancel action can be authorized by a lost ticket.
    let mut source_tickets = BTreeMap::<String, SourceTicket>::new();
    let terminal_path = Path::new(&state).join("terminals");
    if !terminal_path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&terminal_path)?;
    }
    if !fixture {
        checked_root_path(&terminal_path, true)?;
    }
    if let Ok(meta) = fs::symlink_metadata(&socket) {
        if !meta.file_type().is_socket() || meta.uid() != 0 {
            return Err(io::Error::other("unsafe existing socket"));
        }
        fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o660))?;
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else { continue };
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        let result = peer_from_request(&mut stream).and_then(|(operation, payload, peer)| {
            // Request parsing is bounded. Root launch/recovery readiness is
            // governed by exact gates and process death, not a 5s cutoff.
            stream.set_read_timeout(None)?;
            stream.set_write_timeout(None)?;
            if operation == b'J' {
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
                root_join::launch(
                    spec,
                    descriptors,
                    &peer,
                    &runner_image,
                    &mut registry,
                    &mut entries,
                )
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
                if spec.protocol != "native-continuation-v1" {
                    return Err(io::Error::other("unsupported native prepare protocol"));
                }
                let [directory, request, receipt] = descriptors;
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
            } else {
                dispatch_authenticated(
                    operation,
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
        let _ = stream.write_all(response.as_bytes());
    }
    Ok(())
}

pub fn run() {
    if std::env::args_os().len() != 1 {
        eprintln!("no command-line options accepted");
        std::process::exit(2);
    }
    if let Err(error) = serve() {
        eprintln!("kernel broker: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::Command;
    use std::thread;

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
            assert!(matches!(payload, RequestPayload::None));
            assert_eq!(peer.process.host_pid, std::process::id() as i32);
        });
        let mut client = UnixStream::connect(&socket).unwrap();
        let mut challenge = [0u8; 16];
        client.read_exact(&mut challenge).unwrap();
        let mut message = [0u8; 17];
        message[0] = b'C';
        message[1..].copy_from_slice(&challenge);
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 17, 0) },
            17
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
        let message = [b'C'; 17];
        assert_eq!(
            unsafe { libc::send(client.as_raw_fd(), message.as_ptr().cast(), 17, 0) },
            17
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
            assert!(peer_from_request(&mut stream).is_err());
        });
        let script = "import os,socket,sys\ns=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);s.connect(sys.argv[1]);c=s.recv(16);p=os.fork()\nif p==0:\n s.sendall(b'C'+c);os._exit(0)\nos.waitpid(p,0)";
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
message = b'L' + challenge
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
        let mut request = [0u8; 17];
        request[0] = b'C';
        request[1..].copy_from_slice(&challenge);
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
            assert_eq!(libc::sendmsg(client.as_raw_fd(), &message, 0), 17);
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
}
