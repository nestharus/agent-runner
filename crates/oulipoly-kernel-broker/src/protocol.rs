//! One connection, one challenged request. Client supplies only an opcode;
//! executable, UID, PID, root ID, namespace and mount choices are absent.
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const INSTALLED_SOCKET: &str = "/run/oulipoly-kernel-broker/control.sock";

#[derive(Clone, Copy)]
pub enum Operation {
    Classify,
    LaunchFixedRunner,
}

pub fn request_at(path: &Path, operation: Operation) -> io::Result<String> {
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
    let mut request = [0u8; 17];
    request[0] = match operation {
        Operation::Classify => b'C',
        Operation::LaunchFixedRunner => b'L',
    };
    request[1..].copy_from_slice(&challenge);
    // One send yields one SCM_CREDENTIALS-bearing request message.
    let written = unsafe {
        libc::send(
            stream.as_raw_fd(),
            request.as_ptr().cast(),
            request.len(),
            libc::MSG_NOSIGNAL,
        )
    };
    if written != request.len() as isize {
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
