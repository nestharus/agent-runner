//! Private joined completed-v2 wake. The Broker starts the process in the
//! original pinned root PID namespace; neither a UUID nor a caller-selected
//! process can claim the original listener. Product routing remains closed.
#![cfg(feature = "age319-private-broker-fixture")]

use oulipoly_kernel_broker::entry_registry::ProcessStamp;
use oulipoly_kernel_broker::identity::{PinnedProcess, host_proc_file};
use oulipoly_kernel_broker::registry::RootRegistry;
use oulipoly_kernel_broker::source_acceptance::read_v2_recipient_custody;
use oulipoly_kernel_broker::source_physical::SourcePhysicalRegistry;
use oulipoly_state::completion_continuation::sha256;
use oulipoly_state::mailbox::{BrokerSidecar, BrokerV2RecipientBinding};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

const MAX_PAYLOAD_FRAME: usize = 48 * 1024 * 1024;

mod base64_payload {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivery {
    grant_id: String,
    delivery_token: String,
    #[serde(with = "base64_payload")]
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplicitAck {
    grant_id: String,
    delivery_token: String,
    payload_sha256: String,
}

fn io_error(error: String) -> io::Error {
    io::Error::other(error)
}

fn credential_message(mut stream: &UnixStream, limit: usize) -> io::Result<(Vec<u8>, libc::ucred)> {
    let mut body = vec![0u8; limit];
    let mut control = [0u8; 128];
    let mut iov = libc::iovec {
        iov_base: body.as_mut_ptr().cast(),
        iov_len: body.len(),
    };
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = control.len();
    let n = unsafe { libc::recvmsg(stream.as_raw_fd(), &mut msg, 0) };
    if n <= 0 || msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(io::Error::other(
            "v2 wake credential message absent or truncated",
        ));
    }
    let mut credentials = None;
    let mut header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    while !header.is_null() {
        if unsafe { (*header).cmsg_level } == libc::SOL_SOCKET
            && unsafe { (*header).cmsg_type } == libc::SCM_CREDENTIALS
            && unsafe { (*header).cmsg_len }
                >= unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as _) } as usize
        {
            credentials = Some(unsafe { *libc::CMSG_DATA(header).cast::<libc::ucred>() });
        }
        header = unsafe { libc::CMSG_NXTHDR(&msg, header) };
    }
    body.truncate(n as usize);
    while !body.ends_with(b"\n") {
        if body.len() >= limit {
            return Err(io::Error::other("v2 wake message exceeds bound"));
        }
        let mut next = [0u8; 1];
        stream.read_exact(&mut next)?;
        body.push(next[0]);
    }
    Ok((
        body,
        credentials.ok_or_else(|| io::Error::other("v2 wake sender credentials absent"))?,
    ))
}

fn inheritable(file: &impl AsRawFd) -> io::Result<File> {
    let fd = unsafe { libc::dup(file.as_raw_fd()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// This helper is a fresh broker image process. setns affects only children;
/// the second fork is the actual separately running wake in the original
/// root's PID namespace. The caller's inherited socket is its sole launch
/// channel; it cannot choose source/session/row authority.
pub fn launcher(namespace_fd: RawFd, socket_fd: RawFd) -> io::Result<()> {
    if unsafe { libc::setns(namespace_fd, libc::CLONE_NEWPID) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid != 0 {
        return Ok(());
    }
    unsafe {
        libc::close(namespace_fd);
    }
    let socket = unsafe { UnixStream::from_raw_fd(socket_fd) };
    let code = if recipient(socket).is_ok() { 0 } else { 70 };
    unsafe { libc::_exit(code) }
}

fn recipient(mut socket: UnixStream) -> io::Result<()> {
    socket.set_read_timeout(Some(Duration::from_secs(15)))?;
    socket.set_write_timeout(Some(Duration::from_secs(15)))?;
    socket.write_all(b"v2-wake-ready\n")?;
    let mut size = [0u8; 4];
    socket.read_exact(&mut size)?;
    let len = u32::from_be_bytes(size) as usize;
    if len == 0 || len > MAX_PAYLOAD_FRAME {
        return Err(io::Error::other("v2 wake payload frame out of bounds"));
    }
    let mut bytes = vec![0; len];
    socket.read_exact(&mut bytes)?;
    let delivery: Delivery = serde_json::from_slice(&bytes)?;
    let mut reply = serde_json::to_vec(&ExplicitAck {
        grant_id: delivery.grant_id,
        delivery_token: delivery.delivery_token,
        payload_sha256: sha256(&delivery.payload),
    })?;
    reply.push(b'\n');
    if reply.len() > 512 {
        return Err(io::Error::other("v2 wake ACK too large"));
    }
    socket.write_all(&reply)?;
    // ACK is an explicit recipient response. The Broker alone decides whether
    // it matches the grant; a transport write cannot mark the mailbox row.
    let mut settled = [0u8; 1];
    socket.read_exact(&mut settled)?;
    if settled != [b'K'] {
        return Err(io::Error::other("v2 wake settlement receipt changed"));
    }
    Ok(())
}

pub fn activate(
    sidecar: &mut BrokerSidecar,
    physical: &SourcePhysicalRegistry,
    roots: &RootRegistry,
    source_grant_id: &str,
) -> io::Result<()> {
    if sidecar
        .read_v2_recipient_grant(source_grant_id)
        .map_err(io_error)?
        .is_some()
    {
        // A prior reserved/unknown/submitted/acked grant is permanent debt or
        // settlement. Never spawn or transmit again after restart/lost reply.
        return Ok(());
    }
    let custody =
        read_v2_recipient_custody(sidecar, physical, source_grant_id).map_err(io_error)?;
    let record = physical
        .records()
        .iter()
        .find(|r| r.grant.grant_id == source_grant_id)
        .ok_or_else(|| io::Error::other("v2 source physical record absent"))?;
    let root = roots
        .live_roots()
        .find(|r| r.record.root_id == custody.root_id)
        .ok_or_else(|| io::Error::other("original v2 root PID1 absent"))?;
    root.init.verify()?;
    if ProcessStamp::from(&root.init) != record.root_init
        || root.record.owner_uid != 0
        || root.record.pidns_dev != record.root_init.pidns_dev
        || root.record.pidns_ino != record.root_init.pidns_ino
    {
        return Err(io::Error::other("original v2 root tree changed"));
    }
    let (mut broker_socket, child_socket) = UnixStream::pair()?;
    let on: libc::c_int = 1;
    if unsafe {
        libc::setsockopt(
            broker_socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&on as *const libc::c_int).cast(),
            std::mem::size_of_val(&on) as _,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    broker_socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    broker_socket.set_write_timeout(Some(Duration::from_secs(10)))?;
    let namespace = inheritable(root.init.namespace())?;
    let endpoint = inheritable(&child_socket)?;
    let mut launcher = Command::new("/proc/self/exe")
        .arg("--age319-v2-wake-launcher")
        .arg(namespace.as_raw_fd().to_string())
        .arg(endpoint.as_raw_fd().to_string())
        .spawn()?;
    drop(endpoint);
    drop(namespace);
    drop(child_socket);
    let (ready, cred) = credential_message(&broker_socket, 64)?;
    if ready != b"v2-wake-ready\n" || cred.pid <= 0 || cred.uid != 0 {
        return Err(io::Error::other("v2 wake ready actor changed"));
    }
    let recipient = PinnedProcess::open(cred.pid)?;
    let own_image = host_proc_file("self/exe")?;
    if !recipient.in_namespace(root.init.namespace())?
        || recipient.is_namespace_init()?
        || !recipient.same_executable_as(&own_image)?
        || ProcessStamp::from(&recipient) == record.joined_child
    {
        return Err(io::Error::other(
            "v2 wake is not a broker-launched root successor",
        ));
    }
    // The intermediary has no source authority or socket after the fork.
    let status = launcher.wait()?;
    if !status.success() {
        return Err(io::Error::other("v2 wake launcher failed"));
    }
    // setns(2) changes the PID namespace of the forked recipient, but the
    // helper remains its host parent until exit. Linux may reparent this
    // cross-namespace orphan outside the target namespace, so PPID is not a
    // root-tree proof. The pinned root PID1 namespace plus the inherited
    // private launch channel and sender credentials are the admission proof.
    recipient.verify()?;
    let binding = BrokerV2RecipientBinding {
        source_grant_id: custody.source_grant_id,
        source_id: custody.source_id,
        registration_id: custody.registration_id,
        source_generation: custody.source_generation,
        root_id: custody.root_id,
        owner_generation: custody.owner_generation,
        listener_id: custody.listener_id,
        session_id: custody.session_id,
        owner_invocation_uuid: custody.owner_invocation_uuid,
        row_seq: custody.row_seq,
        payload_sha256: custody.payload_sha256,
        payload_byte_len: custody.payload_byte_len,
    };
    let root_stamp = serde_json::to_string(&ProcessStamp::from(&root.init))?;
    let recipient_stamp = serde_json::to_string(&ProcessStamp::from(&recipient))?;
    let grant = sidecar
        .reserve_v2_recipient_grant(&binding, &root_stamp, &recipient_stamp)
        .map_err(io_error)?;
    let payload = sidecar.v2_recipient_payload(&grant).map_err(io_error)?;
    let frame = serde_json::to_vec(&Delivery {
        grant_id: grant.grant_id.clone(),
        delivery_token: grant.delivery_token.clone(),
        payload,
    })?;
    if frame.len() > MAX_PAYLOAD_FRAME {
        return Err(io::Error::other("v2 wake frame too large"));
    }
    recipient.verify()?;
    root.init.verify()?;
    sidecar.begin_v2_recipient_send(&grant).map_err(io_error)?;
    if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_WRITE_V1").is_some() {
        return Err(io::Error::other(
            "private v2 socket write intentionally lost",
        ));
    }
    broker_socket.write_all(&(frame.len() as u32).to_be_bytes())?;
    broker_socket.write_all(&frame)?;
    recipient.verify()?;
    sidecar
        .mark_v2_recipient_submitted(source_grant_id, &recipient_stamp)
        .map_err(io_error)?;
    if std::env::var_os("OULIPOLY_KERNEL_BROKER_FIXTURE_V2_WAKE_DROP_ACK_V1").is_some() {
        return Err(io::Error::other("private v2 ACK reply intentionally lost"));
    }
    let (response, ack_cred) = credential_message(&broker_socket, 512)?;
    recipient.verify()?;
    root.init.verify()?;
    if ack_cred.pid != recipient.host_pid || ack_cred.uid != 0 {
        return Err(io::Error::other("v2 ACK sender changed"));
    }
    let ack: ExplicitAck = serde_json::from_slice(&response)?;
    if ack.grant_id != grant.grant_id || ack.payload_sha256 != binding.payload_sha256 {
        return Err(io::Error::other("v2 ACK grant or payload differs"));
    }
    sidecar
        .acknowledge_v2_recipient(
            source_grant_id,
            &recipient_stamp,
            &ack.delivery_token,
            &sha256(&response),
        )
        .map_err(io_error)?;
    broker_socket.write_all(b"K")?;
    Ok(())
}
