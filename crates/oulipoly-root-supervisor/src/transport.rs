//! Transport to one owned harness that records *why* it reported closed, so
//! a send fault is never mistaken for an observed end of stream. Over stdio
//! the pipe ends come from root PID 1, which keeps its own copies, so this
//! owner's death is not end of stream for the harness. Over a Unix socket
//! the harness listens and each owner connects anew; end of stream there is
//! the end of that connection, never by itself the harness's end.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use oulipoly_acp::{Incoming, PeerClosed, Transport};

use crate::conversation::Inbox;

/// Longest harness stdout line accepted. A longer line is reported as
/// malformed, which `AcpClient` treats as a protocol violation.
pub const MAX_LINE_BYTES: u64 = 1 << 20;

/// What this transport has observed about the connection.
#[derive(Default)]
pub(crate) struct Observed {
    /// The harness's stdout reached end of stream.
    pub(crate) eof: Cell<bool>,
    /// A write to the harness's stdin failed.
    pub(crate) send_fault: Cell<bool>,
    /// A read from the harness's stdout failed (not end of stream).
    pub(crate) read_fault: Cell<bool>,
    /// This owner stopped reading because it detached from the root (its
    /// store authority was lost or a newer owner attached). Not an end.
    pub(crate) detached: Cell<bool>,
    /// Set by the worker only while it waits between turns: then a ring of
    /// its inbox's bell interrupts the wait.
    pub(crate) wake_armed: Cell<bool>,
    /// The last read returned because the bell rang, not because of the
    /// harness. Not an end and not a fault; any partial line is kept.
    pub(crate) woken: Cell<bool>,
}

/// Run-level detach: once triggered, every blocked harness read returns
/// without the harness having ended, so a detaching owner can leave its
/// root's live work to a successor instead of killing it.
pub(crate) struct StopSignal {
    read: OwnedFd,
    write: Mutex<Option<OwnedFd>>,
    /// Root PID 1 said a newer owner attached: this run's authority over
    /// its root is lost, whatever the store says.
    superseded: AtomicBool,
}

impl StopSignal {
    pub(crate) fn new() -> io::Result<Self> {
        let (read, write) = crate::sys::pipe()?;
        Ok(Self {
            read,
            write: Mutex::new(Some(write)),
            superseded: AtomicBool::new(false),
        })
    }

    pub(crate) fn trigger(&self) {
        self.write.lock().expect("stop lock").take();
    }

    /// Records that a newer owner superseded this one, then triggers.
    pub(crate) fn supersede(&self) {
        self.superseded.store(true, Ordering::SeqCst);
        self.trigger();
    }

    pub(crate) fn superseded(&self) -> bool {
        self.superseded.load(Ordering::SeqCst)
    }
}

/// Discards a socket-endpoint harness's stdout on a thread, so its output
/// never fills the pipe and blocks it. Ends at end of stream, a read error
/// or the run's detach; it observes nothing about the harness.
pub(crate) fn drain(file: File, stop: std::sync::Arc<StopSignal>) {
    std::thread::spawn(move || {
        let mut file = file;
        let mut buf = [0u8; 8192];
        loop {
            if !readable(&stop, &file).unwrap_or(false) {
                return;
            }
            match file.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });
}

/// Blocks until `file` is readable (`Ok(true)`) or the run detached
/// (`Ok(false)`).
pub(crate) fn readable(stop: &StopSignal, file: &File) -> io::Result<bool> {
    Ok(matches!(ready(stop, file, None)?, Ready::File))
}

enum Ready {
    File,
    Detached,
    Bell,
}

/// Blocks until `file` is readable, the run detached, or (when given) the
/// bell rang. The harness's own data and a detach come first.
fn ready(stop: &StopSignal, file: &File, bell: Option<&OwnedFd>) -> io::Result<Ready> {
    let pollfd = |fd: i32| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let mut polls = [
        pollfd(stop.read.as_raw_fd()),
        pollfd(file.as_raw_fd()),
        pollfd(bell.map_or(-1, AsRawFd::as_raw_fd)),
    ];
    loop {
        // SAFETY: poll over three pollfds; a negative fd is ignored.
        if unsafe { libc::poll(polls.as_mut_ptr(), 3, -1) } >= 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(if polls[0].revents != 0 {
        Ready::Detached
    } else if polls[1].revents != 0 {
        Ready::File
    } else {
        Ready::Bell
    })
}

/// Whether the run detached, without blocking.
pub(crate) fn detached(stop: &StopSignal) -> bool {
    let mut poll = libc::pollfd {
        fd: stop.read.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll over one valid pollfd, without waiting.
    unsafe { libc::poll(&raw mut poll, 1, 0) > 0 }
}

/// Reads the harness's stdout unless the run detached first, or (while
/// armed) the worker's bell rang.
struct Polled {
    file: File,
    stop: std::sync::Arc<StopSignal>,
    observed: Rc<Observed>,
    inbox: Option<std::sync::Arc<Inbox>>,
}

impl Read for Polled {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let bell = self
            .inbox
            .as_deref()
            .filter(|_| self.observed.wake_armed.get())
            .map(Inbox::bell);
        match ready(&self.stop, &self.file, bell)? {
            Ready::File => self.file.read(buf),
            Ready::Detached => {
                self.observed.detached.set(true);
                Err(io::Error::other("detached"))
            }
            Ready::Bell => {
                self.observed.woken.set(true);
                Err(io::Error::other("woken"))
            }
        }
    }
}

pub(crate) struct HarnessTransport {
    reader: BufReader<Polled>,
    writer: File,
    observed: Rc<Observed>,
    poisoned: bool,
    /// Bytes of a line not yet complete when a read was woken.
    pending: Vec<u8>,
}

impl HarnessTransport {
    pub(crate) fn new(
        stdout: File,
        stdin: File,
        observed: Rc<Observed>,
        stop: std::sync::Arc<StopSignal>,
        inbox: Option<std::sync::Arc<Inbox>>,
    ) -> Self {
        Self {
            reader: BufReader::new(Polled {
                file: stdout,
                stop,
                observed: Rc::clone(&observed),
                inbox,
            }),
            writer: stdin,
            observed,
            poisoned: false,
            pending: Vec::new(),
        }
    }
}

impl Transport for HarnessTransport {
    fn send(&mut self, message: &serde_json::Value) -> Result<(), PeerClosed> {
        let mut line = message.to_string();
        line.push('\n');
        let written = self
            .writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush());
        if written.is_err() {
            self.observed.send_fault.set(true);
            return Err(PeerClosed);
        }
        Ok(())
    }

    fn recv(&mut self) -> Incoming {
        loop {
            if self.poisoned {
                // After an over-long line the framing is lost; report closed
                // without claiming end of stream.
                self.observed.read_fault.set(true);
                return Incoming::Closed;
            }
            let room = (MAX_LINE_BYTES + 1).saturating_sub(self.pending.len() as u64);
            let read = (&mut self.reader)
                .take(room)
                .read_until(b'\n', &mut self.pending);
            match read {
                Ok(0) if self.pending.is_empty() => {
                    self.observed.eof.set(true);
                    return Incoming::Closed;
                }
                Err(_) => {
                    // A woken read keeps what it read of the line so far.
                    if !self.observed.detached.get() && !self.observed.woken.get() {
                        self.observed.read_fault.set(true);
                    }
                    return Incoming::Closed;
                }
                Ok(_) => {}
            }
            let line = std::mem::take(&mut self.pending);
            if line.len() as u64 > MAX_LINE_BYTES {
                self.poisoned = true;
                return Incoming::Malformed("line exceeds bound".to_owned());
            }
            let Ok(text) = std::str::from_utf8(&line) else {
                return Incoming::Malformed("invalid utf-8".to_owned());
            };
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            return match serde_json::from_str(trimmed) {
                Ok(value) => Incoming::Message(value),
                Err(_) => Incoming::Malformed(trimmed.to_owned()),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A bell that rings mid-line interrupts the read without claiming an
    /// end or a fault, and the line's bytes already read are kept.
    #[test]
    fn woken_read_keeps_the_partial_line() {
        let (read, write) = crate::sys::pipe().unwrap();
        let mut harness = File::from(write);
        let observed = Rc::new(Observed::default());
        let inbox = Arc::new(Inbox::new().unwrap());
        let mut transport = HarnessTransport::new(
            File::from(read),
            File::open("/dev/null").unwrap(),
            Rc::clone(&observed),
            Arc::new(StopSignal::new().unwrap()),
            Some(Arc::clone(&inbox)),
        );
        observed.wake_armed.set(true);
        harness.write_all(br#"{"a":"#).unwrap();
        inbox.ring();
        assert!(matches!(transport.recv(), Incoming::Closed));
        assert!(observed.woken.get());
        assert!(!observed.eof.get() && !observed.read_fault.get());
        assert!(inbox.take().is_empty());
        observed.woken.set(false);
        harness.write_all(b"1}\n").unwrap();
        match transport.recv() {
            Incoming::Message(value) => assert_eq!(value, serde_json::json!({ "a": 1 })),
            _ => panic!("line lost"),
        }
        // Unarmed, the bell is not heard: only the harness's data or end.
        observed.wake_armed.set(false);
        inbox.ring();
        drop(harness);
        assert!(matches!(transport.recv(), Incoming::Closed));
        assert!(observed.eof.get() && !observed.woken.get());
    }
}
