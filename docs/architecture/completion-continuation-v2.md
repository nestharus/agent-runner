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
claim. Pre-attachment fork/exec failure orders still require further native fault
coverage and disposition; retained accepted uncertainty is not completion.

## Publication and output

Canonical examples: `crates/oulipoly-state/tests/fixtures/age360-paired-wire.json`,
fixture revision 4. These bytes are coordination examples, not a paired result.

Ready registration accepts a genuine pre-sentinel `exit_root` / `exit_tree` with
matching scope, raw terminal wait/rc and output closure (plus Tree drain for Tree).
It requires null `ready_sentinel`; a successful rc alone is not sentinel success.

Small snapshot output remains an inline JSON string. Full supported output can
instead be an explicit `retained-output-v1` object with exactly:

- `relative: "completion-output-v2.bin"`
- `sha256`: SHA-256 of the complete frozen **raw** log bytes
- `byte_len`: exact length, at most 1 GiB (the supported raw log ceiling)
- `encoding: "utf8-lossy"`: complete raw bytes are interpreted with the existing
  UTF-8 replacement semantics, not modified or truncated during retention
- `representation: "retained-output-v1"`

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

`src-tauri/tests/age360_completion_continuation.rs` uses private user/network/PID/
mount namespaces and an external-process local provider. `native_` cases do not
claim Bash/source pairing. Four paired cases require explicit
`AGE360_AGENT_BASH_BIN`, reject a missing executable, and never select an installed
or simulated fallback. The two added paired cases discriminate early exit and
complete artifact output. The full root-owned paired fault matrix remains larger
than these four cases; no fixture or feature flag proves that matrix ran.
