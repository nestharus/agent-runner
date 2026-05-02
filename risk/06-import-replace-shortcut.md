# 06-import-replace — Phase 4 Shortcut Risk Assessment (Rev 4)

## Verdict: LOW

Rev 4 closes Round 3 finding AIR-R3-F01 by reordering the success flow so the
session lock is acquired **before** any per-session journal artifact is
published. Per-process scratch lives at an operation-uuid staging path; the
per-session canonical-records file and the per-session pending journal are
both written only under `SessionLock`. Round 1 closures (AIR-R1-F01..F04) and
Round 2 closure (AIR-R2-F01) carry forward unchanged. Two Round 1 watchpoints
(W1, W2) remain retired. The Round 2/3 W4 (concurrent-invocation journal
race) is now retired by Fix A — the audit's first listed remedy — and is
explicitly bound by the new T-concurrent-import-replace test row. Two
watchpoints (W3 Codex two-track, W5 downstream NULL-tolerance) persist for
audit-/scope-track and Phase 6 specification precision. New LOW-severity nits
are filed for stale staging-file cleanup specification.

No new finding rises to MEDIUM or HIGH.

## Round 3 closure check (AIR-R3-F01)

The Rev 3 audit (`risk/06-import-replace-audit.md:66-108`) flagged that pre-
lock per-session journal artifacts could be overwritten or deleted by a
second concurrent invocation before the first acquired the lock. The audit
listed three required proposal changes:

1. Acquire `SessionLock` before publishing per-session journal artifacts; or
2. Use operation-unique journal/canonical paths plus an ownership token so
   only the lock owner can publish, consume, or delete the active per-session
   pending entry; and
3. Add a concurrency test where two `import-replace` processes target the
   same session, one wins the lock, the loser exits `13`, and the winner's
   journal, canonical records, transcript, and DB update remain intact.

Rev 4 takes the audit's Fix A path (acquire-then-publish) and adds an
operation-uuid staging discriminator. The per-attempt naming alternative
(Fix B) is implicit in the staging path naming.

### Required change 1 — lock before per-session publication — DONE

Rev 4 §4 success flow (`proposals/06-import-replace.md:300-348`) executes
durable per-session side effects strictly under the lock:

- Step 1 (`:302-304`) writes the operation-unique staging file
  `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
  This is per-process scratch; it is not addressable as a per-session journal
  artifact.
- Step 3 (`:308-310`) acquires `SessionLock` for the resolved active provider
  session id with owner `"import-replace"`. Busy maps to exit `13`; before
  returning, the staging file is unlinked. **No per-session journal artifact
  has been published.**
- Step 4 (`:311-315`) atomically renames the staging canonical file to the
  per-session path `session-<session_id>.canonical.jsonl`, **only after the
  lock has been acquired**, and fsyncs `replace_journal/`.
- Step 5 (`:316-319`) writes the pending journal at
  `session-<session_id>.pending` under the same lock.

§4 closing paragraph (`:350-355`) reaffirms the contract: "A lock-busy
contender exits after deleting only its own staging file, because it never
publishes a per-session journal path."

§8 side-effect contract (`:599-622`) repeats the ordering across three bullet
points:

- "Before acquiring the session lock and before writing any per-session
  journal artifact, import-replace writes normalized canonical records only
  to `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`."
- "If `SessionLock::acquire` returns busy, import-replace unlinks only its
  staging file and exits `13`; it must not create or modify
  `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` or
  `<state-data-dir>/replace_journal/session-<session_id>.pending`."
- "Other handled failures after staging creation but before lock acquisition
  also unlink only the staging file before exit."
- "After acquiring the session lock, import-replace atomically renames the
  staging file to ... `session-<session_id>.canonical.jsonl`."
- "It then writes the pending journal entry under the same lock at
  ... `session-<session_id>.pending`."

§13 row "Durable journal closes post-rename/pre-DB crash recovery"
(`:841`) preserves Round 2's "deletion happens only after verification plus
DB commit" framing.

### Required change 2 — operation-uuid ownership discriminator — DONE

Rev 4 §6 (`:443-450`) updates the journal API contract: the journal first
writes the staging canonical-records file at the operation-uuid staging
path, and only after `SessionLock` is acquired does it rename to the
per-session path. The pending entry contains the resolved identity plus
`operation_uuid`.

§4 journal format (`:359-377`) adds `operation_uuid` to the on-disk schema:

```json
"operation_uuid": "0b67fdde-92c1-45d1-832c-4b1fbf5c8306"
```

§4 closing prose (`:382-389`) names operation_uuid's role: "The
`operation_uuid` identifies the staging source that was atomically renamed
into `canonical_records_path` under lock and is required so crash recovery
can associate the journal with the canonical-records file."

§8 temp-file convention (`:595-597`) extends the operation_uuid suffix to
the transcript temp file as well: `<jsonl_path>.tmp-import-replace-<operation_uuid>`,
which makes per-operation transcript scratch unambiguous across concurrent
attempts that race on lock acquisition.

§1 changelog (`:58-71`) documents the schema delta as a Rev 4 audit-closure
bullet: "journal schema gains `operation_uuid` to associate journal with
canonical-records file across rename."

### Required change 3 — concurrency test — DONE

Rev 4 §9.1 adds T-concurrent-import-replace (`:704`) — the audit-required
race test:

> Spawn two subprocesses calling import-replace on the same `session_id`;
> exactly one process wins the lock and the other exits busy.

Expected observable signal:

> Exactly one returns `0` with valid receipt and final transcript/export
> matching the winner. The loser returns `13 session-busy`, unlinks its
> staging file, leaves no per-session journal/canonical files in
> `<session>.canonical.jsonl` or `<session>.pending`, and performs no
> transcript mutation.

The test directly asserts the audit's three correctness invariants:

| Audit invariant | T-row assertion |
|---|---|
| Loser exits `13` | "the loser returns `13 session-busy`" |
| Loser leaves no per-session journal artifacts | "leaves no per-session journal/canonical files in `<session>.canonical.jsonl` or `<session>.pending`" |
| Winner's journal, canonical records, transcript, and DB update remain intact | "exactly one returns `0` with valid receipt and final transcript/export matching the winner" |

The fixture is named ("Shared temp state DB and transcript fixture; two valid
but distinguishable canonical inputs launched concurrently") and the residual
risk on scheduler nondeterminism is explicitly enumerated ("test needs a
barrier or lock-acquire hook to make the race observable").

Closure verdict: **AIR-R3-F01 CLOSED**. All three audit-required proposal
changes are present, located in §4 / §6 / §8 / §9 / §13, and each is bound
to a typed exit, a specific journal field, an ordered success-flow step, or
a specific T-row.

## Round 1 / Round 2 closures still standing

Rev 4 deltas are scoped to §4 / §6 / §8 / §9 / §13. The §3 renderer surface,
§7 DB-update field set, §5 exit-code namespace, and §1.1 assumption register
are unchanged. Round 1 and Round 2 closures are inspected below.

### AIR-R1-F01 (renderer / canonical-bytes-on-disk) — STILL CLOSED

§3 (`:181-252`) preserves the renderer contract: provider-native bytes on
disk, lossy classes refused with `15 invalid-input-transcript`, anti-scope
listed (multi-modal, tool-use), `Other → UnsupportedStorage`. §6 (`:431-433`)
keeps `CanonicalToProviderRenderer` typed. §13 rows (`:838-839`) still
record YES on "Provider transcript file receives provider-native bytes, not
canonical bytes" and "Lossy canonical-to-provider re-encoding is refused."
§9.1 "Postimage round-trip" row (`:716`) still pins "Export hash equals
receipt postimage_sha256 even though on-disk bytes are provider-native."

The renderer-vs-disk contract remains protected by both orthogonal
verifications (postimage hash + fresh export round-trip) before the SQLite
commit (§4 success-flow steps 10-12 `:333-343`).

### AIR-R1-F02 (post-rename/pre-DB-commit recovery) — STILL CLOSED

The Rev 3 strengthened-closure path (frozen identity, `canonical_records_path`
companion file, no-delete-before-verify ordering) is preserved in Rev 4 and
extended:

- Frozen identity persisted in journal (`:359-377`).
- `canonical_records_path` is the recovery DB source of truth (`:386-389`,
  `:472-481`).
- Journal deletion is the last durable step, after postimage verification +
  fresh export verification + SQLite commit (`:344-346`, `:625-629`).
- Five startup recovery cases enumerated (`:455-485`); recovery uses frozen
  identity from the journal, not `StateDb::resolve_resume`.
- T-recovery-rename-only, T-recovery-ambiguous-hash,
  T-recovery-canonical-records-preserved, T-no-deletion-before-verify
  T-rows preserved (`:707-712`).

Rev 4 adds the operation-uuid handle to the journal but does not weaken any
recovery contract. The §6 startup recovery contract (`:455-485`) explicitly
handles a new pre-rename crash subcase introduced by Rev 4's reorder: "If a
pending journal lacks a completed `preimage_sha256` because the process died
before reading the original transcript, treat it as a pre-rename no-op:
delete the journal and canonical records file, fsync the journal directory,
and do not mutate DB state" (`:463-466`). This binds the new T-rev4 crash
window between staging rename + journal write (under lock) and under-lock
preimage compute to the §8 #2 "after staging rename and pending journal
write, but before transcript temp write" deterministic action (`:641-644`).

§8 crash states #1–#9 (`:637-669`) are renumbered and expanded to include
the staging-file states; every single-instance crash window still maps to a
deterministic recovery action.

**STILL CLOSED, STRENGTHENED on the staging boundary.**

### AIR-R1-F03 (lock observation / cooperative-surface scope) — STILL CLOSED

§13 row "Lock observation for import-replace once pause-handshake lands"
(`:834-835`) still cites 06-pause-handshake's PR #17 as the lock-primitive
dependency and still records the writer-path retrofit as a sibling-PR
concern with explicit "advisory until full retrofit lands" framing for v1
harness consumers. §12 residual #3 (`:813-815`) still names the cooperative-
surface limit as a documented residual. Rev 4 did not touch this surface.

Rev 4 does, however, narrow the cooperative race surface that §13 #3
addresses: under Rev 4 the per-session journal artifact is only published
under the lock, so the only race remaining at the journal layer is the
ordinary cooperative-lock race, which is Phase 6's `SessionLock` correctness
question, not import-replace's.

### AIR-R1-F04 (canonical-record field-loss model) — STILL CLOSED

§6 (`:438-442`) still declares the loss explicitly: "Fields not present in
`CanonicalRecord` (`parent_turn_id`, `is_sidechain`,
`is_compaction_boundary`) are intentionally written as `NULL` or schema
defaults in `session_turns`."

§7 step 4 (`:545-549`) still warns consumers; §7 (`:575-577`) still names
canonical-schema extension as the future fix point. §9.1 row "DB metadata
loss is explicit" (`:714`) still pins the test expectation. §12 residual
(`:818-820`) and §13 row "State consistency covers required rows" (`:842`)
preserve the documented loss.

The W5 watchpoint about asserted-not-verified downstream NULL tolerance
persists (re-confirmed against current branch below).

### AIR-R2-F01 (journal recovery underspecified and cleared too early) — STILL CLOSED

The Rev 3 closure surfaces are unchanged in Rev 4:

- Frozen resolved identity in the journal (`:359-377`).
- `canonical_records_path` companion file as recovery source of truth
  (`:386-389`, `:435-437`).
- No-delete-before-verify ordering: SQLite commit is gated on **both**
  postimage hash verification and fresh export round-trip verification,
  and journal deletion follows commit (`:333-346`, `:625-629`).
- Recovery uses frozen identity, not resolver re-discovery (`:472-481`).
- Four T-rows binding the recovery contract end-to-end (`:707-712`).

Rev 4 only adds operation_uuid as an additional journal field and an
explicit pre-rename pre-preimage no-op subcase to the recovery contract.
Neither change weakens AIR-R2-F01's closure; both strengthen it on adjacent
crash boundaries.

**STILL CLOSED.**

## Race-freeness for the documented threat model

The proposal's threat model is single-instance crash recovery (§8 crash
states #1–#9). Rev 3 audit added a concurrent-invocation race scope on the
basis that two cooperative `import-replace` invocations are inside the
documented threat surface. Rev 4 must be race-free against both.

### Rev 4 success-flow timeline (per process)

```text
T0  validate input, allocate operation_uuid
T1  write staging canonical at staging/<operation_uuid>.canonical.jsonl, fsync
T2  resolve ownership through SessionMetadata; freeze identity
T3  acquire SessionLock(resolved_session_id)
       busy → unlink staging file, exit 13 (NO per-session artifact written)
       acquired → continue
T4  atomic rename staging → session-<session_id>.canonical.jsonl, fsync replace_journal/
T5  write pending journal at session-<session_id>.pending under lock
T6  read existing transcript, compute under-lock preimage, rewrite journal with hash, fsync
T7  render canonical → provider-native bytes, write transcript temp, fsync
T8  atomic rename transcript temp → jsonl_path, fsync parent dir
T9  begin SQLite transaction (uncommitted): replace session_turns from canonical_records_path
T10 compute postimage_sha256 from renamed transcript; verify against journal
       mismatch → rollback + exit 1 + leave artifacts
T11 fresh export round-trip vs canonical_records_path
       mismatch → rollback + exit 1 + leave artifacts
T12 commit SQLite
T13 unlink journal + canonical_records_path, fsync replace_journal dir
T14 release lock; emit receipt
```

### Single-instance crash recovery — RACE-FREE

For a single instance, every crash boundary maps deterministically:

| Crash boundary | Recovery action | §8 crash state |
|---|---|---|
| Before T1 | nothing on disk | n/a |
| T1..T3 (staging only) | stale staging file under `staging/`; opportunistic cleanup by uuid/age | §8 #1 (`:637-640`) |
| T3 lock-busy exit | staging file unlinked by busy path; no per-session artifact ever existed | §8 (`:605-607`) |
| T4..T5 (between staging rename + journal write) | journal write happens under lock; if process dies between step 4 and step 5, recovery sees an orphan canonical file at `session-<X>.canonical.jsonl` with no pending journal | covered by §6 startup recovery scan key (only `session-<X>.pending` files are scanned, `:457-458`) — orphan canonical file is benign and removable by manual cleanup or by next acquire-and-rename |
| T5..T6 (journal written, no preimage_sha256 yet) | recovery sees pending journal lacking preimage; treats as pre-rename no-op (`:463-466`); deletes artifacts, no DB mutation | §8 #2 (`:641-644`) |
| T6..T8 (preimage written, transcript rename pending) | recovery sees journal + canonical + transcript hash = preimage; deletes artifacts, no DB mutation | §8 #3, #4, #8 (`:645-647`, `:662-663`) |
| T8..T12 (transcript renamed, DB not committed) | recovery sees journal + canonical + transcript hash = postimage; re-applies DB from canonical_records_path under frozen identity, deletes artifacts | §8 #5, #6, #7 (`:648-660`) |
| T13..T14 (committed, journal pending delete) | recovery sees postimage state; idempotent re-apply (no-op) + delete | §8 #7 (`:657-660`) |
| After T13 | clean state | n/a |

Every action between T1 and T13 is either fsynced or transactional. SQLite's
T12 commit is all-or-nothing, so a mid-transaction crash reduces to either
the uncommitted or committed boundary on disk. The new T-no-deletion-before-
verify (`:712`) tests that T10/T11 mismatches do **not** trigger T13.

The recovery routine never calls `StateDb::resolve_resume` to rediscover the
segment. It reads `chain_id` and `active_segment_id` from the journal,
replays canonical records into `session_turns` via the same DB helper used
in the forward path, and updates the frozen segment's `last_turn_id` /
`last_used_at`.

**Single-instance race-free.**

### Concurrent-invocation race — RACE-FREE (W4 retired)

Trace two concurrent processes A and B targeting the same resolved
`session_id`:

| Time | Process A | Process B | Per-session disk state |
|---|---|---|---|
| T0_A | write staging/<uuid_A>.canonical.jsonl | — | none |
| T0_B | — | write staging/<uuid_B>.canonical.jsonl | none |
| T2_A | freeze identity (session X) | — | none |
| T2_B | — | freeze identity (session X) | none |
| T3_A | SessionLock.acquire(X) → ok | — | none |
| T3_B | — | SessionLock.acquire(X) → busy | none |
| T3_B+ε | — | unlink staging/<uuid_B>; exit 13 | none |
| T4_A | rename staging/<uuid_A> → session-X.canonical.jsonl | — | A's canonical |
| T5_A | write session-X.pending under lock | — | A's canonical + A's journal |

B never touched `session-X.canonical.jsonl` or `session-X.pending`. B's
busy-cleanup unlinks only its own staging file. A's per-session artifacts
are guarded by the lock that A holds.

The Rev 3 W4 collision pattern (B overwrites A's per-session canonical or
journal pre-lock) is structurally impossible in Rev 4: the per-session
paths are not addressable until the lock is held, and the lock is exclusive.
B cannot rename anything into `session-X.canonical.jsonl` because it has no
acquire-then-rename code path before the lock.

The §13 "Lock observation" residual still applies for non-cooperating
external writers (a writer that does not call `SessionLock::acquire`),
which is the AIR-R1-F03 residual cooperative-surface limit, not a journal
race. That residual is unchanged by Rev 4.

**Concurrent-invocation race-free under cooperative-lock semantics.**
W4 retired.

### Lease-expiry edge case — bounded under documented `SessionLock` semantics

If A holds the lock and crashes after T5 but before T13, A's lease expires
according to `SessionLock` semantics (§8 `:589-591`). B, invoked later, can
acquire the lock. B's flow:

1. B writes its own staging/<uuid_B>.canonical.jsonl.
2. B acquires lock.
3. B's step 4 atomically renames staging/<uuid_B> → session-X.canonical.jsonl.
   POSIX `rename(2)` overwrites A's stale canonical file atomically. A's
   bytes are gone.
4. B's step 5 writes session-X.pending under lock, overwriting A's stale
   pending journal.

A's recovery would have re-applied A's intended DB update if startup
recovery had run before B's invocation. If startup recovery did not run
(e.g., because B is invoked before any agent restart occurs), B's overwrite
is the operator's intended outcome: B is a fresh import-replace request and
should clobber A's failed/abandoned attempt. B's under-lock preimage check
(step 6) computes the preimage against whatever transcript bytes are
on-disk at that moment, which may be A's postimage bytes if A's rename
landed. That preimage value is what B records and verifies against; it is
internally consistent with B's flow. The user's intent — replace transcript
bytes with B's content — is correctly served.

This is not a regression. The CLI is per-invocation; "startup recovery" in
a CLI binary means recovery runs at the start of each invocation. The Rev 4
proposal does not explicitly require per-invocation recovery, but neither
does it forbid it; Phase 6 should pin whether recovery scan runs once at
agent startup or per-CLI-invocation. See N5 below.

### Verdict: race-free for the documented threat model

Rev 4's lock-before-publish reorder is race-free for both single-instance
crash recovery (every T0..T14 boundary maps to a deterministic §8 action)
and concurrent-invocation racing (per-session paths are unaddressable
without the exclusive lock). The `SessionLock` lease-expiry semantics
remain a `SessionLock` concern, not an import-replace concern, and the
overwrite-under-new-lock case yields operator-intended behavior.

## Watchpoints carried forward

### W3 (Rev 1) — Codex two-track in §9.1 — STILL NARROWED-BUT-PERSISTS

A5 (`:115`) still declares `claude_code` and `codex_session` supported.
§3 renderer (`:238`) still names `codex_session` as a first-class storage
type. §6 receipt enum (`:514`) still names `codex_session`. §9.1 last row
(`:716`) still hedges: "If Codex renderer deferred, Codex test becomes
explicit unsupported-storage test."

Not a shortcut because the deferral fallback is typed (exit `12`), but
audit-track should pin which fork is binding before Phase 6 begins.
Unchanged from Rev 3.

### W4 (Rev 2/3) — Concurrent-invocation journal race — RETIRED

Rev 4's lock-before-publish reorder + operation_uuid staging discriminator
+ T-concurrent-import-replace test row jointly retire W4. The audit's
Fix A (acquire-lock-first) is implemented in §4 / §6 / §8 prose, the
audit's per-attempt-naming idea (Fix B) is implemented for staging files
via operation_uuid, and the audit's required test is in §9.1.

### W5 (Rev 2) — Downstream NULL-tolerance asserted, not verified — PERSISTS

§7 step 4 (`:548-549`) still asserts "downstream features such as resume
and trace should not rely on these fields after a replace." Re-verified
against the current branch in this worktree:

- `latest_compaction_boundary` (`src-tauri/src/state/db.rs:2510-2536` in
  prior reviews; line numbers may drift) filters
  `WHERE is_compaction_boundary = 1`. Post-replace rows default to
  `0`/NULL → resume's compaction-boundary handling regresses for any
  session whose transcript was replaced.
- `parent_turn_id` and `is_sidechain` are referenced in `state/db.rs`,
  `balancer/mod.rs`, `sessions/mod.rs`, `trace/mod.rs`. Per-consumer
  NULL-tolerance is not enumerated.

Same disposition as Rev 3: not a shortcut because the loss model is
explicit and tested, but the contract claim ("downstream should not rely")
is currently false for `latest_compaction_boundary`. Phase 5 hookpoints or
Phase 6 implementation should either (a) harden consumer paths, (b) extend
canonical schema to carry the three fields before import-replace ships, or
(c) narrow §7 step 4's prose to enumerate which downstream behaviors are
accepted to regress in v1.

## LOW-severity nits

### N1 — Stale-temp cleanup scope (carries from Rev 1)

§4 step 11 (`:289-291`) is unchanged from prior revisions: "Clean stale
import-replace temp files in the target transcript directory whose names
match this feature's temp-file convention and are not currently locked by
another live replace operation." Per N1 in prior rounds, Claude project
directories and Codex session directories host multiple sessions per
directory, so cleanup must filter by `<jsonl_path>` prefix, not just by
feature suffix. Phase 6 should specify the predicate.

### N2 — `source_file` conditional (carries from Rev 1)

§7 step 5 (`:550-552`) is unchanged: "Set `source_file` to the replaced
`jsonl_path` when the current schema/helper supports it; otherwise keep
existing ingest helper behavior if the column is not meaningful in this
branch." Phase 5 hookpoints should declare which branch state is binding.

### N3 — Quarantine-marker shape (carries from Rev 2)

§6 step 6 (`:480-485`) and §8 crash state #9 (`:664-669`) still instruct
recovery to move the journal to `replace_journal/quarantine/` without
pinning the per-file shape inside it (preserved filename, timestamp suffix,
or `.quarantined` rename). Combined with §6 / §12 #2's anti-scope on a
manual-recovery CLI, Phase 6 should pin the shape and exclude it from the
on-startup scan filter.

### N4 — `canonical_records_path` lifetime in quarantine (carries from Rev 3)

§8 crash state #9 (`:664-669`) and §6 step 6 (`:480-485`) say recovery
"preserves the canonical records file for inspection" / "leave the
canonical records file in place" while moving the journal entry to
`quarantine/`. Quarantined journals (in `quarantine/`) point at
`canonical_records` files that remain in `replace_journal/` (the active
directory). Phase 6 should clarify whether `canonical_records_path` is also
moved into `quarantine/`, or whether the on-startup scan filter
deliberately ignores `*.canonical.jsonl` files. Otherwise an operator's
manual cleanup of `replace_journal/` could orphan canonical material from
quarantined journals, or recovery could misread an orphan canonical file.

### N5 — Staging-file cleanup specification (new in Rev 4)

§8 crash state #1 (`:637-640`) names opportunistic staging cleanup: "future
startup or import-replace runs may unlink stale staging files by age and
operation UUID." The predicate ("by age and operation UUID") is not pinned
to a concrete threshold or ownership rule. Two precision points for
Phase 6:

1. Age threshold (e.g., older than 24h, older than the longest acceptable
   lock-lease window).
2. Live-operation safety: a still-running import-replace process owns its
   `staging/<uuid>.canonical.jsonl` until either lock-busy unlink (T3) or
   under-lock rename (T4). Cleanup must not unlink another live process's
   staging file. Operation UUID alone is not a liveness signal; either an
   age cutoff longer than any plausible flow, or a per-process lock on the
   staging file, is required.

Not a shortcut (the recovery contract is named and the staging path is
operation-unique by construction); flag for Phase 6 specification
precision.

### N6 — Per-invocation vs. once-at-agent-startup recovery (new in Rev 4)

§6 startup recovery contract (`:455-485`) is titled "Startup recovery
contract." For a daemon, "startup" is one event; for a CLI binary, every
invocation is "startup." The proposal does not pin which interpretation is
binding, and the lease-expiry overwrite scenario (above) is correct under
either interpretation but with different operator-visible failure modes.

Phase 6 should pin: does `agents` CLI run the journal recovery scan at the
top of every CLI invocation, only at the top of an `import-replace`
invocation, or only at top-level binary entry? Each is defensible; the
documentation must commit to one.

Not a shortcut (recovery is named, not deferred); flag for Phase 6
specification precision.

## Per-pattern shortcut audit (Rev 4 deltas focus)

Eight canonical shortcut patterns re-checked against Rev 4's three new
surfaces (operation-uuid staging path, lock-before-publish ordering,
T-concurrent-import-replace test row). Round 3 PASS results carry forward
unchanged where Rev 4 did not touch the surface.

### 1. Hidden silent fallback

Rev 4 deltas:

- §4 step 3 (`:308-310`): lock-busy unlinks staging file and exits `13`.
  Typed exit; not a silent fallback.
- §4 step 4 (`:311-315`): under-lock rename of staging → per-session
  canonical. Atomic POSIX rename; no fallback path.
- §4 step 5 (`:316-319`): under-lock journal write. No silent retry.
- §6 step 2 (`:463-466`): pre-rename no-op pending journal recovery
  (introduced for Rev 4 staging boundary) is typed: "delete the journal
  and canonical records file, fsync the journal directory, and do not
  mutate DB state." No silent re-render.

PASS.

### 2. Dual-write / compat shim / backward-compat alias

`operation_uuid` is not a dual-write of state. It is a content-addressable
suffix on the per-process staging path and a journal field used by recovery
to associate the journal with `canonical_records_path` after rename.

Grep `compat|shim|backward|legacy|transitional|dual-write|alias` over the
Rev 4 proposal returns matches only on `compatibility` (schema-probe gate)
and "schema-compatible JSON" (rejection criteria). PASS.

### 3. Deferred stubs without typed errors

Rev 4 deferrals re-checked against `~/ai/conventions/no-deferred-stubs.md`:

| Deferred surface | Typed error / refusal | Test pin |
|---|---|---|
| `Other` storage rendering | `12 unsupported-storage` (§3, §4 step 8, §5); residual §12 #4 | §9.1 "Unsupported storage" |
| Lossy canonical record classes | `15 invalid-input-transcript` with `unsupported-record-class:<class>` | §9.1 "Unsupported record class" |
| Manual recovery CLI | anti-scope explicit (§6 last paragraph, §12 #2); on-startup auto-recovery delivered (§6 startup recovery contract) | §9.1 four "Journal *recovery" + "T-recovery-*" rows |
| Quarantine cleanup | typed quarantine directory `replace_journal/quarantine/` | §9.1 "Journal ambiguous recovery" + "T-recovery-ambiguous-hash" |
| Codex renderer (if Phase 6 finds blockers) | `12 unsupported-storage` fork (§9.1 Postimage round-trip residual) | covered |
| Canonical-schema extension for absent fields | NULL/default writes (§6, §7 step 4); residual §12; §13 row | §9.1 "DB metadata loss is explicit" |
| In-binary writer-path lock observation | `13 session-busy` for cooperative observers; residual §12 #3; sibling-PR retrofit (§13 `:834-835`) | §9.1 "Lock busy" |
| Concurrent-invocation race (NEW Rev 4) | `13 session-busy` for the loser; staging file unlinked; no per-session artifacts created (§4 step 3, §8) | §9.1 T-concurrent-import-replace |
| Stale staging-file cleanup (NEW Rev 4) | "future startup or import-replace runs may unlink stale staging files by age and operation UUID" (§8 crash state #1) | partial (no dedicated T-row); see N5 |

Each Rev 4 deferral has a typed exit and a named follow-up. T-concurrent-
import-replace pins the new concurrent-race contract. Stale staging-file
cleanup is named and bounded but lacks a T-row pin (N5). PASS.

### 4. Hardcoded constants / magic numbers

Grep `hardcode|hard-code|magic|placeholder` over Rev 4 returns zero hits.
New literals introduced in Rev 4 — `replace_journal/staging/<uuid>`,
`tmp-import-replace-<operation_uuid>`, `operation_uuid` field — are
namespaced data and uuid-keyed identifiers, not magic. PASS.

### 5. TODO/FIXME-gated rollout

Grep `TODO|FIXME|for now|in the future|temporary|workaround` over Rev 4
returns matches only on "future" / "later" framings (anti-scope sentences
in §6 / §7 / §13) and "future startup or import-replace runs" (§8 staging
cleanup). No new in-mainline TODOs in Rev 4. PASS.

### 6. Symptom-masking heuristic

Rev 4 ordering surfaces:

- §4 steps 3-5 (`:308-319`): lock acquired before any per-session journal
  artifact is published. The audit's W4 race is closed by reorder, not by
  masking.
- §4 step 3 lock-busy path (`:308-310`): unlinks only the operation-unique
  staging file. Does not touch shared per-session paths because none have
  been written.
- §6 step 2 pre-rename no-op recovery (`:463-466`): explicitly handles the
  Rev 4 crash window between staging rename + journal write and under-lock
  preimage compute, instead of leaving it to ambiguous-hash quarantine.

PASS.

### 7. Feature-flag rollout

Grep `feature flag` over Rev 4 returns matches only on schema-probe feature
flags consumed as input gates (A1, §9.1). The proposal does not introduce a
new feature flag for itself. PASS.

### 8. Atomicity bypass / sed-style rewrite

Rev 4 atomicity surfaces:

- §4 D2 (`:262-264`) still commits to "two-phase replace with same-
  directory temp file, fsync, atomic rename, and a durable replace
  journal."
- §8 fsync ordering (`:676-687`) preserved and extended: staging file
  fsync after writing, `replace_journal/staging/` fsync after busy-unlink,
  `replace_journal/` fsync after under-lock rename and after journal
  write/rewrite.
- §4 step 4 (`:311-315`): atomic rename of staging → per-session canonical
  is on the same filesystem (`replace_journal/staging/` and
  `replace_journal/` share a parent), so POSIX `rename(2)` is atomic.
- No in-place edit, no `sed`-style byte rewrite, no append-only amendment.

The W4 watchpoint (Rev 3) is now retired by reorder, not by atomicity
bypass. PASS.

## Per-pattern grep summary (Rev 4)

| Pattern | Hits | Disposition |
|---|---|---|
| `compat\|compatibility` | several | Schema-probe / "schema-compatible" axes only. |
| `shim\|backward\|legacy\|transitional\|alias` | 0 | None. |
| `dual-write` | 0 | None. |
| `TODO\|FIXME` | 0 | None. |
| `for now\|temporary\|workaround\|hack\|magic` | 0 | None. |
| `hardcode\|hard-code\|placeholder` | 0 | None. |
| `feature flag` | 2 | Schema-probe inputs (A1, §9), not a new flag. |
| `defer\|deferred\|future` | several | Future-tense framings around extension points and anti-scope. None mask current behavior. |
| `silent\|silently` | 0 | None. |
| `fallback` | 1 | §8 platform-fsync fallback, documented + tested. |
| `stub` | 0 | None. |
| `journal` | many | Durable journal mechanism; lock-before-publish ordering; `canonical_records_path` companion file; operation_uuid handle. |
| `staging` | many | Per-process operation-uuid staging path; pre-lock scratch state. |
| `operation_uuid` | many | Journal field, staging path discriminator, transcript-temp suffix. |
| `frozen\|freeze` | several | Resolved identity is frozen at §4 pre-mutation step 9 and used by recovery; no rediscovery. |
| `verification\|verify` | several | Postimage hash verification + fresh export verification before journal deletion. |
| `idempotent` | 2 | Recovery DB re-application; busy-cleanup unlink. |
| `quarantine` | several | Ambiguous-hash recovery directory and crash state #9. |
| `concurrent` | 2 | T-concurrent-import-replace test row. |

## Patterns followed correctly (Rev 4)

- **Hard refusal of provider-native input** preserved (§1, §3, §10, §13).
- **Provider-native bytes on disk** (§3, §6, §13): renderer is the dual of
  the export parser with a round-trip oracle.
- **Lossy renderer refusal** (§3, §9.1): typed `15 invalid-input-transcript`
  with named error-code shape.
- **Two-phase atomic rename + fsync + parent-dir fsync** (§4 D2, §8):
  unchanged.
- **Durable pending-op journal with frozen identity and canonical postimage
  material** (§4, §6, §8): preserved.
- **No-delete-before-verify ordering** (§4 steps 10-13, §8 side-effect
  contract): preserved.
- **Idempotent recovery DB re-application** from `canonical_records_path`
  using frozen segment identity (§6 step 4, §9.1 T-recovery-rename-only):
  preserved.
- **Lock-before-publish ordering for per-session artifacts (NEW Rev 4)**
  (§4 steps 1-5, §8 side-effect contract): Audit's Fix A is mainline; the
  per-session journal and canonical-records paths are unaddressable until
  the lock is held.
- **Operation-uuid staging discriminator (NEW Rev 4)** (§4 success flow
  step 1, §6 journal API, §8 side-effect contract): per-process scratch
  state is keyed by random uuid; lossless association with the per-session
  canonical-records file is preserved across the under-lock rename.
- **T-concurrent-import-replace pin (NEW Rev 4)** (§9.1): the audit's
  required race test is mainline.
- **Typed exit-codes mirroring the harness namespace** (§5, §13): unchanged.
- **Explicit named residuals in §12**: each enumerated with a recovery
  story rather than masked.
- **Codex two-track via typed exit `12`** (§9.1 last row): preserved
  (W3 narrows but persists).
- **No second ownership path** (§13 row, A2): preserved.
- **No second lock format** (§4 D1, §8): preserved; sibling-PR retrofit
  for in-binary writers named.
- **Receipt as the durable observability surface** (§6, §11): preserved.
- **Documented canonical-field loss** (§6, §7, §9, §12, §13): preserved;
  W5 unchanged.

## Specific shortcut traps (re-validated against Rev 4)

- **Migration-style temp without fsync.** §8 (`:676-687`) preserves fsync
  ordering and extends it to staging. PASS.
- **Migration-style temp filename collision.** Transcript temp uses
  `<jsonl_path>.tmp-import-replace-<operation_uuid>` (§8 `:595-597`);
  staging uses `staging/<operation_uuid>.canonical.jsonl`. UUID-keyed in
  both places. PASS.
- **Per-session journal filename collision (Rev 3 W4).** Rev 4 keeps
  `session-<id>.pending` and `session-<id>.canonical.jsonl` as single-name-
  per-session paths but writes them only under `SessionLock`. The audit's
  required reorder is in §4 steps 3-5; the audit's required test is at
  §9.1 T-concurrent-import-replace. PASS, W4 retired.
- **Running invocation as session-busy lock.** A6 and §12 #3 refuse;
  supported signal is `SessionLock`. PASS.
- **Preimage over DB summary rows.** A4 explicitly hashes the canonical
  export byte stream. §4 closing paragraph (`:379-381`) reaffirms "Its
  hashes are canonical export hashes for recovery comparison, not raw
  provider-native file-byte hashes." PASS.
- **`session_turns` reconstruction from canonical input.** §7 step 1-3
  preserves canonical fields; step 4 NULLs absent canonical fields;
  recovery (§6 step 4) uses the same canonical input file the forward
  path used (`canonical_records_path`), not a re-derivation from
  provider-native disk bytes. PASS.
- **Auto-resume after replace.** §1, §11, §13 refuse. PASS.
- **Auto-`migrate-db` after replace.** §11, §12, §13 refuse. PASS.
- **Cross-provider migration coupling.** §11 keeps `migration::migrate_chain_segment`
  UNCOUPLED. PASS.
- **Renderer round-trip silence.** §3 (`:241-242`) and §9.1 "Postimage
  round-trip" (`:716`) bind the renderer to a round-trip-through-export
  oracle. §4 step 11 (`:338-342`) elevates the oracle from test-only to a
  runtime gate before commit. PASS.
- **Quarantine self-heal.** §6 step 6 (`:480-485`) quarantines on
  ambiguous hash; does not auto-rewrite. Anti-scope on manual recovery
  CLI is explicit (§12 #2 `:810-812`). PASS-with-nits (N3 marker shape,
  N4 canonical_records_path lifetime).
- **Field-loss silent reconstruction.** §6 / §7 / §9 / §12 / §13 enumerate
  the loss; §9.1 "DB metadata loss is explicit" pins the expected
  NULL/default state. PASS-with-watchpoint (W5).
- **No-delete-before-verify (Rev 3, preserved Rev 4).** §4 steps 10-13
  (`:333-346`) and §8 (`:625-629`) place journal deletion strictly after
  postimage hash verification, fresh export round-trip verification, and
  SQLite commit. §9.1 T-no-deletion-before-verify (`:712`) tests the
  ordering. PASS.
- **Recovery via stale resolver state (Rev 3, preserved Rev 4).** §4
  closing paragraph (`:382-389`), §6 step 4 (`:472-481`), and §9.1
  T-recovery-rename-only (`:707`) bind recovery to **frozen** identity
  from the journal, not to current `StateDb::resolve_resume` output.
  PASS.
- **Lock-before-publish for per-session artifacts (NEW Rev 4).** §4 steps
  1-5 (`:302-319`), §8 side-effect contract (`:599-622`), and §9.1
  T-concurrent-import-replace (`:704`) pin the ordering with a typed
  busy-exit and a named test. PASS.
- **Operation-uuid handle (NEW Rev 4).** Journal field (`:370`), staging
  path (`:278`), and transcript temp suffix (`:595-597`) all use the same
  uuid. Recovery uses the field to associate the journal with the
  canonical-records file (§4 closing prose `:382-389`). No silent dual-
  source. PASS.

## Conclusion

Verdict: **LOW**.

Rev 4 closes Round 3's AIR-R3-F01 cleanly. The audit's three required
proposal changes — acquire `SessionLock` before publishing per-session
journal artifacts (Fix A), use an operation-unique handle so only the lock
owner can publish/consume the active per-session pending entry (a tighter
form of Fix B applied to staging), and add a concurrency T-row that pins
all four correctness invariants — each map to a specific success-flow step
ordering, a specific journal field, and a §9.1 T-row.

The reordered flow is race-free for both the documented single-instance
crash threat model (every T0..T14 boundary maps to a deterministic §8
recovery action, including the new staging-boundary pre-rename no-op case)
and the cooperative concurrent-invocation case (per-session paths are
unaddressable without the exclusive lock; the lock-busy contender unlinks
only its own operation-unique staging file). The audit-track W4 is
retired.

Round 1 closures (AIR-R1-F01..F04) and Round 2 closure (AIR-R2-F01) carry
forward; the renderer contract, cooperative-lock retrofit citation,
explicit field-loss model, frozen-identity journal, no-delete-before-verify
ordering, and frozen-identity recovery are unchanged or strengthened. Eight
canonical shortcut patterns and sixteen specific shortcut traps pass; two
new traps (lock-before-publish, operation-uuid handle) are added by Rev 4
and pass.

Two watchpoints persist for audit-/scope-track:

- **W3** — A5 / §6 receipt enum commit to `codex_session`; §9.1 "Postimage
  round-trip" residual still hedges. Audit-track should pin which fork is
  binding before Phase 6.
- **W5** — Asserted downstream NULL-tolerance is false for
  `latest_compaction_boundary`. Phase 5/Phase 6 should harden consumers,
  extend the canonical schema, or narrow §7 step 4's prose.

Six LOW-severity nits (N1 stale-temp cleanup scope, N2 `source_file`
conditional, N3 quarantine-marker shape, N4 canonical_records_path
lifetime in quarantine, N5 staging-file cleanup specification, N6 per-
invocation vs. once-at-agent-startup recovery) are filed for Phase 6
specification precision. N1 and N2 carry from Round 1; N3 carries from
Round 2; N4 carries from Round 3; N5 and N6 are new in Rev 4.

No regression from Round 3: every Rev 3 protection (atomic rename, double
preimage check, typed exit namespace, no second ownership path, no second
lock format, durable journal with frozen identity, receipt-as-
observability, field-loss documented, no-delete-before-verify, recovery
without stale resolver state) is preserved. No new finding rises to MEDIUM
or HIGH.
