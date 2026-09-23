//! One connection, one challenged request. The entry may supply only the
//! broker-issued root ID and its direct child's PID for the prepare step;
//! executable, UID, namespace, and mount choices are absent.
use std::io::{self, Read};
use std::os::fd::AsRawFd;
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
    let mut stream = UnixStream::connect(path)?;
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
    let mut response = Vec::new();
    stream.take(257).read_to_end(&mut response)?;
    if response.len() > 256 || !response.ends_with(b"\n") {
        return Err(io::Error::other("invalid broker response"));
    }
    String::from_utf8(response).map_err(|_| io::Error::other("non-UTF8 broker response"))
}

pub fn request(operation: Operation) -> io::Result<String> {
    request_at(Path::new(INSTALLED_SOCKET), operation)
}
