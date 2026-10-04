//! Stdio transport to one owned harness that records *why* it reported
//! closed, so a send fault is never mistaken for an observed end of stream.
//! The pipe ends come from root PID 1, which keeps its own copies, so this
//! owner's death is not end of stream for the harness.

use std::cell::Cell;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::rc::Rc;
use std::sync::Mutex;

use oulipoly_acp::{Incoming, PeerClosed, Transport};

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
}

/// Run-level detach: once triggered, every blocked harness read returns
/// without the harness having ended, so a detaching owner can leave its
/// root's live work to a successor instead of killing it.
pub(crate) struct StopSignal {
    read: OwnedFd,
    write: Mutex<Option<OwnedFd>>,
}

impl StopSignal {
    pub(crate) fn new() -> io::Result<Self> {
        let (read, write) = crate::sys::pipe()?;
        Ok(Self {
            read,
            write: Mutex::new(Some(write)),
        })
    }

    pub(crate) fn trigger(&self) {
        self.write.lock().expect("stop lock").take();
    }
}

/// Reads the harness's stdout unless the run detached first.
struct Polled {
    file: File,
    stop: std::sync::Arc<StopSignal>,
    observed: Rc<Observed>,
}

impl Read for Polled {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut polls = [
            libc::pollfd {
                fd: self.stop.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            // SAFETY: poll over two valid pollfds.
            if unsafe { libc::poll(polls.as_mut_ptr(), 2, -1) } >= 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        if polls[0].revents != 0 {
            self.observed.detached.set(true);
            return Err(io::Error::other("detached"));
        }
        self.file.read(buf)
    }
}

pub(crate) struct HarnessTransport {
    reader: BufReader<Polled>,
    writer: File,
    observed: Rc<Observed>,
    poisoned: bool,
}

impl HarnessTransport {
    pub(crate) fn new(
        stdout: File,
        stdin: File,
        observed: Rc<Observed>,
        stop: std::sync::Arc<StopSignal>,
    ) -> Self {
        Self {
            reader: BufReader::new(Polled {
                file: stdout,
                stop,
                observed: Rc::clone(&observed),
            }),
            writer: stdin,
            observed,
            poisoned: false,
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
            let mut line = Vec::new();
            let read = (&mut self.reader)
                .take(MAX_LINE_BYTES + 1)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) => {
                    self.observed.eof.set(true);
                    return Incoming::Closed;
                }
                Err(_) => {
                    if !self.observed.detached.get() {
                        self.observed.read_fault.set(true);
                    }
                    return Incoming::Closed;
                }
                Ok(_) => {}
            }
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
