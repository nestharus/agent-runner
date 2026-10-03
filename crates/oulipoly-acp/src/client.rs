use std::collections::HashMap;
use std::io::Read;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::transport::{Incoming, Transport};
use crate::wire::{self, method};
use crate::{
    DEDUP_CONTRACT_META, DEDUP_CONTRACT_VERSION, DUPLICATE_META, MESSAGE_KEY_META, PROTOCOL_VERSION,
};

/// Identity this client reports in `initialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// The agent after a successful v2 negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedPeer {
    pub protocol_version: u16,
    pub agent_name: String,
    pub agent_version: String,
    /// The agent advertised the local dedup contract (version 1).
    pub dedup_contract: bool,
}

/// Why `initialize` did not produce a usable v2 peer. None of these is a
/// v2 acceptance, and the client refuses to send prompts afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationFailure {
    /// The agent answered with a version other than 2. `1` means the agent
    /// speaks only ACP v1; its turn-end response is not v2 consumption.
    UnsupportedVersion {
        agent_version: u16,
    },
    /// The agent advertised no `session` capability.
    NoSessionSurface,
    Rejected {
        code: i64,
        message: String,
    },
    PeerGone,
    ProtocolViolation(String),
}

/// Why a session request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestFailure {
    NotNegotiated,
    Rejected { code: i64, message: String },
    PeerGone,
    ProtocolViolation(String),
}

/// A supplied identity for one communication; no history is implied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageKey(String);

impl MessageKey {
    /// Returns `None` for an empty key.
    pub fn new(key: impl Into<String>) -> Option<Self> {
        let key = key.into();
        (!key.is_empty()).then_some(Self(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Evidence supporting a receiver-side at-most-once label. Neither basis is
/// measured receiver compliance or a guarantee about provider effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtMostOnceBasis {
    /// A fresh, unforked identity with exactly one counted attempt.
    SingleAttempt,
    /// Complete history: every attempt advertised our contract in this
    /// session, and the current response echoes the key.
    SessionContract,
}

/// A recorded insertion of an [`OutboundMessage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acceptance {
    /// The agent's `messageId` for the inserted user message.
    pub message_id: String,
    /// The agent declared, under the dedup contract, that this response
    /// returns an earlier insertion of the same key.
    pub recovered: bool,
    /// Whether this identity is known to have been inserted at most once.
    /// `false` after [`DeliveryOutcome::DuplicateUnknown`].
    pub at_most_once: bool,
    pub basis: Option<AtMostOnceBasis>,
}

/// One communication and its delivery history.
///
/// The key is fixed at construction, so every retry carries the same
/// identity. The message is owed until an attempt is acknowledged.
pub struct OutboundMessage {
    key: MessageKey,
    text: String,
    unacknowledged_attempts: u32,
    acceptance: Option<Acceptance>,
    complete_history: bool,
    all_attempts_dedup: bool,
    session_id: Option<String>,
}

impl OutboundMessage {
    /// A supplied, recovered or cloned key has unknown history. Current
    /// advertisements cannot recover that missing evidence.
    pub fn new(key: MessageKey, text: impl Into<String>) -> Self {
        Self {
            key,
            text: text.into(),
            unacknowledged_attempts: 0,
            acceptance: None,
            complete_history: false,
            all_attempts_dedup: true,
            session_id: None,
        }
    }

    /// Mint a fresh Linux identity. Random-key uniqueness is the minting
    /// assumption; no durable registry or restart history is maintained.
    /// The message cannot be cloned, and exporting its key abandons the
    /// complete-history claim because the caller could fork it.
    pub fn fresh(text: impl Into<String>) -> std::io::Result<Self> {
        let mut bytes = [0u8; 32];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
        let key: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let mut message = Self::new(MessageKey(key), text);
        message.complete_history = true;
        Ok(message)
    }

    /// Exporting the key permits re-supply/forking. Both future labels and
    /// any cached acceptance are conservatively downgraded.
    pub fn key(&mut self) -> &MessageKey {
        self.complete_history = false;
        if let Some(acceptance) = &mut self.acceptance {
            acceptance.at_most_once = false;
            acceptance.basis = None;
        }
        &self.key
    }

    /// No attempt has been acknowledged yet.
    pub fn is_owed(&self) -> bool {
        self.acceptance.is_none()
    }

    /// Attempts without an insertion ACK, including valid rejections. Each
    /// may or may not have been inserted.
    pub fn unacknowledged_attempts(&self) -> u32 {
        self.unacknowledged_attempts
    }

    pub fn acceptance(&self) -> Option<&Acceptance> {
        self.acceptance.as_ref()
    }
}

/// Why an attempt ended without an acknowledgement. The attempt may or may
/// not have been inserted; the message stays owed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoAckCause {
    /// The peer exited or disconnected before acknowledging.
    PeerGone,
    /// The peer answered with something that is not a valid acknowledgement
    /// of this request.
    InvalidResponse(String),
}

/// What one `session/prompt` attempt established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryOutcome {
    /// Inserted, and known to be inserted at most once: either this was the
    /// only counted attempt for a fresh unforked identity, or its complete
    /// same-session history is contract-covered. See `Acceptance::basis`.
    Accepted(Acceptance),
    /// An insertion ACK was received, but complete history is unknown or
    /// includes a non-contract attempt. Never at-most-once.
    DuplicateUnknown(Acceptance),
    /// No acknowledgement; still owed.
    NotAcknowledged(NoAckCause),
    /// A valid JSON-RPC error; insertion uncertain, still owed and counted.
    Rejected { code: i64, message: String },
    /// The client has no negotiated v2 peer, so nothing was sent.
    NotNegotiated,
    /// This communication belongs to a different session; nothing sent.
    SessionMismatch,
}

/// A `session/update` the client observed, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    UserMessage {
        session_id: String,
        message_id: String,
    },
    Running {
        session_id: String,
    },
    Idle {
        session_id: String,
        stop_reason: Option<String>,
    },
    Other {
        session_id: String,
        kind: String,
    },
}

/// The session was observed idle after an attempt: readiness evidence,
/// without message-completion correlation or physical-drain evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIdle {
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdleWaitFailure {
    /// No prompt attempt has been tracked for this session; nothing read.
    NoAttempt,
    /// The peer went away before reporting idle. Says nothing about whether
    /// an acknowledged message is still owed (it is not).
    PeerGone,
    ProtocolViolation(String),
}

enum Reply {
    Result(Value),
    Error { code: i64, message: String },
    Gone,
    Violation(String),
}

/// Client side of one ACP v2 connection.
pub struct AcpClient<T> {
    transport: T,
    info: ClientInfo,
    next_id: u64,
    peer: Option<NegotiatedPeer>,
    events: Vec<SessionEvent>,
    /// Independent observation cursors; one session never consumes another.
    idle_cursor: HashMap<String, usize>,
}

impl<T: Transport> AcpClient<T> {
    pub fn new(transport: T, info: ClientInfo) -> Self {
        Self {
            transport,
            info,
            next_id: 0,
            peer: None,
            events: Vec::new(),
            idle_cursor: HashMap::new(),
        }
    }

    pub fn peer(&self) -> Option<&NegotiatedPeer> {
        self.peer.as_ref()
    }

    /// Every session update observed so far, in arrival order.
    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    /// Offers protocol version 2 and accepts only version 2 back.
    pub fn initialize(&mut self) -> Result<NegotiatedPeer, NegotiationFailure> {
        self.peer = None;
        let mut meta = Map::new();
        meta.insert(
            DEDUP_CONTRACT_META.to_owned(),
            json!({ "version": DEDUP_CONTRACT_VERSION }),
        );
        let request = wire::InitializeRequest {
            protocol_version: PROTOCOL_VERSION,
            info: wire::Implementation {
                name: self.info.name.clone(),
                version: self.info.version.clone(),
            },
            capabilities: Map::new(),
            meta,
        };
        let result = match self.call(method::INITIALIZE, &request) {
            Reply::Result(result) => result,
            Reply::Error { code, message } => {
                return Err(NegotiationFailure::Rejected { code, message });
            }
            Reply::Gone => return Err(NegotiationFailure::PeerGone),
            Reply::Violation(why) => return Err(NegotiationFailure::ProtocolViolation(why)),
        };
        let version: wire::InitializeResponseVersion = parse(&result)
            .map_err(|why| NegotiationFailure::ProtocolViolation(format!("initialize: {why}")))?;
        if version.protocol_version != PROTOCOL_VERSION {
            return Err(NegotiationFailure::UnsupportedVersion {
                agent_version: version.protocol_version,
            });
        }
        let response: wire::InitializeResponse = parse(&result)
            .map_err(|why| NegotiationFailure::ProtocolViolation(format!("initialize: {why}")))?;
        if response
            .capabilities
            .session
            .as_ref()
            .is_none_or(Value::is_null)
        {
            return Err(NegotiationFailure::NoSessionSurface);
        }
        if !response
            .capabilities
            .session
            .as_ref()
            .is_some_and(Value::is_object)
        {
            return Err(NegotiationFailure::ProtocolViolation(
                "session capability must be an object".to_owned(),
            ));
        }
        let dedup_contract = response
            .meta
            .as_ref()
            .and_then(|meta| meta.get(DEDUP_CONTRACT_META))
            .and_then(|contract| contract.get("version"))
            .and_then(Value::as_u64)
            == Some(DEDUP_CONTRACT_VERSION);
        let peer = NegotiatedPeer {
            protocol_version: response.protocol_version,
            agent_name: response.info.name,
            agent_version: response.info.version,
            dedup_contract,
        };
        self.peer = Some(peer.clone());
        Ok(peer)
    }

    /// `session/new`; returns the agent's session id.
    pub fn open_session(&mut self, cwd: &str) -> Result<String, RequestFailure> {
        let request = wire::NewSessionRequest {
            cwd: cwd.to_owned(),
        };
        let result = self.session_call(method::SESSION_NEW, &request)?;
        let response: wire::NewSessionResponse = parse(&result)
            .map_err(|why| RequestFailure::ProtocolViolation(format!("session/new: {why}")))?;
        Ok(response.session_id)
    }

    /// `session/resume` without history replay.
    pub fn resume_session(&mut self, session_id: &str, cwd: &str) -> Result<(), RequestFailure> {
        let request = wire::ResumeSessionRequest {
            session_id: session_id.to_owned(),
            cwd: cwd.to_owned(),
        };
        let result = self.session_call(method::SESSION_RESUME, &request)?;
        if !result.is_object() {
            return Err(RequestFailure::ProtocolViolation(
                "session/resume result must be an object".to_owned(),
            ));
        }
        Ok(())
    }

    /// Sends one `session/prompt` attempt for `message` and returns at its
    /// insertion acknowledgement (or at whatever ended the attempt). It
    /// does not wait for turn completion.
    ///
    /// A message that is already acknowledged is not sent again; its
    /// recorded acceptance is returned.
    pub fn submit(&mut self, session_id: &str, message: &mut OutboundMessage) -> DeliveryOutcome {
        if message
            .session_id
            .as_deref()
            .is_some_and(|id| id != session_id)
        {
            return DeliveryOutcome::SessionMismatch;
        }
        if let Some(acceptance) = &message.acceptance {
            return if acceptance.at_most_once {
                DeliveryOutcome::Accepted(acceptance.clone())
            } else {
                DeliveryOutcome::DuplicateUnknown(acceptance.clone())
            };
        }
        let Some(peer) = self.peer.clone() else {
            return DeliveryOutcome::NotNegotiated;
        };
        let mut meta = Map::new();
        meta.insert(
            MESSAGE_KEY_META.to_owned(),
            Value::String(message.key.as_str().to_owned()),
        );
        let request = wire::PromptRequest {
            session_id: session_id.to_owned(),
            prompt: vec![wire::TextBlock {
                kind: "text",
                text: message.text.clone(),
            }],
            meta,
        };
        // Preserve already-observed readiness even across subsequent attempts.
        self.idle_cursor
            .entry(session_id.to_owned())
            .or_insert(self.events.len());
        message.session_id = Some(session_id.to_owned());
        message.all_attempts_dedup &= peer.dedup_contract;
        let result = match self.call(method::SESSION_PROMPT, &request) {
            Reply::Result(result) => result,
            Reply::Error {
                code,
                message: reason,
            } => {
                message.unacknowledged_attempts += 1;
                return DeliveryOutcome::Rejected {
                    code,
                    message: reason,
                };
            }
            Reply::Gone => {
                message.unacknowledged_attempts += 1;
                return DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone);
            }
            Reply::Violation(why) => {
                message.unacknowledged_attempts += 1;
                return DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(why));
            }
        };
        let response = match parse::<wire::PromptResponse>(&result) {
            Ok(response) if !response.message_id.is_empty() => response,
            Ok(_) => {
                message.unacknowledged_attempts += 1;
                return DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(
                    "session/prompt: empty messageId".to_owned(),
                ));
            }
            Err(why) => {
                message.unacknowledged_attempts += 1;
                return DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(format!(
                    "session/prompt: {why}"
                )));
            }
        };
        let meta = response.meta.unwrap_or_default();
        // The contract counts only if the agent advertised it and this
        // response echoes our own key.
        let dedup_confirmed = peer.dedup_contract
            && meta.get(MESSAGE_KEY_META).and_then(Value::as_str) == Some(message.key.as_str());
        let recovered =
            dedup_confirmed && meta.get(DUPLICATE_META).and_then(Value::as_bool) == Some(true);
        let basis = if !message.complete_history {
            None
        } else if message.unacknowledged_attempts == 0 {
            Some(AtMostOnceBasis::SingleAttempt)
        } else if message.all_attempts_dedup && dedup_confirmed {
            Some(AtMostOnceBasis::SessionContract)
        } else {
            None
        };
        let at_most_once = basis.is_some();
        let acceptance = Acceptance {
            message_id: response.message_id,
            recovered,
            at_most_once,
            basis,
        };
        message.acceptance = Some(acceptance.clone());
        if at_most_once {
            DeliveryOutcome::Accepted(acceptance)
        } else {
            DeliveryOutcome::DuplicateUnknown(acceptance)
        }
    }

    /// Observe the next unconsumed idle for this session after an attempt.
    /// This is session readiness, never completion of a particular message.
    pub fn await_session_idle(&mut self, session_id: &str) -> Result<SessionIdle, IdleWaitFailure> {
        let Some(&start) = self.idle_cursor.get(session_id) else {
            return Err(IdleWaitFailure::NoAttempt);
        };
        let mut scanned = start;
        loop {
            if let Some((next, stop_reason)) =
                self.events[scanned..]
                    .iter()
                    .enumerate()
                    .find_map(|(offset, event)| match event {
                        SessionEvent::Idle {
                            session_id: id,
                            stop_reason,
                        } if id == session_id => Some((scanned + offset + 1, stop_reason.clone())),
                        _ => None,
                    })
            {
                self.idle_cursor.insert(session_id.to_owned(), next);
                return Ok(SessionIdle { stop_reason });
            }
            scanned = self.events.len();
            match self.transport.recv() {
                Incoming::Closed => return Err(IdleWaitFailure::PeerGone),
                Incoming::Malformed(line) => {
                    return Err(IdleWaitFailure::ProtocolViolation(format!(
                        "malformed message: {line}"
                    )));
                }
                Incoming::Message(message) => {
                    if message.get("method").is_some() {
                        self.handle_inbound(&message);
                    } else {
                        return Err(IdleWaitFailure::ProtocolViolation(
                            "unsolicited response".to_owned(),
                        ));
                    }
                }
            }
        }
    }

    fn session_call<P: Serialize>(
        &mut self,
        name: &str,
        params: &P,
    ) -> Result<Value, RequestFailure> {
        if self.peer.is_none() {
            return Err(RequestFailure::NotNegotiated);
        }
        match self.call(name, params) {
            Reply::Result(result) => Ok(result),
            Reply::Error { code, message } => Err(RequestFailure::Rejected { code, message }),
            Reply::Gone => Err(RequestFailure::PeerGone),
            Reply::Violation(why) => Err(RequestFailure::ProtocolViolation(why)),
        }
    }

    /// Sends a request and reads until its response, recording session
    /// updates and refusing agent-initiated requests on the way.
    fn call<P: Serialize>(&mut self, name: &str, params: &P) -> Reply {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": name,
            "params": params,
        });
        if self.transport.send(&request).is_err() {
            return Reply::Gone;
        }
        loop {
            let message = match self.transport.recv() {
                Incoming::Closed => return Reply::Gone,
                Incoming::Malformed(line) => {
                    return Reply::Violation(format!("malformed message: {line}"));
                }
                Incoming::Message(message) => message,
            };
            if message.get("method").is_some() {
                self.handle_inbound(&message);
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                return Reply::Violation(format!(
                    "response id {} does not match request id {id}",
                    message.get("id").unwrap_or(&Value::Null)
                ));
            }
            if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                return Reply::Violation("response jsonrpc must be 2.0".to_owned());
            }
            if message.get("result").is_some() == message.get("error").is_some() {
                return Reply::Violation(
                    "response must have exactly one of result/error".to_owned(),
                );
            }
            if let Some(error) = message.get("error") {
                let (Some(code), Some(reason)) = (
                    error.get("code").and_then(Value::as_i64),
                    error.get("message").and_then(Value::as_str),
                ) else {
                    return Reply::Violation("invalid error object".to_owned());
                };
                return Reply::Error {
                    code,
                    message: reason.to_owned(),
                };
            }
            return match message.get("result") {
                Some(result) => Reply::Result(result.clone()),
                None => Reply::Violation("response has neither result nor error".to_owned()),
            };
        }
    }

    fn handle_inbound(&mut self, message: &Value) {
        let name = message.get("method").and_then(Value::as_str);
        if let Some(id) = message.get("id") {
            // This client implements no client-side methods.
            let _ = self.transport.send(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": wire::METHOD_NOT_FOUND, "message": "method not found" },
            }));
            return;
        }
        if name != Some(method::SESSION_UPDATE) {
            return;
        }
        let Some(Ok(notification)) = message
            .get("params")
            .map(parse::<wire::UpdateSessionNotification>)
        else {
            return;
        };
        if let Some(event) = session_event(notification) {
            self.events.push(event);
        }
    }
}

fn session_event(notification: wire::UpdateSessionNotification) -> Option<SessionEvent> {
    let session_id = notification.session_id;
    let update = notification.update;
    let kind = update.get("sessionUpdate")?.as_str()?.to_owned();
    Some(match kind.as_str() {
        "user_message" => SessionEvent::UserMessage {
            session_id,
            message_id: update.get("messageId")?.as_str()?.to_owned(),
        },
        "state_update" => match update.get("state")?.as_str()? {
            "running" => SessionEvent::Running { session_id },
            "idle" => SessionEvent::Idle {
                session_id,
                stop_reason: update
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            },
            other => SessionEvent::Other {
                session_id,
                kind: format!("state_update:{other}"),
            },
        },
        _ => SessionEvent::Other { session_id, kind },
    })
}

fn parse<D: DeserializeOwned>(value: &Value) -> Result<D, String> {
    D::deserialize(value).map_err(|error| error.to_string())
}
