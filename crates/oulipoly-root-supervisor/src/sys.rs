//! Linux primitives shared by the owner and by root PID1: pidfds, exact
//! process identity, `SOCK_SEQPACKET` messages carrying descriptors, and
//! the fork-like `clone` that starts root PID1 in a new PID namespace.
//! Every descriptor created here is close-on-exec.

use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Largest custody message accepted. Larger ones are a protocol error.
pub const MAX_MESSAGE: usize = 1 << 16;
/// Most descriptors carried by one custody message.
const MAX_FDS: usize = 4;

fn check(rc: libc::c_long) -> io::Result<libc::c_long> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

fn owned(fd: libc::c_long) -> io::Result<OwnedFd> {
    let fd = i32::try_from(fd).map_err(|_| io::Error::from_raw_os_error(libc::EBADF))?;
    // SAFETY: the kernel just returned this descriptor and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens a pidfd for `pid` as seen from the caller's PID namespace.
pub fn pidfd_open(pid: i32) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
    owned(check(unsafe {
        libc::syscall(libc::SYS_pidfd_open, pid, 0)
    })?)
}

/// Sends `SIGKILL` to the exact process named by `fd`.
pub fn pidfd_kill(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: pidfd_send_signal on an fd we own, with no siginfo.
    check(unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    })
    .map(drop)
}

/// Whether the process named by `fd` has exited, waiting at most
/// `timeout_ms` (`-1`: no bound). Exit seen this way says nothing about
/// the exit status, which only the process's parent can collect.
pub fn pidfd_exited(fd: &OwnedFd, timeout_ms: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: poll receives one valid pollfd.
        let ready = unsafe { libc::poll(&mut poll, 1, timeout_ms) };
        if ready >= 0 {
            return Ok(ready == 1);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// The pid named by a pidfd in the PID namespace of the mounted `/proc`
/// (the host's, for the owner, root PID 1 and work PID 1s; only a work's
/// own processes see a `/proc` of their own).
pub fn pidfd_host_pid(fd: &OwnedFd) -> io::Result<i32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", fd.as_raw_fd()))?;
    info.lines()
        .find_map(|line| line.strip_prefix("Pid:"))
        .and_then(|pid| pid.trim().parse().ok())
        .ok_or_else(|| io::Error::other("pidfd fdinfo has no Pid"))
}

/// Field 22 of `/proc/<pid>/stat`: the process start time in clock ticks
/// since boot. With the boot id it names one process incarnation.
pub fn start_time(pid: i32) -> io::Result<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let after = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .ok_or_else(|| io::Error::other("malformed stat"))?;
    // Fields after the command start at field 3; start time is field 22.
    after
        .split_whitespace()
        .nth(19)
        .and_then(|field| field.parse().ok())
        .ok_or_else(|| io::Error::other("stat has no start time"))
}

pub fn boot_id() -> io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}

/// 128 random bits, hex encoded.
pub fn random_hex() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 writes two descriptors into the array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned only here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

pub fn seqpacket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: socketpair writes two descriptors into the array.
    let rc = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned only here.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// A socket path reached through an open directory descriptor, so the
/// path stays short however deep the store directory is.
pub fn path_in(dir: &OwnedFd, name: &str) -> String {
    format!("/proc/self/fd/{}/{name}", dir.as_raw_fd())
}

fn sockaddr(path: &str) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: an all-zero sockaddr_un is valid.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    let bytes = path.as_bytes();
    if bytes.len() >= addr.sun_path.len() {
        return Err(io::Error::from_raw_os_error(libc::ENAMETOOLONG));
    }
    for (slot, byte) in addr.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + bytes.len() + 1;
    Ok((addr, len as libc::socklen_t))
}

fn seqpacket_socket() -> io::Result<OwnedFd> {
    // SAFETY: socket returns a new descriptor or -1.
    owned(check(libc::c_long::from(unsafe {
        libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0)
    }))?)
}

pub fn listen(path: &str) -> io::Result<OwnedFd> {
    let socket = seqpacket_socket()?;
    let (addr, len) = sockaddr(path)?;
    // SAFETY: bind/listen on a socket we own with a valid address.
    unsafe {
        if libc::bind(socket.as_raw_fd(), (&raw const addr).cast(), len) != 0
            || libc::listen(socket.as_raw_fd(), 8) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(socket)
}

pub fn connect(path: &str) -> io::Result<OwnedFd> {
    let socket = seqpacket_socket()?;
    let (addr, len) = sockaddr(path)?;
    // SAFETY: connect on a socket we own with a valid address.
    if unsafe { libc::connect(socket.as_raw_fd(), (&raw const addr).cast(), len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(socket)
}

pub fn accept(listener: &OwnedFd) -> io::Result<OwnedFd> {
    // SAFETY: accept4 on a listening socket we own; no peer address wanted.
    owned(check(libc::c_long::from(unsafe {
        libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_CLOEXEC,
        )
    }))?)
}

/// The connected peer's credentials, translated into the caller's
/// namespaces by the kernel.
pub fn peer_cred(socket: &OwnedFd) -> io::Result<libc::ucred> {
    // SAFETY: an all-zero ucred is valid; getsockopt fills it.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: SO_PEERCRED writes one ucred into a buffer of that size.
    let rc = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

/// Sends one message with `fds` attached. Never raises `SIGPIPE`.
pub fn send(socket: &OwnedFd, message: &[u8], fds: &[RawFd]) -> io::Result<()> {
    assert!(fds.len() <= MAX_FDS);
    let mut iov = libc::iovec {
        iov_base: message.as_ptr().cast_mut().cast(),
        iov_len: message.len(),
    };
    let mut control = [0u64; 8];
    // SAFETY: an all-zero msghdr is valid.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    if !fds.is_empty() {
        let bytes = std::mem::size_of_val(fds) as u32;
        // SAFETY: CMSG_SPACE/LEN are pure size computations.
        let (space, len) = unsafe { (libc::CMSG_SPACE(bytes), libc::CMSG_LEN(bytes)) };
        header.msg_control = control.as_mut_ptr().cast();
        header.msg_controllen = space as usize;
        // SAFETY: the control buffer is large enough for one SCM_RIGHTS
        // header carrying at most MAX_FDS descriptors.
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&raw const header);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = len as usize;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(cmsg).cast(), fds.len());
        }
    }
    loop {
        // SAFETY: sendmsg reads the header, iovec and control buffer above.
        let sent =
            unsafe { libc::sendmsg(socket.as_raw_fd(), &raw const header, libc::MSG_NOSIGNAL) };
        if sent >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// Receives one message and any attached descriptors. `None` is end of
/// stream (the peer closed).
pub fn recv(socket: &OwnedFd) -> io::Result<Option<(Vec<u8>, Vec<OwnedFd>)>> {
    let mut buffer = vec![0u8; MAX_MESSAGE];
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    let mut control = [0u64; 8];
    // SAFETY: an all-zero msghdr is valid.
    let mut header: libc::msghdr = unsafe { std::mem::zeroed() };
    header.msg_iov = &raw mut iov;
    header.msg_iovlen = 1;
    header.msg_control = control.as_mut_ptr().cast();
    header.msg_controllen = std::mem::size_of_val(&control);
    let received = loop {
        // SAFETY: recvmsg writes into the buffers described by the header.
        let received =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &raw mut header, libc::MSG_CMSG_CLOEXEC) };
        if received >= 0 {
            break received as usize;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    };
    let mut fds = Vec::new();
    // SAFETY: walk the control messages the kernel just wrote.
    unsafe {
        let mut cmsg = libc::CMSG_FIRSTHDR(&raw const header);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(cmsg).cast::<RawFd>();
                let count =
                    ((*cmsg).cmsg_len - libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                for index in 0..count {
                    fds.push(OwnedFd::from_raw_fd(data.add(index).read_unaligned()));
                }
            }
            cmsg = libc::CMSG_NXTHDR(&raw const header, cmsg);
        }
    }
    if header.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return Err(io::Error::other("custody message truncated"));
    }
    if received == 0 && fds.is_empty() {
        return Ok(None);
    }
    buffer.truncate(received);
    Ok(Some((buffer, fds)))
}

/// How the owner isolates a new root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Host root: a new PID namespace in the host user namespace.
    HostRootPidns,
    /// Unprivileged: a new user namespace mapping only the caller's uid and
    /// gid to 0, and a new PID namespace inside it. It does not stand for
    /// host-root semantics.
    UnprivilegedUserns,
}

/// Chosen from the root's declared work identity (`workload` module),
/// never from the caller's euid.
impl Isolation {
    pub fn label(self) -> &'static str {
        match self {
            Self::HostRootPidns => "host-root-pidns",
            Self::UnprivilegedUserns => "unprivileged-userns-pidns",
        }
    }
}

/// Starts `program` as PID 1 of a new PID namespace, as a direct child of
/// the caller, with `channel` as its descriptor 3, stdin/stdout on
/// `/dev/null` and stderr inherited. Returns the child's host pid.
///
/// No parent-death signal is set: the new PID 1 outlives the caller.
pub fn spawn_pid1(program: &Path, channel: &OwnedFd, isolation: Isolation) -> io::Result<i32> {
    let path = CString::new(program.as_os_str().as_bytes())?;
    let argv = [path.as_ptr(), std::ptr::null()];
    let null = File::options().read(true).write(true).open("/dev/null")?;
    // SAFETY: getuid/getgid have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    // Everything the child touches is prepared here: after clone it may
    // only make async-signal-safe calls on these buffers.
    let uid_map = format!("0 {uid} 1");
    let gid_map = format!("0 {gid} 1");
    let files: [(&[u8], &[u8]); 3] = [
        (b"/proc/self/setgroups\0", b"deny"),
        (b"/proc/self/uid_map\0", uid_map.as_bytes()),
        (b"/proc/self/gid_map\0", gid_map.as_bytes()),
    ];
    let mut flags = libc::CLONE_NEWPID;
    if isolation == Isolation::UnprivilegedUserns {
        flags |= libc::CLONE_NEWUSER;
    }
    let (channel, null) = (channel.as_raw_fd(), null.as_raw_fd());
    // SAFETY: fork-like clone (no new stack). The child runs only the
    // async-signal-safe calls below on memory prepared above, then execs
    // or exits.
    let pid = unsafe {
        libc::syscall(
            libc::SYS_clone,
            libc::c_long::from(flags | libc::SIGCHLD),
            0usize,
            0usize,
            0usize,
            0usize,
        )
    };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // SAFETY: in the child; see above.
        unsafe {
            if isolation == Isolation::UnprivilegedUserns {
                for (file, data) in files {
                    let fd = libc::open(file.as_ptr().cast(), libc::O_WRONLY);
                    if fd < 0
                        || libc::write(fd, data.as_ptr().cast(), data.len()) != data.len() as isize
                    {
                        libc::_exit(126);
                    }
                    libc::close(fd);
                }
            }
            if libc::dup2(null, 0) < 0 || libc::dup2(null, 1) < 0 {
                libc::_exit(126);
            }
            if channel == 3 {
                libc::fcntl(3, libc::F_SETFD, 0);
            } else if libc::dup2(channel, 3) < 0 {
                libc::_exit(126);
            }
            libc::syscall(libc::SYS_close_range, 4u32, u32::MAX, 0u32);
            libc::execv(path.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    i32::try_from(pid).map_err(|_| io::Error::other("pid out of range"))
}

/// Exact `waitpid` of a direct child, retried on `EINTR`.
pub fn wait_child(pid: i32) -> io::Result<i32> {
    let mut status = 0;
    loop {
        // SAFETY: waitpid writes the status of our own child.
        if unsafe { libc::waitpid(pid, &raw mut status, 0) } == pid {
            return Ok(status);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

/// `code:N` or `signal:N` for a raw wait status.
pub fn describe_status(status: i32) -> String {
    if libc::WIFEXITED(status) {
        format!("code:{}", libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        format!("signal:{}", libc::WTERMSIG(status))
    } else {
        format!("raw:{status}")
    }
}
