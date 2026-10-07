//! Registered children: an intent harness (the parent) asks this owner,
//! through the root's ingress, for one child harness on a route the
//! intent's [`ChildPolicy`] allows, and receives that child's outcome on
//! the same connection. See the crate docs (Children) for the contract.
//!
//! * **Admission** (one JSON request line on `bash.sock`:
//!   `{"v":1,"op":"child","route":R,"prompt":P}`) is attributed exactly
//!   like a Bash request (the peer must be inside the PID namespace of one
//!   live harness work of this owner), then, under the custody lock:
//!   refused unless the run is not stopping or closing, the intent has a
//!   child policy naming `R`, the requester is an intent harness (depth 1:
//!   a child never has children), the start and concurrency budgets have
//!   room, and the exact parent work attributed before the request was read
//!   is **still current** at commit. Then the lineage (child harness row,
//!   its one message, parent position and work, route, requester and the
//!   parent inputs open **as attributed when the request arrived**: a
//!   snapshot, not rechecked at commit) commits before `accepted` is
//!   written. "Current" means the parent's whole work (its namespace) is
//!   still in this owner's live view, not that the parent's agent leader is
//!   alive: a parent whose leader died while a descendant keeps its work
//!   and connection can still ask, within the budget, until its work's
//!   receipt, its requester's loss, a stop or the deadline. Nothing is
//!   retried; a refusal records and starts nothing.
//! * **Prepared slots.** On a [`ChildRoute::Prepared`] route (a registered
//!   provider prepared by the trusted entry before this owner started),
//!   admission also takes the route's next unused slot: the k-th admission
//!   on the route, every generation counted from the durable lineage, takes
//!   slot k and no other admission ever does. With the route's slots used,
//!   it refuses (`route-slots-used`), recording and starting nothing. This
//!   owner runs only the slot's prepared argv; it prepares nothing.
//! * **Budget.** `max_concurrent` charges every admitted child until its
//!   end and its attributed Bash runs' ends are positively observed. A
//!   child whose launch or end is unknown (possible start, failed waiter,
//!   left to a successor), or one of whose Bash runs ended unknown, stays
//!   charged for the rest of this owner's life: it may still be running,
//!   so it can exhaust the root's capacity rather than let more start.
//! * **Life.** The child is driven like any harness, with one attempt and
//!   one closure (no relaunch or replay), its prompt prefixed with
//!   [`CHILD_BRIEF`]. After its tagged turn end its harness is stopped
//!   (namespace kill), and its Bash runs with it. It is stopped as well,
//!   with the reason recorded, when its requester's connection ends, its
//!   parent work ends (crash, close stop, relaunch), the root is closed or
//!   cancelled. A stop request is not an end: the end is its waiter's
//!   report, with the namespace's drain reported separately.
//! * **Result.** Stages go to the requester as they happen; the last line
//!   is `result` with the outcome (`answered`, `no-answer`,
//!   `launch-failed`, `launch-unknown`, `stopped`,
//!   `ended-without-turn-end`), the answer text (the last linked agent text
//!   before the child's tagged turn end, if any), the end, and `lifecycle`:
//!   whether its end was observed, how many of its Bash runs were still
//!   open (their kill requested, their drain not yet seen) and whether its
//!   budget is still charged. `answered` is a content outcome only: it
//!   does not say the child ended, its Bash drained, or that the parent
//!   consumed anything. The child's slot is held after `result` until its
//!   Bash runs end. None of it is delivered to any harness as a new input.

use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::bash::{Attributed, Gate, Sink, View, Views};
use crate::custody::{Root, RootSlot};
use crate::live::Custody;
use crate::store::{ChildAdmission, DurableHarness, DurableMessage, Store};
use crate::{Endpoint, Event, HarnessRecord};

/// Ceilings of this first child capability: total child starts and
/// children alive at once, per root (the intent may ask for fewer).
pub const MAX_STARTS: u32 = 4;
pub const MAX_CONCURRENT: u32 = 2;

/// Prefixed by the owner to every child's prompt: who the child is and
/// what it may do. The parent's question follows after a blank line.
pub const CHILD_BRIEF: &str = "You are a registered read-only orientation explorer, a child session started for one parent agent inside its root. Perform this exploration yourself. The parent agent that asked the question below and ROOT, the campaign caller, are both outside your session: you answer the parent; you do not act for either of them, judge or validate their work, or take their seat.

Purpose: find where things are and how they are wired together (files, modules, call paths, configuration, history), so that the parent can then look into the areas that matter itself. Answer concisely with concrete paths, names and relationships, and say what you could not establish.

Bounds: read only. Do not edit, create, move or delete files; do not run tests, builds, installers or package managers; do not use root, sudo or services; make no network requests; do not read private credential or session stores (for example ~/.codex*, ~/.claude*, auth or token files); do not start agents, model CLIs or dispatchers of any kind. You cannot have children. Your shell, where you have one, runs through this root's Bash ingress attributed to you (this child, under the parent's lineage) and has no technical write barrier: these bounds are yours to keep.

The parent's question:";

/// What an intent allows its harnesses to ask for.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildPolicy {
    /// Route name to how a child on it is launched. Site-fixed: the
    /// requester only names one.
    pub routes: BTreeMap<String, ChildRoute>,
    /// Child starts over the root's life, every generation counted.
    pub max_starts: u32,
    /// Children admitted and not yet finished, at once.
    pub max_concurrent: u32,
    /// Absolute child launch base retained in the root intent.
    pub launch_base: String,
}

/// How a child on one route is launched.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "harness", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ChildRoute {
    /// A fixed launch (the trusted intent's own argv, like a harness spec).
    Fixed {
        argv: Vec<String>,
        #[serde(default)]
        endpoint: Endpoint,
    },
    /// A registered provider's resident harness, prepared by the trusted
    /// entry before this owner started: one slot per possible admission on
    /// the route, each with its own fresh provider data root. The k-th
    /// admission on the route (every generation counted) takes slot k; a
    /// slot is never taken twice, and a route whose slots are used up
    /// refuses. An unused slot is setup, not a start or an admission.
    Prepared {
        /// The provider's declared id, a label only.
        provider: String,
        slots: Vec<PreparedSlot>,
        #[serde(default)]
        endpoint: Endpoint,
    },
}

/// One prepared launch of a [`ChildRoute::Prepared`] route.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSlot {
    /// The resident serve argv the provider prepared.
    pub argv: Vec<String>,
    /// The provider data root it was prepared in, for audit; this owner
    /// neither reads nor makes it.
    pub data_root: String,
}

impl ChildRoute {
    fn endpoint(&self) -> Endpoint {
        match self {
            Self::Fixed { endpoint, .. } | Self::Prepared { endpoint, .. } => *endpoint,
        }
    }
}

impl ChildPolicy {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.routes.is_empty() {
            return Err("children: routes names nothing".to_owned());
        }
        if !(1..=MAX_STARTS).contains(&self.max_starts) {
            return Err(format!("children: max_starts must be 1..={MAX_STARTS}"));
        }
        if !(1..=MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Err(format!(
                "children: max_concurrent must be 1..={MAX_CONCURRENT}"
            ));
        }
        if !self.launch_base.starts_with('/') || self.launch_base.contains('\0') {
            return Err("children: launch_base must be absolute".to_owned());
        }
        let mut data_roots = std::collections::BTreeSet::new();
        for (name, route) in &self.routes {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            {
                return Err(format!("children: route name {name:?}"));
            }
            match route {
                ChildRoute::Fixed { argv, .. } if argv.is_empty() => {
                    return Err(format!("children: route {name}: argv is empty"));
                }
                ChildRoute::Fixed { .. } => {}
                ChildRoute::Prepared { slots, .. } => {
                    if slots.is_empty() || slots.len() > self.max_starts as usize {
                        return Err(format!(
                            "children: route {name}: slots must be 1..=max_starts"
                        ));
                    }
                    for slot in slots {
                        if slot.argv.is_empty() {
                            return Err(format!("children: route {name}: a slot argv is empty"));
                        }
                        // Each slot's state is its own: no two share a root.
                        if !slot.data_root.starts_with('/')
                            || !data_roots.insert(slot.data_root.as_str())
                        {
                            return Err(format!(
                                "children: route {name}: slot data roots must be absolute and distinct"
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

/// One admitted child the registry still tracks.
struct Tracked {
    position: usize,
    parent_work: i64,
    /// Why it was stopped, if it was.
    stopped: Option<&'static str>,
    /// Its harness work once launched, for a stop.
    work: Option<(Arc<Root>, i64)>,
}

#[derive(Default)]
struct State {
    /// Admissions over the root's life (every generation).
    starts: u32,
    /// The same, by route: a prepared route's next slot.
    route_starts: BTreeMap<String, u32>,
    live: Vec<Tracked>,
    /// No further admission: the root is closing, stopping or cancelled.
    closed: Option<&'static str>,
    /// Live Bash runs requested from a child's namespace, by work.
    runs: BTreeMap<i64, (usize, Arc<Root>)>,
    /// Children whose Bash run ended unknown (position), counted once.
    runs_unknown: Vec<usize>,
    finished: Vec<Value>,
    refused: u64,
    /// Children whose end (or a Bash run's end) is unknown: still charged
    /// against `max_concurrent` (they may still be running).
    unknown: u64,
}

/// This owner's children: policy, budget, lineage and stops.
pub(crate) struct Registry {
    policy: Option<ChildPolicy>,
    /// Positions from here on are children (intent harnesses come first).
    pub(crate) first_child: usize,
    custody: Arc<Mutex<Custody>>,
    state: Mutex<State>,
}

impl Registry {
    pub(crate) fn new(
        policy: Option<ChildPolicy>,
        first_child: usize,
        starts: u32,
        route_starts: BTreeMap<String, u32>,
        custody: Arc<Mutex<Custody>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            policy,
            first_child,
            custody,
            state: Mutex::new(State {
                starts,
                route_starts,
                ..State::default()
            }),
        })
    }

    pub(crate) fn is_child(&self, position: usize) -> bool {
        position >= self.first_child
    }

    /// Whether a Bash request from `position` must be refused: it is a
    /// child that was stopped (call under the custody lock).
    pub(crate) fn refuses_bash(&self, position: usize) -> Option<&'static str> {
        if !self.is_child(position) {
            return None;
        }
        let state = self.state.lock().expect("children");
        match state.live.iter().find(|child| child.position == position) {
            Some(child) => child.stopped,
            None => Some("child-finished"),
        }
    }

    /// Notes a launched Bash run of a child (call under the custody lock,
    /// after its work is registered there), so a stop reaches it.
    pub(crate) fn add_run(&self, position: usize, root: &Arc<Root>, work: i64) {
        if self.is_child(position) {
            self.state
                .lock()
                .expect("children")
                .runs
                .insert(work, (position, Arc::clone(root)));
        }
    }

    /// A child's Bash run ended (`ended`: its waiter reported the end) or
    /// its end became unknown (not `ended`).
    pub(crate) fn remove_run(&self, work: i64, ended: bool) {
        let mut state = self.state.lock().expect("children");
        if let Some((position, _)) = state.runs.remove(&work)
            && !ended
            && !state.runs_unknown.contains(&position)
        {
            state.runs_unknown.push(position);
        }
    }

    /// A child's Bash run whose launch is unknown (it may run, untracked).
    pub(crate) fn note_run_unknown(&self, position: usize) {
        let mut state = self.state.lock().expect("children");
        if self.is_child(position) && !state.runs_unknown.contains(&position) {
            state.runs_unknown.push(position);
        }
    }

    /// The child's Bash runs not yet ended, and whether one ended unknown.
    fn runs_of(&self, position: usize) -> (usize, bool) {
        let state = self.state.lock().expect("children");
        (
            state
                .runs
                .values()
                .filter(|(owner, _)| *owner == position)
                .count(),
            state.runs_unknown.contains(&position),
        )
    }

    /// Stops one child and its Bash runs; returns how many kill requests
    /// were delivered. A stop under lost authority kills nothing.
    pub(crate) fn stop(&self, position: usize, reason: &'static str) -> usize {
        let custody = self.custody.lock().expect("custody lock");
        let detach = custody.reason() == Some("authority-lost");
        self.stop_locked(&|child| child.position == position, reason, detach)
    }

    /// Stops every child whose parent is exactly `parent_work`.
    pub(crate) fn stop_children_of(&self, parent_work: i64, reason: &'static str) -> usize {
        let custody = self.custody.lock().expect("custody lock");
        let detach = custody.reason() == Some("authority-lost");
        self.stop_locked(&|child| child.parent_work == parent_work, reason, detach)
    }

    /// Refuses further admissions and stops every live child.
    pub(crate) fn stop_all(&self, reason: &'static str) -> usize {
        let custody = self.custody.lock().expect("custody lock");
        let detach = custody.reason() == Some("authority-lost");
        self.state
            .lock()
            .expect("children")
            .closed
            .get_or_insert(reason);
        self.stop_locked(&|_| true, reason, detach)
    }

    fn stop_locked(
        &self,
        which: &dyn Fn(&Tracked) -> bool,
        reason: &'static str,
        detach: bool,
    ) -> usize {
        let mut targets = Vec::new();
        {
            let mut state = self.state.lock().expect("children");
            let mut positions = Vec::new();
            for child in state.live.iter_mut().filter(|child| which(child)) {
                if child.stopped.is_none() {
                    child.stopped = Some(reason);
                }
                positions.push(child.position);
                if let Some((root, work)) = &child.work {
                    targets.push((Arc::clone(root), *work));
                }
            }
            for (work, (position, root)) in &state.runs {
                if positions.contains(position) {
                    targets.push((Arc::clone(root), *work));
                }
            }
        }
        if detach {
            return 0;
        }
        targets
            .iter()
            .filter(|(root, work)| root.kill(*work))
            .count()
    }

    /// Kills a child's live Bash runs without marking the child stopped
    /// (its own end is what ends them). Returns delivered kill requests.
    pub(crate) fn kill_runs_of(&self, position: usize) -> usize {
        let custody = self.custody.lock().expect("custody lock");
        if custody.reason() == Some("authority-lost") {
            return 0;
        }
        let targets: Vec<(Arc<Root>, i64)> = self
            .state
            .lock()
            .expect("children")
            .runs
            .iter()
            .filter(|(_, (owner, _))| *owner == position)
            .map(|(work, (_, root))| (Arc::clone(root), *work))
            .collect();
        targets
            .iter()
            .filter(|(root, work)| root.kill(*work))
            .count()
    }

    fn stopped(&self, position: usize) -> Option<&'static str> {
        let state = self.state.lock().expect("children");
        state
            .live
            .iter()
            .find(|child| child.position == position)
            .and_then(|child| child.stopped)
    }

    /// Records the child's launched work (call under the custody lock).
    fn set_work(&self, position: usize, root: &Arc<Root>, work: i64) {
        let mut state = self.state.lock().expect("children");
        if let Some(child) = state
            .live
            .iter_mut()
            .find(|child| child.position == position)
        {
            child.work = Some((Arc::clone(root), work));
        }
    }

    fn finish(&self, position: usize, mut summary: Value, unknown: bool) {
        let mut state = self.state.lock().expect("children");
        // Decide the charge in the same critical section as release, not
        // from a runs_of snapshot taken before a concurrent notification.
        let run_unknown = state.runs_unknown.contains(&position);
        summary["bash_run_end_unknown"] = json!(run_unknown);
        state.live.retain(|child| child.position != position);
        if unknown || run_unknown {
            state.unknown += 1;
        }
        state.finished.push(summary);
    }

    fn refused(&self) {
        self.state.lock().expect("children").refused += 1;
    }

    /// A child recovered from an earlier owner whose end is unknown.
    pub(crate) fn note_recovered(&self, summary: Value, unknown: bool) {
        let mut state = self.state.lock().expect("children");
        if unknown {
            state.unknown += 1;
        }
        state.finished.push(summary);
    }

    pub(crate) fn ends_unproven(&self) -> bool {
        let state = self.state.lock().expect("children");
        state.unknown > 0 || !state.live.is_empty()
    }

    /// For the terminal report.
    pub(crate) fn summary(&self) -> Value {
        let state = self.state.lock().expect("children");
        json!({
            "enabled": self.policy.is_some(),
            "routes": self.policy.as_ref().map(|policy| policy.routes.keys().collect::<Vec<_>>()),
            "max_starts": self.policy.as_ref().map(|policy| policy.max_starts),
            "max_concurrent": self.policy.as_ref().map(|policy| policy.max_concurrent),
            "starts": state.starts,
            "refused": state.refused,
            "end_unknown": state.unknown,
            "end_unknown_meaning": "charged against max_concurrent for the rest of this owner: possibly still running",
            "live": state.live.len(),
            "children": state.finished,
            "children_scope": "this owner generation's finished admissions and recovered open children only; `starts` counts every start over the root's life; an empty list is not 'no children'",
        })
    }
}

/// A parsed child request.
pub(crate) struct ChildRequest {
    pub(crate) route: String,
    pub(crate) prompt: String,
}

/// `{"v":1,"op":"child","route":R,"prompt":P}`, nothing else.
pub(crate) fn parse_request(value: &Value) -> Result<ChildRequest, String> {
    let fields = value.as_object().ok_or("malformed")?;
    if fields
        .keys()
        .any(|key| !matches!(key.as_str(), "v" | "op" | "route" | "prompt"))
    {
        return Err("unknown-field".to_owned());
    }
    let route = value["route"]
        .as_str()
        .filter(|route| !route.is_empty())
        .ok_or("bad-route")?;
    let prompt = value["prompt"]
        .as_str()
        .filter(|prompt| !prompt.trim().is_empty() && !prompt.contains('\0'))
        .ok_or("bad-prompt")?;
    Ok(ChildRequest {
        route: route.to_owned(),
        prompt: prompt.to_owned(),
    })
}

/// What the ingress lends one child request.
pub(crate) struct Context<'a> {
    pub(crate) root_id: &'a str,
    pub(crate) registry: &'a Arc<Registry>,
    pub(crate) slot: &'a Arc<RootSlot>,
    pub(crate) custody: &'a Arc<Mutex<Custody>>,
    pub(crate) store: &'a Arc<Mutex<Store>>,
    pub(crate) views: &'a Views,
    pub(crate) gate: &'a Arc<(Mutex<Gate>, std::sync::Condvar)>,
    pub(crate) tx: &'a Sender<Event>,
    pub(crate) cwd: &'a str,
}

/// The requester's connection and what the child showed it.
pub(crate) struct ChildLink {
    pub(crate) position: usize,
    pub(crate) id: String,
    pub(crate) parent: String,
    pub(crate) route: String,
    registry: Arc<Registry>,
    sink: Mutex<Sink>,
    seen: Mutex<Seen>,
}

#[derive(Default)]
struct Seen {
    acks: Vec<String>,
    /// Agent texts linked to the child's input, in order.
    linked: Vec<String>,
    turn_end: Option<Value>,
    /// The answer: the last non-empty linked text before the turn end.
    answer: Option<String>,
    end: Option<Value>,
    launch: Option<Value>,
}

impl ChildLink {
    /// The marker every report of this child carries.
    pub(crate) fn marker(&self) -> Value {
        json!({ "parent": self.parent, "route": self.route, "position": self.position })
    }

    /// Why the child was stopped, if it was.
    pub(crate) fn stopped(&self) -> Option<&'static str> {
        self.registry.stopped(self.position)
    }

    /// Records the launched work for stops (call under the custody lock).
    pub(crate) fn set_work(&self, root: &Arc<Root>, work: i64) {
        self.registry.set_work(self.position, root, work);
    }

    fn send(&self, value: &Value) {
        self.sink.lock().expect("child sink").send(value);
    }

    /// Takes one of the child worker's reports: keeps what the result
    /// needs and passes lifecycle stages to the requester.
    pub(crate) fn observe(&self, report: &Value) {
        let event = report["event"].as_str().unwrap_or_default();
        let mut seen = self.seen.lock().expect("child seen");
        let stage = match event {
            "launched" => {
                Some(json!({ "event": "started", "work": report["work"], "pid": report["pid"] }))
            }
            "launch-failed" | "launch-unknown" => {
                seen.launch = Some(report.clone());
                Some(
                    json!({ "event": event, "reason": report["reason"], "not_started": report.get("not_started").and_then(Value::as_bool), "setup_effects": report.get("setup_effects") }),
                )
            }
            "ack" if report["index"] == 0 => {
                if let Some(id) = report["message_id"].as_str() {
                    seen.acks.push(id.to_owned());
                }
                Some(
                    json!({ "event": "ack", "message_id": report["message_id"], "meaning": "consumption, not processing" }),
                )
            }
            "agent-message" => {
                let linked = report["input"] == 0
                    && report["parent_message_id"]
                        .as_str()
                        .is_some_and(|parent| seen.acks.iter().any(|ack| ack == parent));
                if linked && seen.turn_end.is_none() {
                    seen.linked
                        .push(report["text"].as_str().unwrap_or_default().to_owned());
                }
                Some(json!({ "event": "agent-message", "linked": linked, "text": report["text"] }))
            }
            "turn-end" if report["input"] == 0 && seen.turn_end.is_none() => {
                seen.answer = seen
                    .linked
                    .iter()
                    .rev()
                    .find(|text| !text.trim().is_empty())
                    .cloned();
                seen.turn_end = Some(json!({
                    "stop_reason": report["stop_reason"],
                    "own_output": report["own_output"],
                    "message_id": report["message_id"],
                }));
                Some(
                    json!({ "event": "turn-end", "stop_reason": report["stop_reason"], "own_output": report["own_output"], "meaning": "child idle tagged at or after its input; not its exit" }),
                )
            }
            "close-stopping" => Some(
                json!({ "event": "stopping", "reason": "answered-turn-ended", "signalled": report["signalled"] }),
            ),
            "exited" => {
                let end = json!({ "event": "end", "status": report["status"], "observer": "work-pid1-wait", "work_pid1": report["work_pid1"], "namespace": report["namespace"] });
                seen.end = Some(end.clone());
                Some(end)
            }
            "wait-failed" => {
                let end = json!({ "event": "end-unknown", "reason": report["reason"] });
                seen.end = Some(end.clone());
                Some(end)
            }
            "notice" => Some(
                json!({ "event": "notice", "severity": report["severity"], "title": report["title"], "description": report["description"] }),
            ),
            "negotiation-failed" | "session-failed" | "endpoint-failed" | "rejected"
            | "invalid-response" | "outage" => Some(
                json!({ "event": event, "detail": report.get("label").or(report.get("reason")).cloned() }),
            ),
            _ => None,
        };
        drop(seen);
        if let Some(stage) = stage {
            self.send(&stage);
        }
    }

    /// The outcome for the parent, and whether its end is unknown.
    fn result(&self, record: &HarnessRecord) -> (Value, &'static str, bool) {
        let seen = self.seen.lock().expect("child seen");
        let stopped = self.stopped();
        let end_unknown = !record.wait_failures.is_empty()
            || record.detached > 0
            || seen
                .launch
                .as_ref()
                .is_some_and(|launch| launch["event"] == "launch-unknown");
        let outcome = match (&seen.launch, &seen.turn_end) {
            (Some(launch), _) if launch["event"] == "launch-unknown" => "launch-unknown",
            (Some(_), _) if record.launches == 0 => "launch-failed",
            (_, Some(turn)) if seen.answer.is_some() && turn["stop_reason"] == "end_turn" => {
                "answered"
            }
            (_, Some(_)) => "no-answer",
            (_, None) if stopped.is_some() => "stopped",
            (_, None) => "ended-without-turn-end",
        };
        let result = json!({
            "event": "result",
            "child": self.id,
            "route": self.route,
            "outcome": outcome,
            "answer": seen.answer,
            "turn_end": seen.turn_end,
            "stopped": stopped,
            "end": seen.end,
            "launch": seen.launch,
            "meaning": "answer is the child's last linked text before its tagged turn end; end is its waiter's report; neither is task correctness",
        });
        (result, outcome, end_unknown)
    }
}

/// Serves one attributed child request to its end. `stream` is the
/// requester's connection (its request line already read).
pub(crate) fn serve(
    ctx: &Context<'_>,
    who: &Attributed,
    requester: i32,
    request: ChildRequest,
    stream: UnixStream,
) {
    let mut sink = Sink::new(stream.try_clone().ok());
    let admitted = match admit(ctx, who, requester, &request) {
        Ok(admitted) => admitted,
        Err(reason) => {
            ctx.registry.refused();
            sink.send(&json!({ "event": "refused", "reason": reason }));
            let _ = ctx.tx.send(Event::Report(json!({
                "event": "child-refused",
                "reason": reason,
                "harness": who.harness,
                "harness_work": who.harness_work,
                "route": request.route,
                "requester_pid": requester,
            })));
            return;
        }
    };
    let Admitted {
        position,
        id,
        message,
        route,
        slot,
        accepted,
    } = admitted;
    sink.send(&accepted);
    let mut owner = accepted.clone();
    owner["event"] = json!("child-accepted");
    owner["harness"] = json!(who.harness);
    let _ = ctx.tx.send(Event::Report(owner));
    let link = Arc::new(ChildLink {
        position,
        id: id.clone(),
        parent: who.harness.clone(),
        route: request.route.clone(),
        registry: Arc::clone(ctx.registry),
        sink: Mutex::new(sink),
        seen: Mutex::new(Seen::default()),
    });
    // Requester loss stops the child: the requester sends nothing after
    // its request, so end of stream or an error is its going away.
    let watcher = {
        let registry = Arc::clone(ctx.registry);
        let tx = ctx.tx.clone();
        let reader = stream.try_clone();
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&done);
        let handle = reader.ok().map(|reader| {
            thread::spawn(move || {
                let mut byte = [0u8; 1];
                let gone = !matches!(BufReader::new(reader).read(&mut byte), Ok(1));
                if gone && !flag.load(std::sync::atomic::Ordering::SeqCst) {
                    let signalled = registry.stop(position, "requester-gone");
                    let _ = tx.send(Event::Report(json!({
                        "event": "child-stopping",
                        "position": position,
                        "reason": "requester-gone",
                        "signalled": signalled,
                    })));
                }
            })
        });
        (done, handle)
    };
    let argv = launch_argv(&route, slot);
    let record = match argv {
        Ok(argv) => {
            let _ = ctx
                .store
                .lock()
                .expect("store lock")
                .set_harness_argv(position, &argv);
            let harness = DurableHarness {
                id: id.clone(),
                argv,
                endpoint: route.endpoint(),
                session: None,
                messages: vec![DurableMessage {
                    message,
                    follow_up: false,
                    closures: 0,
                    stop: None,
                    ack: None,
                    attempts: 0,
                    prior_unknown: 0,
                    unknown_attempts: 0,
                    turn_ended: false,
                    completion_work: None,
                }],
                open_works: Vec::new(),
            };
            crate::harness::run_child(crate::harness::ChildAssignment {
                position,
                harness,
                cwd: ctx.cwd.to_owned(),
                custody: Arc::clone(ctx.custody),
                store: Arc::clone(ctx.store),
                slot: Arc::clone(ctx.slot),
                tx: ctx.tx.clone(),
                views: Arc::clone(ctx.views),
                children: Arc::clone(ctx.registry),
                link: Arc::clone(&link),
            })
        }
        Err((reason, setup_effects)) => {
            // No process was started: a positive no-start, whatever files
            // setup may have left (a separate fact, reported as such).
            link.observe(&json!({
                "event": "launch-failed",
                "reason": reason,
                "setup_effects": setup_effects,
                "not_started": true,
            }));
            let _ = ctx.tx.send(Event::Report(json!({
                "event": "child-setup-failed",
                "harness": id,
                "reason": reason,
                "setup_effects": setup_effects,
                "process_started": false,
            })));
            HarnessRecord::empty(&id)
        }
    };
    let (mut result, outcome, end_unknown) = link.result(&record);
    let recorded = ctx.store.lock().expect("store lock").resolve_child(
        position,
        &match link.stopped() {
            Some(reason) if outcome == "stopped" => format!("stopped:{reason}"),
            _ => outcome.to_owned(),
        },
    );
    // Its Bash ends with it (normally already ended: tools are sync).
    let bash_signalled = ctx.registry.kill_runs_of(position);
    let (open_runs, run_unknown) = ctx.registry.runs_of(position);
    result["lifecycle"] = json!({
        "end": if end_unknown { "unknown" } else if result["end"].is_null() { "no-process" } else { "observed" },
        "bash_runs_open": open_runs,
        "bash_kill_requests": bash_signalled,
        "bash_run_end_unknown": run_unknown,
        "budget": if end_unknown || run_unknown {
            "still charged (an end is unknown: it may be running)"
        } else if open_runs > 0 {
            "still charged until its Bash runs end"
        } else {
            "still charged (release pending)"
        },
        "outcome_recorded": recorded.is_ok(),
        "meaning": "outcome/answer are content; this says what is known of its end and drain. Neither is parent consumption.",
    });
    link.send(&result);
    watcher.0.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if let Some(handle) = watcher.1 {
        let _ = handle.join();
    }
    // Its slot stays charged until its Bash runs end (their waiters'
    // reports), however long: no bound here; the root's stop and deadline
    // are the backstop.
    let mut drained_open = open_runs;
    while drained_open > 0 {
        thread::sleep(std::time::Duration::from_millis(20));
        drained_open = ctx.registry.runs_of(position).0;
    }
    let run_unknown = ctx.registry.runs_of(position).1;
    let mut summary = json!({
        "id": id,
        "position": position,
        "route": request.route,
        "slot": slot,
        "parent": who.harness,
        "parent_work": who.harness_work,
        "outcome": outcome,
        "stopped": link.stopped(),
        "answered": result["answer"].is_string(),
        "end": result["end"],
        "end_unknown": end_unknown,
        "bash_runs_open_at_result": open_runs,
        "bash_run_end_unknown": run_unknown,
        "outcome_recorded": recorded.is_ok(),
        "record": record.to_json(),
    });
    let mut owner = result;
    owner["event"] = json!("child-result");
    owner["harness"] = json!(id);
    owner["child"] = link.marker();
    let _ = ctx.tx.send(Event::Report(owner));
    // "written": the local writes raised no error; not consumption.
    summary["requester"] = json!(if link.sink.lock().expect("child sink").connected() {
        "written"
    } else {
        "gone"
    });
    ctx.registry
        .finish(position, summary, end_unknown || run_unknown);
    crate::bash::leave_child(ctx.gate, ctx.tx);
}

struct Admitted {
    position: usize,
    id: String,
    message: oulipoly_acp::OutboundMessage,
    route: ChildRoute,
    /// The prepared slot this admission took, on a prepared route.
    slot: Option<usize>,
    accepted: Value,
}

fn admit(
    ctx: &Context<'_>,
    who: &Attributed,
    requester: i32,
    request: &ChildRequest,
) -> Result<Admitted, String> {
    let custody = ctx.custody.lock().expect("custody lock");
    if let Some(reason) = custody.reason() {
        return Err(format!("owner-stopping: {reason}"));
    }
    let registry = ctx.registry;
    let Some(policy) = &registry.policy else {
        return Err("children-not-enabled".to_owned());
    };
    let Some(route) = policy.routes.get(&request.route) else {
        return Err("route-not-allowed".to_owned());
    };
    if registry.is_child(who.position) {
        return Err("depth: a child cannot have children".to_owned());
    }
    let mut state = registry.state.lock().expect("children");
    if let Some(reason) = state.closed {
        return Err(format!("closing: {reason}"));
    }
    if state.starts >= policy.max_starts {
        return Err(format!(
            "budget-starts: {} of {}",
            state.starts, policy.max_starts
        ));
    }
    // Unknown ends stay charged: they may still be running.
    let charged = u32::try_from(state.live.len() as u64 + state.unknown).unwrap_or(u32::MAX);
    if charged >= policy.max_concurrent {
        return Err(format!(
            "budget-concurrent: {charged} of {} ({} unknown-end charged)",
            policy.max_concurrent, state.unknown
        ));
    }
    // A prepared route's slot: the next unused one, or a refusal.
    let route_used = state.route_starts.get(&request.route).copied().unwrap_or(0);
    let slot = match route {
        ChildRoute::Prepared { slots, .. } => {
            let index = route_used as usize;
            if index >= slots.len() {
                return Err(format!("route-slots-used: {route_used} of {}", slots.len()));
            }
            Some(index)
        }
        _ => None,
    };
    let mut views = ctx.views.lock().expect("views");
    // The parent attributed before the request was read must still be
    // the same live work now, at commit.
    let current = views
        .get(who.position)
        .is_some_and(|view| view.works.iter().any(|(work, _)| *work == who.harness_work));
    if !current {
        return Err("parent-not-current".to_owned());
    }
    if !crate::bash::enter_child(ctx.gate) {
        return Err("ingress-closed".to_owned());
    }
    let open: Vec<Value> = who
        .open
        .iter()
        .map(|input| json!({ "index": input.index, "message_id": input.message_id }))
        .collect();
    let inputs_open = Value::Array(open.clone()).to_string();
    let id = format!("child-{}", state.starts + 1);
    let text = format!("{CHILD_BRIEF}\n\n{}", request.prompt);
    let committed = ctx
        .store
        .lock()
        .expect("store lock")
        .admit_child(&ChildAdmission {
            id: &id,
            endpoint: route.endpoint(),
            parent: who.position,
            parent_work: who.harness_work,
            route: &request.route,
            requester_pid: requester,
            inputs_open: &inputs_open,
            text: &text,
        });
    let (position, message) = match committed {
        Ok(committed) => committed,
        Err(error) => {
            drop(views);
            drop(state);
            let label = error.label();
            let mut custody = custody;
            custody.stop(label);
            crate::bash::leave_child(ctx.gate, ctx.tx);
            return Err(label.to_owned());
        }
    };
    if views.len() <= position {
        views.resize_with(position + 1, View::default);
    }
    views[position] = View {
        id: id.clone(),
        ..View::default()
    };
    drop(views);
    state.starts += 1;
    *state.route_starts.entry(request.route.clone()).or_default() += 1;
    state.live.push(Tracked {
        position,
        parent_work: who.harness_work,
        stopped: None,
        work: None,
    });
    let accepted = json!({
        "event": "accepted",
        "durable": true,
        "child": id,
        "position": position,
        "route": request.route,
        "root_id": ctx.root_id,
        "parent": who.harness,
        "parent_work": who.harness_work,
        "parent_session": who.session,
        "requester_pid": requester,
        "inputs_open": open,
        "inputs_open_meaning": "snapshot when the request was attributed, not rechecked at commit",
        "parent_currentness": "the parent's whole work was in the live view at commit; not an observation of its agent leader",
        "input_attribution": match who.open.len() {
            0 => "no-open-input",
            1 => "single-open-input",
            _ => "ambiguous-open-inputs",
        },
        "starts": { "used": state.starts, "max": policy.max_starts },
        "concurrent": { "live": state.live.len(), "unknown_charged": state.unknown, "max": policy.max_concurrent },
        "meaning": "durably admitted; not a start",
    });
    let mut accepted = accepted;
    if let (
        Some(index),
        ChildRoute::Prepared {
            provider, slots, ..
        },
    ) = (slot, route)
    {
        accepted["slot"] = json!({
            "index": index,
            "of": slots.len(),
            "provider": provider,
            "data_root": slots[index].data_root,
            "meaning": "the route's next unused prepared slot, taken by this admission only; prepared before the owner started",
        });
    }
    drop(state);
    drop(custody);
    Ok(Admitted {
        position,
        id,
        message,
        route: route.clone(),
        slot,
        accepted,
    })
}

/// Select the fixed argv or the admission's already prepared slot.
/// Selection performs no setup writes and starts no process.
fn launch_argv(
    route: &ChildRoute,
    slot: Option<usize>,
) -> Result<Vec<String>, (String, &'static str)> {
    match route {
        ChildRoute::Fixed { argv, .. } => Ok(argv.clone()),
        ChildRoute::Prepared { slots, .. } => slot
            .and_then(|index| slots.get(index))
            .map(|slot| slot.argv.clone())
            .ok_or(("no prepared slot was taken".to_owned(), "none")),
    }
}

#[cfg(test)]
mod tests {
    //! CONFIGURED SEAMS: admission against a real fresh store and a test
    //! connection standing in for root PID 1; no harness runs here.

    use super::*;
    use crate::bash::OpenInput;
    use crate::custody::PidNs;
    use crate::transport::StopSignal;
    use crate::{HarnessSpec, Intent};
    use std::sync::mpsc::channel;

    struct Dir(std::path::PathBuf);

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn policy(dir: &std::path::Path, starts: u32) -> ChildPolicy {
        ChildPolicy {
            routes: BTreeMap::from([(
                "echo".to_owned(),
                ChildRoute::Fixed {
                    argv: vec!["peer".into()],
                    endpoint: Endpoint::Stdio,
                },
            )]),
            max_starts: starts,
            max_concurrent: 2,
            launch_base: dir.join("children").display().to_string(),
        }
    }

    struct Fixture {
        _dir: Dir,
        registry: Arc<Registry>,
        slot: Arc<RootSlot>,
        custody: Arc<Mutex<Custody>>,
        store: Arc<Mutex<Store>>,
        views: Views,
        gate: Arc<(Mutex<Gate>, std::sync::Condvar)>,
        tx: Sender<Event>,
        parent: i64,
        _far: std::os::fd::OwnedFd,
    }

    fn fixture(name: &str, starts: u32) -> Fixture {
        fixture_with(name, &|dir| policy(dir, starts))
    }

    fn fixture_with(name: &str, policy: &dyn Fn(&std::path::Path) -> ChildPolicy) -> Fixture {
        let dir = Dir(std::env::temp_dir().join(format!(
            "children-{name}-{}",
            crate::sys::random_hex().unwrap()
        )));
        std::fs::create_dir(&dir.0).unwrap();
        let intent = Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: "/".into(),
            harnesses: vec![HarnessSpec {
                id: "p".into(),
                argv: vec!["peer".into()],
                endpoint: Endpoint::Stdio,
                session: None,
                messages: vec!["m".into()],
            }],
            workload: crate::Workload::UnprivilegedUserns {},
            children: Some(policy(&dir.0)),
        };
        let mut claimed = Store::claim(&dir.0.join("store"), Some(&intent)).unwrap();
        claimed.store.begin_incarnation("t", "test").unwrap();
        let parent = claimed.store.begin_work(0, 1).unwrap();
        let stop = Arc::new(StopSignal::new().unwrap());
        let (root, far) = Root::seam(false, i32::try_from(std::process::id()).unwrap(), &stop);
        let slot = Arc::new(RootSlot::new(
            dir.0.join("store"),
            1,
            crate::workload::unprivileged_for_tests(&dir.0.join("store")),
            Arc::clone(&stop),
            Some(root),
        ));
        let custody = Arc::new(Mutex::new(Custody::new(stop)));
        let registry = Registry::new(
            Some(policy(&dir.0)),
            1,
            0,
            BTreeMap::new(),
            Arc::clone(&custody),
        );
        let views: Views = Arc::new(Mutex::new(vec![View {
            id: "p".into(),
            session: Some("s".into()),
            works: vec![(parent, PidNs { dev: 0, ino: 0 })],
            open: vec![OpenInput {
                index: 0,
                message_id: Some("msg-1".into()),
            }],
            ..View::default()
        }]));
        let (tx, _rx) = channel();
        Fixture {
            _dir: dir,
            registry,
            slot,
            custody,
            store: Arc::new(Mutex::new(claimed.store)),
            views,
            gate: Arc::default(),
            tx,
            parent,
            _far: far,
        }
    }

    fn ctx(f: &Fixture) -> Context<'_> {
        Context {
            root_id: "r",
            registry: &f.registry,
            slot: &f.slot,
            custody: &f.custody,
            store: &f.store,
            views: &f.views,
            gate: &f.gate,
            tx: &f.tx,
            cwd: "/",
        }
    }

    fn who(position: usize, work: i64) -> Attributed {
        Attributed {
            position,
            harness: "p".into(),
            harness_work: work,
            session: Some("s".into()),
            open: vec![OpenInput {
                index: 0,
                message_id: Some("msg-1".into()),
            }],
        }
    }

    fn request() -> ChildRequest {
        ChildRequest {
            route: "echo".into(),
            prompt: "where is X?".into(),
        }
    }

    fn children_rows(f: &Fixture) -> i64 {
        let conn =
            rusqlite::Connection::open(f._dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        conn.query_row("SELECT count(*) FROM child", [], |row| row.get(0))
            .unwrap()
    }

    /// A parent attributed before its request was read, whose work is no
    /// longer current at commit, is refused and nothing is recorded.
    #[test]
    fn stale_parent_at_commit_is_refused_and_records_nothing() {
        let f = fixture("stale", 4);
        f.views.lock().unwrap()[0].works.clear();
        let refused = admit(&ctx(&f), &who(0, f.parent), 1, &request())
            .err()
            .unwrap();
        assert_eq!(refused, "parent-not-current");
        assert_eq!(children_rows(&f), 0);
        assert_eq!(f.registry.state.lock().unwrap().starts, 0);
    }

    /// Lineage commits before admission is reported; a child position is
    /// refused as a requester (depth 1); the start budget counts every
    /// admission; a stopped registry (close) refuses.
    #[test]
    fn admission_records_lineage_and_refuses_depth_budget_and_closing() {
        let f = fixture("admit", 2);
        let admitted = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        assert_eq!(admitted.position, 1);
        assert_eq!(admitted.accepted["parent_work"], f.parent);
        assert_eq!(admitted.accepted["input_attribution"], "single-open-input");
        assert_eq!(children_rows(&f), 1);
        let conn =
            rusqlite::Connection::open(f._dir.0.join("store").join(crate::store::DB_FILE)).unwrap();
        let (parent_work, requester, inputs): (i64, i64, String) = conn
            .query_row(
                "SELECT parent_work, requester_pid, inputs_open FROM child",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!((parent_work, requester), (f.parent, 7));
        assert!(inputs.contains("msg-1"), "{inputs}");
        // The admitted child has a view (for its own Bash attribution).
        assert_eq!(f.views.lock().unwrap()[1].id, "child-1");
        // Depth: the child as a requester (give it a live work first).
        let child_work = f.store.lock().unwrap().begin_work(1, 1).unwrap();
        f.views.lock().unwrap()[1]
            .works
            .push((child_work, PidNs { dev: 1, ino: 1 }));
        let depth = admit(&ctx(&f), &who(1, child_work), 8, &request())
            .err()
            .unwrap();
        assert!(depth.starts_with("depth"), "{depth}");
        // Budget: the second start is allowed, the third refused.
        admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        let budget = admit(&ctx(&f), &who(0, f.parent), 7, &request())
            .err()
            .unwrap();
        assert!(budget.starts_with("budget-"), "{budget}");
        assert_eq!(children_rows(&f), 2);
        // Close: refused, whatever the budget.
        f.registry.stop_all("root-closing");
        f.registry.state.lock().unwrap().starts = 0;
        let closing = admit(&ctx(&f), &who(0, f.parent), 7, &request())
            .err()
            .unwrap();
        assert_eq!(closing, "closing: root-closing");
    }

    /// A prepared route of two slots beside a fixed one, four starts.
    fn prepared_policy(dir: &std::path::Path) -> ChildPolicy {
        let mut policy = policy(dir, 4);
        policy.routes.insert(
            "luna".to_owned(),
            ChildRoute::Prepared {
                provider: "fake".to_owned(),
                slots: (0..2)
                    .map(|index| PreparedSlot {
                        argv: vec![format!("serve-{index}")],
                        data_root: format!("/slots/{index}/provider"),
                    })
                    .collect(),
                endpoint: Endpoint::Stdio,
            },
        );
        policy
    }

    fn luna() -> ChildRequest {
        ChildRequest {
            route: "luna".into(),
            prompt: "where is X?".into(),
        }
    }

    /// Each admission on a prepared route takes the route's next slot once,
    /// whatever other routes do; with its slots used the route refuses
    /// (recording and starting nothing) while the start budget still has
    /// room; a later owner of the same store continues from the durable
    /// admissions and never takes a used slot again.
    #[test]
    fn prepared_slots_are_taken_once_in_order_across_owners() {
        let f = fixture_with("slots", &prepared_policy);
        let first = admit(&ctx(&f), &who(0, f.parent), 7, &luna()).unwrap();
        assert_eq!(first.slot, Some(0));
        assert_eq!(first.accepted["slot"]["index"], 0);
        assert_eq!(first.accepted["slot"]["of"], 2);
        assert_eq!(first.accepted["slot"]["data_root"], "/slots/0/provider");
        assert_eq!(
            launch_argv(&first.route, first.slot).unwrap(),
            vec!["serve-0".to_owned()]
        );
        f.registry.finish(first.position, json!({}), false);
        let fixed = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        assert_eq!(fixed.slot, None);
        assert!(fixed.accepted.get("slot").is_none());
        f.registry.finish(fixed.position, json!({}), false);
        let second = admit(&ctx(&f), &who(0, f.parent), 7, &luna()).unwrap();
        assert_eq!(second.slot, Some(1));
        assert_eq!(
            launch_argv(&second.route, second.slot).unwrap(),
            vec!["serve-1".to_owned()]
        );
        f.registry.finish(second.position, json!({}), false);
        let used = admit(&ctx(&f), &who(0, f.parent), 7, &luna())
            .err()
            .unwrap();
        assert_eq!(used, "route-slots-used: 2 of 2");
        assert_eq!(children_rows(&f), 3);
        assert_eq!(f.registry.state.lock().unwrap().starts, 3);
        // A taken slot never launches without its admission's index.
        assert!(launch_argv(&second.route, None).is_err());
        // A later owner of this store: the per-route count is durable.
        let Fixture { _dir, store, .. } = f;
        drop(store);
        let claimed = Store::claim(&_dir.0.join("store"), None).unwrap();
        assert_eq!(
            claimed.child_route_starts,
            BTreeMap::from([("echo".to_owned(), 1), ("luna".to_owned(), 2)])
        );
        assert_eq!(claimed.child_starts, 3);
    }

    /// Slots are finite (at most the start budget) and never share state.
    #[test]
    fn prepared_routes_are_bounded_and_their_data_roots_distinct() {
        let dir = std::path::Path::new("/r");
        prepared_policy(dir).validate().unwrap();
        let mut over = prepared_policy(dir);
        over.max_starts = 1;
        assert!(
            over.validate()
                .unwrap_err()
                .contains("slots must be 1..=max_starts")
        );
        let mut empty = prepared_policy(dir);
        if let Some(ChildRoute::Prepared { slots, .. }) = empty.routes.get_mut("luna") {
            slots.clear();
        }
        assert!(empty.validate().unwrap_err().contains("slots must be"));
        let mut shared = prepared_policy(dir);
        if let Some(ChildRoute::Prepared { slots, .. }) = shared.routes.get_mut("luna") {
            slots[1].data_root = slots[0].data_root.clone();
        }
        assert!(shared.validate().unwrap_err().contains("distinct"));
        let mut relative = prepared_policy(dir);
        if let Some(ChildRoute::Prepared { slots, .. }) = relative.routes.get_mut("luna") {
            slots[0].data_root = "slots/0".to_owned();
        }
        assert!(relative.validate().unwrap_err().contains("absolute"));
    }

    /// D2/D3: a child whose end is unknown (or one of whose Bash runs
    /// ended unknown) keeps its concurrency charge after it is finished;
    /// a known end releases it. Unknowns can exhaust the root's capacity.
    #[test]
    fn unknown_ends_stay_charged_against_concurrency() {
        let f = fixture("charged", 4);
        let first = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        let second = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        let full = admit(&ctx(&f), &who(0, f.parent), 7, &request())
            .err()
            .unwrap();
        assert_eq!(full, "budget-concurrent: 2 of 2 (0 unknown-end charged)");
        // A known end releases its slot.
        f.registry.finish(first.position, json!({}), false);
        let third = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        // A Bash run of `second` ends unknown: `second` stays charged after
        // it finishes, together with `third` still live.
        let (root, _far) = Root::seam(
            false,
            i32::try_from(std::process::id()).unwrap(),
            &f.slot.stop,
        );
        f.registry.add_run(second.position, &root, 99);
        assert_eq!(f.registry.runs_of(second.position), (1, false));
        f.registry.remove_run(99, false);
        assert_eq!(f.registry.runs_of(second.position), (0, true));
        f.registry.finish(second.position, json!({}), true);
        let refused = admit(&ctx(&f), &who(0, f.parent), 7, &request())
            .err()
            .unwrap();
        assert_eq!(refused, "budget-concurrent: 2 of 2 (1 unknown-end charged)");
        // With nothing live, one slot is free: the unknown still holds the
        // other, and the admission says so.
        f.registry.finish(third.position, json!({}), false);
        let fourth = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        assert_eq!(fourth.accepted["concurrent"]["unknown_charged"], 1);
        assert_eq!(fourth.accepted["concurrent"]["live"], 1);
        let summary = f.registry.summary();
        assert_eq!(summary["end_unknown"], 1);
        assert_eq!(summary["starts"], 4);
    }

    /// Actual ingress spawn-fault path, with child finish scheduled at
    /// custody release (the former late-notification window). A fake PID1
    /// reports a refusal without positive no-start; it is not a real spawn.
    #[test]
    fn spawn_unknown_is_published_before_child_finish_can_release() {
        let f = Arc::new(fixture("spawn-interleave", 4));
        let child = admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        admit(&ctx(&f), &who(0, f.parent), 7, &request()).unwrap();
        let work = f
            .store
            .lock()
            .unwrap()
            .begin_work(child.position, 1)
            .unwrap();
        f.views.lock().unwrap()[child.position]
            .works
            .push((work, PidNs { dev: 1, ino: 1 }));
        let ingress = crate::bash::Ingress::new(
            "r".into(),
            Arc::clone(&f.slot),
            Arc::clone(&f.custody),
            Arc::clone(&f.store),
            Arc::clone(&f.views),
            f.tx.clone(),
            Arc::clone(&f.registry),
            "/".into(),
        );
        // The child sampled no unknown before the fault; finish must not
        // trust that stale boolean. Interleave on the actual failure path.
        let sampled_unknown = f.registry.runs_of(child.position).1;
        assert!(!sampled_unknown);
        let finishing = Arc::clone(&f);
        *ingress.after_spawn_error_unlock.lock().unwrap() = Some(Box::new(move || {
            let custody = finishing.custody.lock().unwrap();
            finishing
                .registry
                .finish(child.position, json!({}), sampled_unknown);
            drop(custody);
            let refused = admit(&ctx(&finishing), &who(0, finishing.parent), 7, &request())
                .err()
                .unwrap();
            assert_eq!(refused, "budget-concurrent: 2 of 2 (1 unknown-end charged)");
        }));
        let replying = Arc::clone(&f);
        let replier = thread::spawn(move || {
            let (bytes, _) = crate::sys::recv(&replying._far).unwrap().unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            crate::sys::send(
                &replying._far,
                json!({
                    "req": request["req"], "event": "refused",
                    "reason": "after-possible-creation", "not_started": false,
                })
                .to_string()
                .as_bytes(),
                &[],
            )
            .unwrap();
        });
        ingress.run_fault_seam(&who(child.position, work), "/".into());
        replier.join().unwrap();
        assert_eq!(f.registry.summary()["end_unknown"], 1);
        assert_eq!(
            f.registry.summary()["children"][0]["bash_run_end_unknown"],
            true
        );
    }

    #[test]
    fn policy_ceilings_and_request_shape_are_enforced() {
        let dir = std::path::Path::new("/tmp/children-policy");
        let mut over = policy(dir, MAX_STARTS + 1);
        assert!(over.validate().unwrap_err().contains("max_starts"));
        over.max_starts = MAX_STARTS;
        over.max_concurrent = MAX_CONCURRENT + 1;
        assert!(over.validate().unwrap_err().contains("max_concurrent"));
        over.max_concurrent = MAX_CONCURRENT;
        over.validate().unwrap();
        for bad in [
            json!({ "v": 1, "op": "child", "route": "echo" }),
            json!({ "v": 1, "op": "child", "route": "", "prompt": "q" }),
            json!({ "v": 1, "op": "child", "route": "echo", "prompt": "  " }),
            json!({ "v": 1, "op": "child", "route": "echo", "prompt": "q", "argv": ["sh"] }),
        ] {
            assert!(parse_request(&bad).is_err(), "{bad}");
        }
        assert_eq!(
            parse_request(&json!({ "v": 1, "op": "child", "route": "echo", "prompt": "q" }))
                .unwrap()
                .route,
            "echo"
        );
    }
    #[test]
    fn embedded_child_route_is_unknown_and_fixed_remains_supported() {
        let old = json!({"harness": "opencode", "deps": "/d", "model": "p/m"});
        assert!(
            serde_json::from_value::<ChildRoute>(old)
                .unwrap_err()
                .to_string()
                .contains("unknown variant")
        );
        let fixed: ChildRoute = serde_json::from_value(json!({
            "harness": "fixed", "argv": ["/bin/true"], "endpoint": "stdio"
        }))
        .unwrap();
        assert_eq!(launch_argv(&fixed, None).unwrap(), ["/bin/true"]);
    }
}
