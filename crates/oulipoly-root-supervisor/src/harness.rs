//! One worker thread per owned harness. Each worker blocks only on its own
//! harness, so a silent harness never holds up delivery to another.

use std::fs::{self, File};
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_provider_contract::acp::resident::{
    self, PreparedEndpoint, SessionStart, StartedSession,
};
use agent_provider_contract::acp::{
    AcpClient, AtMostOnceBasis, ClientInfo, DeliveryOutcome, IdleWaitFailure, NegotiationFailure,
    NoAckCause, OutboundMessage, RequestFailure, SessionEvent,
};
use serde_json::{Value, json};

use crate::bash::{self, OpenInput, Views};
use crate::children::{ChildLink, Registry};
use crate::conversation::{Closing, FollowUp, Inbox, InputHold};
use crate::custody::{self, Adopted, PidNs, ReceiptWait, Root, RootSlot, SpawnError, WorkStdio};
use crate::live::Custody;
use crate::store::{
    Admission, AttemptOutcome, DurableAck, DurableHarness, EarlierRef, Store, StoreError,
};
use crate::transport::{self, HarnessTransport, Observed};
use crate::{Endpoint, Event};

/// Environment variable naming the socket a `unix-socket` harness listens on.
pub const SOCKET_ENV: &str = "OULIPOLY_ACP_V2_SOCKET";
/// How often a worker retries connecting to a socket not yet listening.
const CONNECT_RETRY: Duration = Duration::from_millis(50);

/// The socket path this root's owner chooses for one work's harness.
pub(crate) fn socket_path(store: &Path, work: i64) -> PathBuf {
    store
        .join("acp")
        .join(format!("{}.sock", custody::work_name(work)))
}

/// Why no connection to a socket endpoint was made.
enum Unconnected {
    /// The run detached; the harness is left to a successor.
    Detached,
    /// The harness's end was reported (or can no longer be heard) first.
    Ended,
    /// Connecting failed other than by the socket not listening yet.
    Failed(String),
}

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
    /// All attempts with unresolved insertion outcomes, including recorded
    /// invalid/closed responses. A later refusal does not remove them.
    pub unresolved_attempts: u32,
    /// Observed no-acknowledgement closures charged to this message, by
    /// every generation.
    pub closures: u32,
    /// A caller's follow-up to the live conversation, not part of the intent.
    pub follow_up: bool,
    /// A tagged idle covering its insertion was durably recorded, by any
    /// generation (the agent's tag, not processing).
    pub turn_ended: bool,
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
    /// Earlier owners' harnesses that ended with their root's or their
    /// work's namespace without their own wait being reported: status
    /// unknown, not closures.
    pub prior_unknown_ends: u32,
    /// Missing waiter reports are unknown ends, never exits.
    pub wait_failures: Vec<String>,
    /// Live harnesses left to a newer owner when this one detached.
    pub detached: u32,
    /// This owner attempted to stop the harness for close after tagged
    /// turn ends. Request delivery and the actual waited exit are separate
    /// observations (`close-stopping.signalled` and `exits`).
    pub close_stop_attempted: bool,
    /// Recovery-time snapshot of this owner's conversation decision and
    /// inherited account, not final usability or final completion truth.
    pub recovered_conversation: Option<Value>,
    pub messages: Vec<MessageRecord>,
}

impl HarnessRecord {
    /// A harness that was never launched.
    pub(crate) fn empty(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            launches: 0,
            reattached: 0,
            exits: Vec::new(),
            prior_exits: Vec::new(),
            prior_unknown_ends: 0,
            wait_failures: Vec::new(),
            detached: 0,
            close_stop_attempted: false,
            recovered_conversation: None,
            messages: Vec::new(),
        }
    }

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
            "close": if self.close_stop_attempted { "owner-stop-attempted-after-turns-ended" } else { "not-stopped-by-close" },
            "recovered_conversation": self.recovered_conversation,
            "messages": self.messages.iter().map(|message| json!({
                "index": message.index,
                "origin": if message.follow_up { "follow-up" } else { "intent" },
                "state": if message.owed { "owed" } else { "acknowledged" },
                "label": message.label,
                "at_most_once": message.at_most_once,
                "basis": message.basis,
                "recovered": message.recovered,
                "ack_generation": message.ack_generation,
                "attempts": message.attempts,
                "prior_unknown": message.prior_unknown,
                "unresolved_attempts": message.unresolved_attempts,
                "owed_scope": "insertion-ack-absence",
                "closures": message.closures,
                "turn_end": if message.turn_ended { "tagged-idle-recorded" } else { "not-recorded" },
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
    /// Attempts of any generation classified insertion-unresolved.
    unknown_attempts: u32,
    /// A tagged idle covering its insertion is durably recorded.
    turn_ended: bool,
    follow_up: bool,
}

impl Tracked {
    /// Called only after the attempt's resolution commits. A failed write
    /// keeps the attempt unresolved in both the live and restored accounts.
    fn resolve_attempt(&mut self, outcome: &str) {
        if !AttemptOutcome::classify(Some(outcome)).unresolved() {
            self.unknown_attempts -= 1;
        }
    }
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
    /// Ended while no owner was attached; root PID 1's receipt for it,
    /// which may lack the harness's own wait status.
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
    endpoint: Endpoint,
    resident: Option<agent_provider_contract::resident_session::ResidentPrepareResult>,
    started: Option<StartedSession>,
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
    views: Views,
    /// Native user message ids this instance's agent messages named as
    /// their parent.
    answered: std::collections::HashSet<String>,
    /// The caller's further input to this harness's live conversation.
    inbox: Arc<Inbox>,
    closing: Arc<Closing>,
    hold: Arc<InputHold>,
    close_stop_attempted: bool,
    children: Arc<Registry>,
    /// Set for a registered child: its requester and lineage.
    child: Option<Arc<ChildLink>>,
    /// Background-run completions taken from the inbox while an earlier
    /// input was open: held, in order, until nothing is open.
    pending_completions: std::collections::VecDeque<FollowUp>,
    /// Admitted completion inputs: message index to background work, by any
    /// generation (restored from the durable link at recovery).
    completion_of: std::collections::HashMap<usize, i64>,
    /// The requester work of a recovered conversation that is live-usable
    /// now: only it may be offered completions an earlier owner accepted.
    recovered_recipient: Option<i64>,
    /// The last close disposition reported in the live conversation.
    close_reported: Option<&'static str>,
    /// The caller's recovery asked to continue the attached root's work
    /// (`continue-attached`): a settled survivor may be conversed with
    /// again if its harness declared it can be.
    continue_attached: bool,
    recovered_conversation: Option<Value>,
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
    pub(crate) views: Views,
    pub(crate) inbox: Arc<Inbox>,
    pub(crate) closing: Arc<Closing>,
    /// The root's input hold (caller input only).
    pub(crate) hold: Arc<InputHold>,
    pub(crate) children: Arc<Registry>,
    pub(crate) continue_attached: bool,
}

/// What a registered child's worker is given (see [`crate::children`]).
pub(crate) struct ChildAssignment {
    pub(crate) position: usize,
    pub(crate) harness: DurableHarness,
    pub(crate) cwd: String,
    pub(crate) custody: Arc<Mutex<Custody>>,
    pub(crate) store: Arc<Mutex<Store>>,
    pub(crate) slot: Arc<RootSlot>,
    pub(crate) tx: Sender<Event>,
    pub(crate) views: Views,
    pub(crate) children: Arc<Registry>,
    pub(crate) link: Arc<ChildLink>,
}

/// Drives one registered child to its end and returns its record. One
/// attempt, one closure: never relaunched or delivered to again. Its
/// conversation is closed from the start, so its harness is stopped once
/// its one input's tagged turn ended.
pub(crate) fn run_child(assignment: ChildAssignment) -> HarnessRecord {
    let ChildAssignment {
        position,
        harness,
        cwd,
        custody,
        store,
        slot,
        tx,
        views,
        children,
        link,
    } = assignment;
    let closing = Arc::new(Closing::default());
    closing.request();
    let inbox = match Inbox::new() {
        Ok(inbox) => Arc::new(inbox),
        Err(error) => {
            link.observe(&json!({ "event": "launch-failed", "reason": format!("inbox: {error}") }));
            return HarnessRecord::empty(&harness.id);
        }
    };
    let mut worker = Worker::build(
        Assignment {
            position,
            harness,
            cap: 1,
            attempt_cap: 1,
            cwd,
            custody,
            store,
            slot,
            prior: None,
            tx,
            views,
            inbox,
            closing,
            hold: Arc::default(),
            children,
            continue_attached: false,
        },
        Some(link),
    );
    worker.supervise();
    worker.record()
}

pub(crate) fn run(assignment: Assignment) {
    let mut worker = Worker::build(assignment, None);
    worker.view(|view| {
        view.id.clone_from(&worker.id);
        view.session.clone_from(&worker.session);
    });
    worker.supervise();
    let record = worker.record();
    let _ = worker.tx.send(Event::Done(record));
}

impl Worker {
    fn build(assignment: Assignment, child: Option<Arc<ChildLink>>) -> Self {
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
            views,
            inbox,
            closing,
            hold,
            children,
            continue_attached,
        } = assignment;
        // Completion links durably recorded by earlier generations: each one
        // is reconciled from its own message, never admitted again.
        let completion_of = harness
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, durable)| durable.completion_work.map(|work| (index, work)))
            .collect();
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
                unknown_attempts: durable.unknown_attempts,
                turn_ended: durable.turn_ended,
                follow_up: durable.follow_up,
            })
            .collect();
        Worker {
            id: harness.id,
            argv: harness.argv,
            endpoint: harness.endpoint,
            resident: harness.resident,
            started: None,
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
            views,
            answered: std::collections::HashSet::new(),
            inbox,
            closing,
            hold,
            close_stop_attempted: false,
            children,
            child,
            pending_completions: std::collections::VecDeque::new(),
            completion_of,
            recovered_recipient: None,
            close_reported: None,
            continue_attached,
            recovered_conversation: None,
        }
    }

    /// Completions still owed to this harness (background runs not yet
    /// carried by an input whose turn ended, nor settled undelivered).
    fn owed_async(&self) -> usize {
        self.views
            .lock()
            .expect("views")
            .get(self.position)
            .map_or(0, |view| view.owed_async.len())
    }

    fn settle_async(&self, work: i64, resolution: &str, reason: Option<&str>) -> bool {
        bash::settle_async(
            &self.views,
            &self.store,
            &self.tx,
            self.position,
            work,
            resolution,
            reason,
        )
    }

    /// The conversation ended: settle only completions whose durable facts
    /// support non-delivery. Admitted uncertainty remains owed and unresolved,
    /// for later exact-link ACK/turn-end reconciliation, from any generation.
    /// A recovered completion not admitted by this owner is released, not
    /// resolved: its requester work may outlive this conversation, and its
    /// end, once recorded, settles it (see [`Self::orphaned_completions`]).
    fn settle_unfinished_completions(&mut self, queued: Vec<FollowUp>) {
        self.recovered_recipient = None;
        let held: Vec<FollowUp> = self.pending_completions.drain(..).chain(queued).collect();
        let mut settled: Vec<(i64, String)> = Vec::new();
        for follow_up in held {
            let Some(work) = follow_up.completion else {
                continue;
            };
            if follow_up.recovered.is_some() {
                bash::release_recovered(
                    &self.views,
                    &self.tx,
                    self.position,
                    work,
                    "conversation-ended-before-admission",
                );
            } else {
                settled.push((work, "conversation-ended-before-admission".to_owned()));
            }
        }
        for (&index, &work) in &self.completion_of {
            let tracked = &self.tracked[index];
            let reason = match (&tracked.ack, &tracked.label) {
                (Some(_), _) => "acknowledged-turn-end-not-observed".to_owned(),
                (None, Some(label)) => format!("not-acknowledged: {label}"),
                (None, None) => "not-acknowledged: conversation-ended".to_owned(),
            };
            settled.push((work, reason));
        }
        for (work, reason) in settled {
            self.settle_async(work, "undelivered", Some(&reason));
        }
    }

    /// Admits one completion as this harness's next owed input (durable
    /// first, like a caller's follow-up). Returns false on store loss.
    fn admit_completion(&mut self, follow_up: FollowUp) -> bool {
        let Some(work) = follow_up.completion else {
            return true;
        };
        let position = self.position;
        // A recovered completion must still belong to the requester work of
        // this recovered conversation; the store refuses a second admission.
        let recipient = follow_up.recovered.and(self.recovered_recipient);
        let admitted = self.durable(|store| {
            store.admit_follow_up(
                position,
                follow_up.control,
                follow_up.caller_ref.as_deref(),
                &follow_up.text,
                false,
                Some((work, recipient)),
            )
        });
        let (index, message) = match admitted {
            Some(Admission::Admitted(index, message)) => (index, message),
            Some(Admission::NotReoffered(reason)) => {
                self.report(json!({
                    "event": "bash-async-completion-not-reoffered",
                    "work": work,
                    "reason": reason,
                    "admitted": false,
                    "meaning": "one logical admission per accepted completion in this root; its earlier admission or resolution stands",
                }));
                bash::release_recovered(&self.views, &self.tx, position, work, &reason);
                return true;
            }
            Some(Admission::Duplicate(_)) | None => {
                if follow_up.recovered.is_some() {
                    bash::release_recovered(&self.views, &self.tx, position, work, "store-lost");
                } else {
                    self.settle_async(work, "undelivered", Some("store-lost"));
                }
                return false;
            }
        };
        debug_assert_eq!(index, self.tracked.len());
        self.tracked.push(Tracked {
            message,
            label: None,
            ack: None,
            closures: 0,
            attempts: 0,
            prior_unknown: 0,
            unknown_attempts: 0,
            turn_ended: false,
            follow_up: true,
        });
        self.completion_of.insert(index, work);
        let _ = self.tx.send(Event::Admitted(self.position));
        self.report(json!({
            "event": "bash-async-completion-admitted",
            "work": work,
            "input": index,
            "stage": "durably-committed",
            "durable": true,
            "recovered": follow_up.recovered.map(|accepted| json!({ "accepted_generation": accepted })),
            "meaning": "owed input to this harness; not yet acknowledged",
        }));
        true
    }

    /// Takes up, for this recovered live-usable conversation, every
    /// completion an earlier owner accepted for this same requester work,
    /// whose run ended and which no generation admitted. Each goes through
    /// the same hold/admission as a fresh completion.
    fn recover_completions(&mut self, work: i64) {
        let found = {
            let store = self.store.lock().expect("store lock");
            store.recoverable_completions(work).map(|found| {
                found
                    .iter()
                    .map(|completion| bash::recovered_follow_up(&store, completion))
                    .collect::<Vec<_>>()
            })
        };
        match found {
            Ok(found) => {
                for follow_up in found {
                    self.take_recovered(follow_up);
                }
            }
            Err(error) => self.report(json!({
                "event": "bash-async-recovery-unread",
                "reason": format!("store-unread: {error}"),
                "meaning": "inherited completions keep their durable state; none is offered or resolved",
            })),
        }
    }

    /// Queues one recovered completion behind any open input, once.
    fn take_recovered(&mut self, follow_up: FollowUp) {
        let Some(work) = follow_up.completion else {
            return;
        };
        if self.recovered_recipient.is_none() {
            self.report(json!({
                "event": "bash-async-completion-not-offered",
                "work": work,
                "reason": "recipient-not-recovered-live-usable",
                "completion": "unchanged",
            }));
            return;
        }
        let taken = self
            .pending_completions
            .iter()
            .any(|held| held.completion == Some(work))
            || self
                .completion_of
                .values()
                .any(|admitted| *admitted == work);
        if taken || !bash::owe_recovered(&self.views, &self.tx, self.position, work) {
            return;
        }
        self.report(json!({
            "event": "bash-async-completion-recovered",
            "work": work,
            "accepted_generation": follow_up.recovered,
            "recipient_work": self.recovered_recipient,
            "meaning": "an earlier owner's never-admitted async promise, offered once to the same requester work; admitted under this owner's generation only when no earlier input is open or unresolved",
        }));
        self.pending_completions.push_back(follow_up);
    }

    /// An earlier generation admitted this completion (this owner carries
    /// it only as that same message): its ACK and tagged turn end, now
    /// durable, settle it `turn-ended`. Nothing else here resolves it.
    fn reconcile_completion(&mut self, work: i64, index: usize) {
        if self.durable(|store| store.reconcile_completion(work)) == Some(true) {
            self.report(json!({
                "event": "bash-async-completion-reconciled",
                "work": work,
                "input": index,
                "completion": "turn-ended",
                "meaning": "the input an earlier generation admitted for this completion has a durable ACK and tagged turn end; transport only, not processing",
            }));
        }
    }

    /// Records undelivered the earlier owners' never-admitted completions
    /// owed to `work`, whose end was just recorded: nothing was sent to it,
    /// and it takes no further input.
    fn orphaned_completions(&mut self, work: i64) {
        if let Some(works) = self.durable(|store| store.settle_orphaned_completions(work))
            && !works.is_empty()
        {
            self.report(json!({
                "event": "inherited-completion-undelivered",
                "requester_work": work,
                "works": works,
                "reason": "requester-ended-before-admission",
                "meaning": "accepted by an earlier owner and never admitted as an input; its requester work's end is recorded",
            }));
        }
    }

    /// Admits the next held completion once no earlier input is open.
    /// Returns None when nothing was admitted, Some(false) on store loss.
    fn admit_held_completion(&mut self) -> Option<bool> {
        if self.pending_completions.is_empty()
            || self.head().is_some()
            || !self.no_open_input()
            || self.cancelled()
        {
            return None;
        }
        let follow_up = self.pending_completions.pop_front()?;
        Some(self.admit_completion(follow_up))
    }

    fn view(&self, change: impl FnOnce(&mut bash::View)) {
        let mut views = self.views.lock().expect("views");
        if let Some(view) = views.get_mut(self.position) {
            change(view);
        }
    }

    /// Lets the Bash ingress attribute processes in this work's namespace
    /// to this harness while the work is live.
    fn show_work(&self, work: i64, pidns: Option<PidNs>) {
        match pidns {
            Some(pidns) => self.view(|view| view.works.push((work, pidns))),
            None => self.report(json!({
                "event": "bash-unattributable",
                "work": work,
                "reason": "work-pidns-unreported",
            })),
        }
    }

    fn report(&self, mut value: Value) {
        value["harness"] = Value::String(self.id.clone());
        if let Some(link) = &self.child {
            value["child"] = link.marker();
            link.observe(&value);
        }
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
            // surviving harness is never dropped as gone: it is conversed
            // with again or held (see `settled_survivor`).
            self.report(json!({ "event": "nothing-deliverable" }));
            match prior.take() {
                Some(Prior::Live(adopted)) => {
                    let Some(live) = self.adopt(adopted) else {
                        return;
                    };
                    // A reopened conversation whose harness then closed with
                    // a new input owed continues as any closure does.
                    if !self.settled_survivor(live) {
                        return;
                    }
                }
                Some(Prior::Exited { work, receipt }) => {
                    let _ = self.observe_prior_exit(work, &receipt);
                    return;
                }
                Some(Prior::Unknown { work }) => {
                    self.observe_prior_unknown(work);
                    return;
                }
                None => return,
            }
        }
        loop {
            let live = match prior.take() {
                Some(Prior::Live(adopted)) => match self.adopt(adopted) {
                    Some(live) if self.cancelled() => {
                        // Stopped before or while this survivor was taken
                        // over: it is killed (see `adopt`) and never
                        // connected to, so nothing is resubmitted to it.
                        let (end, observed) = self.finish_live(live, ConnEnd::Stop, "");
                        let _ = self.after_drive(end, observed);
                        return;
                    }
                    Some(live) => live,
                    None => return,
                },
                Some(Prior::Exited { work, receipt }) => {
                    // An end its waiter observed while no owner was attached:
                    // a closure of whatever was owed, like any observed exit.
                    // Without the harness's own wait it is an unknown end:
                    // not a closure, so launch again.
                    if self.observe_prior_exit(work, &receipt)
                        && !self.after_drive(ConnEnd::Gone, "exit-observed-while-owner-absent")
                    {
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

    /// A surviving harness recovered with every input settled. Under an
    /// explicit `continue-attached` recovery it is conversed with again
    /// only when every input's turn is durably known to have ended (or the
    /// input conclusively ended without one), and the harness of this very
    /// work declared the live reattachment contract when it was negotiated
    /// with; then the owner negotiates on the same live process again, and
    /// that harness must declare it again. Nothing settled is resubmitted.
    /// Otherwise the survivor is held, with why, and a close ends it when
    /// its turns are known to have ended (see [`Self::hold`]).
    /// Returns whether a relaunch is authorized (see [`Self::after_drive`]).
    fn settled_survivor(&mut self, live: Live) -> bool {
        let turns = self.settled_turns();
        let declared = self
            .store
            .lock()
            .expect("store lock")
            .work_negotiation(live.work);
        let capability = match &declared {
            Ok(Some(true)) => "declared",
            Ok(Some(false)) => "absent",
            Ok(None) => "unrecorded",
            Err(_) => "unread",
        };
        let decision = if self.cancelled() {
            Err(("unavailable", self.stop_reason().to_owned()))
        } else if !self.continue_attached {
            Err(("unavailable", "not-requested".to_owned()))
        } else if self.closing.requested() {
            Err(("unavailable", "input-closed".to_owned()))
        } else if let Err(reason) = turns {
            Err(("unknown", reason.to_owned()))
        } else if self.session.is_none() {
            Err(("unavailable", "no-recorded-session".to_owned()))
        } else {
            match declared {
                Ok(Some(true)) => Ok(()),
                Ok(Some(false)) => Err(("unavailable", "capability-absent".to_owned())),
                Ok(None) => Err(("unknown", "capability-unrecorded".to_owned())),
                Err(_) => Err(("unknown", "capability-unread".to_owned())),
            }
        };
        let facts = json!({
            "work": live.work,
            "purpose": if self.continue_attached { "continue-attached" } else { "not-continue-attached" },
            "turns": if turns.is_ok() { "settled" } else { "unknown" },
            "turns_reason": turns.err(),
            "capability": capability,
            "session": self.session,
            "inherited_async": bash::inherited_summary(&self.store, Some(self.position)),
        });
        match decision {
            Ok(()) => {
                let (end, observed) = self.reopen(live, facts);
                self.after_drive(end, observed)
            }
            Err((state, reason)) => {
                self.unconversed(&facts, state, &reason);
                self.report(json!({ "event": "holding-survivor", "work": live.work, "conversation": state }));
                let _ = self.hold(live, turns.is_ok(), ConnEnd::Stop, "");
                false
            }
        }
    }

    /// Whether every input's turn is durably settled: acknowledged with a
    /// recorded tagged turn end, or conclusively stopped with no attempt
    /// whose outcome is unknown (which may have been inserted and still be
    /// in a turn).
    fn settled_turns(&self) -> Result<(), &'static str> {
        for tracked in &self.tracked {
            match (&tracked.ack, tracked.label.as_deref()) {
                (Some(_), _) if !tracked.turn_ended => return Err("turn-end-unrecorded"),
                (Some(_), _) => {}
                (None, _) if tracked.unknown_attempts > 0 => return Err("delivery-unresolved"),
                (None, _) => {}
            }
        }
        Ok(())
    }

    /// Reports, and keeps for a refused `send` and the terminal record,
    /// that this survivor is not conversed with and why.
    fn unconversed(&mut self, facts: &Value, state: &str, reason: &str) {
        let mut conversation = facts.clone();
        conversation["scope"] = json!("recovery-time-snapshot");
        conversation["state"] = json!(state);
        conversation["reason"] = json!(reason);
        conversation["meaning"] = json!(match state {
            "unknown" =>
                "not conversed with: what the harness would do with new input is not known; never treated as usable",
            _ => "not conversed with: no new input reaches this harness under this owner",
        });
        self.inbox.hold(conversation.clone());
        self.recovered_conversation = Some(conversation.clone());
        conversation["event"] = json!("recovered-conversation");
        self.report(conversation);
    }

    /// Negotiates again with a settled survivor that declared the live
    /// reattachment contract, resumes its recorded session and, if both
    /// succeed, takes the caller's new input in that conversation. A close
    /// or cancel can interrupt an unanswered negotiation.
    fn reopen(&mut self, live: Live, facts: Value) -> (ConnEnd, &'static str) {
        self.started = None;
        let session = self.session.clone().expect("session checked");
        self.report(json!({ "event": "reopen-attempt", "work": live.work, "session": session, "replay": "none" }));
        let observed = Rc::new(Observed::default());
        let (reader, writer) = match self.streams(&live) {
            Ok(streams) => streams,
            Err(Unconnected::Detached) => return self.finish_live(live, ConnEnd::Stop, ""),
            Err(Unconnected::Ended) => {
                self.unconversed(&facts, "unavailable", "ended-before-endpoint");
                return self.finish_live(live, ConnEnd::Stop, "");
            }
            Err(Unconnected::Failed(reason)) => {
                self.unconversed(&facts, "unavailable", &format!("endpoint-failed: {reason}"));
                return self.hold(live, true, ConnEnd::Stop, "");
            }
        };
        let mut client = self.client(reader, writer, &observed);
        // A pending wake from before the attempt is not an interruption.
        let _ = self.inbox.take();
        observed.wake_armed.set(true);
        let failure = match client.initialize() {
            Ok(peer) => {
                let work = live.work;
                if self
                    .durable(|store| store.record_negotiation(work, peer.live_reattach))
                    .is_none()
                {
                    Some(("unknown", "store-lost".to_owned()))
                } else if !peer.live_reattach {
                    Some(("unavailable", "capability-withdrawn".to_owned()))
                } else {
                    self.report(json!({
                        "event": "negotiated",
                        "work": work,
                        "dedup_contract": peer.dedup_contract,
                        "live_reattach": true,
                    }));
                    let resumed = if let Some(result) = &self.resident {
                        PreparedEndpoint::agree(result)
                            .and_then(|endpoint| {
                                resident::start_session(
                                    &mut client,
                                    &endpoint,
                                    SessionStart::Resume {
                                        session_id: session.clone(),
                                        cwd: self.cwd.clone(),
                                    },
                                )
                            })
                            .map(|started| {
                                self.started = Some(started);
                            })
                            .map_err(|failure| match failure {
                                resident::StartRefusal::Request(error) => error,
                                _ => RequestFailure::ProtocolViolation(
                                    "resident resume refused".to_owned(),
                                ),
                            })
                    } else {
                        client.resume_session(&session, &self.cwd)
                    };
                    match resumed {
                        Ok(()) => None,
                        Err(RequestFailure::PeerGone) => {
                            Some(interrupted(&observed, "session-resume"))
                        }
                        Err(RequestFailure::Rejected { code, .. }) => {
                            Some(("unavailable", format!("session-resume-rejected-{code}")))
                        }
                        Err(_) => Some(("unknown", "session-resume-protocol-violation".to_owned())),
                    }
                }
            }
            Err(NegotiationFailure::PeerGone) => Some(interrupted(&observed, "negotiation")),
            Err(NegotiationFailure::Rejected { code, .. }) => {
                Some(("unavailable", format!("negotiation-rejected-{code}")))
            }
            Err(NegotiationFailure::UnsupportedVersion { agent_version }) => Some((
                "unavailable",
                format!("not-negotiated:unsupported-version-{agent_version}"),
            )),
            Err(NegotiationFailure::NoSessionSurface) => Some((
                "unavailable",
                "not-negotiated:no-session-surface".to_owned(),
            )),
            Err(NegotiationFailure::ProtocolViolation(_)) => {
                Some(("unknown", "negotiation-protocol-violation".to_owned()))
            }
        };
        observed.wake_armed.set(false);
        observed.woken.set(false);
        if let Some((state, reason)) = failure {
            drop(client);
            if observed.detached.get() {
                return self.finish_live(live, ConnEnd::Stop, "");
            }
            self.unconversed(&facts, state, &reason);
            return self.hold(live, true, ConnEnd::Stop, "");
        }
        let mut conversation = facts;
        conversation["scope"] = json!("recovery-time-snapshot");
        conversation["state"] = json!("live-usable");
        conversation["basis"] =
            json!("harness-declared-live-reattach-at-first-and-current-negotiation");
        conversation["meaning"] = json!(
            "new input is admitted durably under this owner generation, then delivered on the resumed session; continuity is the harness's declaration, not observed; nothing settled is resubmitted; inherited_async qualifies recipient obligations separately from transport usability"
        );
        self.recovered_conversation = Some(conversation.clone());
        conversation["event"] = json!("recovered-conversation");
        self.report(conversation);
        client.observe_session(&session);
        self.recovered_recipient = Some(live.work);
        let end = self.conversation(&mut client, &observed, &live, &session);
        self.left_conversation(client, &observed, end, live, true)
    }

    /// Holds a survivor not (or no longer) in conversation until its end is
    /// reported, the caller cancels, or this owner detaches. A close ends it
    /// through its work PID 1 when `settled` (every input's turn known to
    /// have ended), as a close ends a first owner's harness; otherwise the
    /// close is not applied to it, and says so: a close never cuts a turn
    /// that may be open.
    fn hold(
        &mut self,
        live: Live,
        settled: bool,
        end: ConnEnd,
        cause: &'static str,
    ) -> (ConnEnd, &'static str) {
        let watch = HoldWatch::start(self, &live, settled);
        let result = self.finish_live(live, end, cause);
        if watch.finish() {
            self.close_stop_attempted = true;
        }
        result
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
        self.show_work(adopted.work, adopted.pidns);
        let live = Live {
            work: adopted.work,
            stdio: adopted.stdio,
            root,
            token,
        };
        self.discard_output(&live);
        Some(live)
    }

    /// A socket-endpoint harness's stdout is not its protocol stream:
    /// drain it so it never blocks the harness.
    fn discard_output(&self, live: &Live) {
        if self.endpoint == Endpoint::UnixSocket
            && let Ok(stdout) = live.stdio.stdout.try_clone()
        {
            transport::drain(stdout, Arc::clone(&self.slot.stop));
        }
    }

    /// What the harness of `work` is started with beyond root PID 1's own
    /// environment: this root's Bash ingress and, for a socket endpoint,
    /// the socket the owner chose.
    fn launch_env(&self, work: i64) -> Result<serde_json::Map<String, Value>, String> {
        let mut env = serde_json::Map::new();
        let ipc = &self.slot.workload.ipc_dir;
        let ingress = bash::Ingress::socket_path(ipc);
        let ingress = ingress.to_str().ok_or("store path is not UTF-8")?;
        env.insert(bash::BASH_ENV.to_owned(), Value::String(ingress.to_owned()));
        if self.endpoint == Endpoint::UnixSocket {
            let path = socket_path(ipc, work);
            let dir = path.parent().expect("socket directory");
            match fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("socket-directory: {error}")),
            }
            let path = path.to_str().ok_or("socket path is not UTF-8")?;
            env.insert(SOCKET_ENV.to_owned(), Value::String(path.to_owned()));
        }
        Ok(env)
    }

    /// Connects to the socket the harness of `live` listens on, waiting for
    /// it to start listening. Like a silent stdio harness, one that never
    /// listens is waited for until it ends, the run detaches or the caller
    /// cancels: no timer gives up on it or kills it.
    fn connect(&self, live: &Live) -> Result<UnixStream, Unconnected> {
        let path = socket_path(&self.slot.workload.ipc_dir, live.work);
        loop {
            if transport::detached(&self.slot.stop) {
                return Err(Unconnected::Detached);
            }
            match UnixStream::connect(&path) {
                Ok(stream) => return self.attribute_listener(live.work, stream),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) => {}
                Err(error) => return Err(Unconnected::Failed(error.to_string())),
            }
            if live.root.end_known(live.work) {
                return Err(Unconnected::Ended);
            }
            thread::sleep(CONNECT_RETRY);
        }
    }

    /// Admits the listener only if it is the work's own: a process of the
    /// work identity inside that work's exact PID namespace. A socket in a
    /// directory the work identity owns names no one by itself.
    fn attribute_listener(&self, work: i64, stream: UnixStream) -> Result<UnixStream, Unconnected> {
        let expected = {
            let views = self.views.lock().expect("views");
            views.get(self.position).and_then(|view| {
                view.works
                    .iter()
                    .find(|(id, _)| *id == work)
                    .map(|(_, pidns)| *pidns)
            })
        };
        let attributed =
            bash::peer_pidns(&stream, self.slot.workload.peer_uid()).and_then(|(_, pidns)| {
                match expected {
                    Some(expected) if expected == pidns => Ok(()),
                    Some(_) => Err("outside-the-work-namespace".to_owned()),
                    None => Err("work-pidns-unreported".to_owned()),
                }
            });
        match attributed {
            Ok(()) => Ok(stream),
            Err(reason) => {
                self.report(json!({ "event": "endpoint-refused", "work": work, "reason": format!("listener-unattributed: {reason}") }));
                Err(Unconnected::Failed(format!(
                    "listener-unattributed: {reason}"
                )))
            }
        }
    }

    /// Records an end reported while no owner was attached. Returns whether
    /// it is the harness's own wait status. A receipt with only its work
    /// PID 1's status (that PID 1 ended before reporting the harness) shows
    /// the work's namespace is gone, not how the harness ended: an unknown
    /// end, never charged as a closure.
    fn observe_prior_exit(&mut self, work: i64, receipt: &Value) -> bool {
        let Some(status) = receipt["harness"].as_str().map(str::to_owned) else {
            self.report(json!({
                "event": "prior-end-unknown",
                "work": work,
                "meaning": "ended-with-work-namespace-status-unknown",
                "work_pid1": receipt["work_pid1"],
            }));
            self.durable(|store| {
                store.resolve_work(work, "ended-with-work-namespace-status-unknown", None)
            });
            self.orphaned_completions(work);
            self.prior_unknown_ends += 1;
            return false;
        };
        self.report(json!({
            "event": "prior-exit",
            "work": work,
            "status": status,
            "observer": receipt["harness_observer"],
            "work_pid1": receipt["work_pid1"],
        }));
        self.durable(|store| store.resolve_work(work, &status, Some("work-pid1-wait")));
        self.orphaned_completions(work);
        self.prior_exits.push(status);
        true
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
        self.orphaned_completions(work);
        self.prior_unknown_ends += 1;
    }

    fn after_drive(&mut self, end: ConnEnd, observed: &'static str) -> bool {
        if self.cancelled() {
            let reason = self.stop_reason();
            self.label_remaining(reason);
            return false;
        }
        if let Some(reason) = self.child.as_ref().and_then(|link| link.stopped()) {
            // A stopped child's end is its stop, not a closure.
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
        // A child stopped before its launch (requester or parent gone,
        // root closing) is not started; the stop holds this same lock.
        if let Some(reason) = self.child.as_ref().and_then(|link| link.stopped()) {
            drop(custody);
            self.report(json!({ "event": "launch-failed", "reason": format!("stopped-before-launch: {reason}"), "not_started": true }));
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
                self.report(
                    json!({ "event": "launch-failed", "reason": reason, "not_started": true }),
                );
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
        let spawned = match self
            .launch_env(work)
            .map_err(SpawnError::NotStarted)
            .and_then(|env| root.spawn_observed(work, &self.argv, &env, &self.cwd, false))
        {
            Ok(spawned) => spawned,
            Err(SpawnError::NotStarted(reason)) => {
                drop(custody);
                self.report(
                    json!({ "event": "launch-failed", "reason": reason, "not_started": true }),
                );
                self.durable(|store| store.resolve_work(work, "launch-refused", None));
                self.label_remaining("launch-failed");
                return None;
            }
            Err(SpawnError::Unknown(reason)) => {
                // A lost or unproven reply after root PID 1 may have
                // created the work: possible effects, never a no-start
                // and never retried. The work stays unresolved durably.
                drop(custody);
                self.report(json!({ "event": "launch-unknown", "work": work, "reason": reason, "not_started": false }));
                self.wait_failures.push(format!("launch-unknown: {reason}"));
                self.label_remaining("launch-unknown");
                return None;
            }
        };
        let token = custody.register(Arc::clone(&root), work);
        if let Some(link) = &self.child {
            link.set_work(&root, work);
        }
        drop(custody);
        self.launches += 1;
        self.report(json!({
            "event": "launched",
            "launch": self.launches,
            "pid": spawned.harness_host_pid,
            "work": work,
            "incarnation": root.incarnation,
            "identity": spawned.harness_host_pid.map(crate::workload::observe),
        }));
        self.show_work(work, spawned.pidns);
        let live = Live {
            work,
            stdio: spawned.stdio,
            root,
            token,
        };
        self.discard_output(&live);
        if self
            .durable(|store| store.record_work_spawned(work, spawned.harness_host_pid))
            .is_none()
        {
            self.finish_live(live, ConnEnd::Stop, "");
            return None;
        }
        if let Some(error) = spawned.exec_error {
            self.report(json!({ "event": "launch-failed", "reason": error, "not_started": false, "work": work }));
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
        let (reader, writer) = match self.streams(&live) {
            Ok(streams) => streams,
            Err(unconnected) => match unconnected {
                Unconnected::Detached => return self.finish_live(live, ConnEnd::Stop, ""),
                Unconnected::Ended => {
                    // Nothing was sent: like a stdio harness that ended
                    // before answering, a closure once its exit is reported.
                    self.report(json!({ "event": "ended-before-endpoint", "work": live.work }));
                    let end = self.gone_or_drained();
                    return self.finish_live(live, end, "exit-before-endpoint");
                }
                Unconnected::Failed(reason) => {
                    self.report(json!({ "event": "endpoint-failed", "reason": reason }));
                    self.label_remaining("endpoint-failed");
                    return self.finish_live(live, ConnEnd::Stop, "");
                }
            },
        };
        let mut client = self.client(reader, writer, &observed);
        let end = self.converse(&mut client, &observed, &live);
        self.left_conversation(client, &observed, end, live, false)
    }

    /// The protocol streams to `live`'s harness: its stdio from root PID 1,
    /// or a new connection to the socket it listens on.
    fn streams(&self, live: &Live) -> Result<(File, File), Unconnected> {
        match self.endpoint {
            Endpoint::Stdio => Ok((
                live.stdio.stdout.try_clone().expect("stdout descriptor"),
                live.stdio.stdin.try_clone().expect("stdin descriptor"),
            )),
            Endpoint::UnixSocket => {
                let stream = self.connect(live)?;
                self.report(json!({ "event": "endpoint-connected", "work": live.work }));
                let reader = stream.try_clone().expect("socket descriptor");
                Ok((
                    File::from(OwnedFd::from(reader)),
                    File::from(OwnedFd::from(stream)),
                ))
            }
        }
    }

    fn client(
        &self,
        reader: File,
        writer: File,
        observed: &Rc<Observed>,
    ) -> AcpClient<HarnessTransport> {
        let transport = HarnessTransport::new(
            reader,
            writer,
            Rc::clone(observed),
            Arc::clone(&self.slot.stop),
            Some(Arc::clone(&self.inbox)),
        );
        AcpClient::new(
            transport,
            ClientInfo {
                name: "oulipoly-root-supervisor".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
        )
    }

    /// After a conversation on `live` ended: refuses what was queued but
    /// not taken, then waits for the harness's end. `held` keeps a close
    /// able to end a recovered survivor whose conversation ended while it
    /// lives (see [`Self::hold`]).
    fn left_conversation(
        &mut self,
        client: AcpClient<HarnessTransport>,
        observed: &Observed,
        end: ConnEnd,
        live: Live,
        held: bool,
    ) -> (ConnEnd, &'static str) {
        // Out of conversation: nothing more is taken, and what the caller
        // queued but this worker never took is not admitted.
        let (completions, follow_ups): (Vec<_>, Vec<_>) = self
            .inbox
            .shut()
            .into_iter()
            .partition(|follow_up| follow_up.completion.is_some());
        for follow_up in follow_ups {
            self.refuse(&follow_up, "conversation-ended");
        }
        self.settle_unfinished_completions(completions);
        drop(client);
        if observed.detached.get() {
            return self.finish_live(live, ConnEnd::Stop, "");
        }
        let cause = match end {
            ConnEnd::Gone if observed.eof.get() => {
                // Over a socket, end of stream is the connection's end; the
                // harness's own end is still awaited below.
                let event = match self.endpoint {
                    Endpoint::Stdio => "peer-gone",
                    Endpoint::UnixSocket => "connection-closed",
                };
                self.report(json!({ "event": event, "observed": "eof" }));
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
        if held && !self.close_stop_attempted {
            let settled = self.head().is_none() && self.no_open_input();
            return self.hold(live, settled, end, cause);
        }
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
        // This harness process is gone: none of its inputs is in progress
        // in it any more, and nothing in its namespace can ask for Bash.
        self.view(|view| {
            view.works.retain(|(work, _)| *work != live.work);
            view.open.clear();
        });
        // Children are subordinate to this exact work: they stop with it
        // (a relaunch is another work, never their parent). A child's own
        // Bash ends with the child.
        match &self.child {
            None => {
                let signalled = self.children.stop_children_of(live.work, "parent-ended");
                if signalled > 0 {
                    self.report(json!({ "event": "children-stopping", "work": live.work, "reason": "parent-ended", "signalled": signalled }));
                }
            }
            Some(link) => {
                self.children.kill_runs_of(link.position);
            }
        }
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
            self.orphaned_completions(work);
        }
        if self.endpoint == Endpoint::UnixSocket && exit.is_ok() {
            // Its listener ended with it; the path is never reused.
            let _ = fs::remove_file(socket_path(&self.slot.workload.ipc_dir, work));
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
            // Separate facts: whether a stop was asked of the work's PID 1,
            // and its report that nothing was left in its namespace.
            "stop_requested": receipt["stop_requested"],
            "namespace": receipt["namespace"],
        }));
        self.exits.push(status);
        true
    }

    fn converse(
        &mut self,
        client: &mut AcpClient<HarnessTransport>,
        observed: &Observed,
        live: &Live,
    ) -> ConnEnd {
        self.started = None;
        match client.initialize() {
            Ok(peer) => {
                self.report(json!({
                    "event": "negotiated",
                    "launch": self.launches,
                    "dedup_contract": peer.dedup_contract,
                    "live_reattach": peer.live_reattach,
                }));
                // What this work's harness declared, for a successor that
                // finds it alive with nothing owed.
                let work = live.work;
                if self
                    .durable(|store| store.record_negotiation(work, peer.live_reattach))
                    .is_none()
                {
                    return ConnEnd::Stop;
                }
            }
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
        let session = if let Some(result) = &self.resident {
            let endpoint = match PreparedEndpoint::agree(result) {
                Ok(endpoint) => endpoint,
                Err(_) => {
                    self.label_remaining("resident-agreement-refused");
                    return ConnEnd::Stop;
                }
            };
            let start = match &self.session {
                None => SessionStart::New {
                    cwd: self.cwd.clone(),
                },
                Some(id) => SessionStart::Resume {
                    session_id: id.clone(),
                    cwd: self.cwd.clone(),
                },
            };
            match resident::start_session(client, &endpoint, start) {
                Ok(started) => {
                    self.report(json!({
                        "event": "resident-session-started",
                        "session": started.native.session_id,
                        "resumed": started.resumed,
                        "canonical_binding": "unbound",
                        "canonical_publication": "not-established",
                        "agent_name": started.native.agent_name,
                        "agent_version": started.native.agent_version,
                    }));
                    let id = (!started.resumed).then(|| started.native.session_id.clone());
                    self.started = Some(started);
                    Ok(id)
                }
                Err(_) => {
                    self.report(
                        json!({"event": "resident-start-refused", "private_details": "withheld"}),
                    );
                    self.label_remaining("resident-start-refused");
                    return ConnEnd::Stop;
                }
            }
        } else {
            match self.session.clone() {
                None => client.open_session(&self.cwd).map(Some),
                Some(id) => client.resume_session(&id, &self.cwd).map(|()| None),
            }
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
                self.report(json!({ "event": "session-opened", "session": id }));
                self.view(|view| view.session = Some(id.clone()));
                self.session = Some(id);
            }
            Ok(None) => self.report(json!({
                "event": "session-resumed",
                "session": self.session,
            })),
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
        self.conversation(client, observed, live, &session)
    }

    /// Delivers what is owed on `session`, then takes the caller's further
    /// input between turns until the conversation ends.
    fn conversation(
        &mut self,
        client: &mut AcpClient<HarnessTransport>,
        observed: &Observed,
        live: &Live,
        session: &str,
    ) -> ConnEnd {
        let session = session.to_owned();
        let position = self.position;
        let mut seen = client.events().len();
        self.close_reported = None;
        self.inbox.open();
        // Opened first, so a run ending after this read is offered to the
        // inbox instead; the same work is never taken twice.
        if self.recovered_recipient == Some(live.work) {
            self.recover_completions(live.work);
        }
        'conversation: loop {
            while let Some(index) = self.head() {
                // The attempt is durable before it is sent, so no successor can
                // miss an attempt that may have been inserted.
                let Some(attempt) = self.durable(|store| store.begin_attempt(position, index))
                else {
                    return ConnEnd::Stop;
                };
                self.tracked[index].attempts += 1;
                self.tracked[index].unknown_attempts += 1;
                // Open for Bash attribution from before the send: the harness may
                // act on it as soon as it is inserted, before this owner reads
                // the acknowledgement. Its id is known only from the ACK.
                self.view(|view| {
                    if !view.open.iter().any(|input| input.index == index) {
                        view.open.push(OpenInput {
                            index,
                            message_id: None,
                        });
                    }
                });
                let outcome = match &self.started {
                    Some(started) => {
                        resident::send_turn(client, started, &mut self.tracked[index].message)
                            .outcome
                    }
                    None => client.submit(&session, &mut self.tracked[index].message),
                };
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
                            message_id: Some(acceptance.message_id.clone()),
                        };
                        // Reported only once durable: an unrecorded ACK leaves
                        // the attempt unresolved for a successor to classify.
                        if self
                            .durable(|store| store.record_ack(position, index, attempt, &ack))
                            .is_none()
                        {
                            return ConnEnd::Stop;
                        }
                        self.tracked[index].resolve_attempt(&format!("ack:{}", ack.label));
                        self.report(json!({
                            "event": "ack",
                            "index": index,
                            "label": ack.label,
                            "basis": ack.basis,
                            "recovered": ack.recovered,
                            "message_id": ack.message_id,
                            "durable": true,
                        }));
                        let message_id = ack.message_id.clone();
                        self.view(|view| {
                            if let Some(input) =
                                view.open.iter_mut().find(|input| input.index == index)
                            {
                                input.message_id = message_id;
                            }
                        });
                        self.tracked[index].ack = Some(ack);
                        // submit may collect reply/idle notifications before
                        // the insertion response. Correlate them only after
                        // this ACK's identity is durable and in the open view.
                        self.report_turn(client, &mut seen);
                        continue;
                    }
                    DeliveryOutcome::Rejected { .. } => ("rejected-unresolved", None),
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
                        self.view(|view| view.open.retain(|input| input.index != index));
                        ("not-sent", Some(ConnEnd::Stop))
                    }
                    DeliveryOutcome::SessionMismatch => {
                        self.tracked[index].label = Some("session-mismatch".to_owned());
                        self.view(|view| view.open.retain(|input| input.index != index));
                        ("not-sent", Some(ConnEnd::Stop))
                    }
                };
                // No ACK: keep native parent attribution unknown rather than
                // assigning these observations to the attempted input.
                self.report_turn(client, &mut seen);
                if self
                    .durable(|store| store.resolve_attempt(attempt, resolution))
                    .is_none()
                {
                    return ConnEnd::Stop;
                }
                self.tracked[index].resolve_attempt(resolution);
                if let DeliveryOutcome::Rejected { code, data, .. } = &outcome {
                    // The RPC error is not insertion evidence. In particular
                    // INPUT_UNCERTAIN may accompany completed native work.
                    // Stop this message durably: recovery must not authorize
                    // a resend solely because the SDK still calls it owed.
                    self.tracked[index].label = Some("rejected-unresolved".to_owned());
                    self.report(json!({
                        "event": "rejected", "index": index, "code": code,
                        "durable": true, "scope": "rpc-attempt",
                        "insertion": "unresolved", "retry": "not-authorized",
                        "unresolved_attempts": self.tracked[index].unknown_attempts,
                        "native_report": native_summary(data.as_ref().and_then(|d| d.get("nativeTurn"))),
                        "endpoint_record_error": data.as_ref().is_some_and(|d| d.get("recordError").is_some()),
                        "endpoint_durability": "not-established",
                        "canonical_publication": "not-established",
                    }));
                }
                if let Some(end) = end {
                    return end;
                }
            }
            // Nothing owed on this connection. Keep reading through the client
            // until the harness ends its stream; acknowledgement is not an end.
            // Between turns the caller's bell may interrupt the wait: a further
            // input is taken only here, and a close acts only here.
            loop {
                // A completion held during an earlier turn goes next, once
                // that turn has ended; it is never inserted into a busy turn.
                match self.admit_held_completion() {
                    Some(false) => return ConnEnd::Stop,
                    Some(true) => continue 'conversation,
                    None => {}
                }
                // Close waits for owed background completions as well: the
                // harness stays live until each one's carrying turn ended or
                // it was settled undelivered (cancel/deadline end it sooner).
                if self.closing.requested() && !self.close_stop_attempted {
                    if self.no_open_input() && self.owed_async() == 0 {
                        self.stop_for_close(live);
                    } else {
                        self.report_close_wait(live);
                    }
                }
                observed.wake_armed.set(true);
                // Publish on event arrival while the turn is still open.
                // The reporting cursor preserves order and prevents repeats;
                // submit's ACK is already durable before this wait begins.
                let idle = client.await_session_idle_with_events(&session, |client| {
                    self.report_turn(client, &mut seen);
                });
                observed.wake_armed.set(false);
                self.report_turn(client, &mut seen);
                match idle {
                    Ok(idle) => {
                        self.report(json!({
                            "event": "idle",
                            "meaning": "readiness-since-first-attempt",
                            "stop_reason": idle.stop_reason,
                        }));
                        // Idle may precede endpoint publication diagnostics.
                        // A host-bounded window collects what arrives; ending
                        // it proves neither absence nor durable completion.
                        if let Err(failure) = self.collect_diagnostics(client, observed, &mut seen)
                        {
                            return match failure {
                                IdleWaitFailure::PeerGone => ConnEnd::Drained,
                                _ => ConnEnd::Stop,
                            };
                        }
                    }
                    Err(IdleWaitFailure::PeerGone) if observed.woken.replace(false) => {
                        if !self.take_follow_ups() {
                            return ConnEnd::Stop;
                        }
                        if self.head().is_some() {
                            continue 'conversation;
                        }
                    }
                    Err(IdleWaitFailure::PeerGone) => return ConnEnd::Drained,
                    Err(IdleWaitFailure::NoAttempt | IdleWaitFailure::ProtocolViolation(_)) => {
                        return ConnEnd::Stop;
                    }
                }
            }
        }
    }

    fn collect_diagnostics(
        &mut self,
        client: &mut AcpClient<HarnessTransport>,
        observed: &Observed,
        seen: &mut usize,
    ) -> Result<(), IdleWaitFailure> {
        let deadline = Instant::now() + Duration::from_millis(25);
        observed.read_deadline.set(Some(deadline));
        let mut result = Ok(());
        for _ in 0..32 {
            if Instant::now() >= deadline {
                break;
            }
            match client.receive_event() {
                Ok(_) => self.report_turn(client, seen),
                Err(IdleWaitFailure::PeerGone) if observed.timed_out.replace(false) => break,
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        observed.read_deadline.set(None);
        result
    }

    /// What an input admission or a close waits for now: an earlier input
    /// whose insertion or turn end is unresolved in durable history (this
    /// owner cannot see that turn end: only cancel ends the wait), an input
    /// open on this connection (its tagged turn end ends the wait), or
    /// nothing.
    fn turn_wait(&self) -> Option<(&'static str, &'static str)> {
        let views = self.views.lock().expect("views");
        let open = views
            .get(self.position)
            .map(|view| {
                view.open
                    .iter()
                    .map(|input| input.index)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        drop(views);
        for (index, tracked) in self.tracked.iter().enumerate() {
            match &tracked.ack {
                Some(_) if !tracked.turn_ended && !open.contains(&index) => {
                    return Some(("unresolved-history", "turn-end-unrecorded"));
                }
                None if tracked.unknown_attempts > 0 => {
                    return Some(("unresolved-history", "delivery-unresolved"));
                }
                _ => {}
            }
        }
        (!open.is_empty() || self.head().is_some()).then_some(("open-turn", "input-open"))
    }

    /// Says why a completion is held, without promising more than the
    /// facts allow.
    fn report_held(&self, work: i64) {
        let (held_behind, reason, meaning) = match self.turn_wait() {
            Some(("unresolved-history", reason)) => (
                "unresolved-history",
                Some(reason),
                "an earlier input's insertion or turn end is unresolved in durable history; never admitted while that holds (a later refusal or turn cannot settle it); not a promise of later admission under this owner: cancel or the conversation's end disposes it",
            ),
            _ => (
                "open-turn",
                None,
                "an earlier input is open or queued; admitted after its tagged turn end is recorded, never into a busy turn",
            ),
        };
        self.report(json!({
            "event": "bash-async-completion-held",
            "work": work,
            "held_behind": held_behind,
            "reason": reason,
            "meaning": meaning,
        }));
    }

    /// No input is open in this connection or unresolved in its durable
    /// history. A refusal of a retry cannot settle an earlier unknown
    /// insertion. This same floor gates caller/held-completion admission
    /// and ordinary close (neither tagged ends nor refusals prove processing).
    fn no_open_input(&self) -> bool {
        self.settled_turns().is_ok()
            && self
                .views
                .lock()
                .expect("views")
                .get(self.position)
                .is_none_or(|view| view.open.is_empty())
    }

    /// Reports, once per change, why a requested close is not applied yet:
    /// deferred until an open turn's tagged end or an owed completion's
    /// settlement (then applied), or not applied while an unresolved turn
    /// in durable history stands (only cancel ends it under this owner).
    fn report_close_wait(&mut self, live: &Live) {
        let (event, reason, meaning) = match self.turn_wait() {
            Some(("unresolved-history", reason)) => (
                "close-not-applied",
                reason,
                "a close never cuts an unresolved turn; this owner cannot see that turn end, so the close is not applied: the conversation stays live until its end is reported or the caller cancels",
            ),
            Some((_, reason)) => (
                "close-deferred",
                reason,
                "an input is open: the close is applied after its tagged turn end is recorded; not a refusal",
            ),
            None => (
                "close-deferred",
                "async-completion-owed",
                "a background completion is still owed to this harness: the close is applied once it is carried through a turn end or settled undelivered; not a refusal",
            ),
        };
        if self.close_reported == Some(reason) {
            return;
        }
        self.close_reported = Some(reason);
        self.report(json!({
            "event": event,
            "work": live.work,
            "reason": reason,
            "owed_async": self.owed_async(),
            "meaning": meaning,
        }));
    }

    /// The caller closed the conversation and no logical input or async
    /// completion remains open: attempt a stop through its work PID 1. Its end is still
    /// known only from that waiter's report, even if the request was sent.
    fn stop_for_close(&mut self, live: &Live) {
        self.close_stop_attempted = true;
        let signalled = live.root.kill(live.work);
        self.report(json!({
            "event": "close-stopping",
            "work": live.work,
            "signalled": signalled,
            "by": "work-pid1-kill",
            "meaning": "owner stop attempted after input closure and tagged turn ends; signalled records request delivery, actual host end is reported separately; not processing success",
        }));
    }

    fn refuse(&self, follow_up: &FollowUp, reason: &str) {
        self.report(json!({
            "event": "follow-up-refused",
            "control": follow_up.control,
            "ref": follow_up.caller_ref,
            "reason": reason,
            "admitted": false,
        }));
    }

    /// Takes what the caller queued. One input at a time: a follow-up is
    /// admitted only when nothing is owed or open on this harness (every
    /// earlier input ended or conclusively rejected); otherwise it is
    /// refused at this admission-time check. Receipt may have queued it
    /// during a turn that has since ended. A check already passed can
    /// commit after close is requested. Admission is a durable commit of
    /// an owed message, reported only after it. Returns false on store loss.
    fn take_follow_ups(&mut self) -> bool {
        for follow_up in self.inbox.take() {
            if follow_up.recovered.is_some() {
                // An earlier owner's promise offered as its run ended here:
                // taken up only by a recovered live-usable conversation.
                self.take_recovered(follow_up);
                continue;
            }
            if let Some(work) = follow_up.completion {
                // The owner's own completion input: not refused by close or
                // by an open input (held instead); only a stop ends it.
                if self.cancelled() {
                    let reason = self.stop_reason();
                    self.settle_async(work, "undelivered", Some(reason));
                } else if self.head().is_some()
                    || !self.no_open_input()
                    || !self.pending_completions.is_empty()
                {
                    self.report_held(work);
                    self.pending_completions.push_back(follow_up);
                } else if !self.admit_completion(follow_up) {
                    return false;
                }
                continue;
            }
            if let Some(earlier) = self.earlier(&follow_up) {
                self.duplicate(&follow_up, &earlier);
                continue;
            }
            let refused = if self.cancelled() {
                Some(self.stop_reason())
            } else if self.closing.requested() {
                Some("input-closed")
            } else if self.hold.held() {
                Some("input-held")
            } else if self.head().is_some() || !self.no_open_input() {
                Some("input-open")
            } else {
                None
            };
            if let Some(reason) = refused {
                self.refuse(&follow_up, reason);
                continue;
            }
            let position = self.position;
            let admitted = self.durable(|store| {
                store.admit_follow_up(
                    position,
                    follow_up.control,
                    follow_up.caller_ref.as_deref(),
                    &follow_up.text,
                    true,
                    None,
                )
            });
            let (index, message) = match admitted {
                Some(Admission::Admitted(index, message)) => (index, message),
                Some(Admission::Duplicate(earlier)) => {
                    self.duplicate(&follow_up, &earlier);
                    continue;
                }
                // Only for owner completions; a caller input is never one.
                Some(Admission::NotReoffered(reason)) => {
                    self.refuse(&follow_up, &reason);
                    continue;
                }
                None => {
                    self.refuse(&follow_up, "store-lost");
                    return false;
                }
            };
            debug_assert_eq!(index, self.tracked.len());
            self.tracked.push(Tracked {
                message,
                label: None,
                ack: None,
                closures: 0,
                attempts: 0,
                prior_unknown: 0,
                unknown_attempts: 0,
                turn_ended: false,
                follow_up: true,
            });
            let _ = self.tx.send(Event::Admitted(self.position));
            self.report(json!({
                "event": "follow-up-admitted",
                "control": follow_up.control,
                "ref": follow_up.caller_ref,
                "input": index,
                "stage": "durably-committed",
                "durable": true,
            }));
        }
        true
    }

    /// The earlier admission of this follow-up's caller `ref`, by any
    /// generation of this root.
    fn earlier(&self, follow_up: &FollowUp) -> Option<EarlierRef> {
        let caller_ref = follow_up.caller_ref.as_deref()?;
        self.store
            .lock()
            .expect("store lock")
            .earlier_ref(caller_ref)
            .ok()
            .flatten()
    }

    /// A retry of an already admitted logical input: nothing is admitted
    /// or delivered again; the earlier input's durable state is reported.
    fn duplicate(&self, follow_up: &FollowUp, earlier: &EarlierRef) {
        self.report(json!({
            "event": "follow-up-duplicate",
            "control": follow_up.control,
            "ref": follow_up.caller_ref,
            "admitted": false,
            "earlier": {
                "harness_position": earlier.harness,
                "input": earlier.input,
                "admitted_generation": earlier.admitted_generation,
                "acknowledged": earlier.acknowledged,
                "stop": earlier.stop,
                "turn_end": if earlier.turn_ended { "tagged-idle-recorded" } else { "not-recorded" },
            },
            "meaning": "this ref was already admitted in this root; it is not admitted or delivered again",
        }));
    }

    /// The owner input whose acknowledged `messageId` is `message_id`.
    fn input_of(&self, message_id: &str) -> Option<usize> {
        self.tracked.iter().position(|tracked| {
            tracked
                .ack
                .as_ref()
                .and_then(|ack| ack.message_id.as_deref())
                == Some(message_id)
        })
    }

    /// Reports what the turn showed since `seen`: agent output, notices and
    /// the agent requests this owner refused. None of it is an ACK.
    ///
    /// Output names the owner input it answers only by the agent's own
    /// parent tag in this worker's native session. A turn end is reported
    /// for an input only from an idle in that session which the agent
    /// tagged with that input's or a later user message (native
    /// ids ascend); then `own_output` says whether any output named it.
    /// An untagged idle stays readiness: no input's end.
    fn report_turn(&mut self, client: &AcpClient<HarnessTransport>, seen: &mut usize) {
        for event in &client.events()[*seen..] {
            match event {
                SessionEvent::AgentMessage {
                    session_id,
                    message_id,
                    text,
                    parent_message_id,
                } => {
                    let current_session = self.session.as_deref() == Some(session_id.as_str());
                    let input = parent_message_id
                        .as_deref()
                        .filter(|_| current_session)
                        .and_then(|parent| self.input_of(parent));
                    if let Some(parent) = parent_message_id.as_ref().filter(|_| current_session) {
                        self.answered.insert(parent.clone());
                    }
                    self.report(json!({
                        "event": "agent-message",
                        "session": session_id,
                        "message_id": message_id,
                        "text": text,
                        "parent_message_id": parent_message_id,
                        "input": input,
                        "input_attribution": match (parent_message_id, input) {
                            (None, _) => "untagged",
                            (Some(_), Some(_)) => "native-parent",
                            (Some(_), None) => "parent-not-an-owner-input",
                        },
                    }));
                }
                SessionEvent::Idle {
                    session_id,
                    stop_reason,
                    last_user_message_id: Some(last),
                    native_turn_report,
                    ..
                } if self.session.as_deref() == Some(session_id.as_str()) => {
                    let mut covered = Vec::new();
                    self.view(|view| {
                        view.open.retain(|input| match &input.message_id {
                            Some(id) if id.as_str() <= last.as_str() => {
                                covered.push((input.index, id.clone()));
                                false
                            }
                            _ => true,
                        });
                    });
                    for (index, message_id) in covered {
                        // Durable, so that a successor knows this input's
                        // turn ended (the agent's tag, not processing).
                        let position = self.position;
                        if !self.tracked[index].turn_ended
                            && self
                                .durable(|store| store.record_turn_end(position, index))
                                .is_some()
                        {
                            self.tracked[index].turn_ended = true;
                        }
                        self.report(json!({
                            "event": "turn-end",
                            "session": session_id,
                            "input": index,
                            "message_id": message_id,
                            "stop_reason": stop_reason,
                            "last_user_message_id": last,
                            "own_output": self.answered.contains(&message_id),
                            "own_turn_end": message_id == *last,
                            "native_report_for": last,
                            "native_report": native_summary(native_turn_report.as_ref()),
                            "endpoint_durability": "not-established",
                            "canonical_publication": "not-established",
                            "meaning": "agent-idle-tagged-at-or-after-this-input",
                        }));
                        if let Some(&work) = self.completion_of.get(&index)
                            && !self.settle_async(work, "turn-ended", None)
                        {
                            self.reconcile_completion(work, index);
                        }
                    }
                }
                SessionEvent::Other {
                    session_id, update, ..
                } => {
                    // Retained raw SDK payloads stay private. Publish only a
                    // recognized contrary fact and its native attribution.
                    if let Some(report) = update
                        .get("_meta")
                        .and_then(|meta| meta.get(agent_provider_contract::acp::NATIVE_TURN_META))
                        && report.get("record_error").is_some()
                    {
                        let message_id = report.get("message_id").and_then(Value::as_str);
                        let input = message_id
                            .filter(|_| self.session.as_deref() == Some(session_id))
                            .and_then(|id| self.input_of(id));
                        self.report(json!({
                            "event": "endpoint-record-error", "session": session_id,
                            "input": input,
                            "message_id": message_id,
                            "attribution": "endpoint-native-tag",
                            "endpoint_durability": "contrary-diagnostic",
                            "canonical_publication": "not-established",
                            "private_details": "withheld",
                        }));
                    }
                }
                SessionEvent::Notice {
                    session_id,
                    severity,
                    title,
                    description,
                } => self.report(json!({
                    "event": "notice",
                    "session": session_id,
                    "severity": severity,
                    "title": title,
                    "description": description,
                })),
                SessionEvent::RequestRefused { method, session_id } => self.report(json!({
                    "event": "request-refused",
                    "method": method,
                    "session": session_id,
                })),
                _ => {}
            }
        }
        *seen = client.events().len();
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
                    unresolved_attempts: tracked.unknown_attempts,
                    closures: tracked.closures,
                    follow_up: tracked.follow_up,
                    turn_ended: tracked.turn_ended,
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
            close_stop_attempted: self.close_stop_attempted,
            recovered_conversation: self.recovered_conversation.clone(),
            messages,
        }
    }
}

/// A reopen read that ended without a reply: interrupted by a close or
/// cancel (unknown: the harness may still answer), or the stream closed.
fn interrupted(observed: &Observed, stage: &str) -> (&'static str, String) {
    if observed.woken.get() {
        ("unknown", format!("{stage}-interrupted"))
    } else {
        ("unavailable", format!("{stage}-transport-closed"))
    }
}

/// Watches for a close while a recovered survivor is held without a
/// conversation, on its own thread, since the worker is blocked waiting
/// for the survivor's end.
struct HoldWatch {
    done: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    inbox: Arc<Inbox>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HoldWatch {
    fn start(worker: &Worker, live: &Live, settled: bool) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let (id, tx, inbox, closing) = (
            worker.id.clone(),
            worker.tx.clone(),
            Arc::clone(&worker.inbox),
            Arc::clone(&worker.closing),
        );
        let (stop, root, work) = (
            Arc::clone(&worker.slot.stop),
            Arc::clone(&live.root),
            live.work,
        );
        let thread = {
            let (done, stopped, inbox) =
                (Arc::clone(&done), Arc::clone(&stopped), Arc::clone(&inbox));
            thread::spawn(move || {
                let report = |mut value: Value| {
                    value["harness"] = Value::String(id.clone());
                    let _ = tx.send(Event::Report(value));
                };
                loop {
                    if done.load(Ordering::SeqCst) {
                        return;
                    }
                    if closing.requested() {
                        if settled {
                            let signalled = root.kill(work);
                            stopped.store(true, Ordering::SeqCst);
                            report(json!({
                                "event": "close-stopping",
                                "work": work,
                                "signalled": signalled,
                                "by": "work-pid1-kill",
                                "held": true,
                                "meaning": "recovered survivor held without a conversation, every input's turn recorded ended: owner stop attempted for the close; signalled records request delivery, actual host end is reported separately; not processing success",
                            }));
                        } else {
                            report(json!({
                                "event": "close-not-applied",
                                "work": work,
                                "reason": "turn-state-unknown",
                                "meaning": "a close never cuts a turn that may be open: this recovered survivor stays held until its end is reported or the caller cancels",
                            }));
                        }
                        return;
                    }
                    if !transport::rung(&stop, inbox.hold_bell()).unwrap_or(false) {
                        return;
                    }
                    inbox.drain_hold();
                }
            })
        };
        Self {
            done,
            stopped,
            inbox,
            thread: Some(thread),
        }
    }

    /// Ends the watch; returns whether it attempted a stop for a close.
    fn finish(mut self) -> bool {
        self.done.store(true, Ordering::SeqCst);
        self.inbox.ring();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Safe projection: free-form report/status/failure text never becomes public.
fn native_summary(raw: Option<&Value>) -> Value {
    let state = match raw {
        None => "absent",
        Some(Value::Null) => "null",
        Some(value) if agent_provider_contract::acp::NativeTurn::from_report(value).is_some() => {
            "valid"
        }
        Some(_) => "invalid-or-unsupported",
    };
    let typed = raw.and_then(agent_provider_contract::acp::NativeTurn::from_report);
    json!({
        "state": state,
        "custody": typed.as_ref().map(|turn| match turn.custody {
            agent_provider_contract::acp::NativeCustody::Complete => "complete",
            agent_provider_contract::acp::NativeCustody::NotAdmitted => "not-admitted",
            agent_provider_contract::acp::NativeCustody::Reconciled => "reconciled",
            agent_provider_contract::acp::NativeCustody::CompleteWithoutExit => "complete-without-exit",
            agent_provider_contract::acp::NativeCustody::Incomplete => "incomplete",
        }),
        "status_code": typed.as_ref().and_then(|turn| turn.report.pointer("/status/code")).and_then(Value::as_i64),
        "source": "endpoint-report",
        "physical_custody": "not-certified-by-report",
    })
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
                endpoint: crate::Endpoint::Stdio,
                session: None,
                resident: None,
                messages: vec!["x".into()],
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: None,
        };
        let mut claimed = Store::claim(&dir.0, Some(&intent)).unwrap();
        let harness = claimed.harnesses.remove(0);
        let (tx, rx) = channel();
        (
            Worker {
                id: "test".into(),
                argv: vec![],
                endpoint: Endpoint::Stdio,
                started: None,
                position: 0,
                cap: 3,
                attempt_cap: 10,
                cwd: "/".into(),
                custody: Arc::new(Mutex::new(Custody::default())),
                store: Arc::new(Mutex::new(claimed.store)),
                slot: Arc::new(RootSlot::new(
                    dir.0.clone(),
                    1,
                    crate::workload::unprivileged_for_tests(&dir.0),
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
                        unknown_attempts: 0,
                        turn_ended: false,
                        follow_up: false,
                    })
                    .collect(),
                session: None,
                resident: None,
                prior: None,
                launches: 1,
                reattached: 0,
                exits: vec![],
                prior_exits: vec![],
                prior_unknown_ends: 0,
                wait_failures: vec![],
                detached: 0,
                views: Arc::default(),
                answered: std::collections::HashSet::new(),
                inbox: Arc::new(Inbox::new().unwrap()),
                closing: Arc::default(),
                hold: Arc::default(),
                close_stop_attempted: false,
                children: Registry::new(
                    None,
                    1,
                    0,
                    Default::default(),
                    Arc::new(Mutex::new(Custody::default())),
                ),
                child: None,
                pending_completions: std::collections::VecDeque::new(),
                completion_of: std::collections::HashMap::new(),
                recovered_recipient: None,
                close_reported: None,
                continue_attached: false,
                recovered_conversation: None,
            },
            rx,
            dir,
        )
    }

    #[test]
    fn failed_completion_response_keeps_owed_account_until_durable_end() {
        let (worker, rx, _dir) = worker();
        let mut store = worker.store.lock().unwrap();
        let incarnation = store
            .begin_incarnation("token", "unprivileged-userns")
            .unwrap();
        let parent = store.begin_work(0, incarnation).unwrap();
        let work = store
            .begin_bash(0, parent, incarnation, 1, "[]", &["x".into()], "/", true)
            .unwrap();
        let Admission::Admitted(index, _) = store
            .admit_follow_up(0, 0, None, "completion", false, Some((work, None)))
            .unwrap()
        else {
            panic!("first admission")
        };
        let attempt = store.begin_attempt(0, index).unwrap();
        store
            .resolve_attempt(attempt, "no-ack:invalid-response")
            .unwrap();
        drop(store);
        worker.views.lock().unwrap().push(bash::View {
            id: "test".into(),
            ..Default::default()
        });
        bash::owe_async(&worker.views, &worker.tx, 0, work);
        assert!(!worker.settle_async(work, "undelivered", Some("invalid-response")));
        assert!(!worker.settle_async(work, "turn-ended", None));
        let summary = bash::async_summary(&worker.views);
        assert_eq!(summary["owed"], 1);
        assert_eq!(summary["turn_ended"], 0);
        assert_eq!(summary["undelivered"], json!([]));
        let mut store = worker.store.lock().unwrap();
        let later = store.begin_attempt(0, index).unwrap();
        let ack = DurableAck {
            label: "duplicate-unknown".into(),
            basis: None,
            recovered: false,
            generation: store.generation(),
            message_id: Some("m".into()),
        };
        store.record_ack(0, index, later, &ack).unwrap();
        drop(store);
        assert!(!worker.settle_async(work, "turn-ended", None));
        worker
            .store
            .lock()
            .unwrap()
            .record_turn_end(0, index)
            .unwrap();
        assert!(worker.settle_async(work, "turn-ended", None));
        assert!(!worker.settle_async(work, "turn-ended", None));
        let summary = bash::async_summary(&worker.views);
        assert_eq!(summary["owed"], 0);
        assert_eq!(summary["turn_ended"], 1);
        assert_eq!(summary["undelivered"], json!([]));
        let changes: Vec<_> = rx
            .try_iter()
            .filter_map(|event| match event {
                Event::Report(value) if value["event"] == "async-owed" => {
                    Some(value["change"].clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(changes, vec![json!("owed"), json!("turn-ended")]);
    }

    /// CONFIGURED SEAM (A6): a harness launch whose spawn reply is lost or
    /// unproven after root PID 1 may have created the work is possible
    /// effects: reported `launch-unknown`, the work row stays unresolved,
    /// the run cannot read as reaped, and nothing is relaunched. Only a
    /// positive no-start reply resolves the work as never started.
    #[test]
    fn current_rejection_cannot_settle_an_unknown_prior_attempt() {
        let (mut worker, _rx, _dir) = worker();
        worker.tracked[0].label = Some("rejected".into());
        worker.tracked[0].unknown_attempts = 1;
        assert_eq!(worker.settled_turns(), Err("delivery-unresolved"));
        worker.tracked[0].unknown_attempts = 0;
        assert_eq!(worker.settled_turns(), Ok(()));
    }

    #[test]
    fn lost_or_unproven_spawn_reply_is_launch_unknown_not_refused() {
        for mode in ["lost", "unproven", "refused"] {
            let (mut worker, rx, dir) = worker();
            let stop = Arc::new(crate::transport::StopSignal::new().unwrap());
            let (root, far) = Root::seam(false, i32::try_from(std::process::id()).unwrap(), &stop);
            worker.slot = Arc::new(RootSlot::new(
                dir.0.clone(),
                1,
                crate::workload::unprivileged_for_tests(&dir.0),
                stop,
                Some(root),
            ));
            worker.argv = vec!["peer".into()];
            worker
                .store
                .lock()
                .unwrap()
                .begin_incarnation("t", "test")
                .unwrap();
            let replier = std::thread::spawn(move || {
                let (bytes, _) = crate::sys::recv(&far).unwrap().unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                match mode {
                    "lost" => drop(far),
                    "unproven" => crate::sys::send(&far, json!({ "req": request["req"], "event": "refused", "reason": "after-clone", "not_started": false }).to_string().as_bytes(), &[]).unwrap(),
                    _ => crate::sys::send(&far, json!({ "req": request["req"], "event": "refused", "reason": "bad-spawn", "not_started": true }).to_string().as_bytes(), &[]).unwrap(),
                }
            });
            assert!(worker.launch().is_none());
            replier.join().unwrap();
            let events: Vec<Value> = rx
                .try_iter()
                .filter_map(|event| match event {
                    Event::Report(value) => Some(value),
                    _ => None,
                })
                .collect();
            let conn = rusqlite::Connection::open(dir.0.join(crate::store::DB_FILE)).unwrap();
            let outcome: Option<String> = conn
                .query_row("SELECT outcome FROM work", [], |row| row.get(0))
                .unwrap();
            let record = worker.record();
            if mode == "refused" {
                assert!(
                    events.iter().any(|e| e["event"] == "launch-failed"),
                    "{events:?}"
                );
                assert_eq!(outcome.as_deref(), Some("launch-refused"));
                assert!(record.wait_failures.is_empty());
            } else {
                assert!(
                    events.iter().any(|e| e["event"] == "launch-unknown"),
                    "{mode}: {events:?}"
                );
                assert_eq!(outcome, None, "{mode}: possible effects stay unresolved");
                assert_eq!(record.messages[0].label, "launch-unknown");
                let (report, _) =
                    crate::terminal_report(&[("test".into(), 1)], &[record], false, false, None);
                assert_eq!(report["all_harnesses_reaped"], false, "{mode}: {report}");
                assert_ne!(report["status"], "ended-owed");
            }
        }
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
        let (report, code) = crate::terminal_report(
            &[(worker.id.clone(), 1)],
            &[record],
            true,
            false,
            store_lost,
        );
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
