//! One host-side per-root owner **process** for one root tree: it keeps
//! that root's PID 1 and one PID 1 per work in new PID namespaces, delivers
//! the messages it originates to its ACP v2 harnesses through
//! [`oulipoly_acp::AcpClient`], and keeps that root's delivery intent and
//! custody record in its own private durable store, so that a restarted
//! owner continues what an earlier one owed and takes back the harnesses
//! that outlived it. Linux only.
//!
//! # What this is
//!
//! A real launchable OS process (the `oulipoly-root-supervisor` binary)
//! owning every harness of one root concurrently, never one wrapper per
//! agent. It is **unwired**: nothing on the installed route launches it,
//! and it touches no Broker State, opcode, admission record, global debt or
//! fence, guardian, driver or Bash path. Its harnesses in this crate's tests
//! are the deterministic peer binary, not a real harness, except one
//! ignored-by-default test (`tests/native_opencode.rs`) that owns a real,
//! model-less OpenCode host (see Endpoints). Its one caller is the Runner's
//! opt-in `native-root` entry (Linux, source build), which provisions a
//! native OpenCode host with [`native::provision_opencode`] and starts this
//! process for one fresh root with only the environment the request
//! declares.
//!
//! Absorption target: later slices make this lineage the root's
//! harness-delivery owner, replacing the resume-plus-prompt path for the
//! harnesses it owns, and wire its entry and restart trigger into launch
//! and communication. If that lineage is abandoned, this crate is deleted
//! rather than kept as a fallback.
//!
//! # Placement and custody
//!
//! Processes of one root, and nothing else:
//!
//! * the **owner** (this process; ephemeral, one at a time per root);
//! * **root PID 1** (`oulipoly-root-pid1`), started by the owner as its
//!   direct child in a new PID namespace on the first launch;
//! * per work: one **work PID 1** in its own nested PID namespace, child of
//!   root PID 1, and its **harness**, child of the work PID 1.
//!
//! The infrastructure count per root is the owner plus root PID 1 whatever
//! the number of agents; each agent adds only its work PID 1 and its
//! harness. There is no additional stable host process and no per-agent
//! wrapper: root PID 1 itself holds the owner-side ends of every harness's
//! stdio pipes and hands copies to the attached owner (`SCM_RIGHTS`), so the
//! owner's death is not end of stream for any harness.
//!
//! Host root: a new PID namespace. Unprivileged (any other uid): a new user
//! namespace mapping only the caller's uid/gid to 0, with the PID
//! namespaces inside it; the `isolation` field says which. Unprivileged
//! runs do not stand for host-root semantics.
//!
//! **Owner death kills nothing.** No process here has a parent-death signal
//! or a timer. When the owner dies, root PID 1 is reparented outside the
//! owner (to the host's init or a subreaper) and keeps its work and the
//! harnesses' stdio. It exits by itself only once it has neither live work
//! nor an attached owner, after recording what its children's waits
//! reported (`pid1-<incarnation>.receipts` in the store).
//!
//! **Terminal honesty.** A harness's exit status comes only from its work
//! PID 1's actual wait; a work PID 1's from root PID 1's actual wait; root
//! PID 1's only from its parent's wait, which is this owner's only if this
//! owner started it. Otherwise an exit is at most `exit-observed-by-pidfd`
//! with no status, and a process found gone without a report is
//! `absent-exit-not-observed`. A harness whose root PID 1 is gone without a
//! report for it ended with that namespace, status unknown; so did one whose
//! work PID 1 was waited but ended before reporting the harness's wait. No
//! `waitpid` is made for a non-child, and a bare pid is never adopted. A
//! missing reply from root PID 1 is never read as an empty live list or an
//! end.
//!
//! **Identity and scope.** The root's identity is its store directory plus
//! a random `root_id` minted with the intent; an agent's identity is a
//! harness of that root, so it belongs to one root lineage and outlives
//! owners and root PID 1 incarnations. Each root PID 1 incarnation is
//! recorded before it starts, with an attestation token, then with its exact
//! identity (host pid, start time, boot id). Root PID 1 admits an owner only
//! by positive attribution: same uid, that incarnation's token from the
//! private store, and an owner generation newer than any admitted (the
//! store's generation fence); anything else is refused
//! (`peer-unattributed`, `stale-generation`). A newer owner supersedes an
//! older one; for the superseded owner that is authority loss at run level,
//! learned from the root rather than from a store refusal (see Ending). Two
//! roots share nothing.
//!
//! **Bash.** Bash work started inside one of this owner's harness
//! namespaces reaches this owner, and only this owner, through the root's
//! Bash ingress (the [`bash`] module): `<store>/bash.sock`, named to every
//! harness and its descendants in [`bash::BASH_ENV`]. A peer is accepted
//! only by positive attribution (same uid, and a member of the exact PID
//! namespace of one live harness work of this owner); anything else is
//! refused and reported (`bash-refused`), never routed to the old Broker,
//! another root or a fallback. One request per connection, one JSON line:
//! `{"v":1,"op":"run","argv":[...],"cwd":"/abs"}`. Replies, one JSON line
//! each: `refused` (nothing recorded or run), or `accepted` (durably
//! recorded, with the requesting harness, session and the owner inputs open
//! on it; not a start), `started`, `output` (`b64`), `output-closed` (end of
//! stream, not an end) or `output-failed`, then exactly one of `end` (status
//! from its work PID 1's wait, with separate output state), `end-unknown`,
//! `launch-failed` (positive no-start reply), `launch-unknown` (possible
//! effects, no automatic retry), or `left-to-successor`.
//! Each accepted run is its own work under root PID 1, so it is killed on
//! cancel, survives owner death like any work, and the run does not end
//! until its end (or why it is unknown) is reported. Delivery is in-band to
//! the requester only: no completion is supplied to any harness as a new
//! input. Nested requests (from inside a Bash run's own namespace) are
//! refused: they are not inside a harness namespace.
//!
//! **Per-input attribution.** Inputs are still submitted as soon as the
//! previous one is acknowledged, so several can be open on one native
//! session. Agent output names the input it answers only through the
//! agent's own `oulipoly.ai/parentMessageId` tag (`input_attribution`),
//! and `turn-end` is reported for an input only from an idle the agent
//! tagged with `oulipoly.ai/lastUserMessageId` at or after that input
//! (native ids ascend), with `own_output` saying whether any output named
//! it. This is idle-tag coverage, not proof of native processing; the
//! changed native parent/multipart paths remain unwitnessed. An untagged
//! idle is still only readiness. A Bash run's `accepted`
//! says which inputs were open (`single-open-input`, `ambiguous-open-inputs`
//! or `no-open-input`); nothing finer is known.
//!
//! # Interface
//!
//! The first stdin line is a JSON [`Request`]: `{"store": dir, "intent":
//! {...}}` creates a root's intent in a new store, and `{"store": dir}`
//! recovers an existing one. A recovery may name what it is for
//! (`"recover"`, see [`Recover`]): `cancel` or `continue-attached`. Either
//! acts only on the surviving recorded root PID 1 it positively attaches,
//! never starts a new incarnation, and finding none reports
//! `root-absent` ([`EXIT_ROOT_ABSENT`]) with what the store still owes.
//! Later stdin lines are control commands;
//! `{"cmd":"cancel"}` is the only one. Stdin EOF is not a cancel. Stdout
//! carries one JSON object per line: progress events, then exactly one
//! `"event":"terminal"` report. Exit codes: [`EXIT_ENDED`],
//! [`EXIT_ENDED_OWED`], [`EXIT_CANCELLED`], [`EXIT_INCOMPLETE`],
//! [`EXIT_STORE_LOST`], [`EXIT_ROOT_ABSENT`], [`EXIT_SPEC_REFUSED`],
//! [`EXIT_STORE_REFUSED`].
//!
//! # Endpoints
//!
//! Each harness speaks ACP v2 either on its stdio (`"endpoint": "stdio"`,
//! the default) or on a Unix socket it listens on itself
//! (`"unix-socket"`), so a native host serves ACP from inside the process
//! that owns its conversations, with no bridge process. The owner chooses
//! the socket path per work (`<store>/acp/w<work>.sock`, refused at intent
//! validation if the store path is too long for it) and passes it as
//! `OULIPOLY_ACP_V2_SOCKET`; root PID 1 starts every harness in the
//! intent's `cwd`. Custody is unchanged: the harness is still the child of
//! its work PID 1, its end is still known only from that wait, and its
//! stdout (drained, not parsed) still passes through root PID 1. The owner
//! connects when the socket listens; a harness that never listens is waited
//! for like a silent one. End of stream on the socket is the connection's
//! end (`connection-closed`), never the harness's. A restarted owner
//! reconnects to a survivor's socket. The socket file is removed once the
//! harness's end is observed.
//!
//! A harness may name an existing native conversation (`"session"`): the
//! owner resumes it instead of opening one; the harness decides whether it
//! has it. `session-opened` / `session-resumed` report the session id and
//! `ack` the harness's `messageId`, which the store also keeps.
//!
//! `native/opencode/acp-v2-endpoint.ts` is such an endpoint for OpenCode, as
//! a server plugin. In OpenCode 1.18.30 only `opencode acp` loads its
//! directory's instance, and so the plugin, at startup (`serve` does so per
//! HTTP request); its own stdio ACP surface is then present but unused.
//!
//! # Durable store and ownership
//!
//! `store` is one root's private directory (created `0700`; an existing one
//! must be owned by this user and not group/other accessible). It holds an
//! owner lock file and one SQLite database; see the `store` module. One
//! store per root: nothing in it is shared with another root, and global
//! coordination stays where it is.
//!
//! * A second live instance for the same store is refused (`owner-live`)
//!   before it opens the database.
//! * Each instance claims a durable owner generation. Every later write
//!   first checks it is still the latest; a stale instance is refused
//!   (`authority-lost`) before changing anything, and then stops its own
//!   harnesses rather than deliver what it can no longer record.
//! * The premise is a cooperative local administrator. File modes, the lock
//!   and the generation answer accidental stale or duplicate instances; they
//!   do not prove that no other process can write the file.
//!
//! # Five stages, kept distinct
//!
//! 1. **Accepted by interface**: `intent-received`, emitted after the request
//!    is parsed and validated and before anything is written. Not durable.
//! 2. **Durably committed**: `intent-committed`, emitted only after the
//!    transaction holding the intent and every minted message key has
//!    committed. A crash before that leaves no intent; a recover request
//!    then reports `no-durable-intent`. Durability target: survives this
//!    process's crash and an OS/host crash. Means: WAL journal with
//!    `synchronous=FULL` (each commit syncs the WAL before returning), plus
//!    an fsync of the store directory and its parent after the claim, before
//!    the event. The parent sync persists a newly created root directory's
//!    entry and also runs on recovery/retry. Host-crash survival assumes
//!    durably provisioned ancestors and storage that honours fsync; actual
//!    host/power crash is not tested here.
//!    There is no batching: each transition is its own commit.
//! 3. **Attempted delivery**: an attempt row is committed **before** each
//!    `session/prompt` is sent.
//! 4. **Consumption ACK**: the insertion acknowledgement is committed before
//!    its `ack` event. An ACK received but not recorded is not reported.
//! 5. **Transition completion**: never observed (the ACP draft has no
//!    completion correlation); reported as `completion:"not-observed"`.
//!
//! # Restart recovery
//!
//! An explicit recover request for the same root (`{"store": dir}`) is the
//! restart trigger. The new owner first looks for the latest recorded root
//! PID 1 incarnation not known to have ended:
//!
//! * still exactly that process and admitting this owner: **attached**. Each
//!   surviving harness is **reattached** (same process, its stdio from root
//!   PID 1): the owner initializes again, resumes the recorded session and
//!   resubmits what is owed with the original keys. Ends reported while no
//!   owner was attached are reported as such (`prior-exit`, with their
//!   waiter) and count as closures like any observed exit. A report with
//!   only the work PID 1's status, no harness wait, is `prior-end-unknown`
//!   (`ended-with-work-namespace-status-unknown`): not a closure, and the
//!   message is launched again within its attempt budget.
//! * not running (gone, or its pid now names another process): **absent**,
//!   recorded as exactly what was observed. Its harnesses ended with it,
//!   status unknown unless it recorded a report (`prior-end-unknown`, not a
//!   closure). The next launch starts a new incarnation under the same root
//!   identity.
//! * possibly running but not attachable: **owned-unattached**. Nothing is
//!   launched, killed or declared ended; the run reports `owned-unattached`
//!   (exit 4) and owed messages stay in the store.
//!
//! The recovering instance takes the next owner generation and, in the same
//! transaction, labels every earlier-generation attempt that has no recorded
//! outcome `unknown-prior-owner`: it may or may not have been sent or
//! inserted. That is neither an acknowledgement nor a closure: the earlier
//! owner's death is not an observed harness exit. It then resumes the
//! recorded session and resubmits each owed message with its **original**
//! key. A restored key is a supplied identity with unknown history, so an
//! ACK after a restart is at best `duplicate-unknown`, with `recovered`
//! when a dedup-contract receiver says it returned an earlier insertion.
//! Durable storage does not make a restored key or session string a
//! receiver-continuity proof.
//!
//! Attempt counts, closures and the `outage` / `attempts-exhausted` stops
//! persist, so a restart never resets either cap. They are separate facts:
//! a closure is an observed harness exit, while an attempt is a durable
//! reservation committed before a send: it may or may not have been sent,
//! including when its owner died before recording an outcome.
//! [`Intent::delivery_attempt_cap`] bounds attempts per message over every
//! generation, so a loop of owner restarts with unresolved outcomes cannot
//! retry forever; when it is used up the message stops as
//! `attempts-exhausted` with its unknown attempts still unknown, not as an
//! outage, a closure or an acknowledgement. A surviving harness with
//! nothing left to deliver is still held (`holding-survivor`) until its end
//! is reported or the caller cancels; it is never dropped as gone.
//!
//! # Labels
//!
//! * `accepted` / `duplicate-unknown`: a durably recorded insertion
//!   acknowledgement from the harness, with or without an at-most-once
//!   basis. Insertion only: not turn completion, not drain, and never an end
//!   condition by itself.
//! * An owed message keeps its reason: `cancelled`, `outage`, `rejected`,
//!   `not-negotiated:*`, `session-*`, `invalid-response`, `launch-failed`,
//!   `wait-unproven`, `authority-lost`, `store-failed`, `not-attempted`,
//!   `attempts-exhausted`. Only `outage` and `attempts-exhausted` are durable
//!   stops; the others end this instance's attempts and a later recovery
//!   retries the message.
//! * A **closure** is a no-acknowledgement end whose harness exit was
//!   reported by that harness's actual waiter (its work PID 1). Only closures
//!   count toward [`Intent::outage_closure_cap`]. Rejections and negotiation
//!   failures keep their own labels and are not closures. A send fault alone
//!   is labelled `send-fault`, never `PeerGone`; it becomes a closure only
//!   once the harness's exit is observed. Silence is never a closure: a quiet
//!   live harness stays owed and in flight until it answers, exits, or the
//!   caller cancels. No timer ends or kills anything.
//! * After a closure the harness is relaunched, the session resumed, and the
//!   **same** message (same key) submitted again.
//! * `idle` events are readiness since the session's first tracked attempt,
//!   not completion of the latest message.
//!
//! # Ending
//!
//! * `ended`: every launched or reattached harness's end was reported by
//!   its waiter, and nothing is owed. Root PID 1 is then released, and its
//!   end observed; if its end is not observed the run is `incomplete`.
//! * `ended-owed`: every launched or reattached harness's end was reported,
//!   no further attempt is authorized in this instance, and debt remains,
//!   retained in the store (exit 3).
//! * `cancelled`: explicit cancellation is visible at run level. Each live
//!   harness is killed by its own work PID 1 (exact, unreaped child); only
//!   the waiters' reports prove the ends. Earlier refusal/outage causes
//!   survive cancellation. Cancel ends this instance; it does not withdraw
//!   durable intent.
//! * `authority-lost` / `store-failed`: a store write was refused or failed
//!   (exit 5). On store failure this owner has its harnesses killed. On
//!   authority loss it kills nothing and only detaches: a newer owner holds
//!   the root and its live work. Root PID 1 reporting this owner
//!   `superseded` is authority loss too, even when no store write was
//!   refused (for example, everything already acknowledged); the owner then
//!   returns without waiting for the root or its work, and work it adopts
//!   after that is left to the successor at once. What the store holds is
//!   authoritative, this instance's view is not. Authority loss outranks
//!   store failure, which outranks cancellation, regardless of arrival
//!   order.
//! * `owned-unattached`: see restart recovery (exit 4).
//! * `root-absent`: a recovery naming its purpose found no root PID 1 to
//!   attach (gone, or none recorded). Nothing was launched and no new
//!   incarnation started; earlier ends are resolved as observed and what is
//!   owed stays in the store (exit 6). Absence is not an end of the work it
//!   held: those ends stay unknown unless a report says otherwise.
//! * A `cancel` recovery is cancellation from its start: a reattached
//!   survivor is killed by its work PID 1 without being connected to, so
//!   nothing is resubmitted or newly delivered, and nothing is launched.
//! * `incomplete`: records or successful exit/reaping observations are
//!   missing (exit 4). A failed wait reports `wait-failed` / `unproven`,
//!   never an exit, closure count or relaunch authorization. Even a cancelled
//!   terminal may have `all_harnesses_reaped:false`; cancellation is not proof.
//! * `launches` counts launches root PID 1 performed for this instance;
//!   `reattached` counts survivors it took over.
//! * `records_complete` checks every durable harness and message record;
//!   `all_harnesses_reaped` also requires a waiter's report for each launched
//!   or reattached harness, no missing report and nothing detached.
//!   Incomplete records report `owed:null` and `known_owed` as a partial
//!   count.
//! * `root_pid1` says how root PID 1's end was observed at release, with a
//!   status only from this owner's own wait as its parent. `end_observed`
//!   is true only after that wait or a pidfd exit; only then is the
//!   incarnation recorded as ended. Without a `releasing` reply
//!   (`release-unanswered`, or `left-to-successor` if superseded meanwhile)
//!   the owner observes its exit for at most 2 s and does not wait on it,
//!   so the incarnation stays recorded as possibly running and a later
//!   owner attaches to it, finds it gone, or reports it owned-unattached.
//!
//! No Broker settlement is visible here, and none is inferred. A missing
//! terminal report (this process died) says nothing about delivery; the
//! store says what was recorded.
//!
//! # Known gaps
//!
//! * `AcpClient` keeps every session event in an unbounded `Vec` for the
//!   life of each connection. Stdout lines from a harness are bounded
//!   ([`MAX_LINE_BYTES`]).
//! * The internal event channel is unbounded.
//! * Between an attempt's commit and its send, a successor that claimed in
//!   that window could resend the same key concurrently; labels stay honest
//!   (`duplicate-unknown`) but a non-dedup receiver may insert twice.
//! * Reattachment begins reading where the dead owner stopped: output the
//!   dead owner had read but not processed is lost, and output written while
//!   no owner was attached waits in the pipe (a full pipe blocks the
//!   harness). Quiet survivors are the case shown here.
//! * A store refusal after a commit (post-commit refusal) is
//!   acknowledgement-unknown; recovery reconciliation of it, path custody
//!   of the store, a cross-restart launch/time budget and retention bounds
//!   are later work. The store and the custody socket live in the owner's
//!   private directory, not in privileged owner-private custody.
//! * Root PID 1 keeps every receipt for its lifetime, and a root PID 1 whose
//!   owner lost its connection to it mid-run is not restarted by that owner.
//!   Losing that connection does not stop harness reads already blocked.
//! * Nothing current supersedes a live owner: the store lock refuses a
//!   second normal claim. (The Runner entry ties its owner's life to its
//!   own, so a dead entry leaves no live owner behind.) Supersession is
//!   handled for the newer-owner ingress root PID 1 already admits, and is
//!   shown here only with a test process standing in for the newer owner.
//! * Attach waits up to 2 s to observe an unreachable root PID 1's exit
//!   before reporting it owned-unattached (observation only).
//! * Store growth and retention are unbounded; nothing is pruned.
//! * Exit observation waits for protocol-read progress; a descendant holding
//!   stdout can delay it. A caller that does not drain output can delay cancel.
//! * Bash ingress: the agent-bash tool speaks it (root v1). A native
//!   OpenCode host loads that tool only behind
//!   `native/opencode/bash-policy-tool.ts`, whose native permission
//!   decides each whole command string; the [`native`] setup provisions
//!   both with the root's deny-default policy (only named commands run).
//!   That is shown with a scripted stand-in for a model, not a model. The
//!   owner grants no permission request (it answers method-not-found, a
//!   native rejection). No completion
//!   is delivered to a harness as a new input; one thread per connection
//!   and no deadline on reading a request; the peer is identified by its
//!   `SO_PEERCRED` pid, so a requester that exits and whose pid is reused
//!   before attribution is attributed by the reuser's namespace (still only
//!   a harness namespace of this owner); a process that leaves its work's
//!   PID namespace (e.g. a nested `unshare`) is refused, not followed.
//!   Attribution assumes `/proc` shows the owner's own PID namespace. Output
//!   is relayed unbounded and not retained. A requester that stays
//!   connected but stops reading stalls the relay and then, once the pipe
//!   fills, its own run; nothing times it out.
//! * Turn-end attribution relies on the agent's tags and on its message ids
//!   ascending; an agent that tags wrongly is believed.
//! * Socket endpoints: the owner retries connecting every 50 ms without a
//!   deadline until the socket listens or the harness ends. Nothing
//!   authenticates the listener beyond the store directory's mode; a closed
//!   connection to a live harness holds the worker until that harness ends
//!   or the caller cancels. While no owner is attached, nothing drains a
//!   survivor's stdout.

pub mod bash;
mod custody;
mod harness;
mod live;
pub mod native;
mod store;
#[doc(hidden)]
pub mod sys;
mod transport;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use harness::{HarnessRecord, MessageRecord, SOCKET_ENV};
pub use transport::MAX_LINE_BYTES;

/// Every owned harness exited and was reaped; nothing is owed.
pub const EXIT_ENDED: u8 = 0;
/// The caller cancelled; owed messages stay in the store for a recovery.
pub const EXIT_CANCELLED: u8 = 2;
/// All spawned direct children were reaped; owed messages stay in the store.
pub const EXIT_ENDED_OWED: u8 = 3;
/// Worker records or successful exit/reaping observations are incomplete.
pub const EXIT_INCOMPLETE: u8 = 4;
/// A store write was refused (stale owner) or failed; own harnesses were
/// signalled.
pub const EXIT_STORE_LOST: u8 = 5;
/// A recovery naming its purpose found no root PID 1 to attach: nothing was
/// launched and no incarnation started; owed messages stay in the store.
pub const EXIT_ROOT_ABSENT: u8 = 6;
/// The request line was missing or invalid; nothing was written or launched.
pub const EXIT_SPEC_REFUSED: u8 = 64;
/// The store could not be claimed (live owner, missing or existing intent,
/// not private, or a store error); nothing was launched.
pub const EXIT_STORE_REFUSED: u8 = 65;

/// The first stdin line.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Absolute path of this root's private store directory.
    pub store: String,
    /// Present to create the root's intent; absent to recover it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent: Option<Intent>,
    /// A recovery's purpose. Absent: the plain recovery, which may start a
    /// new incarnation when the recorded one is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recover: Option<Recover>,
}

/// What a recovery is for. Both act only on a surviving root PID 1 this
/// owner positively attaches (store token, exact process, newer
/// generation); neither starts a new incarnation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Recover {
    /// End the attached root's work: survivors are killed by their work
    /// PID 1s, nothing is connected to, resubmitted or launched.
    Cancel,
    /// Continue the attached root's work: survivors are reattached and
    /// what is owed is resubmitted with its original keys (an ACK is then
    /// at best `duplicate-unknown`, not receiver continuity). A harness
    /// that needs a relaunch is launched by the attached root PID 1, in its
    /// original environment.
    ContinueAttached,
}

impl Recover {
    fn label(self) -> &'static str {
        match self {
            Self::Cancel => "cancel",
            Self::ContinueAttached => "continue-attached",
        }
    }
}

/// The stop reason of a purposeful recovery that found no root to attach.
const ROOT_ABSENT: &str = "root-absent";

/// What one root's supervisor owes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    /// Observed no-acknowledgement closures of one message before it is
    /// labelled an outage. Must be at least 1. Counts closures, not time.
    pub outage_closure_cap: u32,
    /// Recorded delivery attempts of one message, over every owner
    /// generation, before it stops as `attempts-exhausted`. Must be at least
    /// 1. Counts attempts whatever their outcome, including unknown ones.
    pub delivery_attempt_cap: u32,
    /// Absolute working directory each harness is started in and that is
    /// given to `session/new` and `session/resume`.
    pub cwd: String,
    pub harnesses: Vec<HarnessSpec>,
}

/// One owned harness: how to launch it and what to deliver to it, in order.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessSpec {
    /// Label used in reports.
    pub id: String,
    /// Program and arguments, started in [`Intent::cwd`].
    pub argv: Vec<String>,
    /// Where the harness speaks ACP v2.
    #[serde(default)]
    pub endpoint: Endpoint,
    /// An existing native conversation to resume (`session/resume`) instead
    /// of opening a new one. Trusted scope: the harness decides whether it
    /// names a conversation it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub messages: Vec<String>,
}

/// How the owner reaches one harness's ACP v2 endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Endpoint {
    /// ND-JSON over the harness's stdin/stdout.
    #[default]
    Stdio,
    /// ND-JSON over a Unix socket the harness itself listens on, inside the
    /// process that owns its native conversations (no bridge process). The
    /// owner chooses the path, `<store>/acp/w<work>.sock`, and passes it as
    /// `OULIPOLY_ACP_V2_SOCKET`; the harness's stdout is drained, not read.
    UnixSocket,
}

impl Endpoint {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::UnixSocket => "unix-socket",
        }
    }

    pub(crate) fn parse(label: &str) -> Option<Self> {
        match label {
            "stdio" => Some(Self::Stdio),
            "unix-socket" => Some(Self::UnixSocket),
            _ => None,
        }
    }
}

/// Longest Unix socket path (`sun_path` less its terminating NUL).
const SOCKET_PATH_MAX: usize = 107;

impl Request {
    /// The checks this owner applies to a request before writing anything
    /// (its own refusal, [`EXIT_SPEC_REFUSED`]). A launching caller may
    /// apply them first, before effects of its own.
    pub fn validate(&self) -> Result<(), String> {
        if !self.store.starts_with('/') {
            return Err("store must be absolute".to_owned());
        }
        if self.recover.is_some() && self.intent.is_some() {
            return Err("recover names a recovery's purpose; an intent creates a root".to_owned());
        }
        let Some(intent) = &self.intent else {
            return Ok(());
        };
        intent.validate()?;
        let longest = harness::socket_path(Path::new(&self.store), i64::MAX);
        if intent
            .harnesses
            .iter()
            .any(|harness| harness.endpoint == Endpoint::UnixSocket)
            && longest.as_os_str().len() > SOCKET_PATH_MAX
        {
            return Err(format!(
                "store path too long for harness sockets ({} > {SOCKET_PATH_MAX} bytes)",
                longest.as_os_str().len()
            ));
        }
        Ok(())
    }
}

impl Intent {
    fn validate(&self) -> Result<(), String> {
        if self.outage_closure_cap == 0 {
            return Err("outage_closure_cap must be at least 1".to_owned());
        }
        if self.delivery_attempt_cap == 0 {
            return Err("delivery_attempt_cap must be at least 1".to_owned());
        }
        if !self.cwd.starts_with('/') {
            return Err("cwd must be absolute".to_owned());
        }
        if self.harnesses.is_empty() {
            return Err("at least one harness is required".to_owned());
        }
        let mut ids = HashSet::new();
        for harness in &self.harnesses {
            if harness.argv.is_empty() {
                return Err(format!("harness {}: argv is empty", harness.id));
            }
            if !ids.insert(harness.id.as_str()) {
                return Err(format!("duplicate harness id {}", harness.id));
            }
            if harness.session.as_deref().is_some_and(str::is_empty) {
                return Err(format!("harness {}: session is empty", harness.id));
            }
        }
        Ok(())
    }
}

/// What workers and the control reader tell the owning loop.
pub(crate) enum Event {
    Report(Value),
    Done(HarnessRecord),
    Control(String),
    /// A Bash run's end (or why it is unknown) was reported.
    BashDone,
}

/// Runs one supervisor to its terminal report and returns the exit code.
pub fn run<R, W>(mut input: R, mut out: W) -> u8
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let mut first = String::new();
    let request = match input.read_line(&mut first) {
        Ok(0) | Err(_) => Err("no request line".to_owned()),
        Ok(_) => serde_json::from_str::<Request>(first.trim()).map_err(|error| error.to_string()),
    }
    .and_then(|request| request.validate().map(|()| request));
    let request = match request {
        Ok(request) => request,
        Err(reason) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "spec-refused", "reason": reason }),
            );
            return EXIT_SPEC_REFUSED;
        }
    };
    if request.intent.is_some() {
        emit(
            &mut out,
            &json!({ "event": "intent-received", "stage": "accepted-by-interface", "durable": false }),
        );
    }
    let claimed = match store::Store::claim(Path::new(&request.store), request.intent.as_ref()) {
        Ok(claimed) => claimed,
        Err(error) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "store-refused", "reason": error.reason() }),
            );
            return EXIT_STORE_REFUSED;
        }
    };
    let generation = claimed.store.generation();
    if claimed.created {
        emit(
            &mut out,
            &json!({
                "event": "intent-committed",
                "stage": "durably-committed",
                "durability": "sqlite-wal-synchronous-full",
                "generation": generation,
            }),
        );
    } else {
        emit(
            &mut out,
            &json!({
                "event": "intent-recovered",
                "generation": generation,
                "prior_attempts_unknown": claimed.classified_unknown,
            }),
        );
    }
    let expected: Vec<(String, usize)> = claimed
        .harnesses
        .iter()
        .map(|harness| (harness.id.clone(), harness.messages.len()))
        .collect();

    let isolation = sys::Isolation::current();
    let stop = match transport::StopSignal::new() {
        Ok(stop) => Arc::new(stop),
        Err(error) => {
            emit(
                &mut out,
                &json!({ "event": "terminal", "status": "incomplete", "reason": format!("stop-signal: {error}") }),
            );
            return EXIT_INCOMPLETE;
        }
    };
    let mut store = claimed.store;
    let mut recovery = recover_custody(
        Path::new(&request.store),
        claimed.live_incarnation.as_ref(),
        &claimed.harnesses,
        &claimed.open_bash,
        &mut store,
        &stop,
    );
    emit(&mut out, &recovery.report(&claimed.root_id, isolation));
    if let Some(reason) = recovery.unattached {
        let (report, code) = unattached_report(&claimed.harnesses, &reason);
        emit(&mut out, &report);
        return code;
    }
    let root_absent = request.recover.is_some() && recovery.root.is_none();
    let mut priors = recovery.priors;
    let slot = Arc::new(custody::RootSlot::new(
        PathBuf::from(&request.store),
        generation,
        isolation,
        Arc::clone(&stop),
        recovery.root,
    ));
    let custody = Arc::new(Mutex::new(live::Custody::new(Arc::clone(&stop))));
    // A purposeful recovery's limits hold before any survivor is taken over
    // or anything launched: with no attached root, nothing may start one;
    // under cancel, nothing is delivered.
    let mut cancel_requested = false;
    if let Some(purpose) = request.recover {
        if root_absent {
            custody.lock().expect("custody lock").stop(ROOT_ABSENT);
            let found = if recovery.incarnation.is_some() {
                recovery.outcome
            } else {
                "no-unended-incarnation"
            };
            emit(
                &mut out,
                &json!({
                    "event": "recover-limited",
                    "purpose": purpose.label(),
                    "reason": ROOT_ABSENT,
                    "custody": found,
                    "new_incarnation": "not-started",
                }),
            );
        } else if purpose == Recover::Cancel {
            cancel_requested = true;
            let signalled = custody.lock().expect("custody lock").cancel();
            emit(
                &mut out,
                &json!({
                    "event": "cancel-requested",
                    "signalled": signalled,
                    "by": "recover-cancel",
                }),
            );
        }
    }
    let store = Arc::new(Mutex::new(store));
    let (tx, rx) = mpsc::channel();
    let views: bash::Views = Arc::new(Mutex::new(
        claimed
            .harnesses
            .iter()
            .map(|_| bash::View::default())
            .collect(),
    ));
    let ingress = bash::Ingress::new(
        PathBuf::from(&request.store),
        claimed.root_id.clone(),
        Arc::clone(&slot),
        Arc::clone(&custody),
        Arc::clone(&store),
        Arc::clone(&views),
        tx.clone(),
    );
    match ingress.listen() {
        Ok(path) => emit(
            &mut out,
            &json!({ "event": "bash-ingress", "listening": true, "path": path, "protocol": bash::PROTOCOL }),
        ),
        Err(reason) => emit(
            &mut out,
            &json!({ "event": "bash-ingress", "listening": false, "reason": reason }),
        ),
    }
    for prior in recovery.bash.drain(..) {
        ingress.recover(prior);
    }

    let control_tx = tx.clone();
    thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { return };
            if control_tx.send(Event::Control(line)).is_err() {
                return;
            }
        }
    });

    emit(
        &mut out,
        &json!({
            "event": "started",
            "pid": std::process::id(),
            "generation": generation,
            "root_id": claimed.root_id,
            "isolation": isolation.label(),
            "harnesses": expected.len(),
            "outage_closure_cap": claimed.outage_closure_cap,
            "delivery_attempt_cap": claimed.delivery_attempt_cap,
        }),
    );
    for (position, harness) in claimed.harnesses.into_iter().enumerate() {
        let assignment = harness::Assignment {
            position,
            harness,
            store_dir: PathBuf::from(&request.store),
            cap: claimed.outage_closure_cap,
            attempt_cap: claimed.delivery_attempt_cap,
            cwd: claimed.cwd.clone(),
            custody: Arc::clone(&custody),
            store: Arc::clone(&store),
            slot: Arc::clone(&slot),
            prior: priors.remove(&position),
            tx: tx.clone(),
            views: Arc::clone(&views),
        };
        thread::spawn(move || harness::run(assignment));
    }
    drop(tx);

    let mut records = Vec::new();
    // The run ends once every harness's end and every admitted Bash run's
    // end (or why it is unknown) is reported; then the ingress is closed.
    while records.len() < expected.len() || !ingress.close_if_idle() {
        let Ok(event) = rx.recv() else { break };
        match event {
            Event::Report(value) => emit(&mut out, &value),
            Event::Done(record) => records.push(record),
            Event::BashDone => {}
            Event::Control(line) => {
                let command = serde_json::from_str::<Value>(line.trim()).ok();
                if command
                    .as_ref()
                    .and_then(|value| value.get("cmd"))
                    .and_then(Value::as_str)
                    == Some("cancel")
                {
                    if !cancel_requested {
                        cancel_requested = true;
                        let signalled = custody.lock().expect("custody lock").cancel();
                        emit(
                            &mut out,
                            &json!({ "event": "cancel-requested", "signalled": signalled }),
                        );
                    }
                } else {
                    emit(&mut out, &json!({ "event": "control-refused" }));
                }
            }
        }
    }
    records.sort_by_key(|record| expected.iter().position(|(id, _)| *id == record.id));

    let stopped = custody.lock().expect("custody lock").reason();
    let store_lost = matches!(stopped, Some("authority-lost" | "store-failed"))
        .then_some(stopped)
        .flatten();
    let root_pid1 = end_custody(&slot, store_lost, &store);
    // A newer owner may have superseded this one while it was releasing.
    let store_lost = store_lost.or_else(|| {
        (custody.lock().expect("custody lock").reason() == Some("authority-lost"))
            .then_some("authority-lost")
    });
    let (mut report, mut code) = terminal_report(&expected, &records, cancel_requested, store_lost);
    if root_absent && store_lost.is_none() {
        // Nothing was attached or launched: not an end of what the earlier
        // root held, whatever the records say about this instance.
        report["status"] = json!(ROOT_ABSENT);
        report["new_incarnation"] = json!("not-started");
        code = EXIT_ROOT_ABSENT;
    }
    if let Some(purpose) = request.recover {
        report["recover"] = json!(purpose.label());
    }
    if root_pid1["end_observed"] == false && matches!(code, EXIT_ENDED | EXIT_ENDED_OWED) {
        // Every harness's end was reported, but root PID 1's was not
        // observed: it may still be running, so this is not an end.
        report["status"] = json!("incomplete");
        code = EXIT_INCOMPLETE;
    }
    if ingress.ends_unproven() && matches!(code, EXIT_ENDED | EXIT_ENDED_OWED) {
        // A Bash run may have run and its end is not known.
        report["status"] = json!("incomplete");
        code = EXIT_INCOMPLETE;
    }
    report["bash"] = ingress.summary();
    report["root_pid1"] = root_pid1;
    emit(&mut out, &report);
    code
}

/// What this owner found of its root's earlier custody on starting.
struct Recovery {
    root: Option<Arc<custody::Root>>,
    priors: HashMap<usize, harness::Prior>,
    bash: Vec<bash::Prior>,
    unattached: Option<String>,
    outcome: &'static str,
    incarnation: Option<i64>,
    observed: Option<&'static str>,
    survivors: usize,
    receipts: usize,
}

impl Recovery {
    fn report(&self, root_id: &str, isolation: sys::Isolation) -> Value {
        json!({
            "event": "custody",
            "root_id": root_id,
            "isolation": isolation.label(),
            "outcome": self.outcome,
            "incarnation": self.incarnation,
            "observed": self.observed,
            "unattached_reason": self.unattached,
            "survivors": self.survivors,
            "receipts": self.receipts,
        })
    }
}

/// Attaches to the recorded root PID 1 incarnation if it is still the
/// exact recorded process, and sorts each harness's unresolved launch into
/// a live survivor, an end its waiter reported, or an unknown end.
fn recover_custody(
    store_dir: &Path,
    recorded: Option<&store::IncarnationRow>,
    harnesses: &[store::DurableHarness],
    open_bash: &[store::OpenBash],
    store: &mut store::Store,
    stop: &Arc<transport::StopSignal>,
) -> Recovery {
    let mut recovery = Recovery {
        root: None,
        priors: HashMap::new(),
        bash: Vec::new(),
        unattached: None,
        outcome: "fresh",
        incarnation: None,
        observed: None,
        survivors: 0,
        receipts: 0,
    };
    let Some(recorded) = recorded else {
        return recovery;
    };
    recovery.incarnation = Some(recorded.id);
    let (mut live, receipts, absent) =
        match custody::Root::attach(store_dir, recorded, store.generation(), stop) {
            custody::Attach::Attached {
                root,
                live,
                receipts,
            } => {
                recovery.outcome = "attached";
                recovery.root = Some(root);
                (live, receipts, false)
            }
            custody::Attach::Absent { observed, receipts } => {
                recovery.outcome = "absent";
                recovery.observed = Some(observed);
                // Recorded only as what was seen; the store may refuse (stale).
                let _ = store.end_incarnation(recorded.id, observed);
                (Vec::new(), receipts, true)
            }
            custody::Attach::Unattached { reason } => {
                recovery.outcome = "owned-unattached";
                recovery.unattached = Some(reason);
                return recovery;
            }
        };
    recovery.survivors = live.len();
    recovery.receipts = receipts.len();
    for (position, harness) in harnesses.iter().enumerate() {
        for &(work, incarnation) in &harness.open_works {
            if incarnation != recorded.id {
                continue;
            }
            let name = custody::work_name(work);
            let prior = if let Some(index) = live.iter().position(|adopted| adopted.work == work) {
                harness::Prior::Live(live.swap_remove(index))
            } else if let Some(receipt) = receipts
                .iter()
                .find(|receipt| receipt["work"] == name.as_str())
            {
                harness::Prior::Exited {
                    work,
                    receipt: receipt.clone(),
                }
            } else if absent {
                harness::Prior::Unknown { work }
            } else {
                // The attached root PID 1, the only launcher, never had it:
                // the launch was recorded but never requested.
                let _ = store.resolve_work(work, "never-launched", Some("root-pid1-record"));
                continue;
            };
            recovery.priors.insert(position, prior);
        }
    }
    for run in open_bash
        .iter()
        .filter(|run| run.incarnation == recorded.id)
    {
        let name = custody::work_name(run.work);
        let prior = if let Some(index) = live.iter().position(|adopted| adopted.work == run.work) {
            bash::Prior::Live {
                harness: run.harness,
                adopted: live.swap_remove(index),
            }
        } else if let Some(receipt) = receipts
            .iter()
            .find(|receipt| receipt["work"] == name.as_str())
        {
            bash::Prior::Exited {
                harness: run.harness,
                work: run.work,
                receipt: receipt.clone(),
            }
        } else if absent {
            bash::Prior::Unknown {
                harness: run.harness,
                work: run.work,
            }
        } else {
            // A missing first launch report can follow creation. Absence
            // from attach is not a positive no-start reply for this intent.
            bash::Prior::Unknown {
                harness: run.harness,
                work: run.work,
            }
        };
        recovery.bash.push(prior);
    }
    recovery
}

/// Releases this owner's root PID 1 at the end of its run and reports how
/// its end was observed. On authority loss nothing is released: the root
/// and its live work belong to the newer owner. The incarnation is recorded
/// as ended only once its end was actually observed; otherwise it stays
/// discoverable, so a later owner attaches to it or finds it gone.
fn end_custody(
    slot: &custody::RootSlot,
    store_lost: Option<&str>,
    store: &Mutex<store::Store>,
) -> Value {
    let Some(root) = slot.current() else {
        return json!({ "outcome": "none" });
    };
    if store_lost == Some("authority-lost") {
        root.detach();
        return json!({
            "outcome": "left-to-successor",
            "incarnation": root.incarnation,
            "end_observed": Value::Null,
        });
    }
    let release = root.release();
    if release.ended {
        let label = match &release.status {
            Some(status) => format!("{}:{status}", release.outcome),
            None => release.outcome.to_owned(),
        };
        let _ = store
            .lock()
            .expect("store lock")
            .end_incarnation(root.incarnation, &label);
    }
    json!({
        "outcome": release.outcome,
        "incarnation": root.incarnation,
        "pid": root.host_pid,
        "parent": root.parent,
        "status": release.status,
        "status_known": release.status.is_some(),
        "live": release.live,
        "end_observed": release.ended,
    })
}

/// The terminal for a root whose recorded PID 1 may still be running but
/// could not be attached: nothing is launched, killed or declared ended.
fn unattached_report(harnesses: &[store::DurableHarness], reason: &str) -> (Value, u8) {
    let records: Vec<Value> = harnesses
        .iter()
        .map(|harness| {
            json!({
                "id": harness.id,
                "launches": 0,
                "messages": harness.messages.iter().enumerate().map(|(index, message)| json!({
                    "index": index,
                    "state": if message.ack.is_some() { "acknowledged" } else { "owed" },
                    "label": message.ack.as_ref().map_or("owned-unattached", |ack| ack.label.as_str()),
                    "completion": "not-observed",
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let known_owed = harnesses
        .iter()
        .flat_map(|harness| &harness.messages)
        .filter(|message| message.ack.is_none())
        .count();
    (
        json!({
            "event": "terminal",
            "status": "owned-unattached",
            "reason": reason,
            "cancel_requested": false,
            "owed": known_owed,
            "known_owed": known_owed,
            "owed_history": "retained-in-store",
            "records_complete": false,
            "all_harnesses_reaped": false,
            "harnesses": records,
        }),
        EXIT_INCOMPLETE,
    )
}

fn terminal_report(
    expected: &[(String, usize)],
    records: &[HarnessRecord],
    cancel_requested: bool,
    store_lost: Option<&str>,
) -> (Value, u8) {
    let records_complete = records.len() == expected.len()
        && expected.iter().all(|(id, messages)| {
            let mut matches = records.iter().filter(|record| record.id == *id);
            let Some(record) = matches.next() else {
                return false;
            };
            matches.next().is_none()
                && record.messages.len() == *messages
                && record
                    .messages
                    .iter()
                    .enumerate()
                    .all(|(index, message)| message.index == index)
        });
    let all_reaped = records_complete
        && records.iter().all(|record| {
            record.wait_failures.is_empty()
                && record.detached == 0
                && usize::try_from(record.launches + record.reattached).ok()
                    == Some(record.exits.len())
        });
    let known_owed = records
        .iter()
        .flat_map(|record| &record.messages)
        .filter(|message| message.owed)
        .count();
    let (status, code) = if let Some(lost) = store_lost {
        (lost, EXIT_STORE_LOST)
    } else if cancel_requested {
        ("cancelled", EXIT_CANCELLED)
    } else if !all_reaped {
        ("incomplete", EXIT_INCOMPLETE)
    } else if known_owed > 0 {
        ("ended-owed", EXIT_ENDED_OWED)
    } else {
        ("ended", EXIT_ENDED)
    };
    let owed_history = if store_lost.is_some() {
        "store-holds-authoritative-state"
    } else if !records_complete {
        "unknown-partial-records"
    } else if known_owed > 0 {
        "retained-in-store"
    } else {
        "nothing-owed"
    };
    let report = json!({
        "event": "terminal",
        "status": status,
        "cancel_requested": cancel_requested,
        "owed": records_complete.then_some(known_owed),
        "known_owed": known_owed,
        "owed_history": owed_history,
        "records_complete": records_complete,
        "all_harnesses_reaped": all_reaped,
        "harnesses": records.iter().map(HarnessRecord::to_json).collect::<Vec<_>>(),
    });
    (report, code)
}

fn emit<W: Write>(out: &mut W, value: &Value) {
    // A caller that stopped reading cannot be told anything; the owning
    // loop continues so that owned children are still reaped.
    let _ = writeln!(out, "{value}").and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(store: &str, harness: Value) -> Result<(), String> {
        let line = json!({
            "store": store,
            "intent": {
                "outage_closure_cap": 1,
                "delivery_attempt_cap": 1,
                "cwd": "/",
                "harnesses": [harness],
            },
        });
        serde_json::from_value::<Request>(line)
            .map_err(|error| error.to_string())?
            .validate()
    }

    #[test]
    fn socket_endpoint_needs_a_store_short_enough_for_every_socket_path() {
        let long = format!("/{}", "s".repeat(80));
        let harness = |endpoint: &str| json!({ "id": "h", "argv": ["x"], "endpoint": endpoint, "messages": [] });
        assert!(request(&long, harness("stdio")).is_ok());
        let refused = request(&long, harness("unix-socket")).unwrap_err();
        assert!(refused.contains("too long"), "{refused}");
        assert!(request("/tmp/root", harness("unix-socket")).is_ok());
        assert!(request("/tmp/root", harness("tcp")).is_err());
    }

    #[test]
    fn empty_resume_session_is_refused() {
        let refused = request(
            "/tmp/root",
            json!({ "id": "h", "argv": ["x"], "session": "", "messages": [] }),
        )
        .unwrap_err();
        assert!(refused.contains("session is empty"), "{refused}");
    }

    fn expected(messages: usize) -> Vec<(String, usize)> {
        vec![("test".into(), messages)]
    }

    fn record() -> HarnessRecord {
        HarnessRecord {
            id: "test".into(),
            launches: 1,
            reattached: 0,
            exits: vec![],
            prior_exits: vec![],
            prior_unknown_ends: 0,
            wait_failures: vec![],
            detached: 0,
            messages: vec![],
        }
    }

    #[test]
    fn terminal_done_without_reap_is_not_normal_or_complete() {
        let (report, code) = terminal_report(&expected(0), &[record()], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_record_is_unknown_not_zero_debt_normal_end() {
        let (report, code) = terminal_report(&expected(0), &[], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_ne!(report["status"], "ended");
        assert_eq!(report["records_complete"], false);
        assert!(report["owed"].is_null());
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    #[test]
    fn missing_message_record_is_not_positive_completeness() {
        let mut record = record();
        record.exits.push("code:0".into());
        let (report, code) = terminal_report(&expected(1), &[record], false, None);
        assert_ne!(code, EXIT_ENDED);
        assert_eq!(report["records_complete"], false);
        assert_eq!(report["all_harnesses_reaped"], false);
    }

    fn owed_record() -> HarnessRecord {
        let mut record = record();
        record.exits.push("code:0".into());
        record.messages.push(MessageRecord {
            index: 0,
            owed: true,
            label: "outage".into(),
            at_most_once: false,
            basis: None,
            recovered: false,
            ack_generation: None,
            attempts: 1,
            prior_unknown: 0,
            closures: 1,
        });
        record
    }

    #[test]
    fn terminal_debt_is_retained_in_store_including_cancel() {
        for cancelled in [false, true] {
            let (report, code) = terminal_report(&expected(1), &[owed_record()], cancelled, None);
            assert_eq!(
                code,
                if cancelled {
                    EXIT_CANCELLED
                } else {
                    EXIT_ENDED_OWED
                }
            );
            assert_eq!(report["harnesses"][0]["messages"][0]["state"], "owed");
            assert_eq!(report["harnesses"][0]["messages"][0]["label"], "outage");
            assert_eq!(
                report["harnesses"][0]["messages"][0]["completion"],
                "not-observed"
            );
            assert_eq!(report["owed_history"], "retained-in-store");
        }
    }

    /// CONFIGURED SEAM (see `custody::Root::seam`): this owner's release
    /// of a still-running incarnation gets no reply. The incarnation is not
    /// recorded as ended, so a later claim still finds it.
    #[test]
    fn unanswered_release_leaves_incarnation_discoverable() {
        let dir = std::env::temp_dir().join(format!("root-end-{}", std::process::id()));
        let intent = Intent {
            outage_closure_cap: 1,
            delivery_attempt_cap: 1,
            cwd: "/".into(),
            harnesses: vec![HarnessSpec {
                id: "test".into(),
                argv: vec!["x".into()],
                endpoint: crate::Endpoint::Stdio,
                session: None,
                messages: vec![],
            }],
        };
        let mut store = store::Store::claim(&dir, Some(&intent)).unwrap().store;
        assert_eq!(store.begin_incarnation("t", "test").unwrap(), 1);
        let stop = Arc::new(transport::StopSignal::new().unwrap());
        let (root, far) =
            custody::Root::seam(false, i32::try_from(std::process::id()).unwrap(), &stop);
        drop(far);
        let slot = custody::RootSlot::new(
            dir.clone(),
            store.generation(),
            sys::Isolation::current(),
            stop,
            Some(root),
        );
        let store = Mutex::new(store);
        let ended = end_custody(&slot, None, &store);
        assert_eq!(ended["outcome"], "release-unanswered");
        assert_eq!(ended["end_observed"], false);
        drop(store);
        let conn = rusqlite::Connection::open(dir.join(store::DB_FILE)).unwrap();
        let recorded: Option<String> = conn
            .query_row("SELECT ended FROM incarnation WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(recorded, None, "a missing reply is not an observed end");
    }

    #[test]
    fn lost_store_authority_outranks_cancel_and_claims_no_retention() {
        let (report, code) =
            terminal_report(&expected(1), &[owed_record()], true, Some("authority-lost"));
        assert_eq!(code, EXIT_STORE_LOST);
        assert_eq!(report["status"], "authority-lost");
        assert_eq!(report["owed_history"], "store-holds-authoritative-state");
    }
}
