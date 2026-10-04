//! Deterministic, test-only ACP v2 reference peer. No model, no process.
//!
//! The peer runs on its own thread behind an in-memory channel transport.
//! When the peer "exits", its thread ends and its channel end is dropped, so
//! the client sees `Incoming::Closed`. A [`Store`] outlives connections, so a
//! later connection to "the same agent" sees earlier sessions and insertions.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use oulipoly_acp::{
    DEDUP_CONTRACT_META, DUPLICATE_META, Incoming, MESSAGE_KEY_META, PeerClosed, Transport,
};
use serde_json::{Value, json};

pub struct ChannelTransport {
    tx: Sender<Value>,
    rx: Receiver<Value>,
}

impl Transport for ChannelTransport {
    fn send(&mut self, message: &Value) -> Result<(), PeerClosed> {
        self.tx.send(message.clone()).map_err(|_| PeerClosed)
    }

    fn recv(&mut self) -> Incoming {
        match self.rx.recv() {
            Ok(message) => Incoming::Message(message),
            Err(_) => Incoming::Closed,
        }
    }
}

fn channel_pair() -> (ChannelTransport, ChannelTransport) {
    let (a_tx, b_rx) = channel();
    let (b_tx, a_rx) = channel();
    (
        ChannelTransport { tx: a_tx, rx: a_rx },
        ChannelTransport { tx: b_tx, rx: b_rx },
    )
}

/// One user-message insertion into an agent conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Insertion {
    pub session_id: String,
    pub message_key: Option<String>,
    pub message_id: String,
}

/// Agent state that survives connections.
#[derive(Debug, Default)]
pub struct Store {
    sessions: HashSet<String>,
    next_session: u64,
    next_message: u64,
    by_key: HashMap<(String, String), String>,
    pub insertions: Vec<Insertion>,
    pub prompts_received: usize,
}

pub type SharedStore = Arc<Mutex<Store>>;

pub fn new_store() -> SharedStore {
    Arc::new(Mutex::new(Store::default()))
}

pub fn insertions_for(store: &SharedStore, key: &str) -> usize {
    store
        .lock()
        .unwrap()
        .insertions
        .iter()
        .filter(|insertion| insertion.message_key.as_deref() == Some(key))
        .count()
}

/// What the peer does with the first `session/prompt` it receives on this
/// connection. Later prompts on the same connection behave normally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptFault {
    None,
    /// Exit without inserting or responding.
    ExitBeforeInsert,
    /// Insert, then exit without responding: the acknowledgement is lost.
    ExitAfterInsert,
    /// Insert, then respond with a JSON-RPC id that is not the request's.
    WrongResponseId,
    /// Insert, then respond with `"messageId": null`.
    NullMessageId,
    /// Respond normally, then exit before reporting session idle.
    ExitAfterAck,
}

pub struct PeerConfig {
    /// Version answered to `initialize`.
    pub protocol_version: u16,
    /// Apply and advertise the local dedup contract.
    pub dedup: bool,
    /// Advertise the dedup contract but echo a different key.
    pub echo_wrong_key: bool,
    pub fault: PromptFault,
    /// When set, the peer acknowledges a prompt and then waits for a signal
    /// before reporting session idle.
    pub hold_turn: Option<Receiver<()>>,
}

impl PeerConfig {
    pub fn v2() -> Self {
        Self {
            protocol_version: 2,
            dedup: true,
            echo_wrong_key: false,
            fault: PromptFault::None,
            hold_turn: None,
        }
    }

    pub fn without_dedup(mut self) -> Self {
        self.dedup = false;
        self
    }

    pub fn with_fault(mut self, fault: PromptFault) -> Self {
        self.fault = fault;
        self
    }
}

/// Starts a peer and returns the client's end of the connection.
pub fn spawn_peer(config: PeerConfig, store: SharedStore) -> (ChannelTransport, JoinHandle<()>) {
    let (client_end, peer_end) = channel_pair();
    let handle = thread::spawn(move || run_peer(config, store, peer_end));
    (client_end, handle)
}

fn respond(io: &mut ChannelTransport, id: &Value, result: Value) -> bool {
    io.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }))
        .is_ok()
}

fn respond_error(io: &mut ChannelTransport, id: &Value, code: i64, message: &str) -> bool {
    io.send(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    }))
    .is_ok()
}

fn notify(io: &mut ChannelTransport, session_id: &str, update: Value) -> bool {
    io.send(&json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": { "sessionId": session_id, "update": update },
    }))
    .is_ok()
}

fn run_peer(mut config: PeerConfig, store: SharedStore, mut io: ChannelTransport) {
    let mut fault = config.fault;
    loop {
        let Incoming::Message(request) = io.recv() else {
            return;
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let ok = match request.get("method").and_then(Value::as_str) {
            Some("initialize") => respond(&mut io, &id, initialize_result(&config)),
            Some("session/new") => {
                let mut store = store.lock().unwrap();
                store.next_session += 1;
                let session_id = format!("sess-{}", store.next_session);
                store.sessions.insert(session_id.clone());
                drop(store);
                respond(&mut io, &id, json!({ "sessionId": session_id }))
            }
            Some("session/resume") => {
                let session_id = params["sessionId"].as_str().unwrap_or_default();
                if store.lock().unwrap().sessions.contains(session_id) {
                    respond(&mut io, &id, json!({}))
                } else {
                    respond_error(&mut io, &id, -32002, "unknown session")
                }
            }
            Some("session/prompt") => {
                let this_fault = std::mem::replace(&mut fault, PromptFault::None);
                match handle_prompt(&mut config, &store, &mut io, &id, &params, this_fault) {
                    PromptEnd::Continue => true,
                    PromptEnd::Exit => return,
                }
            }
            _ => respond_error(&mut io, &id, -32601, "method not found"),
        };
        if !ok {
            return;
        }
    }
}

fn initialize_result(config: &PeerConfig) -> Value {
    if config.protocol_version == 1 {
        // ACP v1 response shape.
        return json!({
            "protocolVersion": 1,
            "agentCapabilities": { "loadSession": false },
            "authMethods": [],
        });
    }
    let mut result = json!({
        "protocolVersion": config.protocol_version,
        "info": { "name": "reference-peer", "version": "0.0.0" },
        "capabilities": { "session": {} },
    });
    if config.dedup || config.echo_wrong_key {
        result["_meta"] = json!({ DEDUP_CONTRACT_META: { "version": 1 } });
    }
    result
}

enum PromptEnd {
    Continue,
    Exit,
}

fn handle_prompt(
    config: &mut PeerConfig,
    store: &SharedStore,
    io: &mut ChannelTransport,
    id: &Value,
    params: &Value,
    fault: PromptFault,
) -> PromptEnd {
    let session_id = params["sessionId"].as_str().unwrap_or_default().to_owned();
    let key = params["_meta"][MESSAGE_KEY_META]
        .as_str()
        .map(str::to_owned);
    let (message_id, duplicate) = {
        let mut store = store.lock().unwrap();
        store.prompts_received += 1;
        if !store.sessions.contains(&session_id) {
            drop(store);
            respond_error(io, id, -32002, "unknown session");
            return PromptEnd::Continue;
        }
        if fault == PromptFault::ExitBeforeInsert {
            return PromptEnd::Exit;
        }
        let existing = if config.dedup {
            key.as_ref().and_then(|key| {
                store
                    .by_key
                    .get(&(session_id.clone(), key.clone()))
                    .cloned()
            })
        } else {
            None
        };
        match existing {
            Some(message_id) => (message_id, true),
            None => {
                store.next_message += 1;
                let message_id = format!("msg-{}", store.next_message);
                if let Some(key) = &key {
                    store
                        .by_key
                        .insert((session_id.clone(), key.clone()), message_id.clone());
                }
                store.insertions.push(Insertion {
                    session_id: session_id.clone(),
                    message_key: key.clone(),
                    message_id: message_id.clone(),
                });
                (message_id, false)
            }
        }
    };
    if fault == PromptFault::ExitAfterInsert {
        return PromptEnd::Exit;
    }

    let mut result = json!({ "messageId": message_id });
    if fault == PromptFault::NullMessageId {
        result["messageId"] = Value::Null;
    }
    if config.dedup {
        result["_meta"] = json!({ MESSAGE_KEY_META: key, DUPLICATE_META: duplicate });
    } else if config.echo_wrong_key {
        result["_meta"] = json!({ MESSAGE_KEY_META: "someone-else", DUPLICATE_META: true });
    }
    let response_id = if fault == PromptFault::WrongResponseId {
        json!(id.as_u64().unwrap_or(0) + 1000)
    } else {
        id.clone()
    };
    let sent = notify(
        io,
        &session_id,
        json!({ "sessionUpdate": "user_message", "messageId": message_id }),
    ) && notify(
        io,
        &session_id,
        json!({ "sessionUpdate": "state_update", "state": "running" }),
    ) && respond(io, &response_id, result);
    if !sent || fault == PromptFault::ExitAfterAck {
        return PromptEnd::Exit;
    }
    if duplicate {
        return PromptEnd::Continue;
    }
    if let Some(hold) = &config.hold_turn
        && hold.recv().is_err()
    {
        return PromptEnd::Exit;
    }
    let done = notify(
        io,
        &session_id,
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "ok" },
        }),
    ) && notify(
        io,
        &session_id,
        json!({ "sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn" }),
    );
    if done {
        PromptEnd::Continue
    } else {
        PromptEnd::Exit
    }
}
