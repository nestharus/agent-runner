# Native completion continuation v2

This candidate supports **fresh isolated domains only**. It is not production
transition authority; AGE365 owns legacy domains, old pinned writers and migration.
The State source registration remains the admission authority. No original Bash
workload is launched by completion recovery.

## Lifetimes

- Domain guardian (CG) owns the endpoint and supervises driver (CD). CD retains
  the election open description and promotes itself on loss of its original
  parent, rebinds the endpoint, and starts a new driver generation.
- Native-entry identity leases survive CG loss, including before the first source
  registration. Socket EOF normally releases that admission lease. After loss of
  its socket owner, the entry's exact process lifetime bounds the retained lease;
  this is **not** physical workload/attempt drain evidence.
- Each operation has an original per-attempt adopting guardian above its AC,
  established before AC can launch. AC loss reparents its descendants to that
  original boundary. It polls cancellation and signals only its own children,
  waits the exact AC and then ECHILD, retains an adopting receipt and integrates
  against the exact registered adopter/custodian identities.
- CD/adopter announcement and adopter/AC launch release use separate sockets.
  AC closes the announcement endpoint before exec; adopter relays only the real
  CD grant after attachment. Adopter loss after AC fork therefore closes the
  original AC's release gate instead of creating a circular CD/AC read. A live
  original AC can certify the unreleased gate and actual ECHILD without launching.
- Database observation runs off the physical wait loop. Raw terminal waits retain
  their process incarnation before reaping, so a delayed launcher identity read
  cannot discard cancellation evidence. These reads do not impose a workload
  deadline. An observation timeout/error is not an exit or cancellation.
- Driver replays original retained `result.json` / `adopting-result.json` after
  producer loss. ACK, process absence, new empty ownership and newer attempts
  cannot substitute for either receipt. Existing source recovery reservations
  across every generation enforce one live recovery per source and four per domain
  in the same immediate transaction as reservation.

Simultaneous loss of both per-attempt owners **without a retained result** is
still unknown physical custody. A replacement domain owner does not certify that
old tree. The new per-attempt adopting boundary adds a process and retained records;
it is not a proof against arbitrary repeated owner loss or a production capacity
claim. The existing original CD/CG are subreapers, but their shared reap loops do
not retain per-attempt wait/cancellation association. Their actual domain-wide
ECHILD is not currently integrated as an original-custody certificate. This is a
continuing-ownership gap even without losing or replacing CD/CG.

Pre-attachment orders are distinct: adopter death before AC fork produces no AC
receipt; adopter death after AC fork may leave an original AC able to certify an
unreleased gate; CD loss before attachment can also leave that genuine original
AC receipt. AC exec failure or loss of both receipt producers is not covered by
that counterexample. Retained accepted uncertainty is not completed recovery.

## Synchronous presentation policy

Synchronous completion is presented through the command/tool response, not a
runner completion notification to the original synchronous caller. This remains
true if the host is interrupted or loses that response: the user inspects retained
logs/status and chooses recovery. There is no automatic notification fallback,
host-result ACK bridge, lease, consumed flag, or grace period.

Acceptance and repeated bound registration/repair both preserve the original
sync listener's inactive state. Independently registered listeners still receive
notifications; the original owner identity comes from the immutable source binding,
not the caller performing repair. Async/headless sources retain notification delivery.
Explicit activation/detach before or after completion enables delivery; subsequent
acceptance or repair never deactivates that listener. Polling alone does not ACK it.

Suppression is a presentation choice, **not remote ACK or physical drain**. Source
acceptance, work tracking, retained output and physical custody continue unchanged.
Inactive unacknowledged listeners and accepted output remain retained. Schema 19
separately records notification policy, first explicit request and qualified ACK
evidence. An exactly accepted response-only listener with no request, activation
or mailbox item settles only automatic notification debt for normal owner
retirement; it does not ACK the listener or discharge physical custody. Other
retirement predicates and source/output retention are unchanged.
Already active/materialized notifications from older code are not retroactively
withdrawn: active state can also represent an authorized detach. This policy is
not a production cleanup or migration procedure.

The sidecar `completion_continuation/notification.rs` module owns initial listener
eligibility, exact-binding classification, explicit requests and materialization.
Acceptance and admitted-source repair call its shared reconciliation operation
inside their existing transaction. Request retries keep the event/listener
identity and first-effective request facts; they neither execute work again nor
create authenticated initiating-actor provenance. Bash's local handoff mirror is
not this authoritative commitment or recipient receipt.

## Publication and output

Canonical examples: `crates/oulipoly-state/tests/fixtures/age360-paired-wire.json`,
fixture revision 4. These bytes are coordination examples, not a paired result.

Ready registration accepts a genuine pre-sentinel `exit_root` / `exit_tree` with
matching scope, raw terminal wait/rc and output closure (plus Tree drain for Tree).
It requires null `ready_sentinel`; a successful rc alone is not sentinel success.

New snapshots describe output at every size, including empty output, as a
`retained-output-v1` object with exactly:

- `relative: "completion-output-v2.bin"`
- `sha256`: SHA-256 of the complete frozen **raw** log bytes
- `byte_len`: exact length, at most 1 GiB (the supported raw log ceiling)
- `encoding: "raw"`: complete raw bytes are retained without UTF-8 replacement
- `representation: "retained-output-v1"`

Runner also accepts older `encoding: "utf8-lossy"` artifact descriptors as raw
files. Older inline JSON strings retain their lossy UTF-8 semantics.

Bash freezes and syncs the immutable file before publishing its descriptor.
Runner streams no-follow regular-file verification and copies it to its domain's
content-addressed `outputs/<sha256>`, syncing before event acceptance. The bounded
mailbox payload includes the complete snapshot and
`output_artifact={path,sha256,byte_len,encoding}` for the runner-owned copy.
That attachment is the full event body, **not a prefix**. The old local
snapshot/accept-output byte receipt remains separate from event acceptance and ACK.
Snapshot JSON stays at 16 MiB and registration/outcome at 1 MiB. Neither escape
expansion nor a body-sized JSON allocation is required for the artifact form.

No GC/release transaction is added. Exact accepted output remains available to
late listeners; retained storage and unknown physical debt are not capacity ready.

## Native fault fixtures

Build runner with `--features age360-fault-fixtures`. Normal binaries contain none
of the hold logic. A fixture must supply `AGE360_FAULT_ROOT` and
`AGE360_FAULT_PARENT_NET` (the outer network namespace link); the current network
namespace must differ. A `<name>.hold` file causes `<name>.reached` to record the
actual process PID, then holds that boundary until the hold file is removed.
Only private-namespace test processes may be signalled by the harness.

Boundaries:

| Name | Held boundary |
|---|---|
| `fresh-schema17` | Fully written private staging17 before v2 initialization; final DB name absent |
| `registration-committed` | State + sidecar registration committed, before response |
| `acceptance-committed` | Exact completion accepted, before response |
| `ac-result-retained` | Original AC has persisted wait/ECHILD receipt, before integration |
| `driver-replay` | Driver before retained-result replay |
| `activation-observation` | Observer thread before DB read, physical wait loop remains live |
| `adopted-terminal-wait` | Fixture-only pause after retaining an actual terminal wait |
| `adopter-before-ac-fork` | Original adopter before creating AC; reservation already accepted |
| `adopter-before-ac-announce` | Original adopter after AC fork, before announcing AC to CD |
| `attempt-before-attachment` | CD received AC PID, before State custodian/adopter attachment |
| `adopter-before-ac-release` | Original CD grant received after attachment, before forwarding grant to AC |
| `driver-reaped-echild` | Actual original CD reap loop observed ECHILD; fixture observation only, never attempt discharge |

`src-tauri/tests/age360_completion_continuation.rs` uses private user/network/PID/
mount namespaces and an external-process local provider. `native_` cases do not
claim Bash/source pairing. Four paired cases require explicit
`AGE360_AGENT_BASH_BIN`, reject a missing executable, and never select an installed
or simulated fallback. The two added paired cases discriminate early exit and
complete artifact output. The full root-owned paired fault matrix remains larger
than these four cases; no fixture or feature flag proves that matrix ran.

`fixtures/age360/custody_faults.rs` discriminates those native pre-attachment and
combined attempt-owner loss orders. Tests named `observes_unresolved_*` are
explicit **gap characterizations**, not recovery acceptance tests: a passing test
means unresolved debt was observed. In particular, observing original CD adopt
and reap descendants without producing an attempt receipt must not be reported
as completed recovery. Namespace teardown contains remaining test processes; it
is never product drain evidence.

Native State-token cancellation tests require an actual logical launch joined to
the activation's runtime generation and invocation before calling
`StateDb::request_cancel`, then require the exact token in the AC/adopter receipt.
They do not insert synthetic launch rows or send terminal signals. **Currently
both tests fail at the real State join**: the native streaming fixture creates
no `provider_launch_attempts` row. The observer's logical-launch cancellation
query is implemented but has no native producer link in this candidate. Actual
State-linked cancellation is therefore unverified and unfulfilled here, not
covered by the passing terminal-signal cancellation tests. These red tests are
retained requirements, not ignored tests or a completed cancellation matrix.

### Paired test executable provisioning (Linux)

The AGE360, proactive-wake and S11 paired fixtures use
`tests/fixtures/bounded_runner_image.rs`. Each test process creates a separately
owned ELF copy of its explicit candidate (or Cargo's exact built executable),
runs `/usr/bin/strip --strip-debug`, and fails if the result exceeds the unchanged
256 MiB helper-image bound. Missing strip, non-executable/non-ELF input and copy
failure are errors, not skips. It prints both image hashes and sizes. Cargo's
original executable is never stripped or hard-linked for transformation.

This same provisioning runs under normal package/workspace tests, coverage and
release-workflow tests; it is not a hosted-only profile override. Debug stripping
must preserve executable and LLVM instrumentation/mapping sections. Coverage
uses the original Cargo object for report discovery and the instrumented copy for
execution; the native namespace re-exec forwards `LLVM_PROFILE_FILE`. Local
instrumentation/report checks do not establish a hosted coverage run, full suite
coverage, or signed production artifact equivalence. This fixture transform does
not apply to release publication or installed executables.

The three proactive async tests run their inner assertions as a synthetic provider
of a real outer Runner entry. Registration uses that live entry's inherited
invocation authority and owner endpoint, with a distinct outer session listener.
Removing the inherited endpoint must still reject managed ancestry. The outer
entry stays live through the inner initial/delivery relationship checks; fixture
rows no longer impersonate the test process as an independently bootstrapped owner.
