//! `native-root`: one Linux ACP v2 root with a registered external provider.
//!
//! `--request <file>` requires `provider`, fresh `store` and `launch_dir`,
//! `cwd`, whole root `env`, messages, delivery/outage caps and `workload`.
//! Embedded harness fields are unknown schema fields and are refused before
//! effects. There is no harness/model/credential fallback or translation.
//!
//! `provider` names `executable`, opaque provider/v1 `settings`, optional
//! `config_root`/`env`, `agent_bash_bin`, and exactly one Bash policy form:
//! nonempty `bash_allow` or `bash_authority: "trusted-task"`. Adapter-owned
//! settings/environment determine native behavior and credentials. The
//! [`registered`] module checks executable custody and negotiates schemas,
//! resident, tool mediation and exploration capabilities; source/byte hashes
//! are provenance, never compatibility keys. The provider supplies resident
//! stdio argv and its declared label. Generic Bash uses the root's ingress.
//!
//! `workload` declares `host-root` with a non-root host `user`, or
//! `unprivileged-userns` for a non-root caller. Work runs under that identity;
//! owner/store remain private. Host-root IPC is `<launch_dir>/ipc`. Setup
//! hands provider data roots to the work identity, retaining root/per-work
//! PID1 custody, actual waits and separate logical lineage.
//!
//! Optional `children` names `routes`, `max_starts`, `max_concurrent`.
//! Every route requires `registered` (executable/settings/env/config_root/
//! agent_bash_bin) and optional `slots` at most `max_starts`. Children inherit
//! the parent's Bash policy. Each possible admission gets a fresh prepared
//! provider data root; the owner consumes each slot once, across generations.
//! A parent with children requires `root_child_bin`, offered through negotiated
//! exploration/v1. No children means no requester/offer. The generic owner
//! retains trusted Fixed child intents as well as Prepared ones; this front
//! door admits only registered routes. Neither children nor Bash can acquire
//! another owner's custody. No embedded requester or auth files are staged.
//!
//! Optional `live_output` (raw JSON, also on `--recover`) is passed unchanged
//! to separate view admission. Its grant and SDK advertisement must agree with
//! the attested requester and shared live v3 capabilities. A malformed shape,
//! unsupported agreement or refused grant disables only the view.
//!
//! The whole root environment is explicit; adapter operations use their own
//! declared environment. This entry reports names, never secret values.
//!
//! `--recover <file>` acts on an existing store: `{"store", "purpose",
//! "env", "control"?}`, `purpose` being `cancel` or `continue-attached`;
//! `control` is an optional `session_control/v2` `recover` request, which
//! accounts for the separately authorized physical recovery (`attached`,
//! `root_absent`, ...); it is not an effect gate. An unusable account is
//! control-only unavailable, without denying recovery effects. A new owner
//! claims the store (the next owner generation; earlier unresolved
//! attempts become `unknown-prior-owner`) and positively attaches the
//! recorded root PID 1 if it is still that exact process. `cancel` then
//! has the root's live work killed by its own waiters, connecting to
//! nothing and delivering nothing; `continue-attached` reattaches the
//! survivors and resubmits what is owed with its original keys (at best
//! `duplicate-unknown`: not proof the native conversation continued). A
//! survivor with nothing owed takes new input only if its harness declared
//! the live reattachment contract and every turn is recorded ended;
//! otherwise the owner reports it `unavailable` or `unknown`, refuses
//! `send` naming that, and a close ends it or says why not (the owner
//! crate's Settled survivors).
//! Neither starts a new root incarnation: with no root to attach, the
//! owner reports `root-absent` and what the store still owes. `env` is the
//! recovering owner's whole environment; attached work keeps the original
//! root's environment, which nothing here sees or changes. A live owner is
//! refused, never superseded, and nothing is signalled by a stored pid.
//!
//! The owner's life is tied to this entry's: if this entry dies, the
//! kernel kills its owner (parent-death signal), so no unreachable owner
//! keeps the store locked. Owner death kills no root work: root PID 1 and
//! its work survive it, for a later `--recover`.
//!
//! Stdout carries JSON lines. This entry's own lines have an `entry` key;
//! every other line is the owner's, relayed unchanged. Stdin lines after
//! start go to the owner unchanged: its controls, `{"cmd":"cancel"}`,
//! `{"cmd":"send","text":...,"ref"?:...}` (further input to the same live
//! native session, one turn at a time: `follow-up-admitted` only once it is
//! durable owed debt, else `follow-up-refused`) and `{"cmd":"close"}` (no
//! more input; the native host is stopped once the admitted inputs' turns
//! have ended). See the owner crate's Live conversation docs. Stdin EOF
//! closes the owner's stdin, which is neither a cancel nor a close.
//!
//! Exit status, one meaning each:
//!
//! * `0`: the owner ended (`ended`): physical ends observed, insertion-ACK
//!   absence counter zero; read rich turn/async records for settlement.
//! * `82` to `87`: the owner's own class 2 to 7 (`cancelled`, `ended-owed`,
//!   `incomplete` or `owned-unattached`, `authority-lost` or
//!   `store-failed`, `root-absent`, `closed`). `87` (`closed`) says the
//!   physical run ended after close, subject to insertion/refusal guards.
//!   It is not logical settlement or retirement eligibility: ACK present
//!   without tagged end and async debt can still yield this class (U112/R3). Owed work stays in the store, for an
//!   explicit recovery, never by replaying a request.
//! * `64`: the request was refused before any effect: nothing was run.
//! * `65`: a registered provider refused or failed its `describe` or
//!   `policy.evaluate`. This entry made no store or launch directory, but
//!   the provider ran and its own effects are unknown (`effects` says
//!   which). Do not replay as if nothing ran.
//! * `73`: setup construction failed: the launch directory may hold partial
//!   effects, a registered provider's `resident.prepare` among them. Do not
//!   replay.
//! * `66`: the owner refused the request or its store (its 64 or 65),
//!   after setup's effects for a fresh root. Do not replay.
//! * `69`: the owner process could not be started (after setup's effects
//!   for a fresh root).
//! * `70`: the owner was started, then its end is unknown to this entry: it
//!   ended without a known class (a signal or another status), or waiting
//!   for it failed. The store says what happened. Do not replay.
//! * `74`: this entry could not write to its stdout, or lost the owner's
//!   output: its lines, the owner's included, were not all delivered,
//!   whatever the owner's end. Do not replay.
//!
//! An `ack` is insertion, an `idle` is readiness and a native error is a
//! native result; none of them is completion of processing, and this entry
//! adds no such claim. It does not cancel by itself, retry or select
//! accounts, and it touches no Runner state.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_provider_contract::exploration::{self, Limits};
use oulipoly_root_supervisor::bash::BashAuthority;
use oulipoly_root_supervisor::children::{ChildPolicy, ChildRoute, PreparedSlot};
use oulipoly_root_supervisor::{
    Endpoint, HarnessSpec, Intent, LiveOutput, Recover, Request, Workload,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

mod registered;

const OWNER_BINARY: &str = "oulipoly-root-supervisor";

/// Where registered child routes' slots are prepared, under the launch
/// directory.
const CHILD_SLOTS: &str = "child-slots";

const EXIT_REFUSED: i32 = 64;
const EXIT_PROVIDER_REFUSED: i32 = 65;
const EXIT_OWNER_REFUSED: i32 = 66;
const EXIT_OWNER_NOT_STARTED: i32 = 69;
const EXIT_OWNER_UNKNOWN: i32 = 70;
const EXIT_SETUP_FAILED: i32 = 73;
const EXIT_RELAY_FAILED: i32 = 74;
/// Added to the owner's own nonzero classes so none reads as a Runner or
/// argument error.
const OWNER_CLASS_BASE: i32 = 80;

/// The request file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRootRequest {
    /// Absolute path of the root's store; must not exist.
    store: String,
    /// Absolute path of the native host's launch directory; must not exist.
    launch_dir: String,
    /// Absolute working directory of the root's harness.
    cwd: String,
    /// The root's whole environment (see the module docs).
    env: BTreeMap<String, String>,
    /// Messages delivered to the native host, in order.
    messages: Vec<String>,
    outage_closure_cap: u32,
    delivery_attempt_cap: u32,
    /// The sole parent harness, resolved through provider contract negotiation.
    provider: registered::Registration,
    /// Registered children the harness may ask for (see the module docs).
    #[serde(default)]
    children: Option<ChildrenRequest>,
    /// Who the root's work runs as (see the module docs).
    workload: RequestWorkload,
    /// Optional live view of the root's Bash output, granted by this
    /// request's privileged caller; passed to the owner unchanged, which
    /// admits it only for the root's attested requester.
    #[serde(default)]
    live_output: Option<LiveOutput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildrenRequest {
    routes: BTreeMap<String, ChildRouteRequest>,
    max_starts: u32,
    max_concurrent: u32,
}

/// A registered child and its bounded fresh resident slots.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildRouteRequest {
    registered: registered::ChildRegistration,
    #[serde(default)]
    slots: Option<u32>,
}

/// The request's work declaration; the owner's [`Workload`] adds where the
/// work's IPC is.
#[derive(Debug, Deserialize)]
#[serde(tag = "isolation", rename_all = "kebab-case", deny_unknown_fields)]
enum RequestWorkload {
    HostRoot {
        user: String,
    },
    /// A struct variant so that a nominated `user` is refused, not ignored.
    UnprivilegedUserns {},
}

/// The recovery request file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRecoverRequest {
    /// Absolute path of an existing root's store.
    store: String,
    /// `cancel` or `continue-attached`.
    purpose: Recover,
    /// The recovering owner's whole environment; attached work keeps the
    /// original root's.
    env: BTreeMap<String, String>,
    /// An `oulipoly.session_control/v2` `recover` request record, passed to
    /// the owner unchanged; the recovering owner answers it once it knows
    /// what it found (see the owner crate's `control` module).
    #[serde(default)]
    control: Option<Value>,
    /// The recovering owner's live-view grant (as at creation).
    #[serde(default)]
    live_output: Option<LiveOutput>,
}

/// Where this entry's own and relayed lines go.
struct Out {
    sink: Mutex<Box<dyn Write + Send>>,
    /// A write to the sink failed: later lines are not attempted.
    failed: AtomicBool,
    /// Owner output was read but could not be relayed, or could not be read.
    lost: AtomicBool,
}

impl Out {
    fn new(sink: Box<dyn Write + Send>) -> Self {
        Self {
            sink: Mutex::new(sink),
            failed: AtomicBool::new(false),
            lost: AtomicBool::new(false),
        }
    }

    fn line(&self, line: &str) {
        if self.failed.load(Ordering::SeqCst) {
            return;
        }
        let mut out = self.sink.lock().expect("entry output");
        if writeln!(out, "{line}").and_then(|()| out.flush()).is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    fn entry(&self, value: Value) {
        self.line(&value.to_string());
    }

    fn complete(&self) -> bool {
        !self.failed.load(Ordering::SeqCst) && !self.lost.load(Ordering::SeqCst)
    }

    fn exit(&self, code: i32) -> i32 {
        if self.complete() {
            code
        } else {
            EXIT_RELAY_FAILED
        }
    }
}

fn stdout_out() -> Out {
    Out::new(Box::new(io::stdout()))
}

fn refused(out: &Out, reason: String) -> i32 {
    out.entry(json!({
        "entry": "terminal",
        "stage": "refused",
        "reason": reason,
        "effects": "none",
    }));
    out.exit(EXIT_REFUSED)
}

pub(crate) fn run(request_path: &Path) -> Result<i32, String> {
    let out = stdout_out();
    let request = match read_request(request_path) {
        Ok(request) => request,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    let owner = match owner_binary() {
        Ok(owner) => owner,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    // The owner's own checks first, so its refusal cannot follow setup's
    // effects. They read only that the argv is non-empty; every
    // provisioned argv is. They include resolving the work declaration
    // against this process (the owner's euid is this entry's).
    let checked = owner_request(
        &request,
        harness_kind(&request),
        vec!["/usr/bin/env".to_owned()],
        None,
    );
    if let Err(reason) = checked.validate() {
        return Ok(refused(&out, format!("owner request: {reason}")));
    }
    let workload = checked
        .intent
        .as_ref()
        .map(|intent| &intent.workload)
        .expect("a create request")
        .resolve(Path::new(&request.store));
    let workload = match workload {
        Ok(workload) => workload,
        Err(reason) => return Ok(refused(&out, format!("owner request: {reason}"))),
    };
    // Every provider executable's custody and pin, the root's then each
    // registered child route's, before any of them runs.
    let registration = &request.provider;
    let admitted = registered::check(registration)
        .and_then(|()| registered::offer(registration, child_routes(&request), limits(&request)))
        .and_then(|offer| registered::admit(registration, offer));
    let admitted = match admitted {
        Ok(admitted) => admitted,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    let child_registrations = child_registrations(&request);
    let children = match admit_children(&child_registrations) {
        Ok(children) => children,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    Ok(run_registered(
        &out,
        &request,
        registration,
        admitted,
        children,
        &owner,
        &workload,
    ))
}

/// Resolves, prepares and starts a registered provider's harness (see
/// [`registered`]). Everything this entry checks itself was checked before.
fn run_registered(
    out: &Out,
    request: &NativeRootRequest,
    registration: &registered::Registration,
    admitted: registered::Admitted<'_>,
    children: Vec<ChildProvider<'_>>,
    owner: &Path,
    workload: &oulipoly_root_supervisor::workload::Resolved,
) -> i32 {
    let prepared = admitted.describe().and_then(|declared| {
        out.entry(declared.entry());
        let template = admitted.template()?;
        let prepared = admitted.prepare(Path::new(&request.launch_dir), template)?;
        let handed = match &workload.identity {
            Some(identity) => registered::hand_over(
                Path::new(&request.launch_dir),
                &prepared.data_root,
                identity,
            )
            .map(|count| {
                json!({
                    "to": { "user": identity.user, "uid": identity.uid, "gid": identity.gid },
                    "entries": count,
                })
            })
            .map_err(|reason| registered::Failure::Setup {
                provider: true,
                reason,
            })?,
            None => json!("none: the work runs as this entry's user"),
        };
        Ok((declared, prepared, handed))
    });
    let (declared, prepared, handed) = match prepared {
        Ok(prepared) => prepared,
        Err(failure) => return registered_failure(out, request, failure),
    };
    let (child_routes, child_receipt) =
        match prepare_children(out, request, children, workload.identity.as_ref()) {
            Ok(prepared) => prepared,
            Err(reason) => {
                return registered_failure(
                    out,
                    request,
                    registered::Failure::Setup {
                        provider: true,
                        reason,
                    },
                );
            }
        };
    let receipt = json!({
        "provider_id": declared.provider_id,
        "agreed_contract": declared.contract,
        "resident_session": declared.resident_session,
        "executable": registration.executable,
        "executable_identity_sha256": admitted.identity,
        "argv": prepared.argv,
        "endpoint": prepared.result.invocation.endpoint,
        "acp": prepared.result.acp,
        "operations": prepared.result.operations,
        "config_sha256": prepared.result.config_sha256,
        "data_root": prepared.data_root,
        "data_root_handed_over": handed,
        "template_env": prepared.template_env,
        "tools": registration.mediation(),
        "tool_mediation": declared.tool_mediation,
        "effective_mediation": prepared.effective_mediation,
        "mediation_evidence": "provider-reported configuration; live native efficacy unqualified",
        "exploration": match &admitted.offer {
            Some(offer) => json!(offer),
            None => json!("none offered: the root declares no child routes; mediated Bash alone"),
        },
        "exploration_version": declared.exploration,
        "effective_exploration": prepared.effective_exploration,
        "exploration_evidence": if admitted.offer.is_some() {
            "provider-reported configuration of an offer, not admission: the owner admits or refuses every child; live native efficacy unqualified"
        } else {
            "none selected"
        },
        "identity": {
            "describe_policy_prepare": "this entry's",
            "resident_serve": "the work identity, in the root's work namespaces",
        },
    });
    let reach = env_reach(&request.env, &[], &[], Endpoint::Stdio);
    out.entry(json!({
        "entry": "setup-completed",
        "harness": "registered-provider",
        "launch": receipt,
        "children": child_receipt,
        "owner": owner,
        "env": reach,
        "workload": workload_report(workload),
    }));
    let context = json!({
        "store": request.store,
        "launch_dir": request.launch_dir,
        "setup": "retained",
        "retry": "do-not-replay",
    });
    let mut owner_request = owner_request(
        request,
        (declared.provider_id.as_str(), Endpoint::Stdio),
        prepared.argv,
        Some(&child_routes),
    );
    owner_request
        .intent
        .as_mut()
        .expect("create intent")
        .harnesses[0]
        .resident = Some(prepared.result);
    start_owner(out, owner, &request.env, &owner_request, &context)
}

/// The terminal line of a registered provider's refusal or failure: what
/// ran, and so which effects there may be.
fn registered_failure(out: &Out, request: &NativeRootRequest, failure: registered::Failure) -> i32 {
    match failure {
        registered::Failure::Provider { operation, reason } => {
            out.entry(json!({
                "entry": "terminal",
                "stage": "provider-refused",
                "operation": operation,
                "reason": reason,
                "effects": { "runner_setup": "none", "provider": "unknown: it ran" },
                "retry": "do-not-replay-as-unrun",
            }));
            out.exit(EXIT_PROVIDER_REFUSED)
        }
        registered::Failure::Setup { provider, reason } => {
            out.entry(json!({
                "entry": "terminal",
                "stage": "setup-failed",
                "reason": reason,
                "launch_dir": request.launch_dir,
                "effects": "possible",
                "provider": if provider { "unknown: it ran" } else { "not run" },
                "retry": "do-not-replay",
            }));
            out.exit(EXIT_SETUP_FAILED)
        }
    }
}

fn workload_report(workload: &oulipoly_root_supervisor::workload::Resolved) -> Value {
    json!({
        "isolation": workload.isolation.label(),
        "user": workload.identity.as_ref().map(|identity| &identity.user),
        "uid": workload.identity.as_ref().map(|identity| identity.uid),
        "gid": workload.identity.as_ref().map(|identity| identity.gid),
        "ipc_dir": workload.ipc_dir,
    })
}

/// The configured child route names, in the owner's (sorted) route order:
/// the offer's opaque labels.
fn child_routes(request: &NativeRootRequest) -> Vec<String> {
    request
        .children
        .as_ref()
        .map(|children| children.routes.keys().cloned().collect())
        .unwrap_or_default()
}

/// The root's child ceilings as the owner enforces them (it refuses a
/// request above its own hard limits before any effect).
fn limits(request: &NativeRootRequest) -> Limits {
    let children = request.children.as_ref();
    Limits {
        max_starts: children.map(|children| children.max_starts),
        max_concurrent: children.map(|children| children.max_concurrent),
    }
}

/// The parent harness's `bash` policy, which its children inherit.
fn bash_policy(request: &NativeRootRequest) -> (Vec<String>, Option<BashAuthority>) {
    (
        request.provider.bash_allow.clone(),
        request.provider.bash_authority,
    )
}

/// The intent's child policy, from the request's `children`. A registered
/// route is its `prepared` slots; before they are prepared (the owner's
/// checks before any effect), the slots planned for it.
fn child_policy(
    request: &NativeRootRequest,
    prepared: Option<&BTreeMap<String, ChildRoute>>,
) -> Option<ChildPolicy> {
    let children = request.children.as_ref()?;
    Some(ChildPolicy {
        routes: children
            .routes
            .iter()
            .map(|(name, route)| {
                let launch = match prepared.and_then(|prepared| prepared.get(name)) {
                    Some(prepared) => prepared.clone(),
                    None => ChildRoute::Prepared {
                        provider: "registered-provider".to_owned(),
                        slots: (0..slot_count(children, route))
                            .map(|index| PreparedSlot {
                                argv: vec!["/usr/bin/env".to_owned()],
                                resident: None,
                                data_root: slot_dir(request, name, index)
                                    .join("provider")
                                    .to_string_lossy()
                                    .into_owned(),
                            })
                            .collect(),
                        endpoint: Endpoint::Stdio,
                    },
                };
                (name.clone(), launch)
            })
            .collect(),
        max_starts: children.max_starts,
        max_concurrent: children.max_concurrent,
        launch_base: Path::new(&request.launch_dir)
            .join("children")
            .to_string_lossy()
            .into_owned(),
    })
}

/// How many slots a registered child route has prepared.
fn slot_count(children: &ChildrenRequest, route: &ChildRouteRequest) -> u32 {
    route.slots.unwrap_or(children.max_starts)
}

/// Where a registered child route's slot `index` is prepared.
fn slot_dir(request: &NativeRootRequest, route: &str, index: u32) -> PathBuf {
    Path::new(&request.launch_dir)
        .join(CHILD_SLOTS)
        .join(route)
        .join(index.to_string())
}

/// The request's registered child routes, each with its slot count and the
/// parent's tool policy, which a child inherits.
fn child_registrations(
    request: &NativeRootRequest,
) -> Vec<(String, u32, registered::Registration)> {
    let Some(children) = &request.children else {
        return Vec::new();
    };
    let (bash_allow, bash_authority) = bash_policy(request);
    children
        .routes
        .iter()
        .map(|(name, route)| {
            (
                name.clone(),
                slot_count(children, route),
                route
                    .registered
                    .with_policy(bash_allow.clone(), bash_authority),
            )
        })
        .collect()
}

/// A registered child route after its custody check and pin, before any
/// of its operations ran.
struct ChildProvider<'a> {
    name: &'a str,
    slots: u32,
    registration: &'a registered::Registration,
    admitted: registered::Admitted<'a>,
}

/// Checks and pins every registered child route's executable, running
/// nothing.
fn admit_children(
    registrations: &[(String, u32, registered::Registration)],
) -> Result<Vec<ChildProvider<'_>>, String> {
    registrations
        .iter()
        .map(|(name, slots, registration)| {
            registered::check(registration)
                .and_then(|()| registered::admit(registration, None))
                .map(|admitted| ChildProvider {
                    name,
                    slots: *slots,
                    registration,
                    admitted,
                })
                .map_err(|reason| format!("children: route {name}: {reason}"))
        })
        .collect()
}

/// Describes, evaluates and prepares every registered child route, after
/// the root's own harness setup: each of its slots under
/// `<launch_dir>/child-slots/<route>/<index>` with its own fresh provider
/// data root, handed to the work identity. Any failure is a setup failure
/// (the root's setup and some provider operations had run). Returns the
/// owner's routes and the setup receipt.
fn prepare_children(
    out: &Out,
    request: &NativeRootRequest,
    children: Vec<ChildProvider<'_>>,
    identity: Option<&oulipoly_root_supervisor::workload::Identity>,
) -> Result<(BTreeMap<String, ChildRoute>, Value), String> {
    let mut routes = BTreeMap::new();
    let mut receipt = Map::new();
    if children.is_empty() {
        return Ok((routes, Value::Null));
    }
    let base = Path::new(&request.launch_dir).join(CHILD_SLOTS);
    registered::make_slot_base(&base, identity).map_err(|reason| format!("children: {reason}"))?;
    for child in children {
        let name = child.name;
        let failed = |failure: registered::Failure| match failure {
            registered::Failure::Provider { operation, reason } => {
                format!("children: route {name}: {operation}: {reason}")
            }
            registered::Failure::Setup { reason, .. } => {
                format!("children: route {name}: {reason}")
            }
        };
        let declared = child.admitted.describe().map_err(failed)?;
        let mut entry = declared.entry();
        entry["child_route"] = json!(name);
        out.entry(entry);
        let evaluated = child.admitted.template().map_err(failed)?;
        registered::make_slot_base(&base.join(name), identity)
            .map_err(|reason| format!("children: route {name}: {reason}"))?;
        let mut slots = Vec::new();
        let mut slot_receipts = Vec::new();
        for index in 0..child.slots {
            let dir = slot_dir(request, name, index);
            let prepared = child
                .admitted
                .prepare(&dir, evaluated.clone())
                .map_err(failed)?;
            let handed = match identity {
                Some(identity) => registered::hand_over(&dir, &prepared.data_root, identity)
                    .map(|count| {
                        json!({
                            "to": { "user": identity.user, "uid": identity.uid, "gid": identity.gid },
                            "entries": count,
                        })
                    })
                    .map_err(|reason| format!("children: route {name}: {reason}"))?,
                None => json!("none: the work runs as this entry's user"),
            };
            slot_receipts.push(json!({
                "index": index,
                "argv": prepared.argv,
                "data_root": prepared.data_root,
                "config_sha256": prepared.result.config_sha256,
                "data_root_handed_over": handed,
            }));
            slots.push(PreparedSlot {
                argv: prepared.argv,
                data_root: prepared.data_root.to_string_lossy().into_owned(),
                resident: Some(prepared.result),
            });
        }
        receipt.insert(
            name.to_owned(),
            json!({
                "provider_id": declared.provider_id,
                "agreed_contract": declared.contract,
                "resident_session": declared.resident_session,
                "tool_mediation": declared.tool_mediation,
                "executable": child.registration.executable,
                "executable_identity_sha256": child.admitted.identity,
                "tools": child.registration.mediation(),
                "tools_source": "the parent's bash policy, inherited; read-only exploration is the child's brief, not a write barrier",
                "effective_mediation": evaluated.effective,
                "mediation_evidence": "provider-reported configuration; live native efficacy unqualified",
                "exploration": "none offered or selected for the child; the owner refuses a child's own child request (depth 1)",
                "slots": slot_receipts,
                "slots_meaning": "prepared before the owner started, each with its own fresh data root; the k-th admission on this route takes slot k, once; unused slots are setup, not starts or admissions",
                "identity": {
                    "describe_policy_prepare": "this entry's",
                    "resident_serve": "the work identity, in the child's own work namespaces",
                },
            }),
        );
        routes.insert(
            name.to_owned(),
            ChildRoute::Prepared {
                provider: declared.provider_id.clone(),
                slots,
                endpoint: Endpoint::Stdio,
            },
        );
    }
    Ok((routes, Value::Object(receipt)))
}

/// The harness id and endpoint of the request's one harness. A registered
/// provider's id is what it declares; this placeholder serves the owner's
/// checks before it is described.
fn harness_kind(_request: &NativeRootRequest) -> (&'static str, Endpoint) {
    ("registered-provider", Endpoint::Stdio)
}

/// `--recover`: one new owner for an existing store, for `cancel` or
/// `continue-attached` only.
pub(crate) fn recover(request_path: &Path) -> Result<i32, String> {
    let out = stdout_out();
    let request = match read_recover_request(request_path) {
        Ok(request) => request,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    let owner = match owner_binary() {
        Ok(owner) => owner,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    let owner_request = Request {
        store: request.store.clone(),
        intent: None,
        recover: Some(request.purpose),
        control: request.control.clone(),
        live_output: request.live_output.clone(),
    };
    if let Err(reason) = owner_request.validate() {
        return Ok(refused(&out, format!("owner request: {reason}")));
    }
    let purpose = serde_json::to_value(request.purpose).unwrap_or(Value::Null);
    let names: Vec<&str> = request.env.keys().map(String::as_str).collect();
    out.entry(json!({
        "entry": "recovery",
        "purpose": purpose,
        "store": request.store,
        "owner": owner,
        "env": {
            "declared": names,
            "ambient": "none",
            "recovering_owner": names,
            "attached_work": "original-root-environment",
            "new_incarnation": "never-started-by-this-recovery",
        },
    }));
    let context = json!({
        "store": request.store,
        "purpose": purpose,
        "retry": "explicit-recovery-only",
    });
    Ok(start_owner(
        &out,
        &owner,
        &request.env,
        &owner_request,
        &context,
    ))
}

/// Starts the owner with exactly `env`, hands it `request`, relays its
/// output and control, and reports its end. Every outcome after the start
/// keeps `context` (where the root's state is; how not to retry).
fn start_owner(
    out: &Out,
    owner: &Path,
    env: &BTreeMap<String, String>,
    request: &Request,
    context: &Value,
) -> i32 {
    let terminal = |fields: Value| {
        let mut line = context.clone();
        for (key, value) in fields.as_object().into_iter().flatten() {
            line[key] = value.clone();
        }
        line["entry"] = json!("terminal");
        out.entry(line);
    };
    let line = match serde_json::to_string(request) {
        Ok(line) => line,
        Err(error) => {
            terminal(json!({ "stage": "owner-not-started", "reason": error.to_string() }));
            return out.exit(EXIT_OWNER_NOT_STARTED);
        }
    };
    let mut child = match spawn_owner(owner, env) {
        Ok(child) => child,
        Err(error) => {
            terminal(json!({ "stage": "owner-not-started", "reason": error.to_string() }));
            return out.exit(EXIT_OWNER_NOT_STARTED);
        }
    };
    out.entry(json!({ "entry": "owner-started", "pid": child.id(), "store": request.store }));
    let mut stdin = child.stdin.take().expect("owner stdin");
    // A failed write means the owner is already gone; its exit says why.
    let delivered = writeln!(stdin, "{line}")
        .and_then(|()| stdin.flush())
        .is_ok();
    if delivered {
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
    } else {
        drop(stdin);
    }
    let stdout = child.stdout.take().expect("owner stdout");
    relay(out, stdout);
    let (fields, code) = owner_end(child.wait());
    let mut fields = fields;
    fields["relay"] = json!(if out.complete() {
        "complete"
    } else {
        "incomplete"
    });
    terminal(fields);
    out.exit(code)
}

/// Starts the owner with exactly `env`, its life tied to this entry's.
/// The parent-death signal follows the thread that spawns it, so this runs
/// on the thread that later waits for the owner.
fn spawn_owner(owner: &Path, env: &BTreeMap<String, String>) -> io::Result<Child> {
    let entry = libc::pid_t::try_from(std::process::id()).map_err(io::Error::other)?;
    let mut command = Command::new(owner);
    command
        .env_clear()
        .envs(env)
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(io::Error::last_os_error());
            }
            // This entry may have died before the setting took effect.
            if libc::getppid() != entry {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Relays every owner line to its end, delivered or not: an undrained
/// owner can delay its own cancel. A line that is not UTF-8 is lost (not
/// relayed) and reading continues; a failed read ends reading. Either way
/// the relay is incomplete, whatever the owner's end.
fn relay(out: &Out, stdout: impl Read) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => return,
            Ok(_) => {
                if buffer.last() == Some(&b'\n') {
                    buffer.pop();
                }
                match std::str::from_utf8(&buffer) {
                    Ok(line) => out.line(line),
                    Err(_) => {
                        out.lost.store(true, Ordering::SeqCst);
                        out.entry(
                            json!({ "entry": "relay-lost", "reason": "owner line not UTF-8" }),
                        );
                    }
                }
            }
            Err(error) => {
                out.lost.store(true, Ordering::SeqCst);
                out.entry(
                    json!({ "entry": "relay-lost", "reason": format!("owner output: {error}") }),
                );
                return;
            }
        }
    }
}

/// The owner's end as this entry, its parent, observed it by its wait.
fn owner_end(wait: io::Result<ExitStatus>) -> (Value, i32) {
    let status = match wait {
        Ok(status) => status,
        Err(error) => {
            return (
                json!({
                    "stage": "owner-wait-failed",
                    "reason": error.to_string(),
                    "owner_outcome": "unknown",
                }),
                EXIT_OWNER_UNKNOWN,
            );
        }
    };
    let (stage, code) = match status.code() {
        Some(0) => ("owner-ended", 0),
        Some(class @ 2..=7) => ("owner-ended", OWNER_CLASS_BASE + class),
        Some(64 | 65) => ("owner-refused", EXIT_OWNER_REFUSED),
        _ => ("owner-outcome-unknown", EXIT_OWNER_UNKNOWN),
    };
    (
        json!({
            "stage": stage,
            "owner_exit": status.code(),
            "owner_signal": std::os::unix::process::ExitStatusExt::signal(&status),
        }),
        code,
    )
}

fn check_env(env: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in env {
        if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
            return Err(format!(
                "env name {name:?} or its value is not an environment entry"
            ));
        }
        if name == oulipoly_root_supervisor::bash::BASH_ENV
            || name == oulipoly_root_supervisor::SOCKET_ENV
        {
            return Err(format!("env {name} is the owner's to set"));
        }
        // Every harness, children included, inherits this environment: a
        // value here would be an exploration offer nobody made.
        if name == exploration::ENV {
            return Err(format!("env {name} is this entry's to offer"));
        }
    }
    Ok(())
}

fn read_request(path: &Path) -> Result<NativeRootRequest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("request: {error}"))?;
    let request: NativeRootRequest =
        serde_json::from_str(&text).map_err(|error| format!("request: {error}"))?;
    for (name, path) in [
        ("store", &request.store),
        ("launch_dir", &request.launch_dir),
    ] {
        if !path.starts_with('/') {
            return Err(format!("{name} must be absolute"));
        }
        // Fresh roots only: an existing path may be an earlier root's.
        if std::fs::symlink_metadata(path).is_ok() {
            return Err(format!("{name} exists; a root is started fresh only"));
        }
    }
    check_env(&request.env)?;
    if request.messages.is_empty() {
        return Err("messages names nothing to deliver".to_owned());
    }
    if let Some(children) = &request.children {
        check_child_routes(children)?;
    }
    Ok(request)
}

/// Registered child slots cannot exceed the root's admission ceiling.
fn check_child_routes(children: &ChildrenRequest) -> Result<(), String> {
    for (name, route) in &children.routes {
        if route
            .slots
            .is_some_and(|slots| !(1..=children.max_starts).contains(&slots))
        {
            return Err(format!(
                "children: route {name}: slots must be 1..=max_starts"
            ));
        }
    }
    Ok(())
}

fn read_recover_request(path: &Path) -> Result<NativeRecoverRequest, String> {
    let text = std::fs::read_to_string(path).map_err(|error| format!("request: {error}"))?;
    let request: NativeRecoverRequest =
        serde_json::from_str(&text).map_err(|error| format!("request: {error}"))?;
    if !request.store.starts_with('/') {
        return Err("store must be absolute".to_owned());
    }
    // Existing roots only: a claim would otherwise create a store.
    match std::fs::symlink_metadata(&request.store) {
        Ok(meta) if meta.is_dir() => {}
        _ => {
            return Err(
                "store is not an existing directory; recovery is of an existing root only"
                    .to_owned(),
            );
        }
    }
    check_env(&request.env)?;
    Ok(request)
}

fn owner_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|error| format!("runner binary: {error}"))?;
    let owner = exe
        .parent()
        .ok_or("runner binary has no directory")?
        .join(OWNER_BINARY);
    if !owner.is_file() {
        return Err(format!(
            "no {OWNER_BINARY} next to this runner ({}); build it in the same target",
            owner.display()
        ));
    }
    Ok(owner)
}

fn owner_request(
    request: &NativeRootRequest,
    (id, endpoint): (&str, Endpoint),
    argv: Vec<String>,
    children: Option<&BTreeMap<String, ChildRoute>>,
) -> Request {
    Request {
        store: request.store.clone(),
        intent: Some(Intent {
            outage_closure_cap: request.outage_closure_cap,
            delivery_attempt_cap: request.delivery_attempt_cap,
            cwd: request.cwd.clone(),
            harnesses: vec![HarnessSpec {
                id: id.to_owned(),
                argv,
                endpoint,
                session: None,
                resident: None,
                messages: request.messages.clone(),
            }],
            workload: match &request.workload {
                RequestWorkload::HostRoot { user } => Workload::HostRoot {
                    user: user.clone(),
                    ipc_dir: Path::new(&request.launch_dir)
                        .join("ipc")
                        .to_string_lossy()
                        .into_owned(),
                },
                RequestWorkload::UnprivilegedUserns {} => Workload::UnprivilegedUserns {},
            },
            children: child_policy(request, children),
        }),
        recover: None,
        control: None,
        live_output: request.live_output.clone(),
    }
}

/// Which declared names reach where: names only, never values.
fn env_reach(
    declared: &BTreeMap<String, String>,
    launch: &[&str],
    removal: &[&str],
    endpoint: Endpoint,
) -> Value {
    let mut owner_set = vec![oulipoly_root_supervisor::bash::BASH_ENV];
    if endpoint == Endpoint::UnixSocket {
        owner_set.push(oulipoly_root_supervisor::SOCKET_ENV);
    }
    let names: Vec<&str> = declared.keys().map(String::as_str).collect();
    let removed: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| removal.contains(name))
        .collect();
    let overridden: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| launch.contains(name))
        .collect();
    let inherited: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !removed.contains(name) && !overridden.contains(name))
        .collect();
    json!({
        "declared": names,
        "ambient": "none",
        "owner_root_pid1_and_bash": names,
        "native_host": {
            "inherited": inherited,
            "removed_by_launch": removed,
            "overridden_by_launch": overridden,
            "set_by_launch": launch,
            "set_by_owner": owner_set,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn lines(&self) -> Vec<Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    /// One good line, then a read error.
    struct FailingRead(bool);

    impl Read for FailingRead {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if std::mem::replace(&mut self.0, true) {
                return Err(io::Error::other("injected read fault"));
            }
            let line = b"{\"event\":\"started\"}\n";
            buffer[..line.len()].copy_from_slice(line);
            Ok(line.len())
        }
    }

    #[test]
    fn lost_owner_line_is_incomplete_delivery_whatever_the_owner_end() {
        let sink = Captured::default();
        let out = Out::new(Box::new(sink.clone()));
        relay(
            &out,
            &b"{\"event\":\"a\"}\n\xff\xfe\n{\"event\":\"terminal\"}\n"[..],
        );
        let lines = sink.lines();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0]["event"], "a");
        assert_eq!(lines[1]["entry"], "relay-lost");
        // Reading went on past the lost line.
        assert_eq!(lines[2]["event"], "terminal");
        assert!(!out.complete());
        let (_, code) = owner_end(Ok(ExitStatus::from_raw(2 << 8)));
        assert_eq!(code, 82);
        assert_eq!(out.exit(code), EXIT_RELAY_FAILED);
    }

    #[test]
    fn failed_owner_read_is_incomplete_delivery() {
        let sink = Captured::default();
        let out = Out::new(Box::new(sink.clone()));
        relay(&out, FailingRead(false));
        let lines = sink.lines();
        assert_eq!(lines[0]["event"], "started");
        assert_eq!(lines[1]["entry"], "relay-lost", "{lines:?}");
        assert!(lines[1]["reason"].as_str().unwrap().contains("injected"));
        assert_eq!(out.exit(0), EXIT_RELAY_FAILED);
    }

    #[test]
    fn complete_relay_keeps_the_owner_class() {
        let sink = Captured::default();
        let out = Out::new(Box::new(sink.clone()));
        relay(&out, &b"{\"event\":\"terminal\"}\n"[..]);
        assert!(out.complete());
        assert_eq!(out.exit(82), 82);
    }

    #[test]
    fn failed_owner_wait_is_an_unknown_outcome_not_a_generic_error() {
        let (fields, code) = owner_end(Err(io::Error::other("injected wait fault")));
        assert_eq!(code, EXIT_OWNER_UNKNOWN);
        assert_eq!(fields["stage"], "owner-wait-failed");
        assert_eq!(fields["owner_outcome"], "unknown");
        assert!(fields.get("owner_exit").is_none(), "{fields}");
    }

    #[test]
    fn owner_classes_map_one_meaning_each() {
        for (raw, stage, code) in [
            (0, "owner-ended", 0),
            (2 << 8, "owner-ended", 82),
            (4 << 8, "owner-ended", 84),
            (6 << 8, "owner-ended", 86),
            (7 << 8, "owner-ended", 87),
            (8 << 8, "owner-outcome-unknown", 70),
            (65 << 8, "owner-refused", 66),
            (1 << 8, "owner-outcome-unknown", 70),
            (9, "owner-outcome-unknown", 70),
        ] {
            let (fields, got) = owner_end(Ok(ExitStatus::from_raw(raw)));
            assert_eq!(
                (fields["stage"].as_str(), got),
                (Some(stage), code),
                "{raw}"
            );
        }
    }

    #[test]
    fn recovery_is_of_an_existing_store_with_a_named_purpose_only() {
        let dir = tempfile::tempdir().unwrap();
        let read = |value: Value| {
            let path = dir.path().join("recover.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_recover_request(&path)
        };
        let store = dir.path().join("s");
        let base = |store: &Path, purpose: &str| json!({ "store": store, "purpose": purpose, "env": { "PATH": "/usr/bin" } });
        let missing = read(base(&store, "cancel")).unwrap_err();
        assert!(missing.contains("existing"), "{missing}");
        assert!(!store.exists(), "refusal created the store");
        std::fs::create_dir(&store).unwrap();
        assert_eq!(
            read(base(&store, "cancel")).unwrap().purpose,
            Recover::Cancel
        );
        assert_eq!(
            read(base(&store, "continue-attached")).unwrap().purpose,
            Recover::ContinueAttached
        );
        assert!(read(base(&store, "continue")).is_err());
        let mut extra = base(&store, "cancel");
        extra["inherit_env"] = json!(true);
        assert!(read(extra).unwrap_err().contains("unknown field"));
        let owned = json!({ "store": store, "purpose": "cancel",
            "env": { "OULIPOLY_ACP_V2_SOCKET": "/x" } });
        assert!(read(owned).unwrap_err().contains("owner's to set"));
    }

    #[test]
    fn env_reach_names_what_the_native_launch_replaces_and_never_values() {
        let declared = BTreeMap::from([
            ("HOME".to_owned(), "/owner-home-secret-path".to_owned()),
            ("ADAPTER_OPTION".to_owned(), "{}".to_owned()),
            ("TOKEN".to_owned(), "secret-value".to_owned()),
        ]);
        let reach = env_reach(
            &declared,
            &["HOME", "XDG_CONFIG_HOME"],
            &["ADAPTER_OPTION"],
            Endpoint::UnixSocket,
        );
        assert_eq!(
            reach["declared"],
            json!(["ADAPTER_OPTION", "HOME", "TOKEN"])
        );
        assert_eq!(reach["native_host"]["inherited"], json!(["TOKEN"]));
        assert_eq!(
            reach["native_host"]["overridden_by_launch"],
            json!(["HOME"])
        );
        assert_eq!(
            reach["native_host"]["removed_by_launch"],
            json!(["ADAPTER_OPTION"])
        );
        let text = reach.to_string();
        assert!(!text.contains("secret"), "{text}");
    }

    #[test]
    fn environment_receipt_matches_stdio_and_socket_owner_endpoints() {
        let declared = BTreeMap::new();
        for (endpoint, expected) in [
            (
                Endpoint::Stdio,
                json!([oulipoly_root_supervisor::bash::BASH_ENV]),
            ),
            (
                Endpoint::UnixSocket,
                json!([
                    oulipoly_root_supervisor::bash::BASH_ENV,
                    oulipoly_root_supervisor::SOCKET_ENV
                ]),
            ),
        ] {
            assert_eq!(
                env_reach(&declared, &[], &[], endpoint)["native_host"]["set_by_owner"],
                expected
            );
        }
    }

    /// Each child route is registered; a registered child takes the parent's tool policy (it cannot name its
    /// own) and its slots are planned under the launch directory, at most
    /// the start budget, before anything runs.
    #[test]
    fn registered_child_routes_inherit_policy_and_plan_bounded_slots() {
        let dir = tempfile::tempdir().unwrap();
        let read = |value: Value| {
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        let registered = json!({
            "executable": "/opt/provider/bin/provider",
            "settings": { "settings_id": "child" },
            "env": { "ACCOUNT_CHOICE": "site-supplied" },
            "agent_bash_bin": "/opt/agent-bash",
        });
        let base = json!({
            "store": dir.path().join("store"),
            "launch_dir": dir.path().join("launch"),
            "cwd": "/", "env": {}, "messages": ["m"],
            "outage_closure_cap": 1, "delivery_attempt_cap": 1,
            "provider": provider_setup(),
            "children": {
                "routes": { "luna": { "registered": registered, "slots": 2 } },
                "max_starts": 3, "max_concurrent": 1,
            },
            "workload": { "isolation": "unprivileged-userns" },
        });
        let request = read(base.clone()).unwrap();
        let registrations = child_registrations(&request);
        assert_eq!(registrations.len(), 1);
        let (name, slots, registration) = &registrations[0];
        assert_eq!((name.as_str(), *slots), ("luna", 2));
        // The parent's neutral trusted-task authority, inherited.
        assert_eq!(
            serde_json::to_value(registration.mediation()).unwrap()["bash"],
            json!({ "authority": "trusted-task" })
        );
        let policy = child_policy(&request, None).unwrap();
        match &policy.routes["luna"] {
            ChildRoute::Prepared {
                slots, endpoint, ..
            } => {
                assert_eq!(*endpoint, Endpoint::Stdio);
                let roots: Vec<_> = slots.iter().map(|slot| slot.data_root.as_str()).collect();
                let launch = dir.path().join("launch/child-slots/luna");
                assert_eq!(
                    roots,
                    [
                        launch.join("0/provider").to_string_lossy(),
                        launch.join("1/provider").to_string_lossy()
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        owner_request(
            &request,
            harness_kind(&request),
            vec!["/usr/bin/env".to_owned()],
            None,
        )
        .validate()
        .unwrap();
        // Defaults to one slot per possible start.
        let mut default_slots = base.clone();
        default_slots["children"]["routes"]["luna"]
            .as_object_mut()
            .unwrap()
            .remove("slots");
        let request = read(default_slots).unwrap();
        assert_eq!(child_registrations(&request)[0].1, 3);
        let refuse = |edit: &dyn Fn(&mut Value), fragment: &str| {
            let mut value = base.clone();
            edit(&mut value);
            let error = read(value).unwrap_err();
            assert!(error.contains(fragment), "{fragment}: {error}");
        };
        // A child cannot be given its own tool policy.
        refuse(
            &|v| {
                v["children"]["routes"]["luna"]["registered"]["bash_authority"] =
                    json!("trusted-task")
            },
            "unknown field",
        );
        refuse(
            &|v| v["children"]["routes"]["luna"]["registered"]["bash_allow"] = json!(["true"]),
            "unknown field",
        );
        refuse(
            &|v| v["children"]["routes"]["luna"]["slots"] = json!(4),
            "slots must be 1..=max_starts",
        );
        refuse(
            &|v| v["children"]["routes"]["luna"]["slots"] = json!(0),
            "slots must be 1..=max_starts",
        );
        refuse(
            &|v| v["children"]["routes"]["luna"]["model"] = json!("openai/luna"),
            "unknown field",
        );
        refuse(
            &|v| {
                v["children"]["routes"]["oc"] =
                    json!({ "model": "openai/luna", "provider": { "openai": {} } })
            },
            "unknown field",
        );
        refuse(
            &|v| {
                v["children"]["routes"]["oc"] =
                    json!({ "model": "openai/luna", "provider": { "openai": {} }, "slots": 1 })
            },
            "unknown field",
        );
    }

    /// A registered parent's offer is every configured route, registered
    /// or not, with the root's own ceilings, which the owner's checks
    /// accept; without routes there is none.
    #[test]
    fn registered_parent_is_offered_every_configured_route_within_owner_limits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let requester = dir.path().join("oulipoly-root-child");
        std::fs::write(&requester, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&requester, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().join("request.json");
        let mut value = json!({
            "store": dir.path().join("store"),
            "launch_dir": dir.path().join("launch"),
            "cwd": "/", "env": {}, "messages": ["m"],
            "outage_closure_cap": 1, "delivery_attempt_cap": 1,
            "provider": {
                "executable": "/opt/provider/bin/provider",
                "settings": { "settings_id": "s" },
                "agent_bash_bin": requester,
                "root_child_bin": requester,
                "bash_allow": ["git status"],
            },
            "children": {
                "routes": {
                    "luna": { "registered": {
                        "executable": "/opt/provider/bin/provider",
                        "settings": {}, "agent_bash_bin": "/b" }, "slots": 1 },
                    "other": { "registered": { "executable": "/opt/p", "settings": {}, "agent_bash_bin": "/b" } },
                },
                "max_starts": 2, "max_concurrent": 1,
            },
            "workload": { "isolation": "unprivileged-userns" },
        });
        std::fs::write(&path, value.to_string()).unwrap();
        let request = read_request(&path).unwrap();
        let registration = &request.provider;
        let offer = registered::offer(registration, child_routes(&request), limits(&request))
            .unwrap()
            .unwrap();
        assert_eq!(offer.routes, ["luna", "other"]);
        assert_eq!(offer.ingress_env, oulipoly_root_supervisor::bash::BASH_ENV);
        assert_eq!(
            serde_json::to_value(&offer.limits).unwrap(),
            json!({ "max_starts": 2, "max_concurrent": 1 })
        );
        let policy = child_policy(&request, None).unwrap();
        assert_eq!(policy.routes.keys().collect::<Vec<_>>(), ["luna", "other"]);
        owner_request(
            &request,
            harness_kind(&request),
            vec!["/usr/bin/env".to_owned()],
            None,
        )
        .validate()
        .unwrap();
        // Children's registrations carry no requester.
        assert!(child_registrations(&request)[0].2.root_child_bin.is_none());
        value.as_object_mut().unwrap().remove("children");
        std::fs::write(&path, value.to_string()).unwrap();
        let request = read_request(&path).unwrap();
        let refused =
            registered::offer(&request.provider, child_routes(&request), limits(&request))
                .unwrap_err();
        assert!(refused.contains("declares no child routes"), "{refused}");
    }

    /// A provider that cannot be admitted runs nothing (64); once it ran,
    /// a refusal is the provider's (65) and never says that nothing ran.
    #[test]
    fn registered_failures_say_what_ran() {
        let dir = tempfile::tempdir().unwrap();
        let request: NativeRootRequest = serde_json::from_value(json!({
            "store": dir.path().join("store"),
            "launch_dir": dir.path().join("launch"),
            "cwd": "/", "env": {}, "messages": ["m"],
            "outage_closure_cap": 1, "delivery_attempt_cap": 1,
            "workload": { "isolation": "unprivileged-userns" },
            "provider": provider_setup(),
        }))
        .unwrap();
        let sink = Captured::default();
        let out = Out::new(Box::new(sink.clone()));
        let code = registered_failure(
            &out,
            &request,
            registered::Failure::Provider {
                operation: "describe",
                reason: "r".to_owned(),
            },
        );
        assert_eq!(code, EXIT_PROVIDER_REFUSED);
        let code = registered_failure(
            &out,
            &request,
            registered::Failure::Setup {
                provider: true,
                reason: "r".to_owned(),
            },
        );
        assert_eq!(code, EXIT_SETUP_FAILED);
        let lines = sink.lines();
        assert_eq!(lines[0]["stage"], "provider-refused");
        assert_eq!(lines[0]["operation"], "describe");
        assert_eq!(lines[0]["effects"]["runner_setup"], "none");
        assert_eq!(lines[0]["effects"]["provider"], "unknown: it ran");
        assert_eq!(lines[1]["stage"], "setup-failed");
        assert_eq!(lines[1]["effects"], "possible");
        assert_eq!(lines[1]["provider"], "unknown: it ran");
        assert!(
            lines.iter().all(|line| line["effects"] != "none"),
            "{lines:?}"
        );
    }

    fn provider_setup() -> Value {
        json!({ "executable": "/opt/provider", "settings": {},
            "agent_bash_bin": "/b", "bash_authority": "trusted-task" })
    }

    /// The work identity is declared, never taken from the caller's euid:
    /// no declaration is refused, an unprivileged one names no identity,
    /// and a host-root one from a non-root caller is refused by the owner's
    /// own checks, which this entry applies before setup's effects.
    #[test]
    fn workload_is_declared_and_refused_when_this_process_cannot_honour_it() {
        let dir = tempfile::tempdir().unwrap();
        let request = |workload: Option<Value>| {
            let mut value = json!({
                "store": dir.path().join("store"),
                "launch_dir": dir.path().join("launch"),
                "cwd": "/",
                "env": {},
                "messages": ["m"],
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "provider": provider_setup(),
            });
            if let Some(workload) = workload {
                value["workload"] = workload;
            }
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        let missing = request(None).unwrap_err();
        assert!(missing.contains("missing field `workload`"), "{missing}");
        let nominated = request(Some(
            json!({ "isolation": "unprivileged-userns", "user": "root" }),
        ))
        .unwrap_err();
        assert!(nominated.contains("unknown field"), "{nominated}");
        assert!(request(Some(json!({ "isolation": "host-root" }))).is_err());
        // These tests run unprivileged.
        let host = request(Some(json!({ "isolation": "host-root", "user": "nobody" }))).unwrap();
        let checked = owner_request(
            &host,
            harness_kind(&host),
            vec!["/usr/bin/env".to_owned()],
            None,
        );
        let refused = checked.validate().unwrap_err();
        assert!(
            refused.contains("workload-refused: host-root declared but the owner is euid"),
            "{refused}"
        );
        match &checked.intent.unwrap().workload {
            Workload::HostRoot { ipc_dir, .. } => {
                assert_eq!(Path::new(ipc_dir), dir.path().join("launch/ipc"));
            }
            other => panic!("{other:?}"),
        }
        assert!(!dir.path().join("launch").exists() && !dir.path().join("store").exists());
        let unprivileged = request(Some(json!({ "isolation": "unprivileged-userns" }))).unwrap();
        owner_request(
            &unprivileged,
            harness_kind(&unprivileged),
            vec!["/usr/bin/env".to_owned()],
            None,
        )
        .validate()
        .unwrap();
    }
    /// The live-view grant is passed to the owner unchanged (the owner,
    /// not this entry, decides whether it names the attested requester);
    /// absent stays absent and a malformed one is refused before effects.
    #[test]
    fn live_output_grant_reaches_the_owner_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let request = |live: Option<Value>| {
            let mut value = json!({
                "store": dir.path().join("store"),
                "launch_dir": dir.path().join("launch"),
                "cwd": "/",
                "env": {},
                "messages": ["m"],
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "provider": provider_setup(),
                "workload": { "isolation": "unprivileged-userns" },
            });
            if let Some(live) = live {
                value["live_output"] = live;
            }
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        let owner = |request: &NativeRootRequest| {
            owner_request(
                request,
                harness_kind(request),
                vec!["/usr/bin/env".to_owned()],
                None,
            )
        };
        let granted = request(Some(json!({ "grant": "uid:1000" }))).unwrap();
        assert_eq!(
            owner(&granted).live_output,
            Some(LiveOutput(json!({"grant":"uid:1000"})))
        );
        assert_eq!(owner(&request(None).unwrap()).live_output, None);
        for malformed in [
            json!({"grant":"uid:1000", "uid":0}),
            json!(17),
            json!([false]),
        ] {
            let parsed = request(Some(malformed.clone())).unwrap();
            assert_eq!(owner(&parsed).live_output, Some(LiveOutput(malformed)));
        }
        assert!(!dir.path().join("launch").exists() && !dir.path().join("store").exists());
    }

    #[test]
    fn registered_request_refuses_embedded_shapes_before_effects() {
        let dir = tempfile::tempdir().unwrap();
        let base = json!({
            "store": dir.path().join("store"), "launch_dir": dir.path().join("launch"),
            "cwd": "/", "env": {}, "messages": ["m"],
            "outage_closure_cap": 1, "delivery_attempt_cap": 1,
            "provider": provider_setup(), "workload": {"isolation": "unprivileged-userns"}
        });
        let read = |value: Value| {
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        assert!(read(base.clone()).is_ok());
        for key in ["opencode", "claude", "auth", "model", "credential"] {
            let mut value = base.clone();
            value[key] = json!({});
            assert!(read(value).unwrap_err().contains("unknown field"), "{key}");
        }
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("provider");
        assert!(
            read(missing)
                .unwrap_err()
                .contains("missing field `provider`")
        );
        let mut null = base.clone();
        null["provider"] = Value::Null;
        assert!(read(null).is_err());
        let mut existing = base.clone();
        existing["store"] = json!(dir.path());
        assert!(read(existing).unwrap_err().contains("fresh only"));
        let mut owned = base;
        owned["env"] = json!({"OULIPOLY_ROOT_BASH_V1": "/x"});
        assert!(read(owned).unwrap_err().contains("owner's to set"));
        assert!(!dir.path().join("store").exists());
        assert!(!dir.path().join("launch").exists());
    }
}
