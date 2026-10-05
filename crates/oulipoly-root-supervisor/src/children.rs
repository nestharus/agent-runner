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
//!   parent inputs open) commits before `accepted` is written. Nothing is
//!   retried; a refusal records and starts nothing.
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
//!   before the child's tagged turn end, if any) and the end. None of it is
//!   delivered to any harness as a new input.

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
use crate::native::{BashAuthority, OpenCodeSetup, OpenCodeSetupError, provision_opencode};
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

Bounds: read only. Do not edit, create, move or delete files; do not run tests, builds, installers or package managers; do not use root, sudo or services; make no network requests; do not read private credential or session stores (for example ~/.codex*, ~/.claude*, auth or token files); do not start agents, model CLIs or dispatchers of any kind. You cannot have children. Your shell, where you have one, is the parent's attributed shell and has no technical write barrier: these bounds are yours to keep.

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
    /// Absolute directory under which each OpenCode child's launch
    /// directory (`c<position>`) is made at its admission.
    pub launch_base: String,
}

/// How a child on one route is launched.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "harness", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ChildRoute {
    /// A native OpenCode host provisioned per child (see [`crate::native`]):
    /// these are the setup inputs less the launch directory.
    Opencode {
        deps: String,
        agent_bash_tool: String,
        agent_bash_bin: String,
        #[serde(default)]
        bash_allow: Vec<String>,
        #[serde(default)]
        bash_authority: Option<BashAuthority>,
        model: String,
        provider: serde_json::Map<String, Value>,
        /// A private access-only OpenCode `auth.json` copied into each
        /// child's launch; absent for a credential-free route.
        #[serde(default)]
        auth: Option<String>,
    },
    /// A fixed launch (the trusted intent's own argv, like a harness spec).
    Fixed {
        argv: Vec<String>,
        #[serde(default)]
        endpoint: Endpoint,
    },
}

impl ChildRoute {
    fn endpoint(&self) -> Endpoint {
        match self {
            Self::Opencode { .. } => Endpoint::UnixSocket,
            Self::Fixed { endpoint, .. } => *endpoint,
        }
    }

    /// The OpenCode setup for a child launch in `dir`.
    pub fn opencode_setup(&self, dir: String) -> Option<OpenCodeSetup> {
        match self {
            Self::Opencode {
                deps,
                agent_bash_tool,
                agent_bash_bin,
                bash_allow,
                bash_authority,
                model,
                provider,
                auth,
            } => Some(OpenCodeSetup {
                dir,
                deps: deps.clone(),
                agent_bash_tool: agent_bash_tool.clone(),
                agent_bash_bin: agent_bash_bin.clone(),
                bash_allow: bash_allow.clone(),
                bash_authority: *bash_authority,
                model: Some(model.clone()),
                provider: Some(provider.clone()),
                auth: auth.clone(),
                explore: None,
            }),
            Self::Fixed { .. } => None,
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
                ChildRoute::Opencode { .. } => {
                    let setup = route
                        .opencode_setup(format!("{}/check", self.launch_base))
                        .expect("opencode route");
                    crate::native::Policy::of(&setup)
                        .map_err(|reason| format!("children: route {name}: {reason}"))?;
                    if !setup
                        .model
                        .as_deref()
                        .is_some_and(|model| model.contains('/'))
                    {
                        return Err(format!(
                            "children: route {name}: model must be provider/model"
                        ));
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
    live: Vec<Tracked>,
    /// No further admission: the root is closing, stopping or cancelled.
    closed: Option<&'static str>,
    /// Live Bash runs requested from a child's namespace, by work.
    runs: BTreeMap<i64, (usize, Arc<Root>)>,
    finished: Vec<Value>,
    refused: u64,
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
        custody: Arc<Mutex<Custody>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            policy,
            first_child,
            custody,
            state: Mutex::new(State {
                starts,
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

    pub(crate) fn remove_run(&self, work: i64) {
        self.state.lock().expect("children").runs.remove(&work);
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

    fn finish(&self, position: usize, summary: Value, unknown: bool) {
        let mut state = self.state.lock().expect("children");
        state.live.retain(|child| child.position != position);
        if unknown {
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
            "live": state.live.len(),
            "children": state.finished,
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
                    json!({ "event": event, "reason": report["reason"], "not_started": event == "launch-failed" }),
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
    let argv = launch_argv(ctx, position, &route);
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
        Err((reason, not_started)) => {
            link.observe(&json!({
                "event": if not_started { "launch-failed" } else { "launch-unknown" },
                "reason": reason,
            }));
            let _ = ctx.tx.send(Event::Report(json!({
                "event": "child-setup-failed",
                "harness": id,
                "reason": reason,
                "setup_effects": if not_started { "none" } else { "possible" },
                "process_started": false,
            })));
            HarnessRecord::empty(&id)
        }
    };
    let (result, outcome, end_unknown) = link.result(&record);
    let _ = ctx.store.lock().expect("store lock").resolve_child(
        position,
        &match link.stopped() {
            Some(reason) if outcome == "stopped" => format!("stopped:{reason}"),
            _ => outcome.to_owned(),
        },
    );
    // Its Bash ends with it (normally already ended: tools are sync).
    ctx.registry.kill_runs_of(position);
    link.send(&result);
    watcher.0.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = stream.shutdown(std::net::Shutdown::Both);
    if let Some(handle) = watcher.1 {
        let _ = handle.join();
    }
    let mut summary = json!({
        "id": id,
        "position": position,
        "route": request.route,
        "parent": who.harness,
        "parent_work": who.harness_work,
        "outcome": outcome,
        "stopped": link.stopped(),
        "answered": result["answer"].is_string(),
        "end": result["end"],
        "record": record.to_json(),
    });
    let mut owner = result;
    owner["event"] = json!("child-result");
    owner["harness"] = json!(id);
    owner["child"] = link.marker();
    let _ = ctx.tx.send(Event::Report(owner));
    summary["requester"] = json!(if link.sink.lock().expect("child sink").connected() {
        "delivered"
    } else {
        "gone"
    });
    ctx.registry.finish(position, summary, end_unknown);
    crate::bash::leave_child(ctx.gate, ctx.tx);
}

struct Admitted {
    position: usize,
    id: String,
    message: oulipoly_acp::OutboundMessage,
    route: ChildRoute,
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
    let live = u32::try_from(state.live.len()).unwrap_or(u32::MAX);
    if live >= policy.max_concurrent {
        return Err(format!(
            "budget-concurrent: {live} of {}",
            policy.max_concurrent
        ));
    }
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
        "input_attribution": match who.open.len() {
            0 => "no-open-input",
            1 => "single-open-input",
            _ => "ambiguous-open-inputs",
        },
        "starts": { "used": state.starts, "max": policy.max_starts },
        "concurrent": { "live": state.live.len(), "max": policy.max_concurrent },
        "meaning": "durably admitted; not a start",
    });
    drop(state);
    drop(custody);
    Ok(Admitted {
        position,
        id,
        message,
        route: route.clone(),
        accepted,
    })
}

/// The child's harness argv: a fixed one, or a fresh OpenCode launch
/// provisioned now. `Err((reason, not_started))`: nothing was launched;
/// `not_started` false means setup effects may remain (retired with the
/// run), still with no process started.
fn launch_argv(
    ctx: &Context<'_>,
    position: usize,
    route: &ChildRoute,
) -> Result<Vec<String>, (String, bool)> {
    match route {
        ChildRoute::Fixed { argv, .. } => Ok(argv.clone()),
        ChildRoute::Opencode { .. } => {
            let policy = ctx
                .registry
                .policy
                .as_ref()
                .expect("admitted under a policy");
            let base = std::path::Path::new(&policy.launch_base);
            let identity = ctx.slot.workload.identity.as_ref();
            make_base(base, identity).map_err(|reason| (reason, true))?;
            // Each child's launch is fresh: an existing directory (an
            // earlier owner's) is refused by setup, never reused.
            let dir = base.join(format!("c{position}"));
            let setup = route
                .opencode_setup(dir.to_string_lossy().into_owned())
                .expect("opencode route");
            match provision_opencode(&setup, identity) {
                Ok(launch) => Ok(launch.argv),
                Err(OpenCodeSetupError::InputInvalid(reason)) => {
                    Err((format!("setup-refused: {reason}"), true))
                }
                Err(OpenCodeSetupError::ConstructionFailed(reason)) => {
                    Err((format!("setup-failed: {reason}"), true))
                }
            }
        }
    }
}

/// The children's launch base: the caller's, readable by the work
/// identity's group, made once.
fn make_base(
    base: &std::path::Path,
    identity: Option<&crate::workload::Identity>,
) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(base) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(format!("children launch base: {error}")),
    }
    if let Some(identity) = identity {
        std::os::unix::fs::lchown(base, None, Some(identity.gid))
            .and_then(|()| std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o750)))
            .map_err(|error| format!("children launch base: {error}"))?;
    }
    Ok(())
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
            children: Some(policy(&dir.0, starts)),
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
        let registry = Registry::new(Some(policy(&dir.0, starts)), 1, 0, Arc::clone(&custody));
        let views: Views = Arc::new(Mutex::new(vec![View {
            id: "p".into(),
            session: Some("s".into()),
            works: vec![(parent, PidNs { dev: 0, ino: 0 })],
            open: vec![OpenInput {
                index: 0,
                message_id: Some("msg-1".into()),
            }],
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
}
