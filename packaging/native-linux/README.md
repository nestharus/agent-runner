# Registered-only native-root package (Linux x86_64)

The Linux ACP v2 payload contains the Runner, per-root owner, root PID 1,
Rust child requester, Agent Bash, privileged front door and programmatic
caller. Parent harnesses are external registered providers. Codex and Claude
adapters are acquired separately; each owns its native CLI, settings,
environment, authentication and model behavior. The package contains no
embedded OpenCode/native-Claude harness, Claude Agent SDK, Node runtime,
embedded JS requester, or embedded credential preparation.

Work runs as the declared non-root host user; owner and PID1 custody remain
separate from logical lineage. Host Python and dynamic libraries are trusted
runtime dependencies. Source delivery does not change any installed route.

## Layout and construction

```
<package>/
  bin/oulipoly-agent-runner
  bin/oulipoly-root-supervisor
  bin/oulipoly-root-pid1
  bin/oulipoly-root-child
  bin/oulipoly-native-call
  libexec/oulipoly-native-frontdoor
  agent-bash/agent-bash
  share/install_package.py
  share/sudoers.template
  share/frontdoor.example.json
  share/README.md
  MANIFEST.json
```

```
python3 packaging/native-linux/build_package.py --build-dir B \
  --runner-repo W --agent-bash-repo T --agent-bash-commit C
```

Only B receives Cargo home/targets, the exact Agent Bash source snapshot,
stage and archive. Cargo uses `--locked` and default Runner features. No
npm installation or Node download occurs. Dirty sources refuse unless
explicitly selected. Package id includes both commits; manifest records
source identities, file hashes/modes, toolchain and binary dynamic libraries.
Archive entries have normalized modes, timestamps and 0:0 ownership; archive
metadata does not establish installed root custody or reproducibility.

The child requester is built with the supervisor and staged as an executable.
Frontdoor checks all five executable assets and root custody before effects.
Provider compatibility comes from wire schemas and negotiated capabilities,
including resident sessions, tool mediation and exploration, never source
locks, byte hashes or banners. ROOT acquires current C/H adapter builds with
selected SDK corrections, then builds and qualifies the full candidate.
Finite source/build/stage controls are not full package or work-UID proof.

## Reviewed installation and recovery

ROOT reviews a signed source version, archive digest and root-owned copy
of the installer before running it. Nothing here authorizes installation.

```
/usr/bin/python3 -I <reviewed-installer> plan --archive A --sha256 H --user nes
/usr/bin/python3 -I <root-owned-reviewed-installer> install --archive A --sha256 H --user nes
```

`plan` makes a transient 0700 directory and 0600 archive copy, hashes that
copy, reads its package id, prints intended effects, and removes the copy.
It does not install into destination paths or establish manifest/custody
validation for them. Host-runtime construction requires Python 3.12+,
PID/mount namespaces, sudo and dynamic libraries listed in the manifest.

`install` validates literal and resolved existing destination ancestry.
Before the private copy or any other destination effect it creates a 0600
recovery record in the nearest existing prefix ancestor:
`<ancestor>/.oulipoly-native-<archive sha256 first 12>.install.json`.
Its exact location is printed before effects. The private copy lives in a
fresh 0700 directory alongside that record; only the hashed copy is read.
The record describes each planned effect before creation and records
created identities afterwards. Once the prefix exists it moves to
`<prefix>/<id>.install.json`. Both the relocation and package/rule rename
alternatives are recorded beforehand.

The installer extracts into `<prefix>/.<id>.partial`, verifies manifest
files and modes, renames into place, creates missing run-base parents and
`/var/lib/oulipoly-native/runs` (0711), and writes the site config (0644)
only if absent. Existing config is left after custody validation.
`visudo -cf` checks the staged 0440 rule. The enabling rename to
`/etc/sudoers.d/oulipoly-native` is the last enabling effect, after the
package/config/run base and their record are ready. Existing rules are
refused. No service or runtime is started.

Failures/interruption leave the current record for explicit recovery;
there is no implicit rollback/retry. An interrupted atomic record update
can also leave a root-private `<record>.writing`; its last complete record
remains the authoritative cleanup description and both names must be
reviewed. Recovery is:

```
/usr/bin/python3 -I <root-owned-reviewed-installer> uninstall \
  --record <exact-current-record> --purge-site
```

Uninstall acquires an exclusive package directory lock, also excluding
in-flight admission, and reports live/unknown run locks before changing
anything. It disables its unchanged rule first. It checks recorded
package content, including added paths, before removal; changed package,
site or sudoers files are left and reported. Except for the explicitly inventoried loss accounts under `--purge-site`,
only recorded empty created directories are removed. It keeps the record until removals
finish. If that record blocks removal of its own parent directories, it
moves to a printed `<ancestor>/.<id>.removal.json` recovery path first.
Failures return 3 and retain a record. Without `--purge-site`, intentionally
retained site effects also retain the record and return 3.

A partially written extraction file that does not match the recorded
expected content is left for ROOT inspection; the record still names its
owned partial tree. Neither changed admin content nor unrelated data is
silently deleted. Other nonempty retained run data requires separate explicit
ROOT review; uninstall does not stop tasks, purge runs, or remove caller
output. The root-owned installer copy is a separately owned ROOT effect.

## Site routes and request

`frontdoor.example.json` illustrates a generic registered parent and explorer.
Its executable and model/settings placeholders must be replaced with an
acquired adapter's actual settings. It supplies no Codex/Claude model mapping,
account default or credential fallback. This is not installed configuration.

A site v1 route requires `harness: "provider"`, absolute normalized
`executable`, and provider/v1 `settings` (settings_id/mode/model/optional
launch). Optional `config_root` and `env` are adapter-owned. A parent may
list offered `children`. Child routes use the same provider shape, without
nested children. Site child limits are at most 4 starts and 2 concurrent.
The entry verifies executable/ancestor custody at admission and retains a
handle binding subsequent operations. Settings are opaque except their
provider contract shape and neutral owner/tool environment reservations.

The request v1 has route, message, absolute cwd, Bash policy, environment,
deadline_s, optional children/live, and retention. Bash is exactly
`{"authority":"trusted-task"}` or `{"allow":["whole command", ...]}`.
The caller may lower child limits and select only site-offered routes.
Old embedded site routes, auth/credential request fields and native harness
fields are refused by shape/schema. They are never translated. No old-site,
request, data or parity bridge is provided. The site no longer accepts
credential_margin_s or credential route fields, even `credential: "none"`.
Adapter-specific settings/env remain the adapter's own surface.

Workload allocation normalizes `run_base/<uid>` to 0711 after validating
root custody and non-writability, for fresh and existing restrictive uid
directories alike. Caller umask must not remove this ancestor's search
permission; 0711 grants traversal, not group/other listing or writing.
Ordinary and live allocations use this same normalization. Metadata-only
discovery and loss-account reads do not repair directories. Private request,
lock, store and loss-account custody are not broadened by this change.

Admission fixes requester identity from sudo, checks allowed users, package
and provider custody, cwd access, bounded environment and deadline. The
optional `live_output` request field is an SDK live-stream advertisement.
It is absent by default. For example, opt in with
`{"live_output":{"oulipoly.live_stream/v3":{"channels":["combined","control"],"audiences":["scoped"],"max_data_bytes":16384}}}`.
The front door attaches a grant for exactly its attested requester; malformed
or unsupported advertisements disable viewing only. The owner's `live-output`
event announces a configured path, with `listening: null` because setup runs
asynchronously. An unavailable socket means viewing is unavailable.
`<launch>/ipc/live.sock` is handed to that UID at 0600. Kernel peer credentials
and exclusion of this root's PID namespace tree govern admission. This grants
that principal access to this root's registered Bash work, including children;
it makes no per-app/session confidentiality promise.

The sole live wire is SDK `oulipoly.live_stream/v3`: subscriber/broker hello,
list/directory, attach/attached and record/unavailable messages. It uses genuine
frame sequence cursors and opaque work correlations; combined bytes never
become separate stdout or stderr. Required retained-byte offsets stay in the
Bash ingress. The former `root_bash_live/v1` wire has no fallback.

Optional registration and drain use nonblocking handoffs; terminal metadata
has a separate slot. There are at most 32 registered streams and 8 connection
workers, counted before admission. Requests have an absolute 5-second line
deadline and writes a 2-second line deadline. Quiet EOF is polled; polling does
not renew activity. Maintenance releases held bytes after one idle hour or
30 seconds after a terminal, checked every 50 ms when scheduled. Finished
metadata can disappear after grace; later attaches get `unknown_stream`.
Payload plus 64 bytes per held chunk is bounded to 1 MiB per run / 8 MiB per
owner; this is accounting, not an RSS or scheduling guarantee. Capacity or
contention can omit registration or produce sequence gaps. Owner close never
waits for viewers and may cut an optional tail/terminal short.

`finalized` names `rv1w:<root>:<work>` only after an exact matching durable
Bash result is published with its actual work and sealed-output classifications.
The per-root store schema is 15; older stores are refused, without migration.
The result keeps command-wait knowledge, root PID1's wait of work PID1, and
complete/partial/unsealed retention separately. Finalization promises neither
success, full/readable bytes, report delivery/ACK nor outside retained read.
Missing `exit_observed` means only no exit fact delivered on this live plane.
A vanished owner supplies no invented terminal. Run
store stays root-private; IPC/work data are handed to the work UID. The
entry negotiates provider describe, policy.evaluate and resident.prepare.
Prepared child slots use fresh data roots and inherit the parent tool policy.
The parent receives root-child only when child routes exist. The generic
owner still accepts trusted Fixed child intents; package sites select only
registered routes. Generic root-child ingress, Agent Bash, retention,
root/per-work PID1 and logical/custody ownership remain.

An authorized live owning harness can read the original work witness using
`oulipoly-root-bash --result rv1w:<root>:<work>`, or the local Bash socket's
`{"v":1,"op":"result","root_id":"…","work":N}` operation. This reads
original work/incarnation/generation, command wait or proven no-start,
retention, publication and completion knowledge without executing anything.
The command's exit 0 is query success, separate from the returned work status.
Other harnesses and outside peers are refused; references last only as long
as the private root store and expire on explicit discard. Missing publication
is reported, never reconstructed by this reader or used to permit retry.
Package construction includes the Runner reader in `bin/oulipoly-root-bash`;
use the selected package's absolute path from the authorized harness. It
does not add a provider tool declaration or change the external Bash adapter.

The local result query has one five-second socket deadline, including connect,
request write and partial reply reads. Timeout (75) or unavailable ingress (69)
means unknown query knowledge; neither infers command failure, output or
publication, and neither grants work retry. A successful query (0) reads a
witness whose original command can have a nonzero exit.

The owner checks terminal report write and flush. Failure returns 74 after
cleanup and attempts `terminal-delivery-failed` on stderr with the original
report and original supervisor exit. That diagnostic is best effort: if both
channels fail, only 74 remains observable to a waiter. Successful flush means
sink acceptance, not recipient consumption or hardware durability. Optional
capture/viewer availability is independent of this path.

Owner terminal and inspect reports carry `required_account`, also saved in
the fenced root row. It retains host-observed Bash outcome/seal/publication
failures and endpoint record-error diagnostics independently of optional
events or live output. Physical receipts remain distinct from persistence;
an observed wait or no-start whose outcome write failed is not classified as
an open live Bash work in this owner's settlement. Endpoint observations keep
bounded tags and contrary-diagnostic provenance, with raw details withheld;
they establish no canonical publication and change no ACK, tagged end, input
debt, control or physical closure. Detail is bounded to 64 records of 4096
bytes each; counts include omitted details and overflow makes completeness
false. Store/account write failures are reported from owner memory, without
claiming durability. The frontdoor carries the latest observed required
account through its retirement report even if its private account file write
failed. After explicit discard of a retirable root, that report lives only in
the opening caller's capture under its existing limits; a root whose account
does not describe retirement first keeps a loss account (below). Owner death
before delivery can lose unsaved observations. No second registry, recovery actor or replay permission is added.

The frontdoor does not read, convert, stage, refresh or scrub native credential
files. Keep retains adapter state. Discard removes the package run tree with
descriptor-safe removal after physical entry termination. The final package
terminal retains the owner's logical account (including owed/pending work),
with explicit unknown if the latest account could not be published. A failed
publication normally invalidates the superseded account and does not disable
later saves. If invalidation itself fails, the current relay still returns
unknown, while the older diagnostic file may remain; no durable failure flag
across relay death is promised,
or explicit unknown if unavailable, with `retry: do-not-replay`. Physical
store removal does not claim logical retirement. Keep explicitly preserves
`private/root-terminal.json`, the store and adapter diagnostics. A free run
lock permits later sweep with the same reported distinction; discard of an
unretirable store, at run end or by sweep, first retains its loss account. Direct recovery
stores remain useful until their recovery question ends. Cleanup failure is visible.
None of this is provider credential proof.

## Programmatic caller

```
<package>/bin/oulipoly-native-call --route parent --prompt-file TASK \
  --cwd TREE --out NEW-DIR --trusted-task --deadline 1800 \
  --child-route explorer --child-max-starts 1
```

The caller selects `sudo -n <realpath frontdoor> run`. Credentials are handled
by external adapters; old credential flags are usage errors. It sends request
and controls nonblockingly, captures stdout/stderr, closes on the parent's
first turn end and cancels at deadline/signals. Owed async completion turns
defer close within the same deadline. Stop allowance is 65 s, with 30 s from
terminal/partial EOF plus a 0.2 s helper exit attempt. Lines cap at 8 MiB and
capture at 64 MiB. Stop uncertainty remains incomplete; killing a helper does
not prove a privileged namespace ended. Kernel/filesystem stalls remain
outside these bounds.

A prompt RPC rejection supplies no insertion ACK or retry authority. The
owner keeps input and ordinary close on hold until cancel or peer exit, even
when the endpoint declares `INPUT_NOT_INSERTED`: that declaration is attributed
to the endpoint and is not physical non-insertion proof. The one-shot caller
exports these reports in `result.json.rejections` and keeps its existing
signal/deadline cancellation policy. A live turn returns immediately with
`turn.rejected`, `stop: rejected-unresolved` and class `incomplete`/6; its root
remains live. Use `--root HANDLE --stop` to cancel that root; do not resend the
prompt based on the rejection. If the owner dies after the wire error but
before its durable rejection stop commits, existing successor recovery can
resubmit the same owed key. This window is not an at-most-once guarantee.

The new 0700 output directory contains request.public.json, events.jsonl,
stderr.log, caller.jsonl, result.json, children.json and final.md when linked
parent text exists. Parent answer and automatic close ignore child-marked
events. Bash counts include children. Child exports may lag the parent's
local result while Bash drains; zero exports does not prove zero results or
parent consumption. Output is unredacted and can contain arbitrary secrets.

Caller exits: 0 answered, 1 no-answer, 2 usage, 3 local refusal, 4 frontdoor
refusal, 5 cancelled, 6 incomplete/stop unknown, 7 cleanup failed, 8 launch
failed, 9 ended otherwise, 10 async-undelivered. Type-only unexpected failures
are machine-readable; unwritable output can expose only stderr records.

Insertion conclusiveness, logical settlement and physical waits are distinct.
ACK proves insertion; tagged end plus the async debt account supports logical
settlement. For accepted background work, answered requires linked text,
initial end_turn, ACK/tagged completion end_turn for every owed completion,
consistent owner account, complete transport and successful scratch cleanup.
Unknown/undelivered debt cannot become answered from text alone. Answered/0
still establishes no semantic processing or correctness.

Owner closed/7 and Runner native87 denote physical run termination after
close, subject to insertion/refusal guards. U112's ACK-present/end-absent case
can retain async debt while physically closed. Neither code authorizes
logical root retirement. The relay reads actual entry waits for scratch
cleanup and passes owner records unchanged. Discard reports logical knowledge
separately, including unknown when the entry died without a final owner account.

### Root control face (`session_control/v3`)

The root owner speaks the shared root control vocabulary
`oulipoly.session_control/v3` from `agent-provider-contract` (see the owner
crate's `control` module). The front door relays, besides cancel/close/send,
`{"cmd":"inspect"}` and v3 `request` records (at most 32768 bytes). It
attests the requester: a request whose `requester` is not `uid:<this
requester's uid>` is refused here (`requester-not-attested`) and never reaches
the owner. Close and durable cancel arm kill grace only after the owner reports
the accepted effect, preserving close deferral for owed async completions.
Refused or unknown controls schedule no terminal effect. The independent
run deadline still bounds an unresponsive owner. What the owner answers:

- `input_hold` / `input_release`: refuse caller input received after the hold
  acknowledgment (`input-held`); already in-flight input may still be admitted.
  Running turns, tools and owner completions continue. No physical pause.
- `close`: durable; it enters the claim ladder and a later owner keeps input
  closed. The stdin `{"cmd":"close"}` is the same close.
- `cancel` (v3): the root's durable lifecycle cancel. The stdin
  `{"cmd":"cancel"}` (and this front door's deadline/abandon cancels) stays
  the owner instance's cancellation.
- `recover`: only through `native-root --recover` (`control` field), answered
  by the recovering owner of the same incarnation. Purposeful direct recovery
  is separately authorized; this optional sidecar accounts for effects and
  does not gate them. Unusable sidecars report account unavailable. Under this front door the
  entry is its namespace's init, so the root does not outlive its front door
  and no packaged recover path exists.
- `inspect`: a `control_state` record, settlement `observation`s and a
  `settlement` event whose `retirement.eligible` is true only when every
  input reads settled or not inserted on the owner's warranted reading, no
  async completion is owed, every launch and incarnation is recorded ended by
  its actual waiter and no control intent is pending. Custody is Runner
  aggregation over recorded actors, explicitly separate from SDK one-reference
  physical knowledge; actor completeness/native retirement remains unqualified.

The caller exposes `--root FILE --inspect | --hold | --release`, plus
`--control-request JSON` for an exact submission/replay and `--control-prior`
for its prior caller result. Hold/release use a fresh key. A fresh
`inspection_key` selects one owner envelope containing the actual advertisement,
shared state and settlement. The packaged supervisor's read-only
`--read-control` mode uses the SDK's schema, capability selection, agreement and
trace operations. `--control-reader` may name that reader explicitly; no binary,
banner or source revision participates in negotiation. Missing/incompatible
control disables this control encounter only; the root's execution continues.

A changed submission requires the caller's preserved original, including after
its final outcome. Both exact submission and original-trace conflict joins are
checked. The owner faithfully replays the original before the separate conflict
answer. Returned claims, rejected records and diagnostics remain in the caller
result. Fulfillment is distinct from acknowledgment and non-fulfillment:
`inspected`/`acknowledged`/`fulfilled` exit 0, `control-refused`/`unfulfilled`
exit 18, `control-unknown`/`control-unavailable` exit 19, and a contradictory or
incomplete account exits 6. Transition knowledge does not establish work
settlement. Raw owner events remain in the caller's capture as well.

Direct owner succession can enact admitted intent only under the actual store
owner history and generation fence, in the same recorded incarnation. Its own
present fulfillment preserves the original admission and manufactures no
predecessor ACK. Known positives are retained. Observed absent custody, or close
under durable cancellation, permits attributed terminal non-fulfillment;
missing admission, unknown custody and mismatched lineage remain unknown.

**Direct recovery guarantee scope.** The roots used here are the explicit
owner store (SQLite intent/control and recorded incarnation), its live root
PID 1 socket/receipts, and the recorded harness/provider state paths. This
supports deliberate owner-process loss on the current running host, with
successfully committed owner claims and identity-checked live custody. It does
not promise reconstruction of a run tree after host/power loss or establish
publication of incoming provider-state ancestry. A visible directory or later
SDK success is no receipt for an earlier failed publication. Diagnostic keep is
inspection. When provider publication/lineage is uncertain, use a fresh root
and retain prior input uncertainty/do-not-replay; this control face supplies no
new replay authority or provider durability guarantee. The existing direct
input retry mechanism does not enforce that lineage precondition, and remains
an explicit gap for callers requiring that higher guarantee. Packaged owner-only
restart remains unestablished; entry death has no survivor. Native providers,
all-actor custody, cutover, stress and broader review remain separately owed.

**Discovery.** Through the same sudoers `run` rule, stdin
`{"v":1,"op":"discover"}` lists this requester's run directories that hold a
root store: one `frontdoor: root` line each with a v3 `root_entry` (read by
`oulipoly-root-supervisor --describe` without claiming or locking the store),
whether this front door holds it live and its socket, then a `discovered`
terminal. Entries are derived and rebuildable addressing: not ownership,
admission, scheduling or a capacity reservation; their authority is the
store's last record. The live-root cap stays 4 per requester at this front
door, not a global reservation. Discovery reveals no handle token: attaching
a live root still needs its handle. Its terminal also counts this requester's
retained `loss_accounts`.

### Loss accounts: capture, retrieval and explicit copy disposition

A non-retirable discard first captures a per-input reading for the same OS
requester. After starting the entry, the front door restores its original
PID namespace **for subsequent children** so a reader can fork after entry
init dies. The entry stays its direct child, PID 1 with private `/proc` and
the existing parent-death/pipe race fence. Restore failure reports any started
entry and its actual kill/collected-status evidence in ordinary/live startup
failure and ending capture. Kill/collection is attempted, not guaranteed: an
unobserved end returns unknown (93), without capture, pruning or removal of
its source run. This is not survivor/recovery redesign.

After abrupt front-door death, capture only this requester's lock-free stale
runs without admitting a task or invoking any provider:

```
oulipoly-native-call --loss-op capture-loss-accounts --out NEWDIR
oulipoly-native-call --loss-op loss-accounts --out NEWDIR
oulipoly-native-call --loss-op loss-account --account RUN --out NEWDIR
oulipoly-native-call --loss-op retire-loss-account --account RUN --out NEWDIR
```

Equivalent JSON through the same sudoers `run` rule is
`{"v":1,"op":"capture-loss-accounts"}`, `{"v":1,"op":"loss-accounts"}`,
or `{"v":1,"op":"loss-account","account":"RUN"}` (or
`retire-loss-account`). `SUDO_UID` scopes the requester; no path override.
Task/live caller results carry route hints. Nothing captures/retires
implicitly in that caller; the next ordinary task launch still sweeps.

**List/read are observational; capture is effectful retention/disposition.**
Capture writes accounts and may prune/remove stale `discard` runs; `keep`
runs get an explicit reading without changing their retention policy. Its terminal/caller
result preserves incomplete capture/delivery as a nonzero outcome. Copies
stay available on output failure. Retirement explicitly deletes/syncs one
copy; it settles nothing. Successful exports are transport receipts, not
semantic use, receiver truth, processing or replay authority.

`run_base/<uid>/loss-accounts/<RUN>.json` is root-private (0700 directory,
0600 records). First creation syncs the parent; publication syncs file,
replacement and directory before checking the actual public reader. The
whole saved record must fit the same served **4 MiB byte bound**. Unreadable,
structurally malformed, identity-mismatched, unwritable, oversized or partial
readings keep the original store. Deferred discard prunes adapter scratch and the private entry request;
the kept DB still contains private task text under root custody, not a
scrubbed public store. Capture wording describes discard **intention**;
only the disposition result observes removal.

The outer terminal is a bounded allowlisted status/retirement/custody
summary, not its lineage, subjects or required-record bodies. It excludes
owner tokens, payloads, env/credentials and tool bodies. Known ending
status/kill evidence survives later sweep/refresh separately. Checked malformed
terminal/nested containers and identity mismatches return unknown/refusal or
partial results without discarding source evidence.

**Trusted-producer boundary:** the custody-checked Rust supervisor from the
same package owns per-input semantics and non-authority invariants. Python
checks structure, identity, bounds and listing counts at capture; its public
reader (also used by observational list/read) checks structure, identity and
bounds, not per-input facts, readings or nested retry/retirement semantics.
Reader acceptance and producer `complete:true` are not full semantic
validation. Well-formed but semantically contradictory producer output can
therefore be accepted and permit discard; producer correctness is a retained
dependency, not a malformed-output guarantee. Reopen on producer/schema
changes, a second producer, actual malformed producer output, or a consumer
using nested metadata as authority or completeness evidence.

The inner `root_store_account/v1` reads identity and facts in one SQLite
read transaction, claiming no generation, locking no owner, starting no
provider. ACK-without-end, durable `rejected-unresolved`, owed, NULL
(`unrecorded`) and `unknown-prior-owner` remain distinct. SDK basis is
`unwarranted`; retirement never eligible, replay never authorized. Open
custody rows after loss mean end unknown, not live/settled. These snapshots
establish neither native processing, receiver continuity, complete actor
custody nor final truth.

At most 256 inputs are listed; omissions/counts and `complete:false` are
explicit, and the store stays so unlisted evidence is not destroyed. Public
paging remains missing. This output bound does not bound producer work or
peak memory. **64 accounts is a soft capture threshold**: concurrent captures
can overshoot. No scheduler/reservation/automatic eviction is added.
Capacity/failure-kept stores remain discoverable and owed. Each later
sweep/capture can repeat a reader (30-second subprocess bound) per stale run;
aggregate latency/capacity/peak memory are not strictly bounded. Reopen before
large/live-root ordinary adoption or concrete ordinary pressure/latency.

Normal nonpurging uninstall/replacement preserves accounts. An empty exact
managed loss directory is not a live run; type/mode/custody and live/unknown
run fences remain. Nonpurging removal still returns 3 with intentionally
retained site effects/record, not an assertion that package/rule survived.
Deliberate `--purge-site` inventories and records exact managed account
unlink dispositions, reporting `purged-not-settled`; it does not settle them
or silently purge arbitrary data. Partial deletion accounting stays retained.

Source/owned userns fixtures are not privileged packaged loss/custody G2,
crash-storage durability, receiver semantics, universal privacy,
instruction import or efficacy qualification.

Frontdoor classes: native entry status or 90 refusal, 91 setup failure,
92 kill requested with collection, 93 stop/collection unknown, 94 cleanup
failure. Entry classes: 64 request refused before effects, 65 provider
refused after it ran, 73 setup effects possible, 66 owner refusal after
setup, 69 owner not started, 70 owner outcome unknown, 74 relay lost, or
80 plus the owner's nonzero class. None permits automatic replay.

### Live roots (explicit, bounded)

The fresh per-call behavior above is unchanged and remains the default. A live
root is only opened on explicit request. In that case one front door process
stays the root's owning supervisor after the call that opened it has exited.
Later, independent calls by the same requester can then address that root.

- **Open.** Run `oulipoly-native-call ... --live-handle FILE` with the ordinary
  options. This sends request `live: true` through the same sudoers `run` rule.
  - Admission, routes, children and the namespace entry are the
    same as for a fresh call.
  - After admission the front door forks its supervisor into a new session.
    The supervisor closes the caller's stdio, unshares, starts the entry and
    reports readiness.
  - The front door then answers with terminal stage `live-opened`, which
    carries a handle (`run`, socket, random token, uid), and exits 0. This exit
    is **not** the root's end.
  - The caller writes the handle to a new file (mode 0600), attaches, waits for
    input 0's own tagged turn, writes `final.md` and detaches. It never sends
    the automatic close.
- **Address.** Run `oulipoly-native-call --root FILE --prompt-file P --out DIR
  [--wait S]`.
  - The caller connects to the root's `live.sock` in the root-owned run
    directory. The socket is owned by the requester, mode 0600, and the
    supervisor checks every peer's uid with `SO_PEERCRED`.
  - The caller proves the run and token, then sends `send` with a fresh `ref`.
    It correlates its own input in this order: `follow-up-admitted` (same
    `ref`, giving the input index), then `ack`, then linked `agent-message`,
    then that input's `turn-end`.
  - The caller then detaches. `--wait` bounds this call only; the root is
    unaffected by it.
  - One caller is attached at a time; a concurrent one is refused with `busy`.
    While nobody is attached, records wait in a bounded backlog (8 MiB).
    Overflow is dropped and counted (`backlog_dropped`), never silently.
    Each attachment begins on a whole JSON record. If a caller leaves during
    a partial send, that entire interrupted record is discarded and counted;
    whole unsent records remain queued. Attach and terminal records expose
    cumulative dropped records/bytes, interrupted records and prefix bytes
    sent to departed callers. Counts describe transport loss, not processing.
  - `answered`/0 covers only this input's correlated ACK, linked text and
    tagged `end_turn`; it establishes neither semantic processing nor
    settlement of the whole root or its background obligations.
- **Close or instance stop.** Run `--root FILE --close` (or `--stop`). Close
  follows its durable acknowledgment, with deferral for owed background
  completions. Stop ends this owner instance through stdin cancel; it is
  distinct from the shared face's durable lifecycle `cancel`.
  - The supervisor stops listening and unlinks the socket. It retires the run
    as for a fresh call, sends the terminal record (`live` attaches,
    refusals, drops) to the attached closer, and exits with the entry's code.
  - Physical close and the caller's outcome are separate. `closed`/0 requires
    clean retirement, collected EOF, a complete entry relay terminal and a
    consistent owner terminal async account, with all accepted completion
    inputs covered by ACK/tagged turn ends and no counted transport loss.
    It establishes no semantic processing or task correctness.
  - The live supervisor keeps a runtime-only accounting witness across
    attachments (parent async reports/admissions, ACKs, turn ends and owner/
    entry terminals, without answer bodies). It is bounded at 8 MiB and
    exported in the final front-door terminal as `account_events`;
    `account_errors`, the reconciled `async`, and raw terminals remain visible.
    Missing, contradictory or limit-truncated evidence is `incomplete`/6.
    Known undelivered completions are `async-undelivered`/10, even after clean
    physical retirement. Counted transport loss also precludes `closed`/0.
  - Precedence: observed cleanup failure/7, then incomplete transport/stop
    collection/6, then explicit cancellation or owner kill/deadline/5, then
    accounting incompleteness/6, known async loss/10, and transport loss/6.
    Other owner exits remain `ended-otherwise`/9.
- **Bounds.**
  - `deadline_s` (site maximum) bounds the whole live root, not one call.
    When it expires the root is cancelled, then killed, even if nobody is
    attached.
  - At most 4 live roots per requester are allowed, including admissions
    still in progress before socket creation. A per-requester nonblocking
    admission guard covers count/allocation; the allocated run's locked
    reservation holds its slot until retirement. A competing guard holder
    is refused explicitly (`requester admission busy`); the cap refusal is
    `4 already held`. Dead ones (lock free) are not counted and are swept by
    the next run.
  - The supervisor holds the package's shared lock and the run lock for the
    root's life.
- **Outcomes (caller exits).**

  | Exit | Class | Meaning |
  |---|---|---|
  | 11 | `root-absent` | No address: the root ended and was retired, or never existed. |
  | 12 | `root-dead` | The address is present but nothing is listening; the supervisor died, and the lock is free for the sweep. |
  | 13 | `root-foreign` | The address is not accessible, or the peer is another uid. |
  | 14 | `root-refused` | The token or run is wrong (`handle-token`, `not-this-root`), or the reason is `busy` or `hello-timeout`. |
  | 15 | `follow-up-refused` | The owner refused the input (for example `input-closed`). |
  | 16 | `root-ended` | The root ended before this input's turn did. |
  | 17 | `root-unavailable` | Address/transport could not be used; no root state is inferred. |
  | 3 | `refused-locally` | Malformed/unusable handle (including overlong AF_UNIX address), or another uid. |

  `root-absent` after an unattended deadline or retirement is deliberately
  ambiguous: no terminal outcome can be inferred, including success or
  failure. There is no terminal history, automatic fallback, replay or restart.
  Failure before attachment uses the declared class and stores `result.json`
  in the caller output directory; an unwritable/uncreatable output directory
  can only expose its machine-readable refusal on stderr.
- **Not provided.**
  - Stopped-root delivery, restart, recovery and resume.
  - Dedup across owners.
  - Cross-supervisor messaging.
  - A global daemon.
  - Generic descendants.
  - The ordinary `agents` entry and the temporary dispatcher do not open live
    roots.

`test_live.py` contains unprivileged stand-in controls for separate callers,
constructor-initialized record framing/loss, the actual concurrent admission
boundary and in-progress reservations, malformed public handles, and honest
async close outcomes. The killed-supervisor stand-in did not itself die from
PDEATHSIG and is explicitly killed by the control: it supplies no containment
proof. Real owner/provider, privileged namespace/sudo/session policy and
package replacement while a live root holds its lock remain ROOT qualification.

## Ordinary `agents` entry (`native.toml`)

Ordinary Linux CLI launch requires `<runner config root>/native.toml`
(normally `~/.config/oulipoly-agent-runner/native.toml`). With valid
configuration, the launch forms
`agents -m MODEL PROMPT`, `agents AGENT PROMPT` and `--agent-file` run as one
call of the configured installed caller, before any legacy
maintenance/owner/State/provider path. The selected model name, from `-m` or
from the agent's frontmatter `model:`, must be mapped explicitly; the site route selects an external provider and its opaque adapter settings:

```toml
caller = "/opt/oulipoly-native/<id>/bin/oulipoly-native-call"
runs_dir = "/home/nes/.local/state/oulipoly-native-runs"   # prompt files + caller records
deadline_s = 1800                                            # optional; 1..7200

[models."codex~high"]
route = "sol-high"
bash = "trusted-task"                    # or bash_allow = ["whole command", ...]
children = ["luna-max"]                  # optional explicit explorer opt-in
child_max_starts = 1

[models."claude~medium"]
route = "opus-medium"
bash = "trusted-task"
```

The final answer goes to stdout; a status line (class, caller and front-door
exits, record directory) goes to stderr. Answer read/write/flush failures are
also reported there; after caller success they return entry exit 6, while a
nonzero caller exit is preserved. Caller 0/answered is not correctness.
Unmapped models, a missing/unreadable/invalid file, `--resume`, `repl`, `resume`, `--new`,
provider pinning/rotation and `-i`
with `-m` are refused with exit 3 and nothing launched: no legacy launch and
no substitute model. An unresolved config root is also refused with exit 3.
Missing configuration never falls through to the legacy owner/State path.
Help/usage, non-launch subcommands and GUI retain their separate paths; this
launch closure does not retire those or the wider legacy source.
Authentication belongs to the selected adapter. Credential-source fields in
`native.toml` are unknown fields and are refused. One attempt, synchronous,
no streaming or resume yet.

## Qualification scope

Changed gates retire embedded-only runtime/setup/credential tests and adapt
retained schema, caller, custody, child and transport checks to registered
routes. Stand-in providers and deterministic peers distinguish generic
negotiation/owner behavior; they are not native Codex/Claude qualification.
Unprivileged namespaces are not host-root custody/work-UID evidence.

Full review/cohort/owed visits, cold/alignment/import/efficacy, CI, native,
recovery, stress and benchmarks remain separately owed. Mailbox/supervisor/
StateDb retirement is separate work. Nothing here installs, activates routes,
selects services/default transport/accounts or completes a ticket.
