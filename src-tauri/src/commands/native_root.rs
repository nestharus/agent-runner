//! `native-root`: start one fresh native ACP v2 root (a registered external
//! provider's resident harness, or an embedded OpenCode or Claude Code
//! harness), or recover one this entry started, for cancel or attached
//! continuation (Linux, opt-in, source build).
//!
//! `--request <file>` starts a root. The request file names everything the
//! root gets: a new launch directory and a new store, the native setup
//! inputs, the messages, and **the whole environment** of the root. The
//! per-root owner (`oulipoly-root-supervisor`, next to this binary) is
//! started with only that environment; nothing of this process's
//! environment passes through. Root PID 1 inherits it, and in-root Bash
//! work runs in it unchanged. The native host gets it too, less what its
//! launch argv removes or overrides (its own HOME and XDG directories,
//! among others), plus the owner's ingress and socket variables. Stdout
//! says which names reach where, never a value.
//!
//! `workload` declares who the root's work runs as, never inferred from
//! this entry's euid: `{"isolation":"host-root","user":NAME}` (this entry,
//! and so the owner, runs as host root; the native host and every in-root
//! Bash run are started as host user NAME, which must not be uid 0) or
//! `{"isolation":"unprivileged-userns"}` (a non-root caller; not host-root
//! semantics). A declaration this process cannot honour is refused before
//! any effect. Under `host-root` the launch directory is handed to NAME as
//! the setup's module docs describe, the store stays the owner's private
//! directory, and the work's IPC (the harness socket directory and the
//! Bash ingress) is `<launch_dir>/ipc`, made fresh by the owner.
//!
//! `opencode.bash_allow` names the only whole commands the native host's
//! `bash` may run, the default form. `opencode.bash_authority:
//! "trusted-task"` instead lets it run any command for this task: the
//! caller's explicit once-per-task authority, never implied by anything
//! else and refused together with `bash_allow`. Either way every other
//! native tool is denied and every command goes through the root's Bash
//! ingress. `setup-completed` reports the effective policy (`launch.policy`,
//! with the native permission config as written).
//!
//! The request names exactly one harness: a registered external `provider`,
//! `opencode` (above and below) or `claude` (below). `provider`
//! (`executable`, `settings`, optional `config_root` and `env`,
//! `agent_bash_bin`, `bash_allow` or `bash_authority`): the provider's own
//! resident ACP v2 harness on stdio, resolved through what it declares and
//! prepares, as the [`registered`] module docs describe. Provider settings
//! stay opaque apart from neutral launch-env mediation. Runner supplies its
//! tool policy during policy evaluation through the SDK extension, for the
//! provider to translate. The owner labels the harness with the provider's
//! declared id. `claude` (`deps`, `node`, `agent_bash_bin`, `bash_allow` or
//! `bash_authority`, `model`, `effort`, `config_dir`): one native Claude
//! Code harness through the owner crate's ACP v2 receiver (`stdio`), as
//! its `native_claude` module docs describe. No credential passes: Claude
//! Code uses the work user's own login in `config_dir`, which this entry
//! never reads. Its policy is the same named-list or `trusted-task` choice;
//! `trusted-task` also offers the built-in Read, Write and Edit tools.
//! `opencode` and `claude` are embedded harnesses, to be retired once
//! registered providers can supply theirs.
//!
//! `opencode.auth` (opt-in, with a model) names a private OpenCode
//! `auth.json` that setup checks before any effect and places in the
//! launch's own data directory. Such a launch enables OpenCode's built-in
//! plugins and a loopback password inherited by the native host's requester.
//! Required auth fields are checked, not the full native schema. Setup adds
//! neither value to the root environment; caller-declared credentials can
//! reach mediated Bash. Both files are reachable by the work identity. This
//! entry does not refresh, copy back or remove them: the caller owns the
//! source file and the launch directory's copy after the root ends.
//!
//! `children` (opt-in, any harness) lets the root's harness ask the owner for registered
//! read-only children: `{"routes": {NAME: ROUTE}, "opencode"?: {"deps",
//! "agent_bash_tool", "agent_bash_bin"}, "auth"?, "max_starts",
//! "max_concurrent"}`. An OpenCode ROUTE is `{"model", "provider"}` (and
//! needs `opencode`): a native OpenCode host the owner provisions at its
//! admission under `<launch_dir>/children`, with the parent's `bash` policy
//! and `auth` (a private access-only OpenCode `auth.json`, read again for
//! each child; absent for a credential-free route). A registered ROUTE is
//! `{"registered": {"executable", "settings", "config_root"?, "env"?,
//! "agent_bash_bin"}, "slots"?}`: a registered provider, opaque to this
//! entry as for a root, whose tool policy is always the parent's `bash`
//! policy (it cannot name its own). Its executable's custody is checked and
//! pinned with the root's, before anything runs. After the root's own
//! harness setup, this entry describes and evaluates it and prepares
//! `slots` (1 to `max_starts`, default `max_starts`) fresh slots under
//! `<launch_dir>/child-slots/NAME/K`, each with its own data root handed to
//! the work identity, as for a registered root; any failure there is a
//! setup failure (73), the owner never started. The owner gives the route's
//! k-th admission (every owner generation counted) slot k, once, and
//! refuses the route once its slots are used; it runs that slot's
//! `resident.serve` as the child's harness in the child's own work
//! namespaces. An unused slot is setup, not a start or admission; this
//! entry starts no preparer at run time. A child is offered no routes and
//! the owner refuses its own child requests (depth 1). The parent's
//! `bash` policy is the child's configuration, not a write barrier: the
//! read-only brief is the child's task, not an enforced restriction. An
//! embedded harness gets an `explore` tool naming the routes; a registered
//! parent gets no exploration tool from this entry. Everything named is
//! checked before any effect; the owner enforces route, slot, depth,
//! budget and lineage. The caller owns `auth` and
//! the children's launch copies after the root ends, as for `opencode.auth`.
//! The packaged front door passes `children` only as its site allows a
//! parent route to offer them, stages `auth` once per root (an OpenCode
//! parent's own grant reused, or a Claude parent's separate child-provider
//! grant; never Claude's login), and removes it and every child launch's
//! copy when it retires the run. The packaged caller takes its answer and
//! close from the parent's own events only. Not yet installed.
//!
//! `--recover <file>` acts on an existing store: `{"store", "purpose",
//! "env"}`, `purpose` being `cancel` or `continue-attached`. A new owner
//! claims the store (the next owner generation; earlier unresolved
//! attempts become `unknown-prior-owner`) and positively attaches the
//! recorded root PID 1 if it is still that exact process. `cancel` then
//! has the root's live work killed by its own waiters, connecting to
//! nothing and delivering nothing; `continue-attached` reattaches the
//! survivors and resubmits what is owed with its original keys (at best
//! `duplicate-unknown`: not proof the native conversation continued).
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
//! * `0`: the owner ended (`ended`): every harness's end observed, nothing owed.
//! * `82` to `87`: the owner's own class 2 to 7 (`cancelled`, `ended-owed`,
//!   `incomplete` or `owned-unattached`, `authority-lost` or
//!   `store-failed`, `root-absent`, `closed`). `87` (`closed`) says the
//!   caller's close was followed through, its host ended by a kill: not
//!   that anything was processed. Owed work stays in the store, for an
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

use oulipoly_root_supervisor::children::{ChildPolicy, ChildRoute, PreparedSlot};
use oulipoly_root_supervisor::native::{
    BashAuthority, ExploreTool, OpenCodeSetup, OpenCodeSetupError, REMOVED_ENV, check_opencode,
    provision_opencode,
};
use oulipoly_root_supervisor::native_claude::{
    self, ClaudeSetup, ClaudeSetupError, Effort, provision_claude,
};
use oulipoly_root_supervisor::{Endpoint, HarnessSpec, Intent, Recover, Request, Workload};
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
    /// The native OpenCode setup inputs (`deps`, `agent_bash_tool`,
    /// `agent_bash_bin`, `bash_allow` or `bash_authority`, optional `model`
    /// and `provider`, and optional `auth`: see the module docs).
    #[serde(default)]
    opencode: Option<NativeSetup>,
    /// The native Claude Code setup inputs, instead of `opencode`.
    #[serde(default)]
    claude: Option<ClaudeRequest>,
    /// A registered external provider, instead of `opencode` or `claude`
    /// (see [`registered`]).
    #[serde(default)]
    provider: Option<registered::Registration>,
    /// Registered children the harness may ask for (see the module docs).
    #[serde(default)]
    children: Option<ChildrenRequest>,
    /// Who the root's work runs as (see the module docs).
    workload: RequestWorkload,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildrenRequest {
    routes: BTreeMap<String, ChildRouteRequest>,
    /// The OpenCode child launch inputs; required by an OpenCode route only.
    #[serde(default)]
    opencode: Option<ChildOpenCode>,
    #[serde(default)]
    auth: Option<String>,
    max_starts: u32,
    max_concurrent: u32,
}

/// One child route: an OpenCode `model` and `provider`, or a `registered`
/// provider with how many fresh `slots` to prepare (default `max_starts`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildRouteRequest {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<Map<String, Value>>,
    #[serde(default)]
    registered: Option<registered::ChildRegistration>,
    #[serde(default)]
    slots: Option<u32>,
}

impl ChildRouteRequest {
    /// The OpenCode route's model and provider, if it is one.
    fn opencode(&self) -> Option<(&String, &Map<String, Value>)> {
        self.model.as_ref().zip(self.provider.as_ref())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChildOpenCode {
    deps: String,
    agent_bash_tool: String,
    agent_bash_bin: String,
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeSetup {
    deps: String,
    agent_bash_tool: String,
    agent_bash_bin: String,
    #[serde(default)]
    bash_allow: Vec<String>,
    #[serde(default)]
    bash_authority: Option<BashAuthority>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<Map<String, Value>>,
    #[serde(default)]
    auth: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeRequest {
    deps: String,
    node: String,
    agent_bash_bin: String,
    #[serde(default)]
    bash_allow: Vec<String>,
    #[serde(default)]
    bash_authority: Option<BashAuthority>,
    model: String,
    effort: Effort,
    config_dir: String,
}

/// One provisioned harness, whichever kind.
struct Provisioned {
    argv: Vec<String>,
    receipt: Value,
    /// Names the harness's own launch sets for itself.
    set: Vec<String>,
    /// Names the harness's launch removes.
    removed: Vec<&'static str>,
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
    // Each child route's setup inputs, checked now (no write), so that an
    // unusable child launch refuses the root before any effect rather than
    // only when the harness first asks.
    if let Some(children) = checked
        .intent
        .as_ref()
        .and_then(|intent| intent.children.as_ref())
    {
        for (name, route) in &children.routes {
            let setup = route.opencode_setup(format!("{}/check", children.launch_base));
            if let Some(setup) = setup
                && let Err(reason) = check_opencode(&setup)
            {
                return Ok(refused(&out, format!("children: route {name}: {reason}")));
            }
        }
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
    let root_admitted = match &request.provider {
        Some(registration) => {
            match registered::check(registration).and_then(|()| registered::admit(registration)) {
                Ok(admitted) => Some((registration, admitted)),
                Err(reason) => return Ok(refused(&out, reason)),
            }
        }
        None => None,
    };
    let child_registrations = child_registrations(&request);
    let children = match admit_children(&child_registrations) {
        Ok(children) => children,
        Err(reason) => return Ok(refused(&out, reason)),
    };
    if let Some((registration, admitted)) = root_admitted {
        return Ok(run_registered(
            &out,
            &request,
            registration,
            admitted,
            children,
            &owner,
            &workload,
        ));
    }
    let provisioned = match provision(&request, workload.identity.as_ref()) {
        Ok(provisioned) => provisioned,
        Err((false, reason)) => return Ok(refused(&out, format!("setup: {reason}"))),
        Err((true, reason)) => {
            out.entry(json!({
                "entry": "terminal",
                "stage": "setup-failed",
                "reason": reason,
                "launch_dir": request.launch_dir,
                "effects": "possible",
                "retry": "do-not-replay",
            }));
            return Ok(out.exit(EXIT_SETUP_FAILED));
        }
    };
    let (child_routes, child_receipt) =
        match prepare_children(&out, &request, children, workload.identity.as_ref()) {
            Ok(prepared) => prepared,
            Err(reason) => {
                return Ok(registered_failure(
                    &out,
                    &request,
                    registered::Failure::Setup {
                        provider: true,
                        reason,
                    },
                ));
            }
        };
    let set: Vec<&str> = provisioned.set.iter().map(String::as_str).collect();
    let mut reach = env_reach(
        &request.env,
        &set,
        &provisioned.removed,
        harness_kind(&request).1,
    );
    if request.claude.is_some() {
        reach["claude_code"] = claude_code_reach(&request.env);
    }
    out.entry(json!({
        "entry": "setup-completed",
        "harness": harness_kind(&request).0,
        "launch": provisioned.receipt,
        "children": child_receipt,
        "owner": owner,
        "env": reach,
        "workload": workload_report(&workload),
    }));
    let context = json!({
        "store": request.store,
        "launch_dir": request.launch_dir,
        "setup": "retained",
        "retry": "do-not-replay",
    });
    let owner_request = owner_request(
        &request,
        harness_kind(&request),
        provisioned.argv,
        Some(&child_routes),
    );
    Ok(start_owner(
        &out,
        &owner,
        &request.env,
        &owner_request,
        &context,
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
    let owner_request = owner_request(
        request,
        (declared.provider_id.as_str(), Endpoint::Stdio),
        prepared.argv,
        Some(&child_routes),
    );
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

/// Provisions the request's one harness. `Err((false, _))`: refused before
/// any setup write; `Err((true, _))`: construction failed, effects possible.
fn provision(
    request: &NativeRootRequest,
    identity: Option<&oulipoly_root_supervisor::workload::Identity>,
) -> Result<Provisioned, (bool, String)> {
    if let Some(claude) = &request.claude {
        let setup = ClaudeSetup {
            dir: request.launch_dir.clone(),
            deps: claude.deps.clone(),
            node: claude.node.clone(),
            agent_bash_bin: claude.agent_bash_bin.clone(),
            bash_allow: claude.bash_allow.clone(),
            bash_authority: claude.bash_authority,
            model: claude.model.clone(),
            effort: claude.effort,
            config_dir: claude.config_dir.clone(),
            start_timeout_s: None,
            ack_timeout_s: None,
            explore: explore_tool(request),
        };
        return match provision_claude(&setup, identity) {
            Ok(launch) => Ok(Provisioned {
                receipt: launch.to_json(),
                argv: launch.argv,
                set: Vec::new(),
                removed: native_claude::REMOVED_ENV.to_vec(),
            }),
            Err(ClaudeSetupError::InputInvalid(reason)) => Err((false, reason)),
            Err(ClaudeSetupError::ConstructionFailed(reason)) => Err((true, reason)),
        };
    }
    let opencode = request.opencode.as_ref().expect("checked: one harness");
    let setup = OpenCodeSetup {
        dir: request.launch_dir.clone(),
        deps: opencode.deps.clone(),
        agent_bash_tool: opencode.agent_bash_tool.clone(),
        agent_bash_bin: opencode.agent_bash_bin.clone(),
        bash_allow: opencode.bash_allow.clone(),
        bash_authority: opencode.bash_authority,
        model: opencode.model.clone(),
        provider: opencode.provider.clone(),
        auth: opencode.auth.clone(),
        explore: explore_tool(request),
    };
    match provision_opencode(&setup, identity) {
        Ok(launch) => Ok(Provisioned {
            receipt: launch.to_json(),
            set: launch.env.iter().map(|(name, _)| name.clone()).collect(),
            argv: launch.argv,
            removed: REMOVED_ENV.to_vec(),
        }),
        Err(OpenCodeSetupError::InputInvalid(reason)) => Err((false, reason)),
        Err(OpenCodeSetupError::ConstructionFailed(reason)) => Err((true, reason)),
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

/// The parent's `explore` tool, when the request allows children.
fn explore_tool(request: &NativeRootRequest) -> Option<ExploreTool> {
    request.children.as_ref().map(|children| ExploreTool {
        routes: children.routes.keys().cloned().collect(),
        max_starts: children.max_starts,
        max_concurrent: children.max_concurrent,
    })
}

/// The parent harness's `bash` policy, which its children inherit.
fn bash_policy(request: &NativeRootRequest) -> (Vec<String>, Option<BashAuthority>) {
    match (&request.opencode, &request.claude, &request.provider) {
        (Some(opencode), _, _) => (opencode.bash_allow.clone(), opencode.bash_authority),
        (None, Some(claude), _) => (claude.bash_allow.clone(), claude.bash_authority),
        (None, None, Some(provider)) => (provider.bash_allow.clone(), provider.bash_authority),
        (None, None, None) => (Vec::new(), None),
    }
}

/// The intent's child policy, from the request's `children`. A registered
/// route is its `prepared` slots; before they are prepared (the owner's
/// checks before any effect), the slots planned for it.
fn child_policy(
    request: &NativeRootRequest,
    prepared: Option<&BTreeMap<String, ChildRoute>>,
) -> Option<ChildPolicy> {
    let children = request.children.as_ref()?;
    let (bash_allow, bash_authority) = bash_policy(request);
    Some(ChildPolicy {
        routes: children
            .routes
            .iter()
            .map(|(name, route)| {
                let launch = match (route.opencode(), &children.opencode) {
                    (Some((model, provider)), Some(opencode)) => ChildRoute::Opencode {
                        deps: opencode.deps.clone(),
                        agent_bash_tool: opencode.agent_bash_tool.clone(),
                        agent_bash_bin: opencode.agent_bash_bin.clone(),
                        bash_allow: bash_allow.clone(),
                        bash_authority,
                        model: model.clone(),
                        provider: provider.clone(),
                        auth: children.auth.clone(),
                    },
                    _ => match prepared.and_then(|prepared| prepared.get(name)) {
                        Some(prepared) => prepared.clone(),
                        None => ChildRoute::Prepared {
                            provider: "registered-provider".to_owned(),
                            slots: (0..slot_count(children, route))
                                .map(|index| PreparedSlot {
                                    argv: vec!["/usr/bin/env".to_owned()],
                                    data_root: slot_dir(request, name, index)
                                        .join("provider")
                                        .to_string_lossy()
                                        .into_owned(),
                                })
                                .collect(),
                            endpoint: Endpoint::Stdio,
                        },
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
        .filter_map(|(name, route)| {
            route.registered.as_ref().map(|registration| {
                (
                    name.clone(),
                    slot_count(children, route),
                    registration.with_policy(bash_allow.clone(), bash_authority),
                )
            })
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
                .and_then(|()| registered::admit(registration))
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
                "exploration": "none offered to the child; the owner refuses a child's own child request (depth 1)",
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
fn harness_kind(request: &NativeRootRequest) -> (&'static str, Endpoint) {
    if request.provider.is_some() {
        ("registered-provider", Endpoint::Stdio)
    } else if request.claude.is_some() {
        ("claude", Endpoint::Stdio)
    } else {
        ("opencode", Endpoint::UnixSocket)
    }
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
    let harnesses = [
        request.opencode.is_some(),
        request.claude.is_some(),
        request.provider.is_some(),
    ];
    if harnesses.into_iter().filter(|named| *named).count() != 1 {
        return Err("request names exactly one of opencode, claude and provider".to_owned());
    }
    if let Some(children) = &request.children {
        check_child_routes(children)?;
    }
    Ok(request)
}

/// Each child route is either OpenCode-shaped (`model` and `provider`,
/// with `children.opencode`) or `registered` (with at most `max_starts`
/// slots), never both.
fn check_child_routes(children: &ChildrenRequest) -> Result<(), String> {
    for (name, route) in &children.routes {
        let opencode = route.model.is_some() || route.provider.is_some();
        match (route.opencode(), &route.registered) {
            (Some(_), None) if route.slots.is_none() => {
                if children.opencode.is_none() {
                    return Err(format!(
                        "children: route {name}: an OpenCode route needs children.opencode"
                    ));
                }
            }
            (None, Some(_)) if !opencode => {
                if route
                    .slots
                    .is_some_and(|slots| !(1..=children.max_starts).contains(&slots))
                {
                    return Err(format!(
                        "children: route {name}: slots must be 1..=max_starts"
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "children: route {name}: name model and provider (OpenCode) or registered (with optional slots)"
                ));
            }
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

/// Which declared names the Claude receiver withholds from Claude Code
/// itself (names only), besides its own launch settings.
fn claude_code_reach(declared: &BTreeMap<String, String>) -> Value {
    let withheld = |name: &str| {
        [
            "ANTHROPIC_",
            "CLAUDE",
            "OULIPOLY_",
            "AGENT_BASH_",
            "NODE_",
            "OTEL_",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
            || [
                "AWS_BEARER_TOKEN_BEDROCK",
                "ENABLE_TOOL_SEARCH",
                "DEBUG_CLAUDE_AGENT_SDK",
                "DEBUG",
            ]
            .contains(&name)
    };
    let names: Vec<&str> = declared.keys().map(String::as_str).collect();
    json!({
        "withheld": names.iter().copied().filter(|name| withheld(name)).collect::<Vec<_>>(),
        "inherited": names.iter().copied().filter(|name| !withheld(name)).collect::<Vec<_>>(),
        "set_by_launch": "launch.claude_env_set",
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
            ("OPENCODE_CONFIG_CONTENT".to_owned(), "{}".to_owned()),
            ("TOKEN".to_owned(), "secret-value".to_owned()),
        ]);
        let reach = env_reach(
            &declared,
            &["HOME", "XDG_CONFIG_HOME"],
            &REMOVED_ENV,
            Endpoint::UnixSocket,
        );
        assert_eq!(
            reach["declared"],
            json!(["HOME", "OPENCODE_CONFIG_CONTENT", "TOKEN"])
        );
        assert_eq!(reach["native_host"]["inherited"], json!(["TOKEN"]));
        assert_eq!(
            reach["native_host"]["overridden_by_launch"],
            json!(["HOME"])
        );
        assert_eq!(
            reach["native_host"]["removed_by_launch"],
            json!(["OPENCODE_CONFIG_CONTENT"])
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

    #[test]
    fn request_refuses_existing_paths_owner_variables_and_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh");
        let base = |store: &Path, env: Value| {
            json!({
                "store": store,
                "launch_dir": dir.path().join("launch"),
                "cwd": "/",
                "env": env,
                "messages": ["m"],
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "opencode": {
                    "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b",
                    "bash_allow": ["true"],
                },
                "workload": { "isolation": "unprivileged-userns" },
            })
        };
        let read = |value: Value| {
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        assert!(read(base(&fresh, json!({ "PATH": "/usr/bin" }))).is_ok());
        let existing = read(base(dir.path(), json!({}))).unwrap_err();
        assert!(existing.contains("fresh only"), "{existing}");
        let owned = read(base(&fresh, json!({ "OULIPOLY_ROOT_BASH_V1": "/x" }))).unwrap_err();
        assert!(owned.contains("owner's to set"), "{owned}");
        let mut extra = base(&fresh, json!({}));
        extra["inherit_env"] = json!(true);
        assert!(read(extra).unwrap_err().contains("unknown field"));
        let mut authed = base(&fresh, json!({}));
        authed["opencode"]["auth"] = json!("/private/auth.json");
        assert_eq!(
            read(authed).unwrap().opencode.unwrap().auth.as_deref(),
            Some("/private/auth.json")
        );
        // Exactly one harness kind.
        let mut both = base(&fresh, json!({}));
        both["claude"] = claude_setup();
        assert!(read(both).unwrap_err().contains("exactly one"), "both");
        let mut neither = base(&fresh, json!({}));
        neither.as_object_mut().unwrap().remove("opencode");
        assert!(
            read(neither).unwrap_err().contains("exactly one"),
            "neither"
        );
        let mut claude = base(&fresh, json!({}));
        claude.as_object_mut().unwrap().remove("opencode");
        claude["claude"] = claude_setup();
        let request = read(claude.clone()).unwrap();
        assert_eq!(harness_kind(&request), ("claude", Endpoint::Stdio));
        let owner = owner_request(
            &request,
            harness_kind(&request),
            vec!["/usr/bin/env".to_owned()],
            None,
        );
        let spec = &owner.intent.as_ref().unwrap().harnesses[0];
        assert_eq!(
            (spec.id.as_str(), spec.endpoint),
            ("claude", Endpoint::Stdio)
        );
        claude["claude"]["credential"] = json!("x");
        assert!(read(claude).unwrap_err().contains("unknown field"));
    }

    /// A registered provider is a third harness kind, exclusive of the
    /// embedded ones; its registration is checked when resolved, not here.
    #[test]
    fn request_names_a_registered_provider_instead_of_an_embedded_harness() {
        let dir = tempfile::tempdir().unwrap();
        let read = |value: Value| {
            let path = dir.path().join("request.json");
            std::fs::write(&path, value.to_string()).unwrap();
            read_request(&path)
        };
        let base = json!({
            "store": dir.path().join("store"),
            "launch_dir": dir.path().join("launch"),
            "cwd": "/",
            "env": {},
            "messages": ["m"],
            "outage_closure_cap": 1,
            "delivery_attempt_cap": 1,
            "provider": {
                "executable": "/opt/provider/bin/provider",
                "settings": { "settings_id": "s" },
                "agent_bash_bin": "/opt/agent-bash",
                "bash_allow": ["git status"],
            },
            "workload": { "isolation": "unprivileged-userns" },
        });
        let request = read(base.clone()).unwrap();
        assert_eq!(
            harness_kind(&request),
            ("registered-provider", Endpoint::Stdio)
        );
        assert_eq!(bash_policy(&request), (vec!["git status".to_owned()], None));
        let policy = serde_json::to_value(request.provider.as_ref().unwrap().mediation()).unwrap();
        assert_eq!(policy["bash"], json!({ "allow": ["git status"] }));
        assert_eq!(policy["requester"], "/opt/agent-bash");
        assert_eq!(
            policy["ingress_env"],
            oulipoly_root_supervisor::bash::BASH_ENV
        );
        assert_eq!(
            policy["protocol"],
            agent_provider_contract::tool_mediation::PROTOCOL
        );
        assert!(policy.get("explore").is_none());
        let owner = owner_request(
            &request,
            ("fake-external", Endpoint::Stdio),
            vec!["/p".to_owned()],
            None,
        );
        let spec = &owner.intent.as_ref().unwrap().harnesses[0];
        assert_eq!(
            (spec.id.as_str(), spec.endpoint),
            ("fake-external", Endpoint::Stdio)
        );
        let mut with_claude = base.clone();
        with_claude["claude"] = claude_setup();
        assert!(read(with_claude).unwrap_err().contains("exactly one"));
        let mut native_fields = base.clone();
        native_fields["provider"]["model"] = json!("m");
        assert!(read(native_fields).unwrap_err().contains("unknown field"));
        let mut children = base;
        children["children"] = json!({
            "routes": { "luna": { "model": "openai/luna", "provider": { "openai": {} } } },
            "opencode": { "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b" },
            "max_starts": 2, "max_concurrent": 1,
        });
        let request = read(children).unwrap();
        assert_eq!(
            serde_json::to_value(explore_tool(&request)).unwrap(),
            json!({ "routes": ["luna"], "max_starts": 2, "max_concurrent": 1 })
        );
    }

    /// A child route is OpenCode-shaped or registered, never both; a
    /// registered child takes the parent's tool policy (it cannot name its
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
            "claude": claude_setup(),
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
        // The Claude parent's trusted-task authority, inherited.
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
        // The parent gets the route offered; the owner's checks pass.
        assert_eq!(
            explore_tool(&request).unwrap().routes,
            vec!["luna".to_owned()]
        );
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
            "name model and provider (OpenCode) or registered",
        );
        refuse(
            &|v| {
                v["children"]["routes"]["oc"] =
                    json!({ "model": "openai/luna", "provider": { "openai": {} } })
            },
            "an OpenCode route needs children.opencode",
        );
        refuse(
            &|v| {
                v["children"]["routes"]["oc"] =
                    json!({ "model": "openai/luna", "provider": { "openai": {} }, "slots": 1 })
            },
            "name model and provider",
        );
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

    fn claude_setup() -> Value {
        json!({
            "deps": "/d", "node": "/n", "agent_bash_bin": "/b",
            "bash_authority": "trusted-task",
            "model": "claude-opus-5-5", "effort": "medium",
            "config_dir": "/home/nes/.claude5",
        })
    }

    /// The Claude receiver's withheld names are reported by name only.
    #[test]
    fn claude_code_reach_names_withheld_credentials_never_values() {
        let declared = BTreeMap::from([
            ("ANTHROPIC_API_KEY".to_owned(), "secret-key".to_owned()),
            (
                "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
                "secret-token".to_owned(),
            ),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
        ]);
        let reach = claude_code_reach(&declared);
        assert_eq!(
            reach["withheld"],
            json!(["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"])
        );
        assert_eq!(reach["inherited"], json!(["PATH"]));
        let full = env_reach(&declared, &[], &native_claude::REMOVED_ENV, Endpoint::Stdio);
        assert_eq!(
            full["native_host"]["removed_by_launch"],
            json!(["ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN"])
        );
        assert!(!reach.to_string().contains("secret"));
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
                "opencode": {
                    "deps": "/d", "agent_bash_tool": "/t", "agent_bash_bin": "/b",
                    "bash_allow": ["true"],
                },
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
}
