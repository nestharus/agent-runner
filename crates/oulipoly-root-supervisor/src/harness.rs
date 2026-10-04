//! One worker thread per owned harness. Each worker blocks only on its own
//! harness, so a silent harness never holds up delivery to another.

use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use oulipoly_acp::{
    AcpClient, AtMostOnceBasis, ClientInfo, DeliveryOutcome, IdleWaitFailure, NegotiationFailure,
    NoAckCause, OutboundMessage, RequestFailure,
};
use serde_json::{Value, json};

use crate::pidfd::{self, Custody};
use crate::transport::{HarnessTransport, Observed};
use crate::{Event, HarnessSpec};

/// Final state of one message, as reported in the terminal report.
#[derive(Debug, Clone)]
pub struct MessageRecord {
    pub index: usize,
    /// No insertion acknowledgement was received.
    pub owed: bool,
    /// `accepted`, `duplicate-unknown`, or the reason it is still owed.
    pub label: String,
    pub at_most_once: bool,
    pub basis: Option<&'static str>,
    pub recovered: bool,
    /// Attempts without an acknowledgement, as counted by `AcpClient`.
    pub unacknowledged_attempts: u32,
    /// Observed no-acknowledgement closures charged to this message.
    pub closures: u32,
}

/// What one harness worker owned and how it ended.
#[derive(Debug, Clone)]
pub struct HarnessRecord {
    pub id: String,
    pub launches: u32,
    /// One entry per reaped child, in launch order.
    pub exits: Vec<String>,
    pub messages: Vec<MessageRecord>,
}

impl HarnessRecord {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "launches": self.launches,
            "exits": self.exits,
            "messages": self.messages.iter().map(|message| json!({
                "index": message.index,
                "state": if message.owed { "retained-undelivered" } else { "acknowledged" },
                "label": message.label,
                "at_most_once": message.at_most_once,
                "basis": message.basis,
                "recovered": message.recovered,
                "unacknowledged_attempts": message.unacknowledged_attempts,
                "closures": message.closures,
            })).collect::<Vec<_>>(),
        })
    }
}

struct Tracked {
    message: Option<OutboundMessage>,
    label: Option<String>,
    acked: bool,
    closures: u32,
}

/// How one connection to a launched harness stopped being driven.
enum ConnEnd {
    /// The harness reported closed while a message was owed.
    Gone,
    /// Nothing further can be delivered on this connection or by relaunch;
    /// the reason is already on the owed messages.
    Stop,
    /// Every message was acknowledged and the stream ended.
    Drained,
}

struct Worker {
    spec: HarnessSpec,
    cap: u32,
    cwd: String,
    custody: Arc<Mutex<Custody>>,
    tx: Sender<Event>,
    tracked: Vec<Tracked>,
    session: Option<String>,
    launches: u32,
    exits: Vec<String>,
}

pub(crate) fn run(
    spec: HarnessSpec,
    cap: u32,
    cwd: String,
    custody: Arc<Mutex<Custody>>,
    tx: Sender<Event>,
) {
    let tracked = spec
        .messages
        .iter()
        .map(|text| match OutboundMessage::fresh(text.clone()) {
            Ok(message) => Tracked {
                message: Some(message),
                label: None,
                acked: false,
                closures: 0,
            },
            Err(_) => Tracked {
                message: None,
                label: Some("key-mint-failed".to_owned()),
                acked: false,
                closures: 0,
            },
        })
        .collect();
    let mut worker = Worker {
        spec,
        cap,
        cwd,
        custody,
        tx,
        tracked,
        session: None,
        launches: 0,
        exits: Vec::new(),
    };
    worker.supervise();
    let record = worker.record();
    let _ = worker.tx.send(Event::Done(record));
}

impl Worker {
    fn report(&self, mut value: Value) {
        value["harness"] = Value::String(self.spec.id.clone());
        let _ = self.tx.send(Event::Report(value));
    }

    fn cancelled(&self) -> bool {
        self.custody.lock().expect("custody lock").cancelled()
    }

    /// Index of the first message still to be delivered.
    fn head(&self) -> Option<usize> {
        self.tracked
            .iter()
            .position(|tracked| !tracked.acked && tracked.label.is_none())
    }

    fn label_remaining(&mut self, label: &str) {
        for tracked in &mut self.tracked {
            if !tracked.acked && tracked.label.is_none() {
                tracked.label = Some(label.to_owned());
            }
        }
    }

    fn supervise(&mut self) {
        loop {
            let Some((child, token)) = self.launch() else {
                return;
            };
            let (end, observed) = self.drive(child, token);
            if self.cancelled() {
                self.label_remaining("cancelled");
                return;
            }
            match end {
                ConnEnd::Drained | ConnEnd::Stop => {
                    self.label_remaining("not-attempted");
                    return;
                }
                ConnEnd::Gone => {}
            }
            let Some(index) = self.head() else {
                return;
            };
            // The exact child exit has been observed: this is a closure.
            let tracked = &mut self.tracked[index];
            tracked.closures += 1;
            let closures = tracked.closures;
            self.report(json!({
                "event": "closure-observed",
                "index": index,
                "cause": observed,
                "closures": closures,
            }));
            if closures >= self.cap {
                self.tracked[index].label = Some("outage".to_owned());
                self.report(json!({ "event": "outage", "index": index, "closures": closures }));
                self.label_remaining("not-attempted");
                return;
            }
            self.report(json!({ "event": "relaunch", "index": index, "same_key": true }));
        }
    }

    /// Spawns the harness under the custody lock, so a concurrent cancel
    /// either prevents the launch or signals the new child.
    fn launch(&mut self) -> Option<(Child, u64)> {
        let mut custody = self.custody.lock().expect("custody lock");
        if custody.cancelled() {
            drop(custody);
            self.label_remaining("cancelled");
            return None;
        }
        let spawned = Command::new(&self.spec.argv[0])
            .args(&self.spec.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                drop(custody);
                self.report(json!({ "event": "launch-failed", "reason": error.to_string() }));
                self.label_remaining("launch-failed");
                return None;
            }
        };
        let fd = match pidfd::open(child.id()) {
            Ok(fd) => fd,
            Err(error) => {
                // Still unreaped and owned only here, so kill is exact.
                let _ = child.kill();
                let status = child.wait();
                drop(custody);
                self.report(json!({ "event": "launch-failed", "reason": error.to_string() }));
                if let Ok(status) = status {
                    self.exits.push(describe(status));
                }
                self.label_remaining("launch-failed");
                return None;
            }
        };
        let token = custody.register(fd);
        drop(custody);
        self.launches += 1;
        self.report(json!({ "event": "launched", "launch": self.launches, "pid": child.id() }));
        Some((child, token))
    }

    /// Drives one connection, then reaps the exact child. Returns how the
    /// connection ended and, for [`ConnEnd::Gone`], what was observed.
    fn drive(&mut self, mut child: Child, token: u64) -> (ConnEnd, &'static str) {
        let observed = Rc::new(Observed::default());
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let transport = HarnessTransport::new(stdout, stdin, Rc::clone(&observed));
        let mut client = AcpClient::new(
            transport,
            ClientInfo {
                name: "oulipoly-root-supervisor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        );
        let end = self.converse(&mut client);
        let cause = match end {
            ConnEnd::Gone if observed.eof.get() => {
                self.report(json!({ "event": "peer-gone", "observed": "eof" }));
                "eof-then-exit"
            }
            ConnEnd::Gone if observed.send_fault.get() => {
                // Not PeerGone: nothing has shown that the harness ended.
                // It stays owed and in flight until its exit is observed.
                self.report(json!({ "event": "send-fault", "index": self.head() }));
                "exit-after-send-fault"
            }
            ConnEnd::Gone => {
                self.report(json!({ "event": "read-fault", "index": self.head() }));
                "exit-after-read-fault"
            }
            _ => "",
        };
        // Our ends of the pipes stay open until the exit is observed, so
        // closing them is never a substitute for that observation.
        let status = child.wait();
        drop(client);
        self.custody.lock().expect("custody lock").release(token);
        let exit = match status {
            Ok(status) => describe(status),
            Err(error) => format!("wait-failed:{error}"),
        };
        self.report(json!({ "event": "exited", "launch": self.launches, "status": exit, "reaped": "exact-child" }));
        self.exits.push(exit);
        (end, cause)
    }

    fn converse(&mut self, client: &mut AcpClient<HarnessTransport>) -> ConnEnd {
        match client.initialize() {
            Ok(peer) => self.report(json!({
                "event": "negotiated",
                "launch": self.launches,
                "dedup_contract": peer.dedup_contract,
            })),
            Err(NegotiationFailure::PeerGone) => return self.gone_or_drained(),
            Err(failure) => {
                let label = match failure {
                    NegotiationFailure::UnsupportedVersion { agent_version } => {
                        format!("not-negotiated:unsupported-version-{agent_version}")
                    }
                    NegotiationFailure::NoSessionSurface => {
                        "not-negotiated:no-session-surface".to_owned()
                    }
                    NegotiationFailure::Rejected { code, .. } => {
                        format!("not-negotiated:rejected-{code}")
                    }
                    NegotiationFailure::ProtocolViolation(_) => {
                        "not-negotiated:protocol-violation".to_owned()
                    }
                    NegotiationFailure::PeerGone => unreachable!(),
                };
                self.report(json!({ "event": "negotiation-failed", "label": label }));
                self.label_remaining(&label);
                return ConnEnd::Stop;
            }
        }
        if self.head().is_none() {
            return ConnEnd::Stop;
        }
        let session = match self.session.clone() {
            None => client.open_session(&self.cwd).map(Some),
            Some(id) => client.resume_session(&id, &self.cwd).map(|()| None),
        };
        match session {
            Ok(Some(id)) => self.session = Some(id),
            Ok(None) => self.report(json!({ "event": "session-resumed" })),
            Err(RequestFailure::PeerGone) => return self.gone_or_drained(),
            Err(failure) => {
                let label = match failure {
                    RequestFailure::Rejected { code, .. } => format!("session-rejected-{code}"),
                    RequestFailure::NotNegotiated => "session-not-negotiated".to_owned(),
                    _ => "session-protocol-violation".to_owned(),
                };
                self.report(json!({ "event": "session-failed", "label": label }));
                self.label_remaining(&label);
                return ConnEnd::Stop;
            }
        }
        let session = self.session.clone().expect("session id");
        while let Some(index) = self.head() {
            let message = self.tracked[index]
                .message
                .as_mut()
                .expect("minted message");
            match client.submit(&session, message) {
                DeliveryOutcome::Accepted(acceptance) => {
                    self.tracked[index].acked = true;
                    self.report(json!({
                        "event": "ack",
                        "index": index,
                        "label": "accepted",
                        "basis": acceptance.basis.map(basis_label),
                        "recovered": acceptance.recovered,
                    }));
                }
                DeliveryOutcome::DuplicateUnknown(acceptance) => {
                    self.tracked[index].acked = true;
                    self.report(json!({
                        "event": "ack",
                        "index": index,
                        "label": "duplicate-unknown",
                        "recovered": acceptance.recovered,
                    }));
                }
                DeliveryOutcome::Rejected { code, .. } => {
                    self.tracked[index].label = Some("rejected".to_owned());
                    self.report(json!({ "event": "rejected", "index": index, "code": code }));
                }
                DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone) => return ConnEnd::Gone,
                DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_)) => {
                    self.tracked[index].label = Some("invalid-response".to_owned());
                    self.report(json!({ "event": "invalid-response", "index": index }));
                    return ConnEnd::Stop;
                }
                DeliveryOutcome::NotNegotiated => {
                    self.tracked[index].label = Some("not-negotiated".to_owned());
                    return ConnEnd::Stop;
                }
                DeliveryOutcome::SessionMismatch => {
                    self.tracked[index].label = Some("session-mismatch".to_owned());
                    return ConnEnd::Stop;
                }
            }
        }
        // Nothing owed on this connection. Keep reading through the client
        // until the harness ends its stream; acknowledgement is not an end.
        loop {
            match client.await_session_idle(&session) {
                Ok(idle) => self.report(json!({
                    "event": "idle",
                    "meaning": "readiness-since-first-attempt",
                    "stop_reason": idle.stop_reason,
                })),
                Err(IdleWaitFailure::PeerGone) => return ConnEnd::Drained,
                Err(IdleWaitFailure::NoAttempt | IdleWaitFailure::ProtocolViolation(_)) => {
                    return ConnEnd::Stop;
                }
            }
        }
    }

    fn gone_or_drained(&self) -> ConnEnd {
        if self.head().is_some() {
            ConnEnd::Gone
        } else {
            ConnEnd::Drained
        }
    }

    fn record(&self) -> HarnessRecord {
        let messages = self
            .tracked
            .iter()
            .enumerate()
            .map(|(index, tracked)| {
                let acceptance = tracked
                    .message
                    .as_ref()
                    .and_then(OutboundMessage::acceptance)
                    .cloned();
                let label = match (&acceptance, &tracked.label) {
                    (Some(acceptance), _) if acceptance.at_most_once => "accepted".to_owned(),
                    (Some(_), _) => "duplicate-unknown".to_owned(),
                    (None, Some(label)) => label.clone(),
                    (None, None) => "not-attempted".to_owned(),
                };
                MessageRecord {
                    index,
                    owed: acceptance.is_none(),
                    label,
                    at_most_once: acceptance.as_ref().is_some_and(|a| a.at_most_once),
                    basis: acceptance.as_ref().and_then(|a| a.basis).map(basis_label),
                    recovered: acceptance.as_ref().is_some_and(|a| a.recovered),
                    unacknowledged_attempts: tracked
                        .message
                        .as_ref()
                        .map_or(0, OutboundMessage::unacknowledged_attempts),
                    closures: tracked.closures,
                }
            })
            .collect();
        HarnessRecord {
            id: self.spec.id.clone(),
            launches: self.launches,
            exits: self.exits.clone(),
            messages,
        }
    }
}

fn basis_label(basis: AtMostOnceBasis) -> &'static str {
    match basis {
        AtMostOnceBasis::SingleAttempt => "single-attempt",
        AtMostOnceBasis::SessionContract => "session-contract",
    }
}

fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("code:{code}"),
        (None, Some(signal)) => format!("signal:{signal}"),
        _ => "unknown".to_owned(),
    }
}
