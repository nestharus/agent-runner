//! Exact-process signalling for cancel. A pidfd taken while the child is
//! still unreaped names that process even after it is reaped, so a cancel
//! can never signal an unrelated process that reused the pid.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

pub(crate) fn open(pid: u32) -> io::Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: pidfd_open takes a pid and flags and returns a new fd or -1.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = i32::try_from(fd).map_err(|_| io::Error::from_raw_os_error(libc::EBADF))?;
    // SAFETY: the kernel just returned this fd and nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn kill(fd: &OwnedFd) -> bool {
    // SAFETY: pidfd_send_signal on an fd we own, with no siginfo.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            fd.as_raw_fd(),
            libc::SIGKILL,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    rc == 0
}

/// Live owned harness processes and whether this run has stopped, and why.
/// Spawning and stopping both hold this lock, so no harness can be
/// launched after a stop without being signalled.
#[derive(Default)]
pub(crate) struct Custody {
    stopped: Option<&'static str>,
    next: u64,
    live: HashMap<u64, OwnedFd>,
}

impl Custody {
    pub(crate) fn cancelled(&self) -> bool {
        self.stopped.is_some()
    }

    /// Store authority loss outranks store failure, which outranks cancel.
    pub(crate) fn reason(&self) -> Option<&'static str> {
        self.stopped
    }

    pub(crate) fn register(&mut self, fd: OwnedFd) -> u64 {
        self.next += 1;
        self.live.insert(self.next, fd);
        self.next
    }

    /// Called after the child has been reaped.
    pub(crate) fn release(&mut self, token: u64) {
        self.live.remove(&token);
    }

    /// Marks the run cancelled by the caller and sends `SIGKILL` to every
    /// live owned harness. Returns how many signals were delivered.
    pub(crate) fn cancel(&mut self) -> usize {
        self.stop("cancelled")
    }

    /// Stops the run, retaining the strongest observed store loss even if
    /// cancel arrived first, and sends `SIGKILL` to every live owned harness.
    pub(crate) fn stop(&mut self, reason: &'static str) -> usize {
        self.stopped = Some(match (self.stopped, reason) {
            (Some("authority-lost"), _) | (_, "authority-lost") => "authority-lost",
            (Some("store-failed"), _) | (_, "store-failed") => "store-failed",
            (Some(earlier), _) => earlier,
            (None, reason) => reason,
        });
        self.live.values().filter(|fd| kill(fd)).count()
    }
}
