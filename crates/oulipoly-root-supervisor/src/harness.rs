//! One worker thread per owned harness. Each worker blocks only on its own
//! harness, so a silent harness never holds up delivery to another.

use std::io;
use std::os::fd::OwnedFd;
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
    /// Actual OS spawns, including failed custody acquisition before admission.
    pub launches: u32,
    /// One entry per successfully reaped child, in observed launch order.
    pub exits: Vec<String>,
    /// Failed waits are unknown exit/reaping observations, never exits.
    pub wait_failures: Vec<String>,
    pub messages: Vec<MessageRecord>,
}

impl HarnessRecord {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "launches": self.launches,
            "exits": self.exits,
            "wait_failures": self.wait_failures,
            "messages": self.messages.iter().map(|message| json!({
                "index": message.index,
                "state": if message.owed { "undelivered-owner-lost" } else { "acknowledged" },
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
    /// The exact child's wait failed: physical exit/reaping is unproven.
    WaitUnproven,
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
    wait_failures: Vec<String>,
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
        wait_failures: Vec::new(),
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
            if !self.after_drive(end, observed) {
                return;
            }
        }
    }

    fn after_drive(&mut self, end: ConnEnd, observed: &'static str) -> bool {
        if self.cancelled() {
            self.label_remaining("cancelled");
            return false;
        }
        match end {
            ConnEnd::Drained | ConnEnd::Stop | ConnEnd::WaitUnproven => {
                self.label_remaining("not-attempted");
                return false;
            }
            ConnEnd::Gone => {}
        }
        let Some(index) = self.head() else {
            return false;
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
            return false;
        }
        self.report(json!({ "event": "relaunch", "index": index, "same_key": true }));
        true
    }

    /// Spawns the harness under the custody lock, so a concurrent cancel
    /// either prevents the launch or signals the new child.
    fn launch(&mut self) -> Option<(Child, u64)> {
        let shared = Arc::clone(&self.custody);
        let mut custody = shared.lock().expect("custody lock");
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
        let child = match spawned {
            Ok(child) => child,
            Err(error) => {
                drop(custody);
                self.report(json!({ "event": "launch-failed", "reason": error.to_string() }));
                self.label_remaining("launch-failed");
                return None;
            }
        };
        let fd = pidfd::open(child.id());
        self.admit_spawned(child, fd, &mut custody)
    }

    fn admit_spawned(
        &mut self,
        mut child: Child,
        fd: io::Result<OwnedFd>,
        custody: &mut Custody,
    ) -> Option<(Child, u64)> {
        // A successful OS spawn counts even if custody cannot be acquired.
        self.launches += 1;
        self.report(json!({ "event": "launched", "launch": self.launches, "pid": child.id() }));
        let fd = match fd {
            Ok(fd) => fd,
            Err(error) => {
                // Still unreaped and owned only here, so kill is exact.
                let signalled = child.kill();
                self.report(json!({
                    "event": "launch-cleanup",
                    "launch": self.launches,
                    "reason": "custody-failed-before-admission",
                    "signal_sent": signalled.is_ok(),
                    "signal_error": signalled.err().map(|error| error.to_string()),
                }));
                let status = child.wait();
                self.report(json!({ "event": "launch-failed", "reason": error.to_string() }));
                self.label_remaining("launch-failed");
                self.observe_wait(status);
                return None;
            }
        };
        let token = custody.register(fd);
        Some((child, token))
    }

    /// Drives one connection, then waits on the exact child. Only a successful
    /// wait preserves the connection end as possible closure/relaunch evidence.
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
        if status.is_ok() {
            self.custody.lock().expect("custody lock").release(token);
        }
        // A failed wait leaves custody unproven until this owner exits.
        self.finish_drive(end, cause, status)
    }

    fn finish_drive(
        &mut self,
        end: ConnEnd,
        cause: &'static str,
        status: io::Result<ExitStatus>,
    ) -> (ConnEnd, &'static str) {
        if self.observe_wait(status) {
            (end, cause)
        } else {
            self.label_remaining("wait-unproven");
            (ConnEnd::WaitUnproven, cause)
        }
    }

    fn observe_wait(&mut self, status: io::Result<ExitStatus>) -> bool {
        let exit = match status {
            Ok(status) => describe(status),
            Err(error) => {
                let reason = error.to_string();
                self.report(json!({
                    "event": "wait-failed",
                    "launch": self.launches,
                    "reason": reason,
                    "reaped": "unproven",
                }));
                self.wait_failures.push(reason);
                return false;
            }
        };
        self.report(json!({ "event": "exited", "launch": self.launches, "status": exit, "reaped": "exact-child" }));
        self.exits.push(exit);
        true
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
            wait_failures: self.wait_failures.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, channel};

    fn worker() -> (Worker, Receiver<Event>) {
        let (tx, rx) = channel();
        (
            Worker {
                spec: HarnessSpec {
                    id: "test".into(),
                    argv: vec![],
                    messages: vec!["x".into()],
                },
                cap: 3,
                cwd: "/".into(),
                custody: Arc::new(Mutex::new(Custody::default())),
                tx,
                tracked: vec![Tracked {
                    message: Some(OutboundMessage::fresh("x").unwrap()),
                    label: None,
                    acked: false,
                    closures: 0,
                }],
                session: None,
                launches: 1,
                exits: vec![],
                wait_failures: vec![],
            },
            rx,
        )
    }

    // Classification seam only: this does not reproduce an OS wait fault.
    #[test]
    fn failed_wait_never_reports_successful_exit() {
        let (mut worker, rx) = worker();
        worker.observe_wait(Err(io::Error::from_raw_os_error(libc::ECHILD)));
        assert!(
            worker.record().exits.is_empty(),
            "failed wait is not a successful reap"
        );
        let reports: Vec<_> = rx
            .try_iter()
            .filter_map(|e| match e {
                Event::Report(v) => Some(v),
                _ => None,
            })
            .collect();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["event"], "wait-failed");
        assert_eq!(reports[0]["reaped"], "unproven");
    }

    #[test]
    fn failed_wait_never_counts_closure_or_authorizes_relaunch() {
        let (mut worker, rx) = worker();
        let (end, cause) = worker.finish_drive(
            ConnEnd::Gone,
            "eof-then-exit",
            Err(io::Error::from_raw_os_error(libc::ECHILD)),
        );
        let retry = worker.after_drive(end, cause);
        assert!(!retry, "failed wait cannot authorize same-key relaunch");
        assert_eq!(worker.record().messages[0].closures, 0);
        for event in rx.try_iter() {
            if let Event::Report(v) = event {
                assert!(
                    !["exited", "closure-observed", "relaunch", "outage"]
                        .contains(&v["event"].as_str().unwrap())
                );
            }
        }
    }

    // Actual owned child spawn, kill and wait; supplied custody-open error.
    // This is not a reproduction of a kernel pidfd_open failure.
    #[test]
    fn custody_failure_counts_spawn_and_successful_cleanup_reap() {
        let (mut worker, rx) = worker();
        worker.launches = 0;
        let child = Command::new("/usr/bin/true").spawn().unwrap();
        let shared = Arc::clone(&worker.custody);
        let mut custody = shared.lock().unwrap();
        assert!(
            worker
                .admit_spawned(
                    child,
                    Err(io::Error::from_raw_os_error(libc::EMFILE)),
                    &mut custody
                )
                .is_none()
        );
        let record = worker.record();
        assert_eq!(
            record.launches, 1,
            "custody failure cannot erase an actual spawn"
        );
        assert_eq!(record.exits.len(), 1);
        assert_eq!(record.messages[0].label, "launch-failed");
        assert_eq!(record.messages[0].closures, 0);
        let reports: Vec<_> = rx
            .try_iter()
            .filter_map(|e| match e {
                Event::Report(v) => Some(v),
                _ => None,
            })
            .collect();
        assert_eq!(reports[0]["event"], "launched");
        assert!(
            reports
                .iter()
                .any(|v| v["event"] == "exited" && v["reaped"] == "exact-child")
        );
    }

    #[test]
    fn cancellation_preserves_prior_cause_after_failed_wait() {
        let (mut worker, _) = worker();
        worker.tracked[0].label = Some("rejected".into());
        worker.custody.lock().unwrap().cancel();
        let (end, cause) = worker.finish_drive(
            ConnEnd::Stop,
            "",
            Err(io::Error::from_raw_os_error(libc::ECHILD)),
        );
        assert!(!worker.after_drive(end, cause));
        assert_eq!(worker.record().messages[0].label, "rejected");
        assert!(worker.record().exits.is_empty());
    }
}
