//! This root's Bash ingress: the positive path by which Bash work started
//! inside one of this owner's harness namespaces reaches **this** owner,
//! and nothing else. See the crate docs (Bash) for the wire.
//!
//! * The listener is `bash.sock` in the root's IPC directory: the private
//!   store for `unprivileged-userns`, the owner-made `ipc_dir` for
//!   `host-root`, where the socket is handed to the work identity (`0600`).
//!   Every harness is started with its path in [`BASH_ENV`], which its
//!   descendants inherit.
//! * Attribution is positive: the connecting process (`SO_PEERCRED`) must
//!   have the work's uid (the declared host user under `host-root`, this
//!   owner's otherwise) and be, at that moment, a member of the exact PID
//!   namespace of one live harness work of this owner, as that work's own
//!   PID 1 recorded it. Everything else is refused (`peer-unattributed`),
//!   reported, and nothing is recorded or run: no fallback, no other owner.
//!   A process of that uid elsewhere, or of any other uid (host root
//!   included) inside a harness namespace, is refused.
//! * An accepted run is recorded durably (a `bash` work) before the
//!   requester is told `accepted`, then launched by root PID 1 as its own
//!   work: a per-work PID 1 in a new PID namespace and the command as its
//!   child, reading `/dev/null`, stderr joined to stdout. Its output is
//!   streamed back; its end comes only from its work PID 1's wait.
//! * Each line to the requester is one stage: `accepted` (durable record,
//!   not a start), `started`, `output`, `output-closed` (end of stream, not
//!   an end) or `output-failed`, then exactly one of `end` (with separate
//!   output state), `end-unknown`, `launch-failed` (proven no-start),
//!   `launch-unknown` (possible effects) or `left-to-successor`.
//!   Uncertain launches are never retried. A requester that goes away does not stop the run;
//!   the owner still waits for its end and records it.
//! * The parent attributed before the request was read must still be that
//!   exact live harness work when the run is committed (`parent-not-current`
//!   otherwise), and the durable record keeps that requesting work.
//! * The same socket admits registered children (`op` `child`, the
//!   [`crate::children`] module). A child's own Bash runs are attributed to
//!   the child, refused once it is stopped, and killed when it stops or ends.
//! * Every run's output is also retained by this owner (the
//!   [`crate::retention`] module) and sealed before its `end` is sent; the
//!   `end` (and the owner's `bash-ended`) carry the sealed identity as
//!   `retained`. Two more ops on the same socket, attributed the same way,
//!   serve only the attributed harness's own runs (another harness's run,
//!   or a work that is not a Bash run of this root, is `unknown-work`):
//!   - `{"v":1,"op":"output","root_id":..,"work":N,"offset":O,"length":L}`,
//!     optionally with the `bytes` and `sha256` the requester holds: one
//!     line `output-range` with the sealed identity and the retained bytes
//!     `[O, O+n)` (`b64`), `n` clipped to what was retained and to
//!     [`crate::retention::MAX_READ`].
//!   - `{"v":1,"op":"accept","root_id":..,"work":N,"bytes":B,"sha256":H}`:
//!     only when `B`/`H` are exactly the seal's and the file still hashes
//!     to them, one line `output-accepted` with the durable receipt
//!     (`repeat` when an earlier acceptance of it is returned). Local
//!     acceptance only; it is never an insertion ACK, processing, remote
//!     settlement or drain.
//!   Refusals (`refused`, nothing recorded): `wrong-root`, `unknown-work`,
//!   `not-sealed`, `identity-mismatch`, `offset-beyond-retained`,
//!   `retained-bytes-missing`, `retained-bytes-changed`, `bad-identity`,
//!   `store-failed`, `authority-lost`.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use serde_json::{Value, json};

use crate::Event;
use crate::children::Registry;
use crate::custody::{Adopted, PidNs, ReceiptWait, Root, RootSlot, SpawnError};
use crate::live::Custody;
use crate::retention::{self, Budget, Retainer};
use crate::store::{OutputAccept, Store};
use crate::sys;
use crate::transport;

/// Environment variable naming this root's Bash ingress socket.
pub const BASH_ENV: &str = "OULIPOLY_ROOT_BASH_V1";
/// The ingress socket's name in the store directory.
pub(crate) const SOCKET_FILE: &str = "bash.sock";
/// Protocol version of a run request.
pub const PROTOCOL: u64 = 1;
/// Longest request line accepted.
const MAX_REQUEST: u64 = 1 << 16;
/// Bound the new retained-output hash/range/reply work independently of Bash.
const MAX_OUTPUT_REQUESTS: usize = 8;

/// An owner input sent (or being sent) to a harness's live process and not
/// yet covered by a turn end the harness attributed to it. `message_id` is
/// its acknowledged id, unknown until the acknowledgement is recorded.
#[derive(Debug, Clone)]
pub(crate) struct OpenInput {
    pub(crate) index: usize,
    pub(crate) message_id: Option<String>,
}

/// What the ingress knows of one harness, kept current by its worker.
#[derive(Default)]
pub(crate) struct View {
    pub(crate) id: String,
    pub(crate) session: Option<String>,
    /// Live works of this harness and each one's own PID namespace.
    pub(crate) works: Vec<(i64, PidNs)>,
    pub(crate) open: Vec<OpenInput>,
    /// Background (`delivery` `async`) Bash runs of this harness whose
    /// completion is still owed to it: not yet carried by an input whose
    /// linked turn ended, and not yet ended as undelivered.
    pub(crate) owed_async: Vec<i64>,
    /// Background runs accepted, completions whose linked turn ended, and
    /// completions ended undelivered (each with its reason), on this view.
    pub(crate) async_accepted: u64,
    pub(crate) async_turn_ended: u64,
    pub(crate) async_undelivered: Vec<(i64, String)>,
}

/// Every harness's view, by position.
pub(crate) type Views = Arc<Mutex<Vec<View>>>;

/// Completions still owed across this root (every view).
fn owed_total(views: &[View]) -> usize {
    views.iter().map(|view| view.owed_async.len()).sum()
}

/// Records a background run's completion as owed to the harness at
/// `position`, and reports the root's new owed count. Reported while the
/// views are held, so every `async-owed` report is in owed order.
pub(crate) fn owe_async(views: &Views, tx: &Sender<Event>, position: usize, work: i64) {
    let mut views = views.lock().expect("views");
    let Some(view) = views.get_mut(position) else {
        return;
    };
    view.owed_async.push(work);
    view.async_accepted += 1;
    let harness = view.id.clone();
    let owed = owed_total(&views);
    let _ = tx.send(Event::Report(json!({
        "event": "async-owed",
        "change": "owed",
        "work": work,
        "harness": harness,
        "owed_async": owed,
        "scope": "current-owner-generation",
        "meaning": "a background Bash run's completion is owed to this live harness; the task stays open for it within its deadline",
    })));
}

/// Ends one owed completion: `turn-ended` (an input carrying it was
/// acknowledged and the agent's tagged turn end covered it) or
/// `undelivered` with its reason. False if it was not owed (already ended).
pub(crate) fn settle_async(
    views: &Views,
    store: &Arc<Mutex<Store>>,
    tx: &Sender<Event>,
    position: usize,
    work: i64,
    resolution: &str,
    reason: Option<&str>,
) -> bool {
    let mut views = views.lock().expect("views");
    let Some(view) = views.get_mut(position) else {
        return false;
    };
    let Some(at) = view.owed_async.iter().position(|owed| *owed == work) else {
        return false;
    };
    if let Err(error) = store
        .lock()
        .expect("store lock")
        .resolve_completion(work, resolution, reason)
    {
        let _ = tx.send(Event::Report(
            json!({ "event": error.label(), "reason": format!("{error:?}"), "work": work }),
        ));
        return false;
    }
    view.owed_async.remove(at);
    if resolution == "turn-ended" {
        view.async_turn_ended += 1;
    } else {
        view.async_undelivered
            .push((work, reason.unwrap_or("unknown").to_owned()));
    }
    let harness = view.id.clone();
    let owed = owed_total(&views);
    let _ = tx.send(Event::Report(json!({
        "event": "async-owed",
        "change": resolution,
        "work": work,
        "harness": harness,
        "reason": reason,
        "owed_async": owed,
        "scope": "current-owner-generation",
    })));
    true
}

/// Prior-generation recipient facts, separate from the current view counters.
pub(crate) fn inherited_summary(store: &Arc<Mutex<Store>>, harness: Option<usize>) -> Value {
    match store
        .lock()
        .expect("store lock")
        .inherited_completions(harness)
    {
        Ok(records) => json!({
            "scope": "prior-owner-generations", "records": records,
            "meaning": "async records retain a promised completion; unknown mode retains requester lineage without inferring a promise; delivery-unknown is unresolved, even after run end; recipient delivery is not reconstructed; turn-ended is transport evidence, not processing",
        }),
        Err(error) => {
            json!({ "scope": "prior-owner-generations", "state": "unknown", "reason": format!("store-unread: {error}") })
        }
    }
}

/// The current owner's background-completion account for the terminal report.
pub(crate) fn async_summary(views: &Views) -> Value {
    let views = views.lock().expect("views");
    let undelivered: Vec<Value> = views
        .iter()
        .flat_map(|view| {
            view.async_undelivered
                .iter()
                .map(|(work, reason)| json!({ "harness": view.id, "work": work, "reason": reason }))
        })
        .collect();
    json!({
        "scope": "current-owner-generation",
        "accepted": views.iter().map(|view| view.async_accepted).sum::<u64>(),
        "turn_ended": views.iter().map(|view| view.async_turn_ended).sum::<u64>(),
        "undelivered": undelivered,
        "owed": owed_total(&views),
        "meaning": "turn_ended: a completion input was acknowledged and a tagged turn end covered it; not proof the agent read, used or accepted the output",
    })
}

#[derive(Default)]
pub(crate) struct Gate {
    closed: bool,
    /// Runs admitted and not yet finished, including taken-over ones.
    open: usize,
    /// Children admitted and not yet finished.
    children_open: usize,
    output_open: usize,
    output_requests: u64,
    accepted: u64,
    refused: u64,
    not_run: u64,
    ended: u64,
    unknown: u64,
}

/// How one admitted run finished, for the owner's own accounting.
#[derive(Clone, Copy)]
enum Outcome {
    /// Its end was reported by its actual waiter.
    Ended,
    /// It may have run; how it ended is not known.
    Unknown,
    /// Nothing was started.
    NotRun,
}

/// Admits one child into the count the owning loop waits on, unless the
/// ingress is closed.
pub(crate) fn enter_child(gate: &Arc<(Mutex<Gate>, Condvar)>) -> bool {
    let mut gate = gate.0.lock().expect("bash gate");
    if gate.closed {
        return false;
    }
    gate.children_open += 1;
    true
}

pub(crate) fn leave_child(gate: &Arc<(Mutex<Gate>, Condvar)>, tx: &Sender<Event>) {
    gate.0.lock().expect("bash gate").children_open -= 1;
    let _ = tx.send(Event::BashDone);
}

/// One owner run's Bash ingress.
pub(crate) struct Ingress {
    root_id: String,
    slot: Arc<RootSlot>,
    custody: Arc<Mutex<Custody>>,
    store: Arc<Mutex<Store>>,
    views: Views,
    tx: Sender<Event>,
    gate: Arc<(Mutex<Gate>, Condvar)>,
    children: Arc<Registry>,
    /// Every child harness starts here (the intent's `cwd`).
    cwd: String,
    /// Retained output of this root, against its bound.
    pub(crate) budget: Budget,
    budget_unknown: bool,
    /// Each top-level harness's inbox, by position: where a background
    /// run's completion is offered. Unset (no background runs) in tests
    /// that build an ingress alone.
    inboxes: std::sync::OnceLock<Vec<Arc<crate::conversation::Inbox>>>,
    #[cfg(test)]
    pub(crate) after_spawn_error_unlock: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// A Bash run an earlier owner accepted, as found at attach.
pub(crate) enum Prior {
    Live {
        harness: usize,
        adopted: Adopted,
    },
    Exited {
        harness: usize,
        work: i64,
        receipt: Value,
    },
    Unknown {
        harness: usize,
        work: i64,
    },
}

impl Ingress {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        root_id: String,
        slot: Arc<RootSlot>,
        custody: Arc<Mutex<Custody>>,
        store: Arc<Mutex<Store>>,
        views: Views,
        tx: Sender<Event>,
        children: Arc<Registry>,
        cwd: String,
    ) -> Arc<Self> {
        // Physical files, including unsealed/resolved orphans, are charged
        // synchronously before recovery threads or new work can allocate.
        let usage = retention::initial_usage(&slot.store_dir);
        let used = usage
            .as_ref()
            .copied()
            .unwrap_or(retention::LIMITS.per_root);
        let budget_unknown = usage.is_err();
        let missing = store.lock().expect("store lock").ended_without_output();
        let ingress = Arc::new(Self {
            budget: Budget::new(retention::LIMITS, used),
            budget_unknown,
            root_id,
            slot,
            custody,
            store,
            views,
            tx,
            gate: Arc::default(),
            children,
            cwd,
            inboxes: std::sync::OnceLock::new(),
            #[cfg(test)]
            after_spawn_error_unlock: Mutex::new(None),
        });
        if budget_unknown {
            ingress.report(json!({ "event": "bash-output-budget-unknown", "meaning": "fresh retention denied" }));
        }
        match missing {
            Ok(works) => {
                for work in works {
                    ingress.seal_found_ended(work);
                }
            }
            Err(_) => ingress.report(
                json!({ "event": "bash-output-recovery-unknown", "reason": "store-read-failed" }),
            ),
        }
        ingress
    }

    /// Names where background completions go (top-level harnesses only).
    pub(crate) fn deliver_to(&self, inboxes: Vec<Arc<crate::conversation::Inbox>>) {
        let _ = self.inboxes.set(inboxes);
    }

    /// Why the harness at `position` cannot be owed a background
    /// completion now, if it cannot. Checked before anything is recorded.
    fn async_refusal(&self, position: usize) -> Option<&'static str> {
        if self.children.is_child(position) {
            return Some("async-unavailable: registered child");
        }
        match self.inboxes.get().and_then(|inboxes| inboxes.get(position)) {
            None => Some("async-unavailable: no completion recipient"),
            Some(inbox) if !inbox.accepting() => {
                Some("async-unavailable: requester not in a live conversation")
            }
            Some(_) => None,
        }
    }

    pub(crate) fn socket_path(store: &Path) -> PathBuf {
        store.join(SOCKET_FILE)
    }

    fn report(&self, value: Value) {
        let _ = self.tx.send(Event::Report(value));
    }

    /// Binds the socket and serves it on a thread. Returns the socket path,
    /// or why there is no ingress (then every Bash request fails to
    /// connect, which a requester must read as no owner, never as success).
    pub(crate) fn listen(self: &Arc<Self>) -> Result<PathBuf, String> {
        let path = Self::socket_path(&self.slot.workload.ipc_dir);
        // A socket file left by an earlier owner of this store names no
        // listener: this owner holds the store lock.
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("stale socket: {error}")),
        }
        let listener = UnixListener::bind(&path).map_err(|error| error.to_string())?;
        crate::workload::hand_socket(&self.slot.workload, &path)?;
        let ingress = Arc::clone(self);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                if ingress.gate.0.lock().expect("bash gate").closed {
                    let _ = refuse(&stream, "ingress-closed");
                    if ingress.closed_and_idle() {
                        return;
                    }
                    continue;
                }
                let ingress = Arc::clone(&ingress);
                thread::spawn(move || ingress.serve(stream));
            }
        });
        Ok(path)
    }

    fn closed_and_idle(&self) -> bool {
        let gate = self.gate.0.lock().expect("bash gate");
        gate.closed && gate.open == 0 && gate.output_open == 0
    }

    /// Whether no run is in progress. When none is, closes the ingress: no
    /// later request is accepted. Called by the owning loop once every
    /// harness's end is reported.
    pub(crate) fn close_if_idle(&self) -> bool {
        let mut gate = self.gate.0.lock().expect("bash gate");
        if gate.open > 0 || gate.children_open > 0 || gate.output_open > 0 {
            return false;
        }
        if !gate.closed {
            gate.closed = true;
            drop(gate);
            // Wakes the accept loop so it sees the close and returns.
            let path = Self::socket_path(&self.slot.workload.ipc_dir);
            let _ = UnixStream::connect(&path);
            let _ = std::fs::remove_file(&path);
        }
        true
    }

    /// Counts for the terminal report.
    pub(crate) fn summary(&self) -> Value {
        let gate = self.gate.0.lock().expect("bash gate");
        json!({
            "accepted": gate.accepted,
            "refused": gate.refused,
            "not_run": gate.not_run,
            "ended": gate.ended,
            "end_unknown": gate.unknown,
            "open": gate.open,
            "output_open": gate.output_open,
            "output_requests": gate.output_requests,
        })
    }

    pub(crate) fn ends_unproven(&self) -> bool {
        let gate = self.gate.0.lock().expect("bash gate");
        gate.unknown > 0 || gate.open > 0
    }

    /// Admits one run into the count the owning loop waits on, unless closed.
    fn enter(&self) -> bool {
        let mut gate = self.gate.0.lock().expect("bash gate");
        if gate.closed {
            return false;
        }
        gate.open += 1;
        true
    }

    fn leave(&self, outcome: Outcome) {
        {
            let mut gate = self.gate.0.lock().expect("bash gate");
            gate.open -= 1;
            match outcome {
                Outcome::Ended => gate.ended += 1,
                Outcome::Unknown => gate.unknown += 1,
                Outcome::NotRun => gate.not_run += 1,
            }
        }
        let _ = self.tx.send(Event::BashDone);
    }

    fn refused(&self, stream: &UnixStream, peer: Value, reason: &str) {
        self.gate.0.lock().expect("bash gate").refused += 1;
        let _ = refuse(stream, reason);
        self.report(json!({ "event": "bash-refused", "reason": reason, "peer": peer }));
    }

    fn serve(self: Arc<Self>, stream: UnixStream) {
        let cred = match peer_cred(&stream) {
            Ok(cred) => cred,
            Err(error) => {
                return self.refused(&stream, Value::Null, &format!("peer-unknown: {error}"));
            }
        };
        let peer = json!({ "pid": cred.pid, "uid": cred.uid });
        let attributed = match self.attribute(&cred) {
            Ok(attributed) => attributed,
            Err(reason) => return self.refused(&stream, peer, &reason),
        };
        let request = match read_request(&stream) {
            Ok(request) => request,
            Err(reason) => return self.refused(&stream, peer, &reason),
        };
        match request {
            Request::Run(request) => {
                let mut sink = Sink(Some(stream));
                self.run(&attributed, cred.pid, &request, &mut sink);
            }
            Request::Retained(request) => {
                if let Err(reason) = self.enter_retained() {
                    return self.refused(&stream, peer, reason);
                }
                match self.retained_inner(&attributed, cred.pid, &request, Some(&cred)) {
                    Ok(reply) => Sink(Some(stream)).send(&reply),
                    Err(reason) => self.refused(&stream, peer, &reason),
                }
                self.leave_retained();
            }
            Request::Child(request) => {
                let ctx = crate::children::Context {
                    root_id: &self.root_id,
                    registry: &self.children,
                    slot: &self.slot,
                    custody: &self.custody,
                    store: &self.store,
                    views: &self.views,
                    gate: &self.gate,
                    tx: &self.tx,
                    cwd: &self.cwd,
                };
                crate::children::serve(&ctx, &attributed, cred.pid, request, stream);
            }
        }
    }

    fn enter_retained(&self) -> Result<(), &'static str> {
        let mut gate = self.gate.0.lock().expect("bash gate");
        if gate.closed {
            return Err("ingress-closed");
        }
        if gate.output_open >= MAX_OUTPUT_REQUESTS {
            return Err("output-request-bound");
        }
        gate.output_open += 1;
        gate.output_requests += 1;
        Ok(())
    }

    fn leave_retained(&self) {
        self.gate.0.lock().expect("bash gate").output_open -= 1;
        let _ = self.tx.send(Event::BashDone);
    }

    /// The harness whose live work's own PID namespace the peer is in.
    fn attribute(&self, cred: &libc::ucred) -> Result<Attributed, String> {
        let ns = cred_pidns(cred, self.slot.workload.peer_uid())
            .map_err(|reason| format!("peer-unattributed: {reason}"))?;
        let views = self.views.lock().expect("views");
        views
            .iter()
            .enumerate()
            .find_map(|(position, view)| {
                let &(work, _) = view.works.iter().find(|(_, pidns)| *pidns == ns)?;
                Some(Attributed {
                    position,
                    harness: view.id.clone(),
                    harness_work: work,
                    session: view.session.clone(),
                    open: view.open.clone(),
                })
            })
            .ok_or_else(|| "peer-unattributed: outside-every-harness-namespace".to_owned())
    }

    #[cfg(test)]
    pub(crate) fn run_fault_seam(&self, who: &Attributed, cwd: String) {
        self.run(
            who,
            1,
            &RunRequest {
                argv: vec!["fixture".into()],
                cwd,
                background: false,
            },
            &mut Sink::new(None),
        );
    }

    fn run(&self, who: &Attributed, requester: i32, request: &RunRequest, sink: &mut Sink) {
        let open: Vec<Value> = who
            .open
            .iter()
            .map(|input| json!({ "index": input.index, "message_id": input.message_id }))
            .collect();
        // Which owner input caused this run is known only when exactly one
        // was open on the requesting harness; the request carries no more.
        let input_attribution = match who.open.len() {
            0 => "no-open-input",
            1 => "single-open-input",
            _ => "ambiguous-open-inputs",
        };
        let shared = Arc::clone(&self.custody);
        let mut custody = shared.lock().expect("custody lock");
        if let Some(reason) = custody.reason() {
            drop(custody);
            self.refused(
                sink.stream(),
                json!({ "pid": requester }),
                &format!("owner-stopping: {reason}"),
            );
            return;
        }
        if let Some(reason) = self.children.refuses_bash(who.position) {
            drop(custody);
            self.refused(
                sink.stream(),
                json!({ "pid": requester }),
                &format!("requester-stopped: {reason}"),
            );
            return;
        }
        // The attribution was taken before the request was read: the same
        // live work must still be current at commit.
        let current = self
            .views
            .lock()
            .expect("views")
            .get(who.position)
            .is_some_and(|view| view.works.iter().any(|(work, _)| *work == who.harness_work));
        if !current {
            drop(custody);
            self.refused(
                sink.stream(),
                json!({ "pid": requester }),
                "parent-not-current",
            );
            return;
        }
        if request.background
            && let Some(reason) = self.async_refusal(who.position)
        {
            drop(custody);
            self.refused(sink.stream(), json!({ "pid": requester }), reason);
            return;
        }
        if !self.enter() {
            drop(custody);
            self.refused(sink.stream(), json!({ "pid": requester }), "ingress-closed");
            return;
        }
        let Some(root) = self.slot.current() else {
            drop(custody);
            self.refused(sink.stream(), json!({ "pid": requester }), "no-root-pid1");
            self.leave(Outcome::NotRun);
            return;
        };
        let inputs_open = Value::Array(open.clone()).to_string();
        let begun = self.store.lock().expect("store lock").begin_bash(
            who.position,
            who.harness_work,
            root.incarnation,
            requester,
            &inputs_open,
            &request.argv,
            &request.cwd,
            request.background,
        );
        let work = match begun {
            Ok(work) => work,
            Err(error) => {
                let label = error.label();
                custody.stop(label);
                drop(custody);
                self.report(json!({ "event": label, "reason": format!("{error:?}") }));
                self.refused(sink.stream(), json!({ "pid": requester }), label);
                self.leave(Outcome::NotRun);
                return;
            }
        };
        self.gate.0.lock().expect("bash gate").accepted += 1;
        let attribution = json!({
            "root_id": self.root_id,
            "harness": who.harness,
            "harness_work": who.harness_work,
            "session": who.session,
            "requester_pid": requester,
            "inputs_open": open,
            "input_attribution": input_attribution,
        });
        let mut accepted = json!({ "event": "accepted", "work": work, "durable": true });
        if request.background {
            accepted["delivery"] = json!("async");
        }
        merge(&mut accepted, &attribution);
        sink.send(&accepted);
        let mut owner = json!({ "event": "bash-accepted", "work": work, "argv": request.argv });
        if request.background {
            owner["delivery"] = json!("async");
        }
        merge(&mut owner, &attribution);
        self.report(owner);
        let spawned = root.spawn_observed(
            work,
            &request.argv,
            &serde_json::Map::new(),
            &request.cwd,
            true,
        );
        let spawned = match spawned {
            Ok(spawned) => spawned,
            Err(reason) => {
                let not_started = matches!(reason, SpawnError::NotStarted(_));
                let event = if not_started {
                    "launch-failed"
                } else {
                    "launch-unknown"
                };
                if not_started {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        "launch-refused",
                        Some("root-pid1-no-start-reply"),
                    );
                }
                // Unknown stays unresolved for a successor; never retry here.
                // A child's possible run keeps that child charged.
                if !not_started {
                    self.children.note_run_unknown(who.position);
                }
                // Publish possible child-owned work before releasing the
                // launch/admission lock. A finishing child must see it.
                drop(custody);
                #[cfg(test)]
                if let Some(interleave) = self.after_spawn_error_unlock.lock().unwrap().take() {
                    interleave();
                }
                sink.send(&json!({ "event": event, "reason": reason.reason(), "not_started": not_started }));
                self.report(json!({ "event": format!("bash-{event}"), "work": work, "reason": reason.reason(), "not_started": not_started }));
                self.leave(if not_started {
                    Outcome::NotRun
                } else {
                    Outcome::Unknown
                });
                return;
            }
        };
        let token = custody.register(Arc::clone(&root), work);
        self.children.add_run(who.position, &root, work);
        drop(custody);
        let _ = self
            .store
            .lock()
            .expect("store lock")
            .record_work_spawned(work, spawned.harness_host_pid);
        sink.send(&json!({
            "event": "started",
            "pid": spawned.harness_host_pid,
            "exec_error": spawned.exec_error,
        }));
        self.report(json!({
            "event": "bash-started",
            "work": work,
            "pid": spawned.harness_host_pid,
            "exec_error": spawned.exec_error,
            // At this moment only; a short run may already be gone (null).
            "identity": spawned.harness_host_pid.map(crate::workload::observe),
        }));
        let completion = request.background.then(|| {
            // Owed before the requester hears `detached`, so the owner's
            // `async-owed` report precedes any turn end that follows it.
            owe_async(&self.views, &self.tx, who.position, work);
            sink.send(&json!({
                "event": "detached",
                "work": work,
                "completion": "owed-to-requesting-harness",
                "meaning": "the run continues; its end will be offered to this harness as a later input referencing its retained output; this reply is not its end",
            }));
            // The requester's connection ends here; the run does not.
            sink.0 = None;
            Completion {
                position: who.position,
                argv: request.argv.clone(),
            }
        });
        let mut retainer = Retainer::start(&self.slot.store_dir, work, &self.budget);
        if self.budget_unknown {
            retainer.deny_unknown_budget();
        }
        self.finish(
            &root,
            work,
            token,
            spawned.stdio.stdout,
            Some(sink),
            retainer,
            completion,
        );
    }

    /// Streams the work's output to `sink` (or to no one) while retaining
    /// it, seals what was retained, then waits for its end from its work
    /// PID 1's report, records it and says so.
    fn finish(
        &self,
        root: &Root,
        work: i64,
        token: u64,
        stdout: File,
        sink: Option<&mut Sink>,
        mut retainer: Retainer<'_>,
        completion: Option<Completion>,
    ) {
        let mut sink = sink;
        let (bytes, output) = relay_output(&self.slot.stop, stdout, &mut sink, &mut retainer);
        let retained = retainer.seal(&output, &self.store, &self.root_id);
        let (mut event, outcome) = match root.wait_receipt(work) {
            ReceiptWait::Receipt(receipt) => match receipt["harness"].as_str() {
                Some(status) => {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        status,
                        Some("work-pid1-wait"),
                    );
                    (
                        json!({
                            "event": "end",
                            "status": status,
                            "observer": "work-pid1-wait",
                            "work_pid1": receipt["work_pid1"],
                        }),
                        Outcome::Ended,
                    )
                }
                None => {
                    let _ = self.store.lock().expect("store lock").resolve_work(
                        work,
                        "ended-with-work-namespace-status-unknown",
                        None,
                    );
                    (
                        json!({
                            "event": "end-unknown",
                            "reason": "ended-with-work-namespace-status-unknown",
                            "work_pid1": receipt["work_pid1"],
                        }),
                        Outcome::Unknown,
                    )
                }
            },
            ReceiptWait::Lost => (
                json!({ "event": "end-unknown", "reason": "root-pid1-connection-lost" }),
                Outcome::Unknown,
            ),
            ReceiptWait::Detached => (json!({ "event": "left-to-successor" }), Outcome::Unknown),
        };
        event["output"] = output;
        event["retained"] = retained;
        self.custody.lock().expect("custody lock").release(token);
        self.children
            .remove_run(work, matches!(outcome, Outcome::Ended));
        let caller = match sink {
            _ if completion.is_some() => "detached-async",
            Some(sink) => {
                sink.send(&event);
                if sink.0.is_some() {
                    "connected"
                } else {
                    "gone"
                }
            }
            None => "lost-with-prior-owner",
        };
        let end = event.clone();
        let mut owner = event;
        owner["bash"] = owner["event"].take();
        owner["event"] = json!("bash-ended");
        owner["work"] = json!(work);
        owner["output_bytes"] = json!(bytes);
        owner["requester"] = json!(caller);
        if completion.is_none() {
            owner["inherited_async"] = inherited_summary(&self.store, None);
        }
        self.report(owner);
        if let Some(completion) = completion {
            self.offer_completion(work, &completion, &end);
        }
        // Left only after the completion is offered or settled, so the
        // owning loop cannot end between this run's end and its offer.
        self.leave(outcome);
    }

    /// Offers a background run's end to its requesting harness as one
    /// later input: a reference to its retained output plus its wait and
    /// output facts, never the output itself and never an acceptance.
    fn offer_completion(&self, work: i64, completion: &Completion, end: &Value) {
        let position = completion.position;
        let text = completion_text(&self.root_id, work, &completion.argv, end);
        let undelivered = |reason: &str| {
            settle_async(
                &self.views,
                &self.store,
                &self.tx,
                position,
                work,
                "undelivered",
                Some(reason),
            );
            // A worker held open for this completion re-checks its close.
            if let Some(inbox) = self.inboxes.get().and_then(|inboxes| inboxes.get(position)) {
                inbox.ring();
            }
        };
        if let Some(reason) = self.custody.lock().expect("custody lock").reason() {
            return undelivered(reason);
        }
        let Some(inbox) = self.inboxes.get().and_then(|inboxes| inboxes.get(position)) else {
            return undelivered("no-completion-recipient");
        };
        let follow_up = crate::conversation::FollowUp {
            control: 0,
            caller_ref: Some(format!("bash-completion:{work}")),
            text,
            completion: Some(work),
        };
        match inbox.offer(follow_up) {
            Ok(()) => self.report(json!({
                "event": "bash-async-completion-offered",
                "work": work,
                "harness_position": position,
                "stage": "queued-not-admitted",
                "durable": false,
            })),
            Err(_) => undelivered("recipient-not-in-conversation"),
        }
    }

    /// Takes over a Bash run an earlier owner accepted. Its requester's
    /// connection died with that owner: output is drained, not delivered.
    pub(crate) fn recover(self: &Arc<Self>, prior: Prior) {
        let mut gate = self.gate.0.lock().expect("bash gate");
        gate.open += 1;
        drop(gate);
        let ingress = Arc::clone(self);
        thread::spawn(move || match prior {
            Prior::Live { harness, adopted } => {
                let Some(root) = ingress.slot.current() else {
                    ingress.leave(Outcome::Unknown);
                    return;
                };
                let mut custody = ingress.custody.lock().expect("custody lock");
                let token = custody.register(Arc::clone(&root), adopted.work);
                let stopped = custody.reason();
                drop(custody);
                // A recovered child's Bash is stopped with that child: no
                // child is continued by a successor.
                let child = ingress.children.is_child(harness);
                if stopped.is_some_and(|reason| reason != "authority-lost")
                    || (child && stopped.is_none())
                {
                    root.kill(adopted.work);
                }
                ingress.report(json!({
                    "event": "bash-reattached",
                    "work": adopted.work,
                    "harness_position": harness,
                    "pid": adopted.harness_host_pid,
                }));
                let retainer = ingress.resume_retainer(adopted.work);
                // Recipient facts are durable and reported separately; this
                // path still does not reconstruct delivery to that recipient.
                ingress.finish(
                    &root,
                    adopted.work,
                    token,
                    adopted.stdio.stdout,
                    None,
                    retainer,
                    None,
                );
            }
            Prior::Exited {
                harness,
                work,
                receipt,
            } => {
                ingress.seal_found_ended(work);
                let status = receipt["harness"].as_str();
                let outcome = status.unwrap_or("ended-with-work-namespace-status-unknown");
                let _ = ingress.store.lock().expect("store lock").resolve_work(
                    work,
                    outcome,
                    status.map(|_| "work-pid1-wait"),
                );
                ingress.report(json!({
                    "event": "bash-prior-end",
                    "work": work,
                    "harness_position": harness,
                    "status": status,
                    "meaning": if status.is_some() { "exit-reported-by-its-waiter" } else { "ended-with-work-namespace-status-unknown" },
                }));
                ingress.report(json!({ "event": "inherited-completion-account", "inherited_async": inherited_summary(&ingress.store, Some(harness)) }));
                ingress.leave(if status.is_some() {
                    Outcome::Ended
                } else {
                    Outcome::Unknown
                });
            }
            Prior::Unknown { harness, work } => {
                ingress.seal_found_ended(work);
                let _ = ingress.store.lock().expect("store lock").resolve_work(
                    work,
                    "ended-with-root-namespace-status-unknown",
                    None,
                );
                ingress.report(json!({
                    "event": "bash-prior-end",
                    "work": work,
                    "harness_position": harness,
                    "status": Value::Null,
                    "meaning": "ended-with-root-namespace-status-unknown",
                }));
                ingress.report(json!({ "event": "inherited-completion-account", "inherited_async": inherited_summary(&ingress.store, Some(harness)) }));
                ingress.leave(Outcome::Unknown);
            }
        });
    }

    /// Continues retention of a run an earlier owner accepted.
    fn resume_retainer(&self, work: i64) -> Retainer<'_> {
        let sealed = self
            .store
            .lock()
            .expect("store lock")
            .output(work)
            .ok()
            .flatten()
            .and_then(|lookup| lookup.record);
        let mut retainer = Retainer::resume(&self.slot.store_dir, work, &self.budget, sealed);
        if self.budget_unknown {
            retainer.deny_unknown_budget();
        }
        retainer
    }

    /// Seals what an earlier owner kept of a run found ended (unless it
    /// sealed it itself): its unread remainder, if any, is a loss.
    fn seal_found_ended(&self, work: i64) {
        let retained = self.resume_retainer(work).seal(
            &json!({ "state": "unknown", "reason": "ended-without-this-owner-reading" }),
            &self.store,
            &self.root_id,
        );
        self.report(json!({ "event": "bash-output-sealed", "work": work, "retained": retained }));
    }

    /// Serves one retained-output request of the attributed harness.
    #[cfg(test)]
    fn retained(
        &self,
        who: &Attributed,
        requester: i32,
        request: &RetainedRequest,
    ) -> Result<Value, String> {
        self.retained_inner(who, requester, request, None)
    }

    fn retained_inner(
        &self,
        who: &Attributed,
        requester: i32,
        request: &RetainedRequest,
        peer: Option<&libc::ucred>,
    ) -> Result<Value, String> {
        let custody = self.custody.lock().expect("custody lock");
        if let Some(reason) = custody.reason() {
            return Err(format!("owner-stopping: {reason}"));
        }
        self.retained_current(who, peer)?;
        if request.root_id != self.root_id {
            return Err("wrong-root".to_owned());
        }
        let lookup = self
            .store
            .lock()
            .expect("store lock")
            .output(request.work)
            .map_err(|_| "store-failed".to_owned())?;
        // Another harness's run is not distinguished from no run at all.
        let lookup = lookup
            .filter(|lookup| lookup.harness == who.position)
            .ok_or("unknown-work")?;
        let record = lookup.record.ok_or("not-sealed")?;
        if record.state == "unsealed" {
            return Err("output-unsealed-loss-recorded".into());
        }
        if request.bytes.is_some_and(|bytes| bytes != record.retained)
            || request
                .sha256
                .as_ref()
                .is_some_and(|sha256| *sha256 != record.sha256)
        {
            return Err("identity-mismatch".to_owned());
        }
        let identity = retention::record_json(&self.root_id, request.work, &record);
        if !request.accept {
            if request.offset > record.retained {
                return Err("offset-beyond-retained".to_owned());
            }
            let bytes = retention::read_range(
                &self.slot.store_dir,
                request.work,
                record.retained,
                &record.sha256,
                request.offset,
                request.length,
            )
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    "retained-bytes-changed".to_owned()
                } else {
                    "retained-bytes-missing".to_owned()
                }
            })?;
            self.retained_current(who, peer)?;
            let next = request.offset + bytes.len() as u64;
            self.report(json!({
                "event": "bash-output-read",
                "work": request.work,
                "harness": who.harness,
                "offset": request.offset,
                "length": bytes.len(),
            }));
            return Ok(json!({
                "event": "output-range",
                "retained": identity,
                "offset": request.offset,
                "length": bytes.len(),
                "next_offset": next,
                "eof": next == record.retained,
                "b64": base64(&bytes),
            }));
        }
        // The bytes acceptance names must still be the bytes on disk.
        match retention::hash_prefix(
            &retention::path(&self.slot.store_dir, request.work),
            record.retained,
        ) {
            Ok(sha256) if sha256 == record.sha256 => {}
            Ok(_) => return Err("retained-bytes-changed".to_owned()),
            Err(_) => return Err("retained-bytes-missing".to_owned()),
        }
        self.retained_current(who, peer)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
            });
        let accept = OutputAccept {
            harness_work: who.harness_work,
            requester_pid: requester,
            retained: record.retained,
            sha256: record.sha256.clone(),
            generation: 0,
            accepted_unix: now,
        };
        // Hold the live-work view through the local acceptance transaction:
        // a harness-end update cannot slip between this check and commit.
        let views = self.views.lock().expect("views");
        if !views
            .get(who.position)
            .is_some_and(|view| view.works.iter().any(|(work, _)| *work == who.harness_work))
        {
            return Err("parent-not-current".into());
        }
        let (receipt, repeat) = self
            .store
            .lock()
            .expect("store lock")
            .accept_output(request.work, who.position, &accept)
            .map_err(|error| error.label().to_owned())?;
        drop(views);
        let receipt = json!({
            "root_id": self.root_id,
            "work": request.work,
            "harness": who.harness,
            "harness_work": receipt.harness_work,
            "requester_pid": receipt.requester_pid,
            "bytes": receipt.retained,
            "sha256": receipt.sha256,
            "generation": receipt.generation,
            "accepted_unix": receipt.accepted_unix,
            "verified": "retained-file-rehashed",
        });
        self.report(json!({
            "event": "bash-output-accepted",
            "work": request.work,
            "repeat": repeat,
            "receipt": receipt,
        }));
        Ok(json!({
            "event": "output-accepted",
            "durable": true,
            "repeat": repeat,
            "retained": identity,
            "receipt": receipt,
            "meaning": "local acceptance of exactly these retained bytes by their requesting harness; \
                        not an insertion ACK, processing, remote settlement or drain",
        }))
    }
    fn retained_current(&self, who: &Attributed, peer: Option<&libc::ucred>) -> Result<(), String> {
        if let Some(peer) = peer {
            let now = self.attribute(peer)?;
            if now.position != who.position || now.harness_work != who.harness_work {
                return Err("parent-not-current".into());
            }
        }
        if let Some(reason) = self.children.refuses_bash(who.position) {
            return Err(format!("requester-stopped: {reason}"));
        }
        if !self
            .views
            .lock()
            .expect("views")
            .get(who.position)
            .is_some_and(|view| view.works.iter().any(|(work, _)| *work == who.harness_work))
        {
            return Err("parent-not-current".into());
        }
        Ok(())
    }
}

/// Where a background run's completion goes, and what names it.
pub(crate) struct Completion {
    position: usize,
    argv: Vec<String>,
}

/// Longest command text quoted back in a completion input.
const COMPLETION_COMMAND_CHARS: usize = 400;

/// The completion input's text: what the agent reads. A human summary,
/// then one JSON line with the exact facts. Output is referenced by its
/// retained identity, never inlined; wait, output state and retention are
/// kept separate, and delivery is said not to be acceptance.
pub(crate) fn completion_text(root_id: &str, work: i64, argv: &[String], end: &Value) -> String {
    let command = argv.last().map_or(String::new(), |last| {
        let mut text: String = last.chars().take(COMPLETION_COMMAND_CHARS).collect();
        if last.chars().count() > COMPLETION_COMMAND_CHARS {
            text.push_str("…[truncated]");
        }
        text
    });
    let ended = match end["event"].as_str() {
        Some("end") => format!(
            "ended {} (observer {})",
            end["status"].as_str().unwrap_or("?"),
            end["observer"].as_str().unwrap_or("?")
        ),
        Some(other) => format!(
            "{other}: how it ended is not known ({})",
            end["reason"].as_str().unwrap_or("no reason given")
        ),
        None => "end not reported".to_owned(),
    };
    let output = &end["output"];
    let stream = match output["state"].as_str() {
        Some("closed") => format!("output stream closed after {} bytes", output["bytes"]),
        Some(state) => format!(
            "output stream {state} ({})",
            output["reason"].as_str().unwrap_or("")
        ),
        None => "output stream state unknown".to_owned(),
    };
    let retained = &end["retained"];
    let retention = match retained["identity"].as_str() {
        Some(identity) => format!(
            "retained {} of {} received bytes, state {}: {identity}. Read it with the Bash tool's output_identity (and offset/length); accept exact bytes only with output_identity plus accept_output. This message is a delivery, not an acceptance.",
            retained["bytes"],
            retained["received"],
            retained["state"].as_str().unwrap_or("?"),
        ),
        None => format!(
            "no retained identity ({}); try output_identity rv1w:{root_id}:{work} for what the owner kept.",
            retained["reason"]
                .as_str()
                .or(retained["state"].as_str())
                .unwrap_or("unknown")
        ),
    };
    let facts = json!({
        "completion": "agent-bash-root-v1-async",
        "version": 1,
        "root_id": root_id,
        "work": work,
        "reference": format!("rv1w:{root_id}:{work}"),
        "end": {
            "event": end["event"],
            "status": end["status"],
            "observer": end["observer"],
            "reason": end["reason"],
        },
        "output": output,
        "retained": retained,
        "accepted_locally": false,
    });
    format!(
        "[Background Bash completion] Your background command (work {work}) {ended}; {stream}; {retention}\nCommand: {command}\n{facts}"
    )
}

/// EOF, read failure and detach are different observations; none is a wait.
fn relay_output(
    stop: &transport::StopSignal,
    mut stdout: File,
    sink: &mut Option<&mut Sink>,
    retainer: &mut Retainer<'_>,
) -> (u64, Value) {
    let mut bytes = 0u64;
    let mut buf = [0u8; 16 * 1024];
    let output = loop {
        match transport::readable(stop, &stdout) {
            Ok(true) => {}
            Ok(false) => break json!({ "state": "unknown", "reason": "detached" }),
            Err(error) => break json!({ "state": "failed", "reason": error.to_string() }),
        }
        match stdout.read(&mut buf) {
            Ok(0) => break json!({ "state": "closed", "bytes": bytes }),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => break json!({ "state": "failed", "reason": error.to_string() }),
            Ok(read) => {
                bytes += read as u64;
                // Kept before it is sent: a chunk the requester saw is kept
                // unless a recorded loss says otherwise.
                retainer.take(&buf[..read]);
                if let Some(sink) = sink.as_deref_mut() {
                    sink.send(&json!({ "event": "output", "b64": base64(&buf[..read]) }));
                }
            }
        }
    };
    if let Some(sink) = sink.as_deref_mut() {
        match output["state"].as_str() {
            Some("closed") => sink.send(&json!({ "event": "output-closed", "bytes": bytes })),
            Some("failed") => {
                sink.send(&json!({ "event": "output-failed", "reason": output["reason"] }))
            }
            _ => {}
        }
    }
    (bytes, output)
}

#[derive(Clone)]
pub(crate) struct Attributed {
    pub(crate) position: usize,
    pub(crate) harness: String,
    pub(crate) harness_work: i64,
    pub(crate) session: Option<String>,
    pub(crate) open: Vec<OpenInput>,
}

struct RunRequest {
    argv: Vec<String>,
    cwd: String,
    /// `delivery` `async`: answer after `started` with `detached`, then
    /// deliver the completion to the requesting harness as a later input.
    background: bool,
}

/// A read (`op` `output`) or local acceptance (`op` `accept`) of one run's
/// retained output.
struct RetainedRequest {
    accept: bool,
    root_id: String,
    work: i64,
    /// What the requester holds; required for acceptance.
    bytes: Option<u64>,
    sha256: Option<String>,
    offset: u64,
    length: u64,
}

enum Request {
    Run(RunRequest),
    Child(crate::children::ChildRequest),
    Retained(RetainedRequest),
}

fn parse_retained(value: &Value) -> Result<RetainedRequest, String> {
    let accept = value["op"] == "accept";
    let bad = || "bad-identity".to_owned();
    let root_id = value["root_id"].as_str().ok_or_else(bad)?.to_owned();
    let work = value["work"]
        .as_i64()
        .filter(|work| *work > 0)
        .ok_or_else(bad)?;
    let bytes = match &value["bytes"] {
        Value::Null if !accept => None,
        bytes => Some(bytes.as_u64().ok_or_else(bad)?),
    };
    let sha256 = match &value["sha256"] {
        Value::Null if !accept => None,
        sha256 => Some(
            sha256
                .as_str()
                .filter(|sha256| {
                    sha256.len() == 64
                        && sha256
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
                .ok_or_else(bad)?
                .to_owned(),
        ),
    };
    let number = |key: &str, default: u64| match &value[key] {
        Value::Null => Ok(default),
        number => number.as_u64().ok_or_else(|| format!("bad-{key}")),
    };
    Ok(RetainedRequest {
        accept,
        root_id,
        work,
        bytes,
        sha256,
        offset: number("offset", 0)?,
        length: number("length", retention::MAX_READ)?,
    })
}

/// The requester's connection; writes stop being attempted once it fails.
pub(crate) struct Sink(Option<UnixStream>);

impl Sink {
    pub(crate) fn new(stream: Option<UnixStream>) -> Self {
        Self(stream)
    }

    pub(crate) fn connected(&self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn send(&mut self, value: &Value) {
        if let Some(stream) = self.0.as_mut()
            && writeln!(stream, "{value}")
                .and_then(|()| stream.flush())
                .is_err()
        {
            self.0 = None;
        }
    }

    fn stream(&self) -> &UnixStream {
        self.0
            .as_ref()
            .expect("requester stream before any write failed")
    }
}

fn refuse(mut stream: &UnixStream, reason: &str) -> std::io::Result<()> {
    writeln!(
        stream,
        "{}",
        json!({ "event": "refused", "reason": reason })
    )?;
    stream.flush()
}

fn merge(into: &mut Value, from: &Value) {
    if let (Some(into), Some(from)) = (into.as_object_mut(), from.as_object()) {
        for (key, value) in from {
            into.insert(key.clone(), value.clone());
        }
    }
}

/// The PID namespace of a peer with uid `uid`, read while it provably
/// still runs; `Err` names why it is not attributable.
fn cred_pidns(cred: &libc::ucred, uid: u32) -> Result<PidNs, String> {
    if cred.uid != uid {
        return Err("other-uid".to_owned());
    }
    let pidfd = sys::pidfd_open(cred.pid).map_err(|error| format!("pidfd: {error}"))?;
    let ns = std::fs::metadata(format!("/proc/{}/ns/pid", cred.pid))
        .map_err(|error| format!("pidns: {error}"))?;
    // Still running after the read, so the pid named the peer, not a
    // reuse; a pid can only be reused after its process is reaped.
    if sys::pidfd_exited(&pidfd, 0).unwrap_or(true) {
        return Err("gone".to_owned());
    }
    Ok(PidNs {
        dev: ns.dev(),
        ino: ns.ino(),
    })
}

/// The connected peer's pid and PID namespace, if it has uid `uid`.
pub(crate) fn peer_pidns(stream: &UnixStream, uid: u32) -> Result<(i32, PidNs), String> {
    let cred = peer_cred(stream).map_err(|error| format!("peer-unknown: {error}"))?;
    cred_pidns(&cred, uid).map(|ns| (cred.pid, ns))
}

fn peer_cred(stream: &UnixStream) -> std::io::Result<libc::ucred> {
    let fd: OwnedFd = stream.as_fd().try_clone_to_owned()?;
    sys::peer_cred(&fd)
}

fn read_request(stream: &UnixStream) -> Result<Request, String> {
    let mut line = String::new();
    BufReader::new(stream.take(MAX_REQUEST))
        .read_line(&mut line)
        .map_err(|error| format!("malformed: {error}"))?;
    let value: Value = serde_json::from_str(line.trim()).map_err(|_| "malformed".to_owned())?;
    if value["v"].as_u64() != Some(PROTOCOL) {
        return Err("unsupported-version".to_owned());
    }
    if value["op"] == "child" {
        return crate::children::parse_request(&value).map(Request::Child);
    }
    if value["op"] == "output" || value["op"] == "accept" {
        return parse_retained(&value).map(Request::Retained);
    }
    if value["op"] != "run" {
        return Err("unknown-op".to_owned());
    }
    let argv: Option<Vec<String>> = value["argv"].as_array().and_then(|argv| {
        argv.iter()
            .map(|arg| {
                arg.as_str()
                    .filter(|arg| !arg.contains('\0'))
                    .map(str::to_owned)
            })
            .collect()
    });
    let argv = argv.filter(|argv| !argv.is_empty()).ok_or("bad-argv")?;
    let cwd = value["cwd"]
        .as_str()
        .filter(|cwd| cwd.starts_with('/') && !cwd.contains('\0'))
        .ok_or("bad-cwd")?
        .to_owned();
    let background = match &value["delivery"] {
        Value::Null => false,
        Value::String(mode) if mode == "sync" => false,
        Value::String(mode) if mode == "async" => true,
        _ => return Err("bad-delivery".to_owned()),
    };
    Ok(Request::Run(RunRequest {
        argv,
        cwd,
        background,
    }))
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (i, &byte)| {
            acc | u32::from(byte) << (16 - 8 * i)
        });
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(triple >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Inverse of [`base64`]; `None` for anything it did not produce.
pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let value = |byte: u8| match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    };
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let chunks = text.as_bytes().chunks(4);
    let count = chunks.len();
    for (index, chunk) in chunks.enumerate() {
        let pad = chunk.iter().rev().take_while(|&&byte| byte == b'=').count();
        if pad > 2 || (pad > 0 && index + 1 != count) {
            return None;
        }
        let mut triple = 0u32;
        for (i, &byte) in chunk[..4 - pad].iter().enumerate() {
            triple |= u32::from(value(byte)?) << (18 - 6 * i);
        }
        if (pad == 2 && triple & 0xffff != 0) || (pad == 1 && triple & 0xff != 0) {
            return None;
        }
        for i in 0..3 - pad {
            out.push((triple >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::StopSignal;
    use crate::{Endpoint, HarnessSpec, Intent};
    use std::sync::mpsc::channel;
    use std::time::Duration;

    // Configured custody peer, not a native root: faults occur after an
    // actual deterministic command effect in a fresh private fixture.
    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("bash-honesty-{}", sys::random_hex().unwrap()));
            std::fs::create_dir(&dir).unwrap();
            eprintln!("owned-fixture: {}", dir.display());
            Self(dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    /// The ingress, the seam's root, its far end, and the requesting
    /// harness's (recorded, current) work.
    fn fixture(dir: &Path) -> (Arc<Ingress>, Arc<Root>, OwnedFd, i64) {
        let intent = Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: dir.display().to_string(),
            harnesses: vec![HarnessSpec {
                id: "h".into(),
                argv: vec!["peer".into()],
                endpoint: Endpoint::Stdio,
                session: None,
                messages: vec!["one".into()],
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: None,
        };
        let mut claimed = Store::claim(&dir.join("store"), Some(&intent)).unwrap();
        claimed
            .store
            .begin_incarnation("fixture", "fixture")
            .unwrap();
        let parent = claimed.store.begin_work(0, 1).unwrap();
        let (ingress, root, far) = ingress_over(dir, claimed, parent);
        (ingress, root, far, parent)
    }

    /// An ingress over an already claimed store (a successor's, say).
    fn ingress_over(
        dir: &Path,
        claimed: crate::store::Claimed,
        parent: i64,
    ) -> (Arc<Ingress>, Arc<Root>, OwnedFd) {
        let generation = claimed.store.generation();
        let views: Views = Arc::new(Mutex::new(vec![View {
            id: "h".into(),
            session: None,
            works: vec![(parent, PidNs { dev: 0, ino: 0 })],
            open: vec![],
            ..View::default()
        }]));
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = Root::seam(false, i32::try_from(std::process::id()).unwrap(), &stop);
        let slot = Arc::new(RootSlot::new(
            dir.join("store"),
            generation,
            crate::workload::unprivileged_for_tests(&dir.join("store")),
            Arc::clone(&stop),
            Some(Arc::clone(&root)),
        ));
        let (tx, _rx) = channel();
        let custody = Arc::new(Mutex::new(Custody::new(stop)));
        let children = Registry::new(None, 2, 0, Default::default(), Arc::clone(&custody));
        let ingress = Ingress::new(
            claimed.root_id,
            slot,
            custody,
            Arc::new(Mutex::new(claimed.store)),
            views,
            tx,
            children,
            dir.display().to_string(),
        );
        (ingress, root, far)
    }

    #[test]
    fn spawn_reply_faults_keep_possible_effects_unknown_and_intent_open() {
        for mode in ["lost-reply", "missing-stdio", "unproven-refusal", "refused"] {
            let dir = Fixture::new();
            let (ingress, _root, far, parent) = fixture(&dir.0);
            let marker = dir.0.join("effects");
            let replier = thread::spawn(move || {
                let (bytes, _) = sys::recv(&far).unwrap().unwrap();
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                if mode != "refused" {
                    let status = std::process::Command::new("/bin/sh")
                        .args(["-c", "printf x >> effects"])
                        .current_dir(marker.parent().unwrap())
                        .status()
                        .unwrap();
                    assert!(status.success());
                }
                if mode != "lost-reply" {
                    let reply = if mode == "refused" {
                        json!({ "req": request["req"], "event": "refused", "reason": "fixture-no-start", "not_started": true })
                    } else if mode == "unproven-refusal" {
                        json!({ "req": request["req"], "event": "refused", "reason": "fixture-after-creation", "not_started": false })
                    } else {
                        json!({ "req": request["req"], "event": "spawned" })
                    };
                    sys::send(&far, reply.to_string().as_bytes(), &[]).unwrap();
                }
            });
            let (near, far) = UnixStream::pair().unwrap();
            far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let who = Attributed {
                position: 0,
                harness: "h".into(),
                harness_work: parent,
                session: None,
                open: vec![],
            };
            let request = RunRequest {
                argv: vec!["fixture".into()],
                cwd: dir.0.display().to_string(),
                background: false,
            };
            ingress.run(&who, 1, &request, &mut Sink(Some(near)));
            replier.join().unwrap();
            let events: Vec<Value> = BufReader::new(far)
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect();
            assert_eq!(events[0]["event"], "accepted");
            let conn = rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE))
                .unwrap();
            let outcome: Option<String> = conn
                .query_row("SELECT outcome FROM work WHERE kind = 'bash'", [], |row| {
                    row.get(0)
                })
                .unwrap();
            if mode == "refused" {
                assert_eq!(events[1]["event"], "launch-failed");
                assert_eq!(outcome.as_deref(), Some("launch-refused"));
                assert!(!dir.0.join("effects").exists());
                assert_eq!(ingress.summary()["not_run"], 1);
            } else {
                assert_eq!(
                    std::fs::read(dir.0.join("effects")).unwrap(),
                    b"x",
                    "one effect, no retry"
                );
                assert_eq!(events[1]["event"], "launch-unknown");
                assert_eq!(outcome, None, "retain unresolved durable intent");
                assert_eq!(ingress.summary()["not_run"], 0);
                assert_eq!(ingress.summary()["end_unknown"], 1);
                assert!(ingress.ends_unproven());
            }
        }
    }

    /// The requesting work attributed before the request was read must
    /// still be current at commit: otherwise refused, nothing recorded or
    /// run, and nothing is asked of root PID 1.
    #[test]
    fn stale_requesting_work_at_commit_is_refused_and_nothing_runs() {
        let dir = Fixture::new();
        let (ingress, _root, _far, parent) = fixture(&dir.0);
        ingress.views.lock().unwrap()[0].works.clear();
        let (near, far) = UnixStream::pair().unwrap();
        far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        let request = RunRequest {
            argv: vec!["fixture".into()],
            cwd: "/".into(),
            background: false,
        };
        ingress.run(&who, 1, &request, &mut Sink(Some(near)));
        let mut reply = String::new();
        BufReader::new(far).read_line(&mut reply).unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["event"], "refused");
        assert_eq!(reply["reason"], "parent-not-current");
        let conn =
            rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        let runs: i64 = conn
            .query_row("SELECT count(*) FROM work WHERE kind = 'bash'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(runs, 0);
        assert_eq!(ingress.summary()["accepted"], 0);
    }

    #[test]
    fn output_read_failure_preserves_wait_but_never_claims_eof() {
        let dir = Fixture::new();
        let (ingress, root, far, parent) = fixture(&dir.0);
        let work = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        assert!(ingress.enter());
        let token = ingress
            .custody
            .lock()
            .unwrap()
            .register(Arc::clone(&root), work);
        let receipt = json!({ "event": "receipt", "work": crate::custody::work_name(work), "harness": "code:0", "work_pid1": "code:0" });
        sys::send(&far, receipt.to_string().as_bytes(), &[]).unwrap();
        let (near, far) = UnixStream::pair().unwrap();
        far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut sink = Sink(Some(near));
        // Linux directory read yields EISDIR; it is not EOF.
        let retainer = Retainer::start(&dir.0.join("store"), work, &ingress.budget);
        ingress.finish(
            &root,
            work,
            token,
            File::open(&dir.0).unwrap(),
            Some(&mut sink),
            retainer,
            None,
        );
        drop(sink);
        let events: Vec<Value> = BufReader::new(far)
            .lines()
            .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
            .collect();
        assert_eq!(events[0]["event"], "output-failed");
        assert_eq!(events[1]["event"], "end");
        assert_eq!(events[1]["status"], "code:0");
        assert_eq!(events[1]["output"]["state"], "failed");
        assert!(events.iter().all(|event| event["event"] != "output-closed"));
        assert_eq!(
            ingress.summary()["ended"],
            1,
            "actual wait remains distinct"
        );
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Bytes no prefix bound keeps whole: binary, NUL, invalid UTF-8.
    fn payload(len: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9u32;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect()
    }

    /// Runs one recorded Bash work whose stdout is `data` and whose work
    /// PID 1 reports `status`; returns the requester's stage lines.
    fn finished_run(
        ingress: &Arc<Ingress>,
        root: &Arc<Root>,
        far: &OwnedFd,
        dir: &Path,
        parent: i64,
        data: &[u8],
        status: &str,
    ) -> (i64, Vec<Value>) {
        let work = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        assert!(ingress.enter());
        let token = ingress
            .custody
            .lock()
            .unwrap()
            .register(Arc::clone(root), work);
        let receipt = json!({ "event": "receipt", "work": crate::custody::work_name(work), "harness": status, "work_pid1": "code:0" });
        sys::send(far, receipt.to_string().as_bytes(), &[]).unwrap();
        let source = dir.join(format!("stdout-{work}"));
        std::fs::write(&source, data).unwrap();
        let (near, far) = UnixStream::pair().unwrap();
        far.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        near.set_write_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let reader = thread::spawn(move || {
            BufReader::new(far)
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect::<Vec<Value>>()
        });
        let mut sink = Sink(Some(near));
        let retainer = Retainer::start(&dir.join("store"), work, &ingress.budget);
        ingress.finish(
            root,
            work,
            token,
            File::open(&source).unwrap(),
            Some(&mut sink),
            retainer,
            None,
        );
        drop(sink);
        let events = reader.join().unwrap();
        (work, events)
    }

    fn read_request(root_id: &str, work: i64, offset: u64, length: u64) -> RetainedRequest {
        RetainedRequest {
            accept: false,
            root_id: root_id.to_owned(),
            work,
            bytes: None,
            sha256: None,
            offset,
            length,
        }
    }

    fn accept_request(root_id: &str, work: i64, bytes: u64, sha256: &str) -> RetainedRequest {
        RetainedRequest {
            accept: true,
            root_id: root_id.to_owned(),
            work,
            bytes: Some(bytes),
            sha256: Some(sha256.to_owned()),
            offset: 0,
            length: 0,
        }
    }

    /// Reassembles a run's retained bytes through `op` `output` alone.
    fn read_all(ingress: &Ingress, who: &Attributed, root_id: &str, work: i64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let reply = ingress
                .retained(
                    who,
                    1,
                    &read_request(root_id, work, out.len() as u64, 50_000),
                )
                .unwrap();
            assert_eq!(reply["offset"], out.len());
            out.extend(unbase64(reply["b64"].as_str().unwrap()).unwrap());
            assert_eq!(reply["next_offset"], out.len());
            if reply["eof"] == true {
                return out;
            }
        }
    }

    /// The whole output, well past any inline prefix, is retained exactly,
    /// named by an identity the requester gets with its actual wait, and
    /// served and accepted only for that harness, that root and those bytes.
    #[test]
    fn retained_output_is_exact_and_accepted_only_by_its_requester_for_its_identity() {
        let dir = Fixture::new();
        let (ingress, root, far, parent) = fixture(&dir.0);
        let root_id = ingress.root_id.clone();
        let data = payload(300_000);
        let (work, events) = finished_run(&ingress, &root, &far, &dir.0, parent, &data, "code:7");
        let end = events.last().unwrap();
        // The wait is its waiter's, apart from retention.
        assert_eq!(end["event"], "end");
        assert_eq!(end["status"], "code:7");
        assert_eq!(
            end["output"],
            json!({ "state": "closed", "bytes": 300_000 })
        );
        let retained = &end["retained"];
        assert_eq!(retained["state"], "complete");
        assert_eq!(retained["bytes"], 300_000);
        assert_eq!(retained["received"], 300_000);
        assert_eq!(retained["sha256"], sha256_hex(&data));
        assert_eq!(retained["root_id"], root_id.as_str());
        assert_eq!(
            retained["identity"],
            format!("rv1o:{root_id}:{work}:300000:{}", sha256_hex(&data))
        );
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        assert_eq!(read_all(&ingress, &who, &root_id, work), data);
        // A range is clipped to what was retained and says where it ended.
        let tail = ingress
            .retained(&who, 1, &read_request(&root_id, work, 299_990, 4096))
            .unwrap();
        assert_eq!(
            unbase64(tail["b64"].as_str().unwrap()).unwrap(),
            data[299_990..]
        );
        assert_eq!(tail["eof"], true);
        let sha = sha256_hex(&data);
        let other = Attributed {
            position: 1,
            harness: "other".into(),
            ..who.clone()
        };
        ingress.views.lock().unwrap().push(View {
            id: "other".into(),
            works: vec![(parent, PidNs { dev: 0, ino: 0 })],
            ..View::default()
        });
        let refusals = [
            (&other, read_request(&root_id, work, 0, 10), "unknown-work"),
            (
                &who,
                read_request(&"0".repeat(32), work, 0, 10),
                "wrong-root",
            ),
            (&who, read_request(&root_id, parent, 0, 10), "unknown-work"),
            (&who, read_request(&root_id, 999, 0, 10), "unknown-work"),
            (
                &who,
                read_request(&root_id, work, 300_001, 10),
                "offset-beyond-retained",
            ),
            (
                &who,
                accept_request(&root_id, work, 300_000, &"0".repeat(64)),
                "identity-mismatch",
            ),
            (
                &who,
                accept_request(&root_id, work, 299_999, &sha),
                "identity-mismatch",
            ),
            (
                &other,
                accept_request(&root_id, work, 300_000, &sha),
                "unknown-work",
            ),
        ];
        for (asker, request, reason) in &refusals {
            assert_eq!(ingress.retained(asker, 1, request).unwrap_err(), *reason);
        }
        let db =
            || rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        let accepts = |conn: &rusqlite::Connection| -> i64 {
            conn.query_row("SELECT count(*) FROM bash_output_accept", [], |row| {
                row.get(0)
            })
            .unwrap()
        };
        assert_eq!(accepts(&db()), 0, "no refusal records an acceptance");
        let first = ingress
            .retained(&who, 4242, &accept_request(&root_id, work, 300_000, &sha))
            .unwrap();
        assert_eq!(first["event"], "output-accepted");
        assert_eq!(first["durable"], true);
        assert_eq!(first["repeat"], false);
        assert_eq!(first["receipt"]["bytes"], 300_000);
        assert_eq!(first["receipt"]["sha256"], sha.as_str());
        assert_eq!(first["receipt"]["requester_pid"], 4242);
        assert_eq!(first["receipt"]["harness_work"], parent);
        let row: (i64, String, i64) = db()
            .query_row(
                "SELECT retained, sha256, harness FROM bash_output_accept WHERE work = ?1",
                [work],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (300_000, sha.clone(), 0));
        let again = ingress
            .retained(&who, 7, &accept_request(&root_id, work, 300_000, &sha))
            .unwrap();
        assert_eq!(again["repeat"], true);
        assert_eq!(
            again["receipt"], first["receipt"],
            "the earlier receipt, unchanged"
        );
        assert_eq!(accepts(&db()), 1);

        // A successor owner of the same store serves and recognizes the same
        // identity: retention and acceptance outlive the owner that made them.
        drop(ingress);
        let claimed = Store::claim(&dir.0.join("store"), None).unwrap();
        assert_eq!(claimed.store.generation(), 2);
        let (successor, _root2, _far2) = ingress_over(&dir.0, claimed, parent);
        assert_eq!(read_all(&successor, &who, &root_id, work), data);
        let later = successor
            .retained(&who, 9, &accept_request(&root_id, work, 300_000, &sha))
            .unwrap();
        assert_eq!(later["repeat"], true);
        assert_eq!(later["receipt"]["generation"], 1);
        // Changed bytes on disk are never accepted, even as a repeat.
        let path = retention::path(&dir.0.join("store"), work);
        let mut changed = std::fs::read(&path).unwrap();
        changed[150_000] ^= 1;
        std::fs::write(&path, &changed).unwrap();
        assert_eq!(
            successor
                .retained(&who, 9, &read_request(&root_id, work, 0, 10))
                .unwrap_err(),
            "retained-bytes-changed"
        );
        assert_eq!(
            successor
                .retained(&who, 9, &accept_request(&root_id, work, 300_000, &sha))
                .unwrap_err(),
            "retained-bytes-changed"
        );
        std::fs::write(&path, &changed[..1000]).unwrap();
        assert_eq!(
            successor
                .retained(&who, 9, &read_request(&root_id, work, 5000, 10))
                .unwrap_err(),
            "retained-bytes-missing"
        );
    }

    /// Bounds keep a prefix and say so: never `complete`, the received
    /// count stays exact, and the requester's own stream is unaffected.
    #[test]
    fn retention_bounds_keep_a_counted_prefix_and_record_the_loss() {
        let dir = Fixture::new();
        let (ingress, root, far, parent) = fixture(&dir.0);
        let root_id = ingress.root_id.clone();
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        ingress.budget.set_limits(retention::Limits {
            per_run: 40_000,
            per_root: 60_000,
        });
        let data = payload(100_000);
        let (work, events) = finished_run(&ingress, &root, &far, &dir.0, parent, &data, "code:0");
        let streamed: Vec<u8> = events
            .iter()
            .filter(|event| event["event"] == "output")
            .flat_map(|event| unbase64(event["b64"].as_str().unwrap()).unwrap())
            .collect();
        assert_eq!(streamed, data, "the live relay is not bounded by retention");
        let retained = &events.last().unwrap()["retained"];
        assert_eq!(retained["state"], "partial");
        assert_eq!(retained["bytes"], 40_000);
        assert_eq!(retained["received"], 100_000);
        assert_eq!(retained["losses"][0]["reason"], "per-run-bound");
        assert_eq!(retained["sha256"], sha256_hex(&data[..40_000]));
        assert_eq!(read_all(&ingress, &who, &root_id, work), data[..40_000]);
        // The root bound leaves 20 000 for the next run.
        let (second, events) = finished_run(&ingress, &root, &far, &dir.0, parent, &data, "code:0");
        let retained = &events.last().unwrap()["retained"];
        assert_eq!(retained["state"], "partial");
        assert_eq!(retained["bytes"], 20_000);
        assert_eq!(retained["losses"][0]["reason"], "per-root-bound");
        assert_eq!(read_all(&ingress, &who, &root_id, second), data[..20_000]);
        // An empty run is complete with zero bytes, not a missing record.
        let (_, events) = finished_run(&ingress, &root, &far, &dir.0, parent, b"", "code:0");
        let retained = &events.last().unwrap()["retained"];
        assert_eq!(retained["state"], "complete");
        assert_eq!(retained["bytes"], 0);
    }

    /// A run a successor finds ended keeps what the earlier owner wrote,
    /// sealed as partial with the takeover recorded; a run that owner had
    /// already sealed keeps that seal.
    #[test]
    fn takeover_seals_what_was_kept_and_records_the_owner_change() {
        let dir = Fixture::new();
        let (ingress, root, far, parent) = fixture(&dir.0);
        let root_id = ingress.root_id.clone();
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        let data = payload(5000);
        let (sealed, events) = finished_run(&ingress, &root, &far, &dir.0, parent, &data, "code:0");
        let first_seal = events.last().unwrap()["retained"].clone();
        let orphan = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        std::fs::create_dir_all(dir.0.join("store").join(retention::DIR)).unwrap();
        std::fs::write(retention::path(&dir.0.join("store"), orphan), &data[..1234]).unwrap();
        assert_eq!(
            ingress
                .retained(&who, 1, &read_request(&root_id, orphan, 0, 10))
                .unwrap_err(),
            "not-sealed"
        );
        ingress.seal_found_ended(orphan);
        ingress.seal_found_ended(sealed);
        let reply = ingress
            .retained(&who, 1, &read_request(&root_id, orphan, 0, 10_000))
            .unwrap();
        let retained = &reply["retained"];
        assert_eq!(retained["state"], "partial");
        assert_eq!(retained["bytes"], 1234);
        assert_eq!(retained["received"], Value::Null);
        assert_eq!(retained["losses"][0]["reason"], "owner-changed");
        assert_eq!(retained["losses"][0]["at"], 1234);
        assert_eq!(retained["losses"][1]["reason"], "stream-not-closed");
        assert_eq!(
            unbase64(reply["b64"].as_str().unwrap()).unwrap(),
            data[..1234]
        );
        let reply = ingress
            .retained(&who, 1, &read_request(&root_id, sealed, 0, 1))
            .unwrap();
        assert_eq!(
            reply["retained"], first_seal,
            "an earlier seal is kept as it was"
        );
    }

    #[test]
    fn retained_stale_parent_and_owner_stop_never_commit_acceptance() {
        let dir = Fixture::new();
        let (ingress, root, far, parent) = fixture(&dir.0);
        let data = payload(100);
        let (work, _) = finished_run(&ingress, &root, &far, &dir.0, parent, &data, "code:7");
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        let accept = accept_request(&ingress.root_id, work, 100, &sha256_hex(&data));
        ingress.views.lock().unwrap()[0].works.clear();
        assert_eq!(
            ingress.retained(&who, 1, &accept).unwrap_err(),
            "parent-not-current"
        );
        assert_eq!(
            ingress
                .retained(&who, 1, &read_request(&ingress.root_id, work, 0, 10))
                .unwrap_err(),
            "parent-not-current"
        );
        let conn =
            rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM bash_output_accept", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        ingress.views.lock().unwrap()[0]
            .works
            .push((parent, PidNs { dev: 0, ino: 0 }));
        ingress.custody.lock().unwrap().stop("fixture-stop");
        assert!(
            ingress
                .retained(&who, 1, &accept)
                .unwrap_err()
                .starts_with("owner-stopping:")
        );
    }

    #[test]
    fn failed_file_seal_is_a_persisted_loss_not_a_recoverable_identity() {
        let dir = Fixture::new();
        let (ingress, _root, _far, parent) = fixture(&dir.0);
        let work = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        let mut retainer = Retainer::start(&dir.0.join("store"), work, &ingress.budget);
        retainer.take(b"abc");
        // Removing the name leaves the open FD but makes its hash impossible.
        std::fs::remove_file(retention::path(&dir.0.join("store"), work)).unwrap();
        let failed = retainer.seal(
            &json!({ "state": "closed", "bytes": 3 }),
            &ingress.store,
            &ingress.root_id,
        );
        assert_eq!(failed["state"], "unsealed");
        assert!(failed["identity"].is_null());
        assert_eq!(failed["losses"][0]["reason"], "seal-failed");
        ingress
            .store
            .lock()
            .unwrap()
            .resolve_work(work, "code:7", Some("work-pid1-wait"))
            .unwrap();
        drop(ingress);
        let claimed = Store::claim(&dir.0.join("store"), None).unwrap();
        let (next, _, _) = ingress_over(&dir.0, claimed, parent);
        let lookup = next.store.lock().unwrap().output(work).unwrap().unwrap();
        assert_eq!(lookup.record.unwrap().state, "unsealed");
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        assert_eq!(
            next.retained(&who, 1, &read_request(&next.root_id, work, 0, 3))
                .unwrap_err(),
            "output-unsealed-loss-recorded"
        );
    }

    #[test]
    fn takeover_charges_all_files_before_grants_and_revisits_resolved_missing_seals() {
        let dir = Fixture::new();
        let (ingress, _root, _far, parent) = fixture(&dir.0);
        let work = ingress
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        let mut retainer = Retainer::start(&dir.0.join("store"), work, &ingress.budget);
        retainer.take(b"orphan-prefix");
        let conn =
            rusqlite::Connection::open(dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        conn.execute_batch("CREATE TRIGGER fail_seal BEFORE INSERT ON bash_output BEGIN SELECT RAISE(ABORT, 'fixture'); END;").unwrap();
        let result = retainer.seal(
            &json!({ "state": "closed", "bytes": 13 }),
            &ingress.store,
            &ingress.root_id,
        );
        assert_eq!(result["state"], "unsealed");
        ingress
            .store
            .lock()
            .unwrap()
            .resolve_work(work, "code:7", Some("work-pid1-wait"))
            .unwrap();
        conn.execute_batch("DROP TRIGGER fail_seal").unwrap();
        drop(ingress);
        // An additional open prefix must already count at construction.
        std::fs::write(
            retention::path(&dir.0.join("store"), 999),
            b"another-orphan",
        )
        .unwrap();
        let claimed = Store::claim(&dir.0.join("store"), None).unwrap();
        let (next, _, _) = ingress_over(&dir.0, claimed, parent);
        next.budget.set_limits(retention::Limits {
            per_run: 100,
            per_root: 30,
        });
        let new = next
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        let mut retainer = Retainer::start(&dir.0.join("store"), new, &next.budget);
        retainer.take(b"abcdefghij");
        let record = retainer.seal(
            &json!({ "state": "closed", "bytes": 10 }),
            &next.store,
            &next.root_id,
        );
        assert_eq!(record["bytes"], 3); // 30 - 13 - 14, no deferred recovery charge
        assert_eq!(record["losses"][0]["reason"], "per-root-bound");
        let who = Attributed {
            position: 0,
            harness: "h".into(),
            harness_work: parent,
            session: None,
            open: vec![],
        };
        let old = next
            .retained(&who, 1, &read_request(&next.root_id, work, 0, 20))
            .unwrap();
        assert_eq!(old["retained"]["state"], "partial");
        assert_eq!(old["retained"]["received"], Value::Null);
        assert_eq!(old["retained"]["losses"][0]["reason"], "owner-changed");
        assert_eq!(
            unbase64(old["b64"].as_str().unwrap()).unwrap(),
            b"orphan-prefix"
        );
    }

    #[test]
    fn unknown_initial_budget_denies_storage_and_reports_the_uncertainty() {
        let dir = Fixture::new();
        let (ingress, _root, _far, parent) = fixture(&dir.0);
        std::fs::create_dir_all(dir.0.join("store/output/unexpected-directory")).unwrap();
        drop(ingress);
        let claimed = Store::claim(&dir.0.join("store"), None).unwrap();
        let (next, root, far) = ingress_over(&dir.0, claimed, parent);
        assert!(next.budget_unknown);
        let work = next
            .store
            .lock()
            .unwrap()
            .begin_bash(0, parent, 1, 1, "[]", &["fixture".into()], "/", false)
            .unwrap();
        // This is the same unknown-budget denial the production start applies.
        let mut retainer = Retainer::start(&dir.0.join("store"), work, &next.budget);
        retainer.deny_unknown_budget();
        retainer.take(b"lost");
        let record = retainer.seal(
            &json!({ "state": "closed", "bytes": 4 }),
            &next.store,
            &next.root_id,
        );
        assert_eq!(record["state"], "partial");
        assert_eq!(record["bytes"], 0);
        assert_eq!(record["received"], 4);
        assert_eq!(record["losses"][0]["reason"], "budget-unknown");
        drop((root, far));
    }

    #[test]
    fn retained_request_budget_is_bounded_and_reply_work_prevents_idle_close() {
        let dir = Fixture::new();
        let (ingress, _root, _far, _parent) = fixture(&dir.0);
        for _ in 0..MAX_OUTPUT_REQUESTS {
            ingress.enter_retained().unwrap();
        }
        assert_eq!(
            ingress.enter_retained().unwrap_err(),
            "output-request-bound"
        );
        assert_eq!(ingress.summary()["output_open"], MAX_OUTPUT_REQUESTS);
        assert_eq!(ingress.summary()["accepted"], 0); // retrieval is not a Bash launch
        assert!(!ingress.close_if_idle());
        for _ in 0..MAX_OUTPUT_REQUESTS {
            ingress.leave_retained();
        }
        assert!(ingress.close_if_idle());
        assert_eq!(ingress.summary()["output_open"], 0);
        assert_eq!(ingress.summary()["output_requests"], MAX_OUTPUT_REQUESTS);
        assert_eq!(ingress.enter_retained().unwrap_err(), "ingress-closed");
    }

    #[test]
    fn retained_requests_parse_strictly() {
        let sha = "a".repeat(64);
        let ok = parse_retained(&json!({ "op": "output", "root_id": "r", "work": 3 })).unwrap();
        assert_eq!(
            (ok.offset, ok.length, ok.accept),
            (0, retention::MAX_READ, false)
        );
        for bad in [
            json!({ "op": "accept", "root_id": "r", "work": 3, "bytes": 1 }),
            json!({ "op": "accept", "root_id": "r", "work": 3, "sha256": sha }),
            json!({ "op": "accept", "root_id": "r", "work": 3, "bytes": 1, "sha256": "A".repeat(64) }),
            json!({ "op": "output", "root_id": "r", "work": 0 }),
            json!({ "op": "output", "work": 3 }),
            json!({ "op": "output", "root_id": "r", "work": 3, "offset": -1 }),
        ] {
            assert!(parse_retained(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn base64_round_trips_every_tail_length() {
        for len in 0..9 {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 37 + 200) as u8).collect();
            assert_eq!(unbase64(&base64(&bytes)).unwrap(), bytes);
        }
        assert_eq!(base64(b"hi\n"), "aGkK");
        assert!(unbase64("a").is_none());
        for corrupt in ["aG==aGkK", "aGl=", "aH==", "!!!!"] {
            assert!(unbase64(corrupt).is_none(), "{corrupt}");
        }
    }
}
