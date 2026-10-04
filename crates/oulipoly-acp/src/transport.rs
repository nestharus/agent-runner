use std::io::{BufRead, Write};

use serde_json::Value;

/// The peer exited or the connection to it is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerClosed;

/// One item read from the peer.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A complete JSON-RPC message.
    Message(Value),
    /// Bytes that did not parse as one JSON value.
    Malformed(String),
    /// The peer exited or disconnected; nothing more will arrive.
    Closed,
}

/// A bidirectional JSON-RPC message channel to one ACP agent.
///
/// Implementations deliver messages in order and report [`Incoming::Closed`]
/// once the peer is gone. `recv` blocks until something arrives.
pub trait Transport {
    fn send(&mut self, message: &Value) -> Result<(), PeerClosed>;
    fn recv(&mut self) -> Incoming;
}

/// ACP stdio framing: one JSON message per line, over any reader and writer
/// (for example a child process's stdout and stdin).
pub struct LineTransport<R, W> {
    reader: R,
    writer: W,
}

impl<R: BufRead, W: Write> LineTransport<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }

    pub fn into_parts(self) -> (R, W) {
        (self.reader, self.writer)
    }
}

impl<R: BufRead, W: Write> Transport for LineTransport<R, W> {
    fn send(&mut self, message: &Value) -> Result<(), PeerClosed> {
        let mut line = message.to_string();
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|_| PeerClosed)
    }

    fn recv(&mut self) -> Incoming {
        loop {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) | Err(_) => return Incoming::Closed,
                Ok(_) => {}
            }
            let trimmed = line.trim();
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
