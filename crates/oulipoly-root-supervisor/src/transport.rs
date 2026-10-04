//! Stdio transport to one owned harness that records *why* it reported
//! closed, so a send fault is never mistaken for an observed end of stream.

use std::cell::Cell;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, ChildStdout};
use std::rc::Rc;

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
}

pub(crate) struct HarnessTransport {
    reader: BufReader<ChildStdout>,
    writer: ChildStdin,
    observed: Rc<Observed>,
    poisoned: bool,
}

impl HarnessTransport {
    pub(crate) fn new(stdout: ChildStdout, stdin: ChildStdin, observed: Rc<Observed>) -> Self {
        Self {
            reader: BufReader::new(stdout),
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
                    self.observed.read_fault.set(true);
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
