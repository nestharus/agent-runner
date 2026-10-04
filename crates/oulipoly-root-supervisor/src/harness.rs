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

use crate::Event;
use crate::pidfd::{self, Custody};
use crate::store::{DurableAck, DurableHarness, Store, StoreError};
use crate::transport::{HarnessTransport, Observed};

/// Final state of one message, as reported in the terminal report.
#[derive(Debug, Clone)]
pub struct MessageRecord {
    pub index: usize,
    /// No durably recorded insertion acknowledgement.
    pub owed: bool,
    /// `accepted`, `duplicate-unknown`, or the reason it is still owed.
    pub label: String,
    pub at_most_once: bool,
    pub basis: Option<String>,
    pub recovered: bool,
    /// Owner generation that recorded the acknowledgement.
    pub ack_generation: Option<i64>,
    /// Attempts recorded in the store by every generation, this one included.
    pub attempts: u32,
    /// Earlier-owner attempts with no outcome, classified unknown by this
    /// instance. Each may or may not have been sent or inserted.
    pub prior_unknown: u32,
    /// Observed no-acknowledgement closures charged to this message, by
    /// every generation.
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
                "state": if message.owed { "owed" } else { "acknowledged" },
                "label": message.label,
                "at_most_once": message.at_most_once,
                "basis": message.basis,
                "recovered": message.recovered,
                "ack_generation": message.ack_generation,
                "attempts": message.attempts,
                "prior_unknown": message.prior_unknown,
                "closures": message.closures,
                "completion": "not-observed",
            })).collect::<Vec<_>>(),
        })
    }
}

struct Tracked {
    message: OutboundMessage,
    /// Why this instance stopped trying, while still owed.
    label: Option<String>,
    /// A durably recorded acknowledgement, by any generation.
    ack: Option<DurableAck>,
    closures: u32,
    attempts: u32,
    prior_unknown: u32,
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
    id: String,
    argv: Vec<String>,
    position: usize,
    cap: u32,
    attempt_cap: u32,
    cwd: String,
    custody: Arc<Mutex<Custody>>,
    store: Arc<Mutex<Store>>,
    tx: Sender<Event>,
    tracked: Vec<Tracked>,
    session: Option<String>,
    launches: u32,
    exits: Vec<String>,
    wait_failures: Vec<String>,
}

/// What one worker is given: its durable harness record and shared state.
pub(crate) struct Assignment {
    pub(crate) position: usize,
    pub(crate) harness: DurableHarness,
    pub(crate) cap: u32,
    pub(crate) attempt_cap: u32,
    pub(crate) cwd: String,
    pub(crate) custody: Arc<Mutex<Custody>>,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) tx: Sender<Event>,
}

pub(crate) fn run(assignment: Assignment) {
    let Assignment {
        position,
        harness,
        cap,
        attempt_cap,
        cwd,
        custody,
        store,
        tx,
    } = assignment;
    let tracked = harness
        .messages
        .into_iter()
        .map(|durable| Tracked {
            // A durable stop (`outage`, `attempts-exhausted`) keeps the
            // message from being retried.
            label: durable.stop,
            message: durable.message,
            ack: durable.ack,
            closures: durable.closures,
            attempts: durable.attempts,
            prior_unknown: durable.prior_unknown,
        })
        .collect();
    let mut worker = Worker {
        id: harness.id,
        argv: harness.argv,
        position,
        cap,
        attempt_cap,
        cwd,
        custody,
        store,
        tx,
        tracked,
        session: harness.session,
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
        value["harness"] = Value::String(self.id.clone());
        let _ = self.tx.send(Event::Report(value));
    }

    fn cancelled(&self) -> bool {
        self.custody.lock().expect("custody lock").cancelled()
    }

    /// Index of the first message still to be delivered.
    fn head(&self) -> Option<usize> {
        self.tracked
            .iter()
            .position(|tracked| tracked.ack.is_none() && tracked.label.is_none())
    }

    fn label_remaining(&mut self, label: &str) {
        for tracked in &mut self.tracked {
            if tracked.ack.is_none() && tracked.label.is_none() {
                tracked.label = Some(label.to_owned());
            }
        }
    }

    /// Why the run stopped: the caller's cancel or lost store authority.
    fn stop_reason(&self) -> &'static str {
        self.custody
            .lock()
            .expect("custody lock")
            .reason()
            .unwrap_or("cancelled")
    }

    /// Runs one store write. On failure this owner may no longer record
    /// anything, so it stops the whole run (signalling its own harnesses)
    /// rather than deliver what it cannot record.
    fn durable<T>(&mut self, write: impl FnOnce(&mut Store) -> Result<T, StoreError>) -> Option<T> {
        let result = write(&mut self.store.lock().expect("store lock"));
        match result {
            Ok(value) => Some(value),
            Err(error) => {
                let label = error.label();
                self.report(json!({ "event": label, "reason": format!("{error:?}") }));
                self.custody.lock().expect("custody lock").stop(label);
                self.label_remaining(label);
                None
            }
        }
    }

    fn supervise(&mut self) {
        if self.head().is_none() && !self.tracked.is_empty() {
            // Recovered with every message acknowledged or durably stopped
            // (`outage`, `attempts-exhausted`): nothing to launch.
            self.report(json!({ "event": "nothing-deliverable" }));
            return;
        }
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
            let reason = self.stop_reason();
            self.label_remaining(reason);
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
        let position = self.position;
        let (cap, attempt_cap) = (self.cap, self.attempt_cap);
        let Some((closures, stop)) =
            self.durable(|store| store.record_closure(position, index, cap, attempt_cap))
        else {
            return false;
        };
        self.tracked[index].closures = closures;
        self.report(json!({
            "event": "closure-observed",
            "index": index,
            "cause": observed,
            "closures": closures,
        }));
        if let Some(stop) = stop {
            self.tracked[index].label = Some(stop.to_owned());
            self.report(json!({
                "event": stop,
                "index": index,
                "closures": closures,
                "attempts": self.tracked[index].attempts,
            }));
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
        if let Some(reason) = custody.reason() {
            drop(custody);
            self.label_remaining(reason);
            return None;
        }
        let spawned = Command::new(&self.argv[0])
            .args(&self.argv[1..])
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
            Ok(Some(id)) => {
                // Durable before any prompt, so a successor resumes this
                // session string (trusted scope, not continuity proof).
                let position = self.position;
                if self
                    .durable(|store| store.set_session(position, &id))
                    .is_none()
                {
                    return ConnEnd::Stop;
                }
                self.session = Some(id);
            }
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
        let position = self.position;
        while let Some(index) = self.head() {
            // The attempt is durable before it is sent, so no successor can
            // miss an attempt that may have been inserted.
            let Some(attempt) = self.durable(|store| store.begin_attempt(position, index)) else {
                return ConnEnd::Stop;
            };
            self.tracked[index].attempts += 1;
            let outcome = client.submit(&session, &mut self.tracked[index].message);
            let (resolution, end) = match &outcome {
                DeliveryOutcome::Accepted(acceptance)
                | DeliveryOutcome::DuplicateUnknown(acceptance) => {
                    let ack = DurableAck {
                        label: if acceptance.at_most_once {
                            "accepted"
                        } else {
                            "duplicate-unknown"
                        }
                        .to_owned(),
                        basis: acceptance.basis.map(|basis| basis_label(basis).to_owned()),
                        recovered: acceptance.recovered,
                        generation: self.store.lock().expect("store lock").generation(),
                    };
                    // Reported only once durable: an unrecorded ACK leaves
                    // the attempt unresolved for a successor to classify.
                    if self
                        .durable(|store| store.record_ack(position, index, attempt, &ack))
                        .is_none()
                    {
                        return ConnEnd::Stop;
                    }
                    self.report(json!({
                        "event": "ack",
                        "index": index,
                        "label": ack.label,
                        "basis": ack.basis,
                        "recovered": ack.recovered,
                        "durable": true,
                    }));
                    self.tracked[index].ack = Some(ack);
                    continue;
                }
                DeliveryOutcome::Rejected { code, .. } => {
                    self.tracked[index].label = Some("rejected".to_owned());
                    self.report(json!({ "event": "rejected", "index": index, "code": code }));
                    ("rejected", None)
                }
                DeliveryOutcome::NotAcknowledged(NoAckCause::PeerGone) => {
                    ("no-ack:transport-closed", Some(ConnEnd::Gone))
                }
                DeliveryOutcome::NotAcknowledged(NoAckCause::InvalidResponse(_)) => {
                    self.tracked[index].label = Some("invalid-response".to_owned());
                    self.report(json!({ "event": "invalid-response", "index": index }));
                    ("no-ack:invalid-response", Some(ConnEnd::Stop))
                }
                DeliveryOutcome::NotNegotiated => {
                    self.tracked[index].label = Some("not-negotiated".to_owned());
                    ("not-sent", Some(ConnEnd::Stop))
                }
                DeliveryOutcome::SessionMismatch => {
                    self.tracked[index].label = Some("session-mismatch".to_owned());
                    ("not-sent", Some(ConnEnd::Stop))
                }
            };
            if self
                .durable(|store| store.resolve_attempt(attempt, resolution))
                .is_none()
            {
                return ConnEnd::Stop;
            }
            if let Some(end) = end {
                return end;
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
                // Only a durably recorded acknowledgement counts; an ACK held
                // in memory but not recorded leaves the message owed.
                let ack = tracked.ack.as_ref();
                let label = match (ack, &tracked.label) {
                    (Some(ack), _) => ack.label.clone(),
                    (None, Some(label)) => label.clone(),
                    (None, None) => "not-attempted".to_owned(),
                };
                MessageRecord {
                    index,
                    owed: ack.is_none(),
                    label,
                    at_most_once: ack.is_some_and(|ack| ack.label == "accepted"),
                    basis: ack.and_then(|ack| ack.basis.clone()),
                    recovered: ack.is_some_and(|ack| ack.recovered),
                    ack_generation: ack.map(|ack| ack.generation),
                    attempts: tracked.attempts,
                    prior_unknown: tracked.prior_unknown,
                    closures: tracked.closures,
                }
            })
            .collect();
        HarnessRecord {
            id: self.id.clone(),
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

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn worker() -> (Worker, Receiver<Event>, Dir) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = Dir(std::env::temp_dir().join(format!(
            "root-harness-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )));
        let intent = crate::Intent {
            outage_closure_cap: 3,
            delivery_attempt_cap: 10,
            cwd: "/".into(),
            harnesses: vec![crate::HarnessSpec {
                id: "test".into(),
                argv: vec![],
                messages: vec!["x".into()],
            }],
        };
        let mut claimed = Store::claim(&dir.0, Some(&intent)).unwrap();
        let harness = claimed.harnesses.remove(0);
        let (tx, rx) = channel();
        (
            Worker {
                id: "test".into(),
                argv: vec![],
                position: 0,
                cap: 3,
                attempt_cap: 10,
                cwd: "/".into(),
                custody: Arc::new(Mutex::new(Custody::default())),
                store: Arc::new(Mutex::new(claimed.store)),
                tx,
                tracked: harness
                    .messages
                    .into_iter()
                    .map(|durable| Tracked {
                        message: durable.message,
                        label: None,
                        ack: None,
                        closures: 0,
                        attempts: 0,
                        prior_unknown: 0,
                    })
                    .collect(),
                session: None,
                launches: 1,
                exits: vec![],
                wait_failures: vec![],
            },
            rx,
            dir,
        )
    }

    // Classification seam only: this does not reproduce an OS wait fault.
    #[test]
    fn failed_wait_never_reports_successful_exit() {
        let (mut worker, rx, _dir) = worker();
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
        let (mut worker, rx, _dir) = worker();
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
        let (mut worker, rx, _dir) = worker();
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
        let (mut worker, _, _dir) = worker();
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
