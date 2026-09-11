//! Proxy-free provider commands. C owns W before execution, keeps W's zombie
//! until its group can no longer be signalled, and alone consumes every wait.
//! The creator owns C and a private status/control socket, never W's numeric PID.
use super::*;
use std::os::unix::process::ExitStatusExt;
use std::sync::Mutex;

pub struct RemoteStatus(Mutex<Channel>);
struct Channel {
    fd: OwnedFd,
    status: Option<i32>,
    // Exactly one request may be outstanding. A timeout retains its slot, not
    // a permanent cancellation veto. Drain its reply before sending another.
    pending_signal: bool,
}
impl Channel {
    fn response(&mut self, deadline: std::time::Instant) -> io::Result<Option<bool>> {
        match receive_until(self.fd.as_raw_fd(), deadline)? {
            [kind, status] if kind == b'X' as i32 => {
                self.status = Some(status);
                self.pending_signal = false;
                Ok(None)
            }
            [kind, errno] if kind == b'A' as i32 && self.pending_signal => {
                self.pending_signal = false;
                match errno {
                    0 => Ok(Some(true)),
                    libc::ESRCH => Ok(Some(false)),
                    _ => Err(io::Error::from_raw_os_error(errno)),
                }
            }
            _ => Err(io::Error::other("invalid command owner response")),
        }
    }
}
impl RemoteStatus {
    pub fn wait_status(&self) -> io::Result<std::process::ExitStatus> {
        let mut channel = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while channel.status.is_none() {
            channel.response(deadline)?;
        }
        Ok(std::process::ExitStatus::from_raw(channel.status.unwrap()))
    }

    /// Serialized, bounded requests. A late reply belongs only to the retained
    /// request; it can never certify a later KILL. Final status is a no-op,
    /// never an inference that a signal syscall killed the whole tree.
    pub fn signal(&self, signal: i32) -> io::Result<bool> {
        if ![libc::SIGTERM, libc::SIGKILL].contains(&signal) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported command signal",
            ));
        }
        let mut channel = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        if channel.pending_signal {
            // Discard the OLD syscall result, including its error. A valid
            // response frees the slot; a transport error leaves it outstanding.
            let old = channel.response(deadline);
            if channel.pending_signal {
                old?;
            }
        }
        if channel.status.is_some() {
            return Ok(false);
        }
        let sent = unsafe { packet(channel.fd.as_raw_fd(), [b'S' as i32, signal]) };
        if sent {
            channel.pending_signal = true;
        }
        // Even if sending races owner completion, consume the queued X.
        let response = channel.response(deadline)?;
        Ok(response.unwrap_or(false))
    }
}
fn receive_until(fd: RawFd, deadline: std::time::Instant) -> io::Result<[i32; 2]> {
    let mut message = [0i32; 2];
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "command custodian response timeout",
            ));
        }
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().max(1) as i32) };
        if ready < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        if ready <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "command custodian response unavailable",
            ));
        }
        let rc = unsafe { libc::recv(fd, message.as_mut_ptr().cast(), 8, libc::MSG_DONTWAIT) };
        if rc == 8 {
            return Ok(message);
        }
        if rc < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        return Err(io::Error::other("command custodian lost before completion"));
    }
}
unsafe fn packet(fd: RawFd, message: [i32; 2]) -> bool {
    loop {
        let rc = unsafe {
            libc::send(
                fd,
                message.as_ptr().cast(),
                8,
                libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
            )
        };
        if rc == 8 {
            return true;
        }
        if rc < 0 && unsafe { *libc::__errno_location() } == libc::EINTR {
            continue;
        }
        return false;
    }
}

pub(in crate::launch_custody) fn configure(
    custody: &LaunchCustody,
    command: &mut Command,
) -> io::Result<RemoteStatus> {
    let endpoint = custody.endpoint.lock().unwrap_or_else(|e| e.into_inner());
    let inherited = Arc::clone(
        endpoint
            .as_ref()
            .ok_or_else(|| io::Error::other("launch custody sealed"))?,
    );
    let mut sockets = [-1; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            sockets.as_mut_ptr(),
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let client = unsafe { OwnedFd::from_raw_fd(sockets[0]) };
    let server = unsafe { OwnedFd::from_raw_fd(sockets[1]) };
    unsafe {
        command.pre_exec(move || {
            let fd = inherited.as_raw_fd();
            if libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0) != 0
                || libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0
                || libc::setpgid(0, 0) != 0
            {
                return Err(io::Error::last_os_error());
            }
            reset_handlers();
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            if !send(fd, b'B') {
                return Err(io::Error::from_raw_os_error(libc::EIO));
            }
            let workload = fork_process();
            if workload < 0 {
                send(fd, b'D');
                return Err(io::Error::last_os_error());
            }
            if workload == 0 {
                libc::close(fd);
                libc::close(server.as_raw_fd());
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            own_tree(fd, server.as_raw_fd(), workload);
        });
    }
    Ok(RemoteStatus(Mutex::new(Channel {
        fd: client,
        status: None,
        pending_signal: false,
    })))
}

// Reads direct children from the kernel, not historical descendant attribution.
// Streaming parsing has no fixed child-count limit and uses no post-fork allocator.
// W is retained as a zombie to pin its group through every accepted signal.
unsafe fn reap_others(root: i32) -> Result<(bool, bool), ()> {
    unsafe {
        let fd = libc::open(
            c"/proc/thread-self/children".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        );
        if fd < 0 {
            return Err(());
        }
        let mut buffer = [0u8; 1024];
        let mut pid = 0i32;
        let mut other = false;
        let mut reaped = false;
        loop {
            let n = libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len());
            if n < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            if n < 0 {
                libc::close(fd);
                return Err(());
            }
            if n == 0 {
                break;
            }
            for &byte in &buffer[..n as usize] {
                if byte.is_ascii_digit() {
                    let Some(next) = pid
                        .checked_mul(10)
                        .and_then(|p| p.checked_add((byte - b'0') as i32))
                    else {
                        libc::close(fd);
                        return Err(());
                    };
                    pid = next;
                } else if pid != 0 {
                    if pid != root {
                        other = true;
                        let mut status = 0;
                        let rc = libc::waitpid(pid, &mut status, libc::WNOHANG);
                        reaped |= rc > 0;
                        if rc < 0 && *libc::__errno_location() != libc::EINTR {
                            libc::close(fd);
                            return Err(());
                        }
                    }
                    pid = 0;
                }
            }
        }
        libc::close(fd);
        if pid != 0 {
            return Err(());
        }
        Ok((other, reaped))
    }
}

unsafe fn own_tree(launch: RawFd, control: RawFd, root: i32) -> ! {
    unsafe {
        close_except(launch, control);
        let mut mask: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut mask);
        libc::sigaddset(&mut mask, libc::SIGCHLD);
        if libc::sigprocmask(libc::SIG_BLOCK, &mask, std::ptr::null_mut()) != 0 {
            libc::_exit(125);
        }
        let events = libc::signalfd(-1, &mask, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if events < 0 {
            libc::_exit(125);
        }
        let mut connected = true;
        loop {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            let rc = libc::waitid(
                libc::P_PID,
                root as u32,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            );
            if rc < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                libc::_exit(125);
            }
            let root_exited = info.assume_init().si_pid() != 0;
            match reap_others(root) {
                Ok((false, _)) if root_exited => {
                    let mut status = 0;
                    if libc::waitpid(root, &mut status, 0) != root {
                        libc::_exit(125);
                    }
                    let mut extra = 0;
                    // Never infer tree cessation from the /proc enumeration.
                    if libc::waitpid(-1, &mut extra, libc::WNOHANG) != -1
                        || *libc::__errno_location() != libc::ECHILD
                    {
                        libc::_exit(125);
                    }
                    if !send(launch, b'D') {
                        libc::_exit(125);
                    }
                    packet(control, [b'X' as i32, status]);
                    libc::_exit(0);
                }
                Ok((_, true)) => continue,
                Err(()) => libc::_exit(125),
                _ => (),
            }
            let mut polls = [
                libc::pollfd {
                    fd: if connected { control } else { -1 },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: events,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            if libc::poll(polls.as_mut_ptr(), 2, -1) <= 0 {
                continue;
            }
            if polls[1].revents != 0 {
                let mut event: libc::signalfd_siginfo = std::mem::zeroed();
                libc::read(
                    events,
                    (&mut event as *mut libc::signalfd_siginfo).cast(),
                    std::mem::size_of_val(&event),
                );
            }
            if polls[0].revents == 0 {
                continue;
            }
            let mut message = [0i32; 2];
            let n = libc::recv(control, message.as_mut_ptr().cast(), 8, libc::MSG_DONTWAIT);
            if n == 0 {
                connected = false;
                continue;
            }
            if n < 0 {
                continue;
            }
            if n != 8
                || message[0] != b'S' as i32
                || ![libc::SIGTERM, libc::SIGKILL].contains(&message[1])
            {
                connected = false;
                continue;
            }
            // Root remains our unreaped child even when already exited; its PID
            // cannot name an unrelated group. C is outside this kill group.
            let rc = libc::kill(-root, message[1]);
            let errno = if rc == 0 {
                0
            } else {
                *libc::__errno_location()
            };
            packet(control, [b'A' as i32, errno]);
        }
    }
}
