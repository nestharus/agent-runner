//! One worker thread per owned harness. Each worker blocks only on its own
//! harness, so a silent harness never holds up delivery to another.

use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use oulipoly_acp::{
    AcpClient, AtMostOnceBasis, ClientInfo, DeliveryOutcome, IdleWaitFailure, NegotiationFailure,
    NoAckCause, OutboundMessage, RequestFailure,
};
use serde_json::{Value, json};

use crate::Event;
use crate::custody::{Adopted, ReceiptWait, Root, RootSlot, WorkStdio};
use crate::live::Custody;
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
    /// Harness launches this instance requested and root PID 1 performed.
    pub launches: u32,
    /// Surviving harnesses of an earlier owner this instance attached to.
    pub reattached: u32,
    /// One entry per launched or reattached harness whose end its actual
    /// waiter (its work PID 1) reported, in order.
    pub exits: Vec<String>,
    /// Ends of earlier owners' harnesses that happened while no owner was
    /// attached, as their actual waiters reported them.
    pub prior_exits: Vec<String>,
    /// Earlier owners' harnesses whose root PID 1 is gone without a report:
    /// they ended with its namespace, status unknown.
    pub prior_unknown_ends: u32,
    /// Missing waiter reports are unknown ends, never exits.
    pub wait_failures: Vec<String>,
    /// Live harnesses left to a newer owner when this one detached.
    pub detached: u32,
    pub messages: Vec<MessageRecord>,
}

impl HarnessRecord {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "launches": self.launches,
            "reattached": self.reattached,
            "exits": self.exits,
            "prior_exits": self.prior_exits,
            "prior_unknown_ends": self.prior_unknown_ends,
            "wait_failures": self.wait_failures,
            "detached": self.detached,
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
    /// No waiter reported the harness's end: its exit is unproven.
    WaitUnproven,
}

/// What an earlier owner left for this harness, found at attach.
pub(crate) enum Prior {
    /// Still running under the attached root PID 1.
    Live(Adopted),
    /// Ended while no owner was attached; its waiter's report.
    Exited { work: i64, receipt: Value },
    /// Its root PID 1 is gone and left no report for it.
    Unknown { work: i64 },
}

/// One launched or adopted harness this worker is driving.
struct Live {
    work: i64,
    stdio: WorkStdio,
    root: Arc<Root>,
    token: u64,
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
    slot: Arc<RootSlot>,
    tx: Sender<Event>,
    tracked: Vec<Tracked>,
    session: Option<String>,
    prior: Option<Prior>,
    launches: u32,
    reattached: u32,
    exits: Vec<String>,
    prior_exits: Vec<String>,
    prior_unknown_ends: u32,
    wait_failures: Vec<String>,
    detached: u32,
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
    pub(crate) slot: Arc<RootSlot>,
    pub(crate) prior: Option<Prior>,
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
        slot,
        prior,
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
        slot,
        tx,
        tracked,
        session: harness.session,
        prior,
        launches: 0,
        reattached: 0,
        exits: Vec::new(),
        prior_exits: Vec::new(),
        prior_unknown_ends: 0,
        wait_failures: Vec::new(),
        detached: 0,
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
        result.map_err(|error| self.store_lost(&error)).ok()
    }

    /// A store write failed: report it, stop the run, and label the rest.
    /// Must be called without the custody lock held.
    fn store_lost(&mut self, error: &StoreError) {
        let label = error.label();
        self.report(json!({ "event": label, "reason": format!("{error:?}") }));
        self.custody.lock().expect("custody lock").stop(label);
        self.label_remaining(label);
    }

    fn supervise(&mut self) {
        let mut prior = self.prior.take();
        if self.head().is_none() && !self.tracked.is_empty() {
            // Recovered with every message acknowledged or durably stopped
            // (`outage`, `attempts-exhausted`): nothing to launch. A
            // surviving harness is still held until its end is reported or
            // the caller cancels; it is never dropped as gone.
            self.report(json!({ "event": "nothing-deliverable" }));
            match prior {
                Some(Prior::Live(adopted)) => {
                    let Some(live) = self.adopt(adopted) else {
                        return;
                    };
                    self.report(json!({ "event": "holding-survivor", "work": live.work }));
                    let (end, observed) = self.finish_live(live, ConnEnd::Stop, "");
                    let _ = (end, observed);
                }
                Some(Prior::Exited { work, receipt }) => {
                    self.observe_prior_exit(work, &receipt);
                }
                Some(Prior::Unknown { work }) => self.observe_prior_unknown(work),
                None => {}
            }
            return;
        }
        loop {
            let live = match prior.take() {
                Some(Prior::Live(adopted)) => match self.adopt(adopted) {
                    Some(live) => live,
                    None => return,
                },
                Some(Prior::Exited { work, receipt }) => {
                    // An end its waiter observed while no owner was attached:
                    // a closure of whatever was owed, like any observed exit.
                    self.observe_prior_exit(work, &receipt);
                    if !self.after_drive(ConnEnd::Gone, "exit-observed-while-owner-absent") {
                        return;
                    }
                    continue;
                }
                Some(Prior::Unknown { work }) => {
                    // Ended with its root namespace, status unknown: not an
                    // observed exit, so not a closure. Launch again.
                    self.observe_prior_unknown(work);
                    continue;
                }
                None => match self.launch() {
                    Some(live) => live,
                    None => return,
                },
            };
            let (end, observed) = self.drive(live);
            if !self.after_drive(end, observed) {
                return;
            }
        }
    }

    /// Takes custody of a surviving harness from an earlier owner.
    fn adopt(&mut self, adopted: Adopted) -> Option<Live> {
        let root = self.slot.current()?;
        let shared = Arc::clone(&self.custody);
        let mut custody = shared.lock().expect("custody lock");
        let token = custody.register(Arc::clone(&root), adopted.work);
        let stopped = custody.reason();
        drop(custody);
        self.reattached += 1;
        self.report(json!({
            "event": "reattached",
            "work": adopted.work,
            "pid": adopted.harness_host_pid,
            "incarnation": root.incarnation,
        }));
        if let Some(reason) = stopped {
            // Stopped before this survivor was registered: stop it the same way.
            if reason != "authority-lost" {
                root.kill(adopted.work);
            }
        }
        Some(Live {
            work: adopted.work,
            stdio: adopted.stdio,
            root,
            token,
        })
    }

    fn observe_prior_exit(&mut self, work: i64, receipt: &Value) {
        let status = receipt["harness"].as_str().map(str::to_owned);
        self.report(json!({
            "event": "prior-exit",
            "work": work,
            "status": status,
            "observer": receipt["harness_observer"],
            "work_pid1": receipt["work_pid1"],
        }));
        let outcome = status
            .clone()
            .unwrap_or_else(|| "status-unknown".to_owned());
        let observer = status.as_ref().map(|_| "work-pid1-wait");
        self.durable(|store| store.resolve_work(work, &outcome, observer));
        self.prior_exits.push(outcome);
    }

    fn observe_prior_unknown(&mut self, work: i64) {
        self.report(json!({
            "event": "prior-end-unknown",
            "work": work,
            "meaning": "ended-with-root-namespace-status-unknown",
        }));
        self.durable(|store| {
            store.resolve_work(work, "ended-with-root-namespace-status-unknown", None)
        });
        self.prior_unknown_ends += 1;
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

    /// Launches the harness through root PID 1 under the custody lock, so a
    /// concurrent cancel either prevents the launch or stops the new work.
    /// The launch is recorded before it is requested.
    fn launch(&mut self) -> Option<Live> {
        let shared = Arc::clone(&self.custody);
        let mut custody = shared.lock().expect("custody lock");
        if let Some(reason) = custody.reason() {
            drop(custody);
            self.label_remaining(reason);
            return None;
        }
        let root = match self.slot.ensure(&self.store, &self.tx) {
            Ok(root) => root,
            Err(Ok(error)) => {
                drop(custody);
                self.store_lost(&error);
                return None;
            }
            Err(Err(reason)) => {
                drop(custody);
                self.report(json!({ "event": "launch-failed", "reason": reason }));
                self.label_remaining("launch-failed");
                return None;
            }
        };
        let position = self.position;
        let begun = self
            .store
            .lock()
            .expect("store lock")
            .begin_work(position, root.incarnation);
        let work = match begun {
            Ok(work) => work,
            Err(error) => {
                drop(custody);
                self.store_lost(&error);
                return None;
            }
        };
        let spawned = match root.spawn(work, &self.argv) {
            Ok(spawned) => spawned,
            Err(reason) => {
                drop(custody);
                self.report(json!({ "event": "launch-failed", "reason": reason }));
                self.durable(|store| store.resolve_work(work, "launch-refused", None));
                self.label_remaining("launch-failed");
                return None;
            }
        };
        let token = custody.register(Arc::clone(&root), work);
        drop(custody);
        self.launches += 1;
        self.report(json!({
            "event": "launched",
            "launch": self.launches,
            "pid": spawned.harness_host_pid,
            "work": work,
            "incarnation": root.incarnation,
        }));
        let live = Live {
            work,
            stdio: spawned.stdio,
            root,
            token,
        };
        if self
            .durable(|store| store.record_work_spawned(work, spawned.harness_host_pid))
            .is_none()
        {
            self.finish_live(live, ConnEnd::Stop, "");
            return None;
        }
        if let Some(error) = spawned.exec_error {
            self.report(json!({ "event": "launch-failed", "reason": error }));
            self.label_remaining("launch-failed");
            self.finish_live(live, ConnEnd::Stop, "");
            return None;
        }
        Some(live)
    }

    /// Drives one connection, then waits for its harness's end as reported
    /// by the harness's actual waiter. Only such a report preserves the
    /// connection end as possible closure/relaunch evidence.
    fn drive(&mut self, live: Live) -> (ConnEnd, &'static str) {
        let observed = Rc::new(Observed::default());
        let stdin = live.stdio.stdin.try_clone().expect("stdin descriptor");
        let stdout = live.stdio.stdout.try_clone().expect("stdout descriptor");
        let transport = HarnessTransport::new(
            stdout,
            stdin,
            Rc::clone(&observed),
            Arc::clone(&self.slot.stop),
        );
        let mut client = AcpClient::new(
            transport,
            ClientInfo {
                name: "oulipoly-root-supervisor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        );
        let end = self.converse(&mut client);
        drop(client);
        if observed.detached.get() {
            return self.finish_live(live, ConnEnd::Stop, "");
        }
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
        self.finish_live(live, end, cause)
    }

    /// Waits for the harness's end. Our stdio copies stay open until then,
    /// so closing them is never a substitute for that observation.
    fn finish_live(
        &mut self,
        live: Live,
        end: ConnEnd,
        cause: &'static str,
    ) -> (ConnEnd, &'static str) {
        let waited = live.root.wait_receipt(live.work);
        drop(live.stdio);
        let exit = match waited {
            ReceiptWait::Receipt(receipt) => {
                self.custody
                    .lock()
                    .expect("custody lock")
                    .release(live.token);
                match receipt["harness"].as_str() {
                    Some(status) => Ok((status.to_owned(), receipt)),
                    None => Err(format!(
                        "harness-end-not-reported; work-pid1 {}",
                        receipt["work_pid1"]
                    )),
                }
            }
            ReceiptWait::Lost => Err("root-pid1-connection-lost".to_owned()),
            ReceiptWait::Detached => {
                self.custody
                    .lock()
                    .expect("custody lock")
                    .release(live.token);
                self.detached += 1;
                self.report(
                    json!({ "event": "detached", "work": live.work, "left_to": "successor" }),
                );
                return (ConnEnd::Stop, cause);
            }
        };
        let work = live.work;
        if let Ok((status, _)) = &exit {
            self.durable(|store| store.resolve_work(work, status, Some("work-pid1-wait")));
        }
        self.finish_drive(end, cause, exit)
    }

    fn finish_drive(
        &mut self,
        end: ConnEnd,
        cause: &'static str,
        exit: Result<(String, Value), String>,
    ) -> (ConnEnd, &'static str) {
        if self.observe_exit(exit) {
            (end, cause)
        } else {
            self.label_remaining("wait-unproven");
            (ConnEnd::WaitUnproven, cause)
        }
    }

    fn observe_exit(&mut self, exit: Result<(String, Value), String>) -> bool {
        let (status, receipt) = match exit {
            Ok(exit) => exit,
            Err(reason) => {
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
        self.report(json!({
            "event": "exited",
            "launch": self.launches,
            "status": status,
            "reaped": "work-pid1-wait",
            "work_pid1": receipt["work_pid1"],
        }));
        self.exits.push(status);
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
            reattached: self.reattached,
            exits: self.exits.clone(),
            prior_exits: self.prior_exits.clone(),
            prior_unknown_ends: self.prior_unknown_ends,
            wait_failures: self.wait_failures.clone(),
            detached: self.detached,
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
                slot: Arc::new(RootSlot::new(
                    dir.0.clone(),
                    1,
                    crate::sys::Isolation::current(),
                    Arc::new(crate::transport::StopSignal::new().unwrap()),
                    None,
                )),
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
                prior: None,
                launches: 1,
                reattached: 0,
                exits: vec![],
                prior_exits: vec![],
                prior_unknown_ends: 0,
                wait_failures: vec![],
                detached: 0,
            },
            rx,
            dir,
        )
    }

    // Classification seam only: this does not reproduce a lost waiter report.
    #[test]
    fn failed_wait_never_reports_successful_exit() {
        let (mut worker, rx, _dir) = worker();
        worker.observe_exit(Err("root-pid1-connection-lost".to_owned()));
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
            Err("root-pid1-connection-lost".to_owned()),
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

    #[test]
    fn cancellation_preserves_prior_cause_after_failed_wait() {
        let (mut worker, _, _dir) = worker();
        worker.tracked[0].label = Some("rejected".into());
        worker.custody.lock().unwrap().cancel();
        let (end, cause) = worker.finish_drive(
            ConnEnd::Stop,
            "",
            Err("root-pid1-connection-lost".to_owned()),
        );
        assert!(!worker.after_drive(end, cause));
        assert_eq!(worker.record().messages[0].label, "rejected");
        assert!(worker.record().exits.is_empty());
    }

    fn pending_attempt(worker: &mut Worker) -> i64 {
        // A real in-flight durable reservation, without spawning a harness.
        worker.launches = 0;
        let attempt = worker.durable(|store| store.begin_attempt(0, 0)).unwrap();
        worker.tracked[0].attempts += 1;
        attempt
    }

    fn fail_attempt_writes(dir: &Dir) {
        // Abort a real SQLite write in this test's newly created store.
        let conn = rusqlite::Connection::open(dir.0.join(crate::store::DB_FILE)).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_resolution BEFORE UPDATE ON attempt
             BEGIN SELECT RAISE(ABORT, 'test resolution refused'); END;",
        )
        .unwrap();
    }

    fn assert_loss_terminal(worker: &Worker, loss: &str) {
        let stop = worker.custody.lock().unwrap().reason();
        // Same state-derived loss selection used by the root's terminal path.
        let store_lost = matches!(stop, Some("authority-lost" | "store-failed"))
            .then_some(stop)
            .flatten();
        let record = worker.record();
        let (report, code) =
            crate::terminal_report(&[(worker.id.clone(), 1)], &[record], true, store_lost);
        assert_eq!(
            report["status"], loss,
            "real write loss must survive stop ordering"
        );
        assert_eq!(code, crate::EXIT_STORE_LOST);
        assert_eq!(report["cancel_requested"], true);
        assert_eq!(report["owed_history"], "store-holds-authoritative-state");
        assert_eq!(report["owed"], 1);
        assert_eq!(report["harnesses"][0]["messages"][0]["attempts"], 1);
        assert_eq!(report["harnesses"][0]["messages"][0]["closures"], 0);
        assert_eq!(
            report["harnesses"][0]["messages"][0]["completion"],
            "not-observed"
        );
    }

    #[test]
    fn cancel_then_refused_worker_write_reports_authority_loss() {
        let (mut worker, rx, dir) = worker();
        let attempt = pending_attempt(&mut worker);
        worker.custody.lock().unwrap().cancel();
        std::fs::remove_file(dir.0.join(crate::store::LOCK_FILE)).unwrap();
        let successor = Store::claim(&dir.0, None).unwrap();
        assert_eq!(successor.store.generation(), 2);
        assert_eq!(successor.classified_unknown, 1);
        assert!(
            worker
                .durable(|store| store.resolve_attempt(attempt, "no-ack:transport-closed"))
                .is_none()
        );
        assert!(
            rx.try_iter()
                .any(|event| matches!(event, Event::Report(v) if v["event"] == "authority-lost"))
        );
        assert_eq!(worker.record().messages[0].label, "authority-lost");
        assert_loss_terminal(&worker, "authority-lost");
    }

    #[test]
    fn cancel_then_failed_worker_write_reports_store_failure() {
        let (mut worker, rx, dir) = worker();
        let attempt = pending_attempt(&mut worker);
        fail_attempt_writes(&dir);
        worker.custody.lock().unwrap().cancel();
        assert!(
            worker
                .durable(|store| store.resolve_attempt(attempt, "no-ack:transport-closed"))
                .is_none()
        );
        assert!(
            rx.try_iter()
                .any(|event| matches!(event, Event::Report(v) if v["event"] == "store-failed"))
        );
        assert_eq!(worker.record().messages[0].label, "store-failed");
        assert_loss_terminal(&worker, "store-failed");
    }

    #[test]
    fn failed_then_refused_worker_write_escalates_and_keeps_message_cause() {
        let (mut worker, rx, dir) = worker();
        let attempt = pending_attempt(&mut worker);
        // A per-message cause is independent of the run-level store loss.
        worker.tracked[0].label = Some("rejected".into());
        fail_attempt_writes(&dir);
        assert!(
            worker
                .durable(|store| store.resolve_attempt(attempt, "rejected"))
                .is_none()
        );
        std::fs::remove_file(dir.0.join(crate::store::LOCK_FILE)).unwrap();
        let conn = rusqlite::Connection::open(dir.0.join(crate::store::DB_FILE)).unwrap();
        conn.execute_batch("DROP TRIGGER refuse_resolution")
            .unwrap();
        let successor = Store::claim(&dir.0, None).unwrap();
        assert_eq!(successor.classified_unknown, 1);
        assert!(
            worker
                .durable(|store| store.resolve_attempt(attempt, "rejected"))
                .is_none()
        );
        let events: Vec<_> = rx
            .try_iter()
            .filter_map(|event| match event {
                Event::Report(v) => Some(v["event"].as_str().unwrap().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(events, ["store-failed", "authority-lost"]);
        worker.custody.lock().unwrap().cancel();
        assert_eq!(worker.record().messages[0].label, "rejected");
        assert_loss_terminal(&worker, "authority-lost");
    }
}
