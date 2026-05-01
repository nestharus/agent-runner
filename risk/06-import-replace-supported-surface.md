# 06-import-replace — Phase 4 Supported-Surface Risk Report (Rev 4)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

This is the Round 4 supported-surface review of `proposals/06-import-replace.md`
Rev 4. Round 1 verdict was LOW with four non-terminal findings; the audit
track was HIGH (AIR-R1-F01..F04). Rev 2 closed AIR-R1-F01..F04 and was
re-reviewed at Round 2 supported-surface as LOW with five carryover findings
(R2-F01..R2-F05). Round 2 audit re-opened HIGH on AIR-R2-F01 (journal
recovery underspecified, cleared too early). Rev 3 closed AIR-R2-F01 and was
re-reviewed at Round 3 supported-surface as LOW with one new ordering note
(R3-F01) plus four carryovers (R3-F02..R3-F05). Round 3 audit re-opened HIGH
on AIR-R3-F01 (per-session journal artifacts published before `SessionLock`
acquire, allowing two `import-replace` processes for the same resolved session
to overwrite each other's recovery signal before the loser observes
`session-busy`). Rev 4's announced scope is to close AIR-R3-F01 by reordering
the success flow so canonical records first land in a per-process
operation-unique staging path, then are atomically renamed to the per-session
canonical-records path **after** `SessionLock` is acquired, and the pending
journal entry is written under lock; §6 journal schema gains `operation_uuid`
to associate journal with canonical-records across the rename; §9 adds
`T-concurrent-import-replace`; §8 side-effects updates to include the staging
directory and operation_uuid usage.

This review confirms that closure from the supported-surface lens, runs the
no-regression check on adjacent paths and cohorts under the documented threat
model, registers the four Round 3 prose carryovers (still untouched in Rev
4), and verifies that the Rev 4 lock-before-journal sequencing is race-free
under the cooperative-lock threat model. Net value remains positive on the
supported surface; no termination signal fires.

The originally referenced `risk/06-import-replace-audit-history.md` is not
present at HEAD; the Round 1 history exists at git commit `4a598ac` and is
treated as authoritative for prior rounds. Round 2 / Round 3 audit findings
are read from the Round 2 / Round 3 audit files at their respective commits
(`6f3f1fd`, `9191a52`).

## Concern 1 — Closure of AIR-R3-F01 from the supported-surface lens

This concern is closure-only audit on the single Round 3 audit finding. It
asks whether the closure is real on the public-CLI surface and whether it
introduces an unbounded blast-radius item.

### AIR-R3-F01 — pre-lock journal publication is racy — CLOSED

Round 3 audit had three required changes:

1. Acquire `SessionLock` before publishing per-session journal artifacts; **or**
   use operation-unique journal/canonical paths plus an ownership token so
   only the lock owner can publish, consume, or delete the active per-session
   pending entry.
2. Eliminate the "may unlink the journal and canonical records file
   idempotently before exit" clause for the lock-busy loser, since that
   clause itself was the deletion of another contender's recovery signal.
3. Add a concurrency test where two `import-replace` processes target the
   same resolved session, one wins the lock, the loser exits `13`, and the
   winner's journal, canonical records, transcript, and DB update remain
   intact.

Rev 4 delivers all three by adopting **both** halves of the alternative
fix — operation-unique staging path *and* lock-before-publish ordering — so
no part of the per-session journal or per-session canonical records file is
ever written outside the lock.

**Operation-unique staging path before the lock.** §4 pre-mutation step 3
allocates `operation_uuid` for all scratch paths in this process. §4
pre-mutation step 6 writes normalized canonical records only to
`<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`,
explicitly annotated "This is not a per-session journal artifact." §8 side
effects mirror this: "Before acquiring the session lock and before writing
any per-session journal artifact, import-replace writes normalized canonical
records only to
`<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
This staging path is operation-unique scratch state."

**Per-session paths only under lock.** §4 success-flow step 3 acquires
`SessionLock`; step 4 executes "Now under lock, atomically rename the staging
canonical file to its journal-attached per-session path:
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`";
step 5 writes the pending journal at
`<state-data-dir>/replace_journal/session-<session_id>.pending` "under the
same lock." §8 makes the invariant explicit: "Only the lock owner can land
canonical records at this path." This is exactly the Round 3 alternative
"acquire `SessionLock` first and write the journal under lock," with the
operation-unique staging file solving the secondary problem of where to
park bytes computed before the lock attempt.

**Loser unlinks only its own staging file.** §4 success-flow step 3 says of
the busy outcome: "before returning, unlink the staging file. No
per-session journal artifact has been published." §8 repeats: "If
`SessionLock::acquire` returns busy, import-replace unlinks only its
staging file and exits `13`; it must not create or modify
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`
or `<state-data-dir>/replace_journal/session-<session_id>.pending`." Rev
3's "may unlink the journal and canonical records file idempotently
before exit" clause is gone. Other handled failures after staging
creation but before lock acquisition are also explicitly scoped to the
operation-unique staging file only (§4 closing prose, §8).

**Operation_uuid pins the staging→canonical rename across the journal.** §4
journal format adds `operation_uuid`; §6 startup recovery contract reads
`operation_uuid` and explicitly notes the field is "required so crash
recovery can associate the journal with the canonical-records file." This
keeps the §6 recovery determinism intact under the Rev 4 ordering: the
canonical-records file at the per-session path is unambiguously the one
this journal entry was written for, because both arrived under the same
lock and share `operation_uuid`.

**T-row coverage.** §9.1 adds `T-concurrent-import-replace`: two
subprocesses, exactly one returns `0` with valid receipt and final
transcript/export matching the winner; the loser returns `13 session-busy`,
unlinks its staging file, leaves no per-session journal/canonical files in
`<session>.canonical.jsonl` or `<session>.pending`, and performs no
transcript mutation. This is the Round 3 required concurrency test verbatim.

**Rev 4 success-flow vs Rev 3 (delta only):**

| Step | Rev 3 | Rev 4 |
| --- | --- | --- |
| Pre-lock canonical write | Per-session path `session-<id>.canonical.jsonl` | Operation-unique staging path `staging/<operation_uuid>.canonical.jsonl` |
| Pre-lock journal write | Per-session path `session-<id>.pending` written **before** lock | Not written pre-lock |
| Lock-busy cleanup | "May unlink the journal and canonical records file idempotently before exit" | Unlink **only** the staging file; per-session paths untouched |
| Per-session canonical-records publish | Pre-lock write | Atomic rename **under lock** from staging path |
| Per-session pending-journal publish | Pre-lock write | Write **under lock** |
| Recovery association | Path-name only | Path-name + `operation_uuid` field |
| Concurrency test | Not present | `T-concurrent-import-replace` |

Supported-surface effects of the closure:

- Receipt JSON shape is unchanged from Rev 3 (`session_id`, `provider_name`,
  `storage_type`, `operation`, `preimage_sha256`, `postimage_sha256`,
  `jsonl_path`, `state_updated`, `committed_at`); cohort A parsers do not
  need to update. The new `operation_uuid` field is journal-private, not in
  the receipt.
- One new private filesystem surface is introduced under the existing
  `<state-data-dir>/replace_journal/`: the `staging/` subdirectory holding
  per-process operation-unique scratch canonical files. Documented as
  private implementation state in §4 / §8 / §11.1; no public CLI flag
  exposes it. Cohort A / B do not gain or need to read this path.
- The per-session canonical-records path
  (`session-<id>.canonical.jsonl`) and the quarantine subdirectory remain
  the only non-pending recovery artifacts, exactly as Rev 3.
- The lock-busy contract is now stricter in Rev 4: a loser cannot affect any
  recovery artifact owned by another contender. This eliminates the
  AIR-R3-F01 specific failure mode (B unlinks A's pending entry) entirely,
  not just for the cohort-A single-orchestrator case but across the full
  cooperative threat model.
- The recovery contract continues to read identity from the journal; §6
  startup recovery is unchanged in shape and gains `operation_uuid` only as
  an integrity association, not as a recovery branch.

Closure verdict: **real and complete on the supported surface.** The audit
HIGH retires; no new unbounded blast-radius item is introduced. The new
private `staging/` subdirectory is bounded private state under the existing
data-dir, fsynced, with stale-file cleanup contemplated by §8 crash state #1
("future startup or import-replace runs may unlink stale staging files by age
and operation UUID"). Rev 3's R3-F01 ordering note is closed, not carried
over.

## Concern 2 — Closures of AIR-R1-F01..F04 and AIR-R2-F01 still standing

Round 3 supported-surface review confirmed all five prior closures real and
complete on the supported surface. Rev 4 changes are restricted to:

- §4 success flow / journal format (staging→rename ordering and
  `operation_uuid`).
- §6 reusable API and journal-write description (staging step + under-lock
  rename + `operation_uuid` extraction during recovery).
- §8 side-effect contract / crash states (staging path; loser scope
  narrowed).
- §9.1 (one new T-row: `T-concurrent-import-replace`; existing rows updated
  in their lock-busy / preimage-mismatch language to reflect the new loser
  scope).
- §1 "Rev 4 changes" log.

No Rev 4 change touches the rendering contract, the field-loss contract, the
cooperative-lock prose at §13, the receipt JSON shape, the schema-probe
gating, or the cross-feature constraint compliance in §13. Each prior closure
is re-verified below.

### AIR-R1-F01 — provider-native rendering — STILL CLOSED

§3 renderer contract is unchanged in Rev 4 (`CanonicalToProviderRenderer`,
`claude_code` and `codex_session` implementations, `UnsupportedStorage` for
`other`, `15 invalid-input-transcript` with
`unsupported-record-class:<class>` for lossy classes,
round-trip-through-export requirement). §1 still states: "The replacement
transcript file does not store canonical JSONL in v1. It stores
provider-native bytes rendered from canonical input for the resolved storage
type." §13 compliance row "Provider transcript file receives provider-native
bytes, not canonical bytes" is unchanged. Round 3 supported-surface evidence
holds.

### AIR-R1-F02 — durable journal recovery — STILL CLOSED

The R1-F02 mechanism (durable journal, startup recovery, deterministic
reconciliation) is preserved. Rev 4 does not retract the
verification-before-deletion ordering or the canonical_records_path-as-source
recovery — both are unchanged. The journal lifecycle is now strictly
narrower (only the lock owner can publish per-session entries), which makes
the recovery determinism stronger, not weaker. §8 crash states #1 (staging
crash) and #2 (post-rename, pre-temp-write) reflect the new ordering;
neither admits ambiguous DB recovery. Round 3 evidence holds; closure is
tightened.

### AIR-R1-F03 — cooperative-lock contract — STILL CLOSED at §13

§13 row "Lock observation for import-replace once pause-handshake lands"
prose is unchanged in Rev 4. The Round 2 / Round 3 prose carryover at §12
residual #3 and §11.1 cohort-A is also unchanged in Rev 4 and is
re-registered as R4-F02 below (same content as R3-F03). The contract
remains unambiguous to a §13 reader.

### AIR-R1-F04 — canonical record field-loss — STILL CLOSED

§6 DB update API, §7 step 4, §12 residual, and §13 compliance row are
unchanged in Rev 4. Replaced sessions still write `parent_turn_id`,
`is_sidechain`, and `is_compaction_boundary` as `NULL` or schema defaults;
§9.1 T-row "DB metadata loss is explicit" is unchanged. The R3-F05 cohort-C
prose gap (§11.1 does not enumerate the partial DEGRADED state) is unchanged
in Rev 4 and is re-registered as R4-F04 below.

### AIR-R2-F01 — journal recovery underspecified / cleared too early — STILL CLOSED

Rev 3's frozen resolved identity in the journal, persistence of canonical
postimage material, verification-before-deletion ordering, and four
recovery T-rows are all preserved verbatim in Rev 4. Rev 4's only edit to
this surface is the addition of `operation_uuid` to the journal format and
the reordering of *when* canonical records are published at the per-session
path (under lock instead of pre-lock). Neither edit weakens AIR-R2-F01's
closure; both strengthen it (operation_uuid tightens the journal-to-records
association; under-lock publication eliminates the pre-lock concurrent-
overwrite case that AIR-R3-F01 surfaced).

### AIR closure summary (Rev 4)

| Audit finding | Round 3 status | Rev 4 status | Supported-surface residual |
| --- | --- | --- | --- |
| AIR-R1-F01 (HIGH, native bytes) | Closed | Still closed | Renderer record-class scope (R4-F01). |
| AIR-R1-F02 (HIGH, crash recovery) | Tightened by AIR-R2-F01 closure | Still closed; tightened again by AIR-R3-F01 closure | None new. |
| AIR-R1-F03 (MEDIUM, lock observation) | Closed at §13 | Still closed at §13 | §12 / §11.1 prose carryover (R4-F02). |
| AIR-R1-F04 (MEDIUM, field loss) | Closed | Still closed | Cohort-C prose gap (R4-F04). |
| AIR-R2-F01 (HIGH, journal underspecified / cleared early) | Closed | Still closed; tightened by `operation_uuid` | None new. |
| AIR-R3-F01 (HIGH, journal write before lock) | n/a (Round 3 audit blocker) | Closed | None new — staging path is private bounded state. |

All six closures are real and bounded on the supported surface. No
termination signal fires from the closure check.

## Concern 3 — Race-free check on the Rev 4 lock-before-journal reorder

The Round 4 obligation specifically asks whether the Rev 4 lock-before-
journal reorder is race-free for the documented threat model. The threat
model (R1-F03 / §13 / §11.1) is unchanged from Round 3:

- Cooperative-lock surface keyed by `SessionLock` for the resolved active
  provider session id.
- v1 in-binary writer paths (`run_repl`, `run_resume`, balanced one-shot,
  `migration::migrate_chain_segment`) retrofit on PR #17's timeline; until
  retrofit, `session-busy` is advisory.
- Cohort A (`agent-harness`) is the primary consumer and is expected to be
  the sole orchestrator of `agents` invocations against any session it is
  actively replacing.

Under that threat model, the Rev 4 sequencing is race-free. Critically, the
Rev 4 sequencing is **also race-free against two concurrent
non-orchestrated** `import-replace` invocations against the same resolved
session id — i.e. the Rev 3 R3-F01 boundary case that fell outside the
documented threat model is now closed inside it.

### Two-process concurrent import-replace under Rev 4

| Time | Process A | Process B | Per-session disk state |
| --- | --- | --- | --- |
| t0 | Validates input; writes `staging/<uuid_A>.canonical.jsonl` | Validates input; writes `staging/<uuid_B>.canonical.jsonl` | None |
| t1 | Resolves session metadata; freezes identity | Resolves session metadata; freezes identity | None |
| t2 | `SessionLock::acquire` → owner | `SessionLock::acquire` → busy | None |
| t3 | Atomic rename `staging/<uuid_A>` → `session-<id>.canonical.jsonl`; write `session-<id>.pending` | Unlinks `staging/<uuid_B>`; exits `13` | A's canonical + journal |
| t4..N | Continues protected flow under lock | Already exited | A's canonical + journal |

There is no per-session disk path that B can write or delete. B's pre-lock
write target is operation-unique; B's lock-busy cleanup target is also B's
own operation-unique staging file. A and B's staging files do not collide
because UUIDs are unique. The per-session paths
(`session-<id>.canonical.jsonl`, `session-<id>.pending`) are written by the
lock-owner only.

If A crashes after writing its journal under lock and before deletion at
step 13, the `<state-data-dir>/replace_journal/session-<id>.pending` and
`session-<id>.canonical.jsonl` paths reflect A's identity unambiguously,
because no other process ever wrote to those paths. `operation_uuid` in
the journal points to A's canonical records. Recovery (§6) reads A's
identity, not anybody else's.

### Crash windows under Rev 4

| Crash point | Per-session journal state | Recovery action | Determinism guarantee |
| --- | --- | --- | --- |
| Before staging write | None | None needed (no per-session journal exists) | Fine — §8 crash state #1; staging-file cleanup deferred. |
| After staging write, before lock acquire | None | None needed (still no per-session journal) | Fine — staging file may linger; opportunistic cleanup contemplated. §8 crash state #1. |
| After lock acquire, before staging→canonical rename | None | None needed (staging-only state) | Fine — staging file may linger; opportunistic cleanup. |
| After staging→canonical rename, before journal write | Canonical file present, no `.pending` | §6 / §8 now explicitly handle the orphan canonical side-file case: recovery deletes it only when no live `SessionLock` exists, fsyncs `replace_journal/`, and does not mutate transcript or DB state. | R4-F05 resolved in Phase 6. |
| After pending journal write, before transcript temp write | Canonical file + `.pending` (preimage TBD) | §6 step 2 fallback: pending journal lacks completed `preimage_sha256` → treat as pre-rename no-op; delete journal + canonical records, no DB mutation | §8 crash state #2; deterministic. |
| After transcript temp write, before transcript rename | Same as above | Same recovery (preimage matches; no rename landed) | §8 crash state #3; deterministic. |
| After transcript rename, before SQLite begin | Canonical file + `.pending` (preimage recorded) | §6 step 4: postimage match → re-apply DB from canonical records; delete artifacts | §8 crash state #5; deterministic. |
| During SQLite txn (uncommitted) | Same as above | SQLite rolls back; recovery same as above on next start | §8 crash state #6; deterministic. |
| After SQLite commit, before deletion | Same as above | §6 step 4: postimage match → re-apply idempotently → delete artifacts; durable state equals post-commit | §8 crash state #7; deterministic. |
| Ambiguous external mutation between rename and recovery | Canonical file + `.pending` | §6 step 6: quarantine journal; preserve canonical records; transcript and DB untouched | §8 crash state #9; deterministic; `T-recovery-ambiguous-hash`. |
| Postimage hash mismatch under lock (verification step 10) | Canonical file + `.pending` | SQLite rollback; journal + canonical records preserved; exit `1`; next-start recovery either re-applies or quarantines depending on disk hash | §4 step 10 / §9.1 `T-no-deletion-before-verify`. |
| Fresh export verification mismatch under lock (step 11) | Canonical file + `.pending` | Same as above | §4 step 11. |

Every named crash window has a deterministic recovery rule, and every
recovery rule reads identity from the per-session journal that was written
under lock. The hairline window between staging→canonical rename and
pending-journal write is now explicitly covered by §6 / §8: recovery sees a
canonical-records file with no matching `.pending`, deletes it only when no
live `SessionLock` exists, and never mutates transcript or DB state without
journal authority. R4-F05 is therefore retained only as a resolved Phase 6
note.

### Race-free verdict

**The Rev 4 lock-before-journal reorder is race-free under the documented
threat model and additionally race-free against two concurrent
non-orchestrated `import-replace` invocations against the same resolved
session id.** The expanded journal carries enough frozen identity for
deterministic recovery; the under-lock publication of per-session
artifacts means the only writer of per-session paths is the lock owner;
the operation-unique staging path means contenders never collide on disk
pre-lock. The prior hairline-window observation (R4-F05) is resolved by the
Phase 6 orphan canonical recovery rule.

## Concern 4 — Fresh assessment of Rev 4 changes (assumption / net-value)

### Assumption register (Rev 4)

Rev 4 §1.1 republishes A1–A10 verbatim. No assumption is restated, narrowed,
or withdrawn. Rev 4's edits live below the §1.1 register at §4 / §6 / §8 /
§9.1. All ten **HOLD** under Rev 4 evidence (current migration code does not
have a pending-op table, no equivalent durable transcript-replace journal
landed in another sibling feature, `SessionLock` from 06-pause-handshake
remains the cooperative primitive, etc.).

**Termination signal #1 (`assumption_invalidated`) does not fire.**

### Net value (Rev 4 vs Rev 3 vs current state)

Round 3 retired thirteen problem-map / audit entries (twelve carried plus
the AIR-R2-F01 underlying gap). Rev 4 retains all thirteen and additionally
retires the audit entry that AIR-R3-F01 surfaced as unfinished business
(per-session journal artifacts published outside the lock, allowing
contender races to corrupt recovery state):

| Additional retirement | Retired by Rev 4 |
| --- | --- |
| AIR-R3-F01 underlying gap: per-session journal artifacts published before `SessionLock` admit a contender race | §4 success-flow staging→rename-under-lock + `operation_uuid`; §8 staging-only loser cleanup; §9.1 `T-concurrent-import-replace`. |

Fourteen problem-map / audit entries retired total against pre-Rev-1 state.

Blast-radius items vs Round 3:

| Blast-radius item | Round 3 status | Rev 4 status |
| --- | --- | --- |
| Wrong canonical bytes written under a valid lock | Bounded | Bounded (§3 / §6 unchanged). |
| Caller-supplied preimage stale by acquisition time | Bounded | Bounded (§4 success-flow under-lock recheck preserved). |
| Crash after rename before DB commit | Closed deterministically by frozen-identity journal + canonical_records_path | Closed; tightened — `operation_uuid` removes any path-name ambiguity in associating recovery records to journal. |
| Crash after DB commit before journal deletion | Closed by verification-before-deletion ordering | Closed (unchanged). |
| Postimage hash mismatch under lock | Explicit (§4 step 10) | Explicit (renumbered to step 10 under Rev 4 success-flow but content unchanged). |
| Fresh export verification mismatch | Explicit (§4 step 11) | Explicit (unchanged). |
| Stale temp files in transcript dir | Bounded (R3-F04 cosmetic) | Bounded (R4-F03 carryover). |
| In-binary writers not honoring `SessionLock` | Tightened at §13 | Tightened at §13 (unchanged); §12 / §11.1 prose carryover (R4-F02). |
| Provider-native renderer record-class scope | Bounded by `15 invalid-input-transcript` | Bounded (R4-F01 carryover). |
| Startup-recovery scope on every `agents` invocation | Bounded but ambiguous (carryover) | Bounded but ambiguous (no Rev 4 change; not separately re-registered, was R3-F01b sub-note). |
| Replaced-session metadata loss on resume / trace | Bounded by §6 / §7 / §12 (R3-F05 cohort gap) | Bounded (R4-F04 carryover). |
| Quarantine subdirectory | Bounded private filesystem state under existing data-dir | Bounded (unchanged). |
| **Pre-lock per-session journal publication race** (Rev 3 R3-F01 / Rev 3 audit AIR-R3-F01) | OPEN (Rev 3 supported-surface logged as non-terminal under threat model; Rev 3 audit logged as HIGH blocker) | **CLOSED**: §4 / §6 / §8 / §9.1 reordering. |
| **NEW** Staging subdirectory (`replace_journal/staging/`) | n/a | Bounded private filesystem state under existing data-dir; opportunistic stale-file cleanup contemplated. |
| **NEW** Hairline window between staging→canonical rename and pending-journal write (both under lock) | n/a | Resolved in Phase 6; recovery sees canonical-records orphan with no journal, deletes only when no live `SessionLock` exists, and has no mutation authority. |
| Receipt lost after commit | Bounded (export+hash recovery) | Bounded (§12 residual #6 unchanged). |
| `migrate-db` / `migrate_chain_segment` adjacency | UNCOUPLED | UNCOUPLED unchanged. |

Fourteen problem-map / audit entries retired total; nine existing
blast-radius items preserved or tightened; one prior open item (the Rev 3
audit blocker AIR-R3-F01 / R3-F01) explicitly closed; one new item added
(staging subdirectory, bounded private state); the hairline window tracked
as R4-F05 is resolved. Net value is unambiguously positive against (a) the
v1 adapter the harness uses today, (b) the Rev 1 / Rev 2 /
Rev 3 supported surfaces.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 5 — Adjacent-path no-regression check (Rev 4)

Rev 4 changes are restricted to §4 (success flow + journal format), §6
(reusable API + recovery), §8 (side-effect contract / crash states), and
§9.1 (one new T-row, lock-busy / preimage-mismatch language updated). No
change touches §1 scope, §2 CLI surface, §3 input validation / rendering,
§5 exit codes, §7 DB consistency contract, §10 README, §11 supported-
surface customer cohort prose, §12 residuals, or §13 constraint compliance.
The adjacent-path table from Round 3 is unchanged.

| Path | Verdict | Evidence (Rev 4 delta) |
| --- | --- | --- |
| `agents resume`, `repl --resume`, top-level `--resume` | PRESERVED for non-replaced sessions; partial DEGRADED for replaced sessions on parent_turn_id / is_sidechain / is_compaction_boundary | Rev 4 unchanged. |
| `agents trace --json` | PRESERVED for invocation-tree; partial DEGRADED for any future per-turn parentage feature on replaced sessions | Rev 4 unchanged. |
| `agents migrate-config` | UNCOUPLED | Rev 4 unchanged. |
| `agents migrate-db` | UNCOUPLED | Rev 4 unchanged; journal still not consumed by `migrate-db`; manual-recovery flag still anti-scope (§13). |
| Hidden `agents resume-list` | PRESERVED | Rev 4 unchanged. |
| Direct CLI `claude` / `codex` | PRESERVED | Rev 4 unchanged; provider files still receive provider-native bytes. |
| `agents session locate` | PRESERVED + REUSED | Rev 4 unchanged. |
| `agents session schema-probe` | PRESERVED + REUSED | Rev 4 unchanged. |
| `agents session export` | PRESERVED + REUSED | Rev 4 unchanged; round-trip oracle still gated by §4 fresh-export verification before commit. |
| `agents session pause-handshake` / `resume-handshake` | PRESERVED + REUSED | Rev 4 unchanged; the lock primitive is now strictly more authoritative under Rev 4 because per-session recovery artifacts are also gated by it. |
| `migration::migrate_chain_segment` | UNCOUPLED | Rev 4 unchanged. |
| GUI / Tauri command surface | UNCOUPLED | Rev 4 unchanged; new private `staging/` subdirectory is under same default state root, no GUI surface added. |

Zero BROKEN paths. The two paths carrying conditional partial DEGRADED for
replaced sessions only (R1 / R2 / R3 / R4 unchanged) remain bounded by
opt-in. The new private `replace_journal/staging/` subdirectory introduces
no public-CLI adjacency; cohort A / B do not need to read it. The
`pause-handshake` adjacency is materially strengthened by Rev 4: the same
`SessionLock` now also gates per-session journal publication, not just
transcript mutation, which reduces blast-radius of any future bug in
import-replace's recovery path.

## Concern 6 — Migration / rollback / observability (Rev 4 deltas)

**No user state one-shot.** Rev 4 §11.1 unchanged. The new
`<state-data-dir>/replace_journal/staging/` subdirectory is created on
demand by import-replace; existing installs without it are not affected by
its absence. The pre-existing `<state-data-dir>/replace_journal/quarantine/`
and `session-<id>.canonical.jsonl` from Rev 3 remain unchanged in shape.

**Rollback.** Three paths from Round 3 are preserved:

1. PR-level rollback: Rev 4 adds no DB schema and no public-CLI surface
   delta. `git revert` remains clean at the binary level. Leftover
   `replace_journal/`, `replace_journal/staging/`, and
   `replace_journal/quarantine/` directories on disk after revert are
   benign — they contain only `.pending` JSON and `.canonical.jsonl` files
   that nothing else reads.
2. Operation-level rollback: re-import the prior canonical transcript with
   the current postimage as preimage. Unchanged from Round 3.
3. Crash-window rollback: identical to Round 3 in shape, with the
   Rev 4 ordering eliminating the contender-overwrite case Rev 3 audit
   flagged. Cohort A no longer needs to consider the case where another
   contender's bytes are masquerading as the lock-owner's recovery
   identity.

**Observability.** Receipt JSON shape is unchanged from Round 3; cohort A
parsers do not need to update. The journal file remains private (§4
unchanged in shape; only adds `operation_uuid`). Stderr structured JSON
still covers every domain failure (§5). `committed_at` remains a
post-DB-commit timestamp. One new private filesystem signal is introduced:
`replace_journal/staging/` (per-process operation-unique scratch). It is
not a public observability surface; documented as private implementation
state in §4 / §6 / §8 / §11.1.

## Concern 7 — Harness acceptance criteria coverage (Rev 4)

Round 3's mapping is preserved. Rev 4 §9.1 adds one new row and updates
language in two existing rows:

| Rev 4 capability | §9.1 row added or updated | Closure |
| --- | --- | --- |
| Concurrent two-process import-replace race | `T-concurrent-import-replace` (added) | AIR-R3-F01 (3). |
| Lock-busy loser scope (no per-session artifact mutation) | "Lock busy" row updated to: "staging file unlinked; no per-session journal/canonical file, transcript, or DB mutation by this process." | AIR-R3-F01 (2). |
| Preimage-mismatch journal/recovery preservation | "Preimage mismatch" row updated to: "journal and `canonical_records_path` remain for deterministic preimage cleanup." | AIR-R2-F01 / AIR-R3-F01 alignment. |

All sixteen test-intent rows in Rev 4 §9.1 map to declared behaviors in
§3 / §4 / §5 / §6 / §7 / §8. No bullet is orphaned. The Round 1 / Round 2 /
Round 3 caveat "in-flight sessions return exit `13`" remains covered for
cooperative observers; §13 prose remains the contract authority.

## Concern 8 — Initiative-06 sequencing forward-compat (Rev 4)

Import-replace is still the **last** Initiative-06 feature; there is no
downstream sibling consumer of its surface. Rev 4 changes that touch
forward-compat:

- **Receipt JSON evolution.** §6 fields are unchanged. Stable consumer pin
  remains `operation: "import-replace"`. The expanded journal (now with
  `operation_uuid`) is private and does not enter the receipt; future
  fields can still be added additively.
- **Reserved exit codes 16 / 17.** Unchanged.
- **Cross-provider migration adjacency.** UNCOUPLED unchanged. A future
  refactor that lifts the renderer + atomic-replace primitive +
  replace_journal + staging + quarantine into
  `migration::migrate_chain_segment` is allowed but not required. The
  staging-then-rename-under-lock pattern is itself a clean reusable
  primitive for any future cross-feature replace operation.
- **Future canonical-schema extension** (parent_turn_id, is_sidechain,
  is_compaction_boundary). §6 / §12 explicitly leave room for this.
- **Future manual recovery CLI.** Anti-scope confirmed at §12 / §13.
  `agents migrate-db --recover` and `agents session import-replace
  --recover` can be layered without changing v1 CLI shape. The Rev 3
  quarantine path and Rev 4 staging path together give a future CLI two
  stable input directories to drain.
- **Provider renderer scope expansion.** Unchanged.
- **Journal schema versioning.** §4 journal format pins
  `schema_version: 1`; §6 step 2 says recovery should "ignore files whose
  `operation` is not `"import-replace"` or whose `schema_version` is
  unsupported." Rev 4's `operation_uuid` is additive and does not bump
  `schema_version`; that is acceptable because `operation_uuid` is
  required only for Rev 4-emitted journals (which all carry it) and
  recovery treating an absent `operation_uuid` as a Rev 3-or-earlier
  pending record can still match the canonical-records file by path
  alone (Rev 3 had no concurrent-publication race because Rev 4 is the
  Round 4 closure of Rev 3's audit blocker — but recovery can still read
  any in-flight Rev 3 journal at upgrade time). Forward-compat preserved.

No forward-compat hazard. Six additive evolution paths remain open.

## Concern 9 — Cohort-specific concerns (Rev 4)

**Cohort A: `agent-harness` (primary consumer).** §11.1 cohort prose
unchanged. Rev 4 strengthens cohort A in three ways: (a) concurrent two-
process import-replace against the same session is now guaranteed
deterministic (winner's recovery identity is unambiguous; loser cannot
affect winner's state); (b) the staging-then-rename-under-lock pattern
removes the Rev 3 boundary case that fell outside the documented threat
model — cohort A no longer needs to assert single-orchestrator at the
process level for recovery determinism; (c) `operation_uuid` in the
journal gives the harness or any future support tooling a stable
correlation key between a journal entry and its canonical records file.

Rev 4 narrows cohort A in zero new ways relative to Rev 3 — the renderer
record-class refusal scope (R4-F01) is unchanged.

**Cohort B: local automation scripts using `agents session export`.**
§11.1 unchanged. Same surface as cohort A. The Rev 3 R3-F01 boundary
concern (cohort B does not have single-orchestrator expectation) is
materially closed by Rev 4. Cohort B can now run two `import-replace`
processes for the same session concurrently and rely on `SessionLock` /
exit `13` to serialize them with no recovery hazard.

**Cohort C: existing `agents repl` / `agents resume` / `agents -m <model>
<prompt>` users not using import-replace.** PRESERVED for any session
never import-replaced. Partial DEGRADED for any session import-replaced by
an authorized caller (R4-F04 carryover prose gap).

**Cohort D: GUI / Tauri users.** PRESERVED unchanged.

**Cohort E: direct CLI `claude` / `codex` users.** PRESERVED unchanged.
Rev 4 strengthens this cohort indirectly via the same lock-tightening
that benefits cohort A: any future bug in import-replace's recovery path
cannot affect a session another writer holds.

No cohort regressed. Cohorts A, B, and E are strengthened by the Rev 4
lock-before-journal reorder.

## Verdict rationale

**Termination signal #1** (`assumption_invalidated`) does not fire — A1–A10
all hold under Rev 4 evidence; no assumption restated, narrowed, or
withdrawn.

**Termination signal #2** (`non-positive-value`) does not fire — fourteen
problem-map / audit entries retired (one more than Round 3); one HIGH audit
finding closed (AIR-R3-F01); one prior implicit blast-radius item
(concurrent-process pre-lock journal race) explicitly closed; one new
private filesystem item added (`replace_journal/staging/`); the hairline
window tracked as R4-F05 is resolved by the Phase 6 orphan canonical
recovery rule.

**Standard verdict: LOW.** Adjacent-path blast-radius is bounded — twelve
adjacent paths, zero BROKEN, two paths still carrying conditional partial
DEGRADED for opt-in replaced sessions only (Concern 5). Migration /
rollback mechanized: no schema added; uninstall is clean; operation-level
rollback documented; crash-window rollback strengthened by under-lock
journal publication (Concern 6). All sixteen harness acceptance bullets
covered, including one new concurrency row (Concern 7). Forward-compat
preserved on receipt JSON, exit-code reservation, migration uncoupling,
canonical-schema extensibility, manual-recovery layering, renderer scope
expansion, and journal schema versioning (Concern 8). All five cohorts
non-regressed; cohorts A, B, and E strengthened (Concern 9). The Rev 4
lock-before-journal reorder is race-free under the documented threat
model and additionally race-free against the Rev 3 boundary case
(non-orchestrated concurrent same-session import-replace); the prior
hairline-window prose observation (Concern 3 / R4-F05) is resolved.

**Recommendation:** Phase 5 (hookpoints) and Phase 6 (implementation) may
proceed. Four live non-terminal findings below; none fires a termination
signal. R4-F05 is retained only as a resolved Phase 6 note for the under-lock
hairline recovery window. R4-F01..R4-F04 are Round 3 carryovers
(R3-F02..R3-F05 prose issues that Rev 4 did not touch).

## Findings

- **R4-F01 (renderer record-class coverage, LOW, non-terminal — carryover
  of R3-F02 / R2-F02)** — §3's renderer contract refuses lossy record
  classes with `15 invalid-input-transcript` and
  `unsupported-record-class:<class>`. Multi-modal blocks and tool-use are
  listed as examples; the proposal does not enumerate the exact set of
  record classes the v1 renderer supports, leaving cohort A's effective
  coverage as a Phase 6 implementation detail. Recommendation: §11.1
  should add a cohort-A note bounding effective coverage to the v1
  `CanonicalToProviderRenderer` scope for `claude_code` and
  `codex_session`, and Phase 6 should publish the supported record-class
  list at PR time. Non-terminal because the refusal is loud (exit `15`
  with a structured error code).

- **R4-F02 (R1-F03 prose carryover, cosmetic, non-terminal — carryover of
  R3-F03 / R2-F03)** — Rev 4 did not update §12 residual #3 to name
  in-binary writers (`run_resume`, `run_repl`, balanced one-shot,
  `migration::migrate_chain_segment`), nor add the §11.1 cohort-A
  orchestrator-role sentence. The §13 row prose remains the contract
  authority. Recommendation unchanged: a one-paragraph edit to §12
  residual #3 and one sentence in §11.1 cohort A. Non-terminal because §13
  carries the contract. (Note: Rev 4's lock-before-journal closure
  materially reduces the *practical* importance of the cohort-A
  orchestrator-role sentence; cohort B can now safely run concurrent
  same-session attempts without recovery hazard. The prose carryover
  remains for completeness, not as a real cohort-B risk.)

- **R4-F03 (stale-temp cleanup scoping, cosmetic, non-terminal —
  carryover of R3-F04 / R2-F04)** — Rev 4 §4 pre-mutation step 11 still
  reads "Clean stale import-replace temp files in the target transcript
  directory whose names match this feature's temp-file convention and are
  not currently locked by another live replace operation." The §8
  convention is `<jsonl_path>.tmp-import-replace-<operation_uuid>`
  (per-jsonl-path), and Claude / Codex place many sessions' JSONLs in
  shared directories. Phase 5 / Phase 6 implementer should scope cleanup
  to `<resolved.jsonl_path>.tmp-import-replace-*` rather than a
  directory-wide sweep matching the feature prefix. Cosmetic — §9.1
  atomic-temp/rename test bound this in code; the prose is the only
  ambiguity. Note Rev 4 also adds an analogous opportunistic cleanup case
  for `replace_journal/staging/<operation_uuid>.canonical.jsonl` files
  (§8 crash state #1); the same scoping caution applies and Phase 6
  should age-and-uuid scope rather than blanket-prefix sweep.

- **R4-F04 (cohort-C partial-degraded prose gap, cosmetic, non-terminal —
  carryover of R3-F05 / R2-F05)** — AIR-R1-F04's closure documents
  `parent_turn_id` / `is_sidechain` / `is_compaction_boundary` loss in
  §6 / §7 / §12 and a §13 compliance row. Rev 4 §11.1 cohort-C prose
  still does not enumerate the resulting partial DEGRADED state for
  `agents resume` / `repl --resume` / `--resume` / `trace --json` on
  replaced sessions. Recommendation unchanged. Non-terminal because the
  contract is documented in §6 / §7 / §12 prose.

- **R4-F05 (hairline window between staging→canonical rename and
  pending-journal write, RESOLVED in Phase 6)** — The current
  `proposals/06-import-replace.md` now explicitly handles the case where
  `<state-data-dir>/replace_journal/session-<id>.canonical.jsonl` exists
  without a matching `session-<id>.pending`: startup recovery deletes the
  orphan canonical side file only when no live `SessionLock` exists, fsyncs
  `replace_journal/`, and does not mutate transcript or DB state. The matching
  crash state is also named in §8, so this supported-surface concern is
  obsolete for the implementation under review.

## Audit-history note

This is a Phase 4 supported-surface gate only. I did not review or change
an implementation; Rev 4 remains a proposal artifact. Termination signal
is `none`; verdict is LOW; Phase 5 and Phase 6 may proceed once the audit
track also clears AIR-R3-F01 closure (verified in Concern 1 of this
report). The four live non-terminal findings above are recommendations for
Phase 6 prose / scoping rather than blockers; R4-F05 is now resolved.
