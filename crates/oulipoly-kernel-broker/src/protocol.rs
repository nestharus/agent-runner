//! One connection, one challenged request. The entry may supply only the
//! broker-issued root ID and its direct child's PID for the prepare step;
//! executable, UID, namespace, and mount choices are absent.
use std::io::{self, Read};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const INSTALLED_SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";

#[derive(Clone, Copy)]
pub enum Operation {
    Classify,
    ReserveEntry,
    ReadEntry,
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
    pub args: Vec<String>,
    pub environment: Vec<(String, String)>,
}

/// Host PID identities from the durable native owner. A root-namespace client
/// cannot interpret SO_PEERCRED's PID for its outside guardian; the host broker
/// checks these against pinned host processes and the connected owner socket.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerWitness {
    pub root_id: String,
    pub domain_id: String,
    pub supervisor_id: String,
    pub guardian: ProcessWitness,
    pub driver: ProcessWitness,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessWitness {
    pub host_pid: i32,
    pub boot_id: String,
    pub starttime_ticks: u64,
}

/// Only CLI surfaces that dispatch without native service bootstrap may enter
/// the root child until host/local PID handling is explicit throughout the
/// completion owner and provider launch path.
pub fn supported_entry_args(args: &[String]) -> bool {
    matches!(args, [only] if only == "--help" || only == "-h")
        || args.first().is_some_and(|first| first == "diagnostics")
}

/// Transfer stdin, stdout, stderr, the working directory, and a one-way
/// completion receipt socket held by the entry until the child exits.
/// A single challenged sendmsg keeps credentials, data and descriptors together.
pub fn join_at(path: &Path, spec: &JoinSpec, descriptors: [RawFd; 5]) -> io::Result<String> {
    let body = serde_json::to_vec(spec)?;
    if body.len() > 48 * 1024 {
        return Err(io::Error::other("join environment too large"));
    }
    let mut stream = checked_connection(path)?;
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = Vec::with_capacity(17 + body.len());
    request.push(b'J');
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
    let mut challenge = [0u8; 16];
    stream.read_exact(&mut challenge)?;
    let mut request = [0u8; 65];
    request[0] = match operation {
        Operation::Classify => b'C',
        Operation::ReserveEntry if matches!(payload, Payload::Prepare(..)) => b'P',
        Operation::ReserveEntry if matches!(payload, Payload::Bind(..)) => b'G',
        Operation::ReserveEntry => b'E',
        Operation::ReadEntry => b'A',
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
