# 06-import-replace — Phase 4 Scope Risk Assessment (Rev 4)

**Assessor:** scope reviewer
**Verdict:** **LOW.** Rev 4 is a targeted closing pass against the
single Round 3 audit finding (AIR-R3-F01). The finding is closed at
the scope level on all three required-change axes the Round 3 audit
named. (1) `SessionLock::acquire` is now strictly **before** any
per-session journal artifact is published: the pre-lock window writes
only an operation-unique staging file at
`<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`,
which no other concurrent contender can name or delete
(`proposals/06-import-replace.md:276-279`,
`proposals/06-import-replace.md:599-610`). (2) The atomic rename to
the per-session path
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`
and the pending journal write at
`<state-data-dir>/replace_journal/session-<session_id>.pending` both
happen **under the lock** (`proposals/06-import-replace.md:308-318`,
`proposals/06-import-replace.md:611-615`); a busy contender unlinks
**only its own** staging file and is explicitly forbidden from
touching the per-session paths (`proposals/06-import-replace.md:605-608`).
(3) A new T-row `T-concurrent-import-replace` exercises two
subprocesses racing the same `session_id` and pins the invariants
(exactly one wins; loser leaves no per-session artifacts; winner's
journal/canonical records/transcript/DB all remain intact)
(`proposals/06-import-replace.md:704`). The five prior closures
(AIR-R1-F01..F04 + AIR-R2-F01) all still hold — Rev 4 only changes
the lock-vs-journal ordering inside the AIR-R2-F01 closure path and
does not loosen any of the others. Anti-scope is intact, the
single-PR boundary is still justified after the journal grows by one
identity field (`operation_uuid`) plus a per-process staging
directory, and every cross-feature constraint in
`06-session-override-contract.md:106-122` still maps to a numbered
section. The Rev 3 W4'/W5 watch-flag (concurrent same-session
import-replace racing the pre-lock journal/side-file writes) is
**retired by Rev 4** because the pre-lock window no longer publishes
shared per-session paths. Two carry-over watch-flags (W1, W2, W3')
carry forward; one new informational watch-flag (W6) covers
opportunistic `staging/` directory cleanup formalization (Phase 5
hookpoint, not scope). Three carry-over Rev 1 nits (N1/N2/N3)
untouched. No findings at MEDIUM or higher.

---

## 1. Closure check on AIR-R3-F01

Audit-only closure: each required-change bullet from
`risk/06-import-replace-audit.md` (Rev 3 audit, AIR-R3-F01) is
matched against the Rev 4 text that resolves it. No new audit
work is performed here.

### Rev 3 audit required-change recap

> - Acquire `SessionLock` before publishing per-session journal artifacts; or
> - Use operation-unique journal/canonical paths plus an ownership token so only
>   the lock owner can publish, consume, or delete the active per-session pending
>   entry; and
> - Add a concurrency test where two `import-replace` processes target the same
>   session, one wins the lock, the loser exits `13`, and the winner's journal,
>   canonical records, transcript, and DB update remain intact.

Rev 4 takes the **first** option (lock before per-session
publication) and lays per-process staging behind an `operation_uuid`
suffix, then renames into the per-session path under lock. That
satisfies both the "before lock" requirement and the
"operation-unique paths" requirement at once.

### Required change 1 — acquire `SessionLock` before publishing per-session journal artifacts

Rev 4 §1 changelog (`proposals/06-import-replace.md:58-70`) explicitly
records the reorder: "reordered acquire flow so canonical records
first land in a per-process staging path (operation_uuid suffix),
then are renamed to the per-session canonical-records path AFTER
SessionLock acquired. Journal entry is written under lock."

§4 success-flow walk (`proposals/06-import-replace.md:300-348`):

| Step | Before lock? | Per-session shared path touched? |
| --- | --- | --- |
| 1 — write `staging/<operation_uuid>.canonical.jsonl` | yes | **no** — operation-unique |
| 2 — resolve metadata + freeze identity | yes | no |
| 3 — `SessionLock::acquire`; busy → unlink staging only, exit `13` | acquire | no |
| 4 — atomic rename staging → `session-<id>.canonical.jsonl`; fsync `replace_journal/` | under lock | yes |
| 5 — write `session-<id>.pending` under lock | under lock | yes |
| 6 — read existing transcript, compute preimage, record in journal, verify against `--preimage-sha256` | under lock | yes |
| 7 — render canonical → provider-native, write `<jsonl_path>.tmp-import-replace-<operation_uuid>`, fsync | under lock | n/a (operation-unique transcript temp) |
| 8 — atomic rename to `jsonl_path`, fsync parent | under lock | n/a |
| 9 — SQLite transaction: replace `session_turns` from `canonical_records_path`, refresh segment/chain | under lock | n/a |
| 10 — postimage hash verify against journal | under lock | n/a |
| 11 — fresh export verify against `canonical_records_path` | under lock | n/a |
| 12 — SQLite commit | under lock | n/a |
| 13 — delete journal + canonical records file, fsync `replace_journal/` | under lock | yes (deletion only) |
| 14 — release `SessionLock` | release | n/a |
| 15 — emit receipt, exit `0` | post-lock | n/a |

§8 side-effects section restates the same invariant explicitly
(`proposals/06-import-replace.md:599-615`):

> Before acquiring the session lock and before writing any per-session journal
> artifact, import-replace writes normalized canonical records only to
> `<state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl`.
> This staging path is operation-unique scratch state.
>
> If `SessionLock::acquire` returns busy, import-replace unlinks only its
> staging file and exits `13`; it must not create or modify
> `<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl` or
> `<state-data-dir>/replace_journal/session-<session_id>.pending`.

The contract pins both halves the audit asked for: pre-lock writes
are *operation-unique only* and the busy-contender's cleanup is
*explicitly forbidden from per-session paths*.

**Required change 1: closed.**

### Required change 2 — operation-unique paths plus an ownership token

Rev 4 satisfies this in spirit even though it elects option 1
(lock-before-publish) as the primary mechanism:

- The pre-lock staging file is operation-unique by construction
  (`<operation_uuid>.canonical.jsonl`,
  `proposals/06-import-replace.md:276-279`,
  `proposals/06-import-replace.md:603`). Two contenders have disjoint
  staging paths.
- The journal payload now carries `operation_uuid`
  (`proposals/06-import-replace.md:370`,
  `proposals/06-import-replace.md:618`) so recovery can associate the
  pending journal with the canonical-records file across the
  staging-to-session rename. This is the "ownership token" leg in
  recovery semantics.
- §4 explicitly notes "Only the lock owner can land canonical
  records at this path" (`proposals/06-import-replace.md:312-314`).
  Combined with the busy-contender prohibition above, only the
  lock-holding process ever publishes, consumes, or deletes the
  active per-session pending entry within a process lifetime.

Note: the journal-attached `operation_uuid` is a recovery
association token, not a write-authorization token. Write
authorization is provided by `SessionLock` itself
(`proposals/06-import-replace.md:583-591`). That is consistent with
the audit's "OR" between lock-before-publish and ownership-token —
Rev 4 takes both: lock for write authorization, `operation_uuid` for
recovery-time association.

**Required change 2: closed.**

### Required change 3 — concurrency test where two processes race the same session

Rev 4 §9.1 adds T-concurrent-import-replace
(`proposals/06-import-replace.md:704`):

> Spawn two subprocesses calling import-replace on the same `session_id`;
> exactly one process wins the lock and the other exits busy.
> ...
> Exactly one returns `0` with valid receipt and final transcript/export
> matching the winner. The loser returns `13 session-busy`, unlinks its
> staging file, leaves no per-session journal/canonical files in
> `<session>.canonical.jsonl` or `<session>.pending`, and performs no
> transcript mutation.

The test pins the four observables AIR-R3-F01 cared about:

| Observable | Asserted? | Citation |
| --- | --- | --- |
| Exactly one process wins | yes | "Exactly one returns `0` with valid receipt" |
| Loser exits `13 session-busy` | yes | "The loser returns `13 session-busy`" |
| Loser does not delete per-session artifacts | yes | "leaves no per-session journal/canonical files in `<session>.canonical.jsonl` or `<session>.pending`" |
| Winner's mutation succeeds | yes | "final transcript/export matching the winner" |

The residual-risk column flags the scheduling timing gap honestly
("test needs a barrier or lock-acquire hook to make the race
observable") — that's a Phase 5/6 fixture concern, not a scope
issue.

**Required change 3: closed.**

### Audit-history reconciliation

The Round 3 audit observed that
`risk/06-import-replace-audit-history.md` was not in the current
checkout. That file existed at commit `4a598ac` recording the four
R1 closures. The expected next-row entries after Rev 3 and Rev 4
respectively are:

- "Round 2 — AIR-R2-F01 closed by Rev 3 (journal expansion +
  deletion-last ordering + recovery T-rows)."
- "Round 3 — AIR-R3-F01 closed by Rev 4 (lock-before-journal
  reorder + per-process staging + concurrency T-row)."

Rev 4's own §1 changelog (`proposals/06-import-replace.md:58-70`)
tags the change set against AIR-R3-F01 in §4, §6, §8, and §9 — the
four sections the audit's required-change bullets touched.

---

## 2. Regression check on R1/R2 closures (AIR-R1-F01..F04 + AIR-R2-F01)

Walked Rev 4 against each prior closure path. No regressions.

### AIR-R1-F01 — Canonical-bytes-vs-provider-native (still closed)

Rev 4 leaves the renderer contract intact:

- §1 anti-scope clause "The replacement transcript file does not
  store canonical JSONL in v1" (`proposals/06-import-replace.md:99-101`)
  unchanged.
- §3 step 11 + `CanonicalToProviderRenderer` contract
  (`proposals/06-import-replace.md:207-247`) unchanged.
- §6 `src-tauri/src/session_replace/render/` module
  (`proposals/06-import-replace.md:431-433`) unchanged.
- §13 rows "Provider transcript file receives provider-native
  bytes" / "Lossy canonical-to-provider re-encoding is refused" —
  both still Yes (`proposals/06-import-replace.md:838-839`).
- §9.1 unsupported-record-class test row preserved
  (`proposals/06-import-replace.md:702`).

The Rev 3 invariant that recovery does not re-derive DB rows from
provider-native postimage bytes is preserved verbatim
(`proposals/06-import-replace.md:387-389`); Rev 4 does not loosen
it. **Held.**

### AIR-R1-F02 — Crash recovery (still closed; further hardened by Rev 4)

The pre-lock journal-publication race that AIR-R3-F01 surfaced was
itself a hidden weakness in F02's closure path: if a busy
contender could delete a lock holder's per-session journal, the
post-rename/pre-DB recovery branch could be left with no signal.
Rev 4 closes that hidden weakness directly. **Held and slightly
hardened.**

### AIR-R1-F03 — Cooperative-lock surface (still closed)

- §1 Rev 2 changelog bullet on PR #17 dependency unchanged
  (`proposals/06-import-replace.md:34-36`).
- §13 row text on PR #17 + advisory carve-out unchanged
  (`proposals/06-import-replace.md:835`).
- §12 residual #3 ("Running invocation rows are not treated as
  authoritative busy locks") unchanged
  (`proposals/06-import-replace.md:813-815`).

Rev 4 did not retouch the lock-observation claim. **Held.**

### AIR-R1-F04 — Canonical-record field-loss (still closed)

- §6 reusable-API bullet on intentional `NULL`/defaults for
  `parent_turn_id` / `is_sidechain` / `is_compaction_boundary`
  (`proposals/06-import-replace.md:439-442`) unchanged.
- §7 #4 "documented data loss in v1"
  (`proposals/06-import-replace.md:546-549`) unchanged.
- §7 last-paragraph future-extension note
  (`proposals/06-import-replace.md:575-577`) unchanged.
- §9.1 "DB metadata loss is explicit" row
  (`proposals/06-import-replace.md:714`) unchanged.
- §12 residual #5 (`proposals/06-import-replace.md:818-820`)
  unchanged.
- §13 "State consistency" row text
  (`proposals/06-import-replace.md:842`) unchanged.

Rev 4 did not retouch the field-loss model. **Held.**

### AIR-R2-F01 — Journal recovery identity / deletion-last ordering (still closed; strengthened by Rev 4)

The Rev 3 closure had three axes (resolved-identity, deletion-last,
recovery T-rows). Rev 4 strengthens axis 1 by adding `operation_uuid`
to the journal payload (`proposals/06-import-replace.md:370`) so
recovery can associate the pending journal with the canonical-records
file across rename. Axis 2 (deletion-last) is unchanged: journal
deletion still happens after postimage verify + fresh export verify +
SQLite commit (`proposals/06-import-replace.md:344-346`,
`proposals/06-import-replace.md:401-403`). Axis 3 recovery T-rows are
unchanged in scope and gain a sibling row T-concurrent-import-replace
that proves the loser-cleanup leg pre-staged for AIR-R3-F01. **Held
and strengthened.**

§6 startup-recovery contract (`proposals/06-import-replace.md:455-486`)
adds a graceful sub-branch for "pending journal lacks a completed
`preimage_sha256` because the process died before reading the
original transcript" (`proposals/06-import-replace.md:462-466`).
That covers the new crash window opened by writing the journal at
step 5 and recording the preimage at step 6 — a side-effect of the
lock-before-journal reorder. The branch treats it as a pre-rename
no-op (delete journal + canonical records, no DB mutation), which
is the correct deterministic recovery for that state.

### Round 1/2 closure summary

| ID | Status under Rev 4 |
| --- | --- |
| AIR-R1-F01 | held |
| AIR-R1-F02 | held; hidden weakness closed by AIR-R3-F01 fix |
| AIR-R1-F03 | held |
| AIR-R1-F04 | held |
| AIR-R2-F01 | held; axis 1 strengthened by `operation_uuid` field |

---

## 3. Race-freeness of Rev 4 lock-before-journal reorder

**Documented threat model** (carried forward from Rev 3 + audit
expansion):

- Single-process crash at any point in the success flow
  (`proposals/06-import-replace.md:635-669` enumerates 9 crash
  states under Rev 4).
- Cooperative SessionLock surface; non-cooperating external
  writers are an explicit residual
  (`proposals/06-import-replace.md:813-815`).
- TOCTOU between the early/preflight preimage hash and the
  protected commit window (`proposals/06-import-replace.md:350-355`).
- **Newly documented in Rev 4**: two cooperative `import-replace`
  processes targeting the same `session_id` concurrently. Round 3
  audit promoted this from "outside threat model" to "inside
  threat model" because it is a race between two instances of the
  new cooperative command. Rev 4 §9.1 T-concurrent-import-replace
  is the test anchor (`proposals/06-import-replace.md:704`).

### 3.1 Two-process race walk (the AIR-R3-F01 case)

Two processes A and B run `agents session import-replace <id>` on
the same resolved session-id concurrently:

| Step | Process A | Process B | Notes |
| --- | --- | --- | --- |
| 0 | starts | starts | both pass clap parsing |
| 1 | writes `staging/A_uuid.canonical.jsonl`, fsyncs | writes `staging/B_uuid.canonical.jsonl`, fsyncs | disjoint per-process paths; no overwrite |
| 2 | resolves metadata, freezes identity | resolves metadata, freezes identity | independent |
| 3a | `SessionLock::acquire` wins | `SessionLock::acquire` returns busy | OS-serialized; one winner |
| 3b | proceeds to step 4 | unlinks only `staging/B_uuid.canonical.jsonl`, exits `13` | per `proposals/06-import-replace.md:605-608`, B may not touch per-session paths |
| 4 | atomic rename `staging/A_uuid.canonical.jsonl` → `session-<id>.canonical.jsonl` under lock | (already exited) | only A's records can land at the per-session path |
| 5 | writes `session-<id>.pending` with `operation_uuid = A_uuid` under lock | (already exited) | A's journal is intact |
| 6–13 | continues to commit and cleanup | (already exited) | A's transcript + DB update + journal deletion all complete |

Invariants preserved:

1. B's pre-lock state is bounded to `staging/B_uuid.canonical.jsonl`
   alone; B never names the per-session path
   `session-<id>.canonical.jsonl` or `session-<id>.pending`.
2. A's per-session paths are written *after* A holds the lock; no
   contender can race A on those paths.
3. If B is the lock winner instead, the symmetric outcome holds with
   roles swapped.

This is the exact concurrency invariant AIR-R3-F01 required and
T-concurrent-import-replace pins. **Closed.**

### 3.2 Single-process crash walk against §8 crash states 1–9

Rev 4 §8 expanded to 9 crash states (Rev 3 had 8). The new state 1
covers the pre-lock staging window:

| Crash state | When | Recovery deterministic under Rev 4? |
| --- | --- | --- |
| 1 — pre-lock staging | After step 1 staging write, before step 3 lock acquire | Yes. No per-session journal artifact exists. Stale `staging/<uuid>.canonical.jsonl` may linger; §8 specifies opportunistic cleanup by age and operation UUID (`proposals/06-import-replace.md:637-640`). No transcript or DB mutation. |
| 2 — under lock, after rename + pending write, before transcript temp | After step 5, before step 7 begin | Yes. Recovery item 5 (`proposals/06-import-replace.md:477-479`) sees transcript hash matching preimage and deletes journal + canonical records file. Pre-step-6 case is handled by the new "pending journal lacks `preimage_sha256`" branch (`:462-466`) as pre-rename no-op. |
| 3 — post-temp pre-rename | After step 7, before step 8 | Yes. Same as #2 (transcript still preimage); transcript temp `.tmp-import-replace-<uuid>` lingers and is opportunistically unlinked by the next attempt (`:645-647`). |
| 4 — post-fsync pre-rename | After step 7 fsync, before step 8 | Yes. Same as #3. |
| 5 — post-rename pre-DB | After step 8 rename, before step 9 begin | Yes. Recovery item 4 (`:471-476`) sees postimage hash, re-applies DB updates idempotently from `canonical_records_path`, refreshes the journal-frozen segment, deletes journal + canonical records file. |
| 6 — mid-DB transaction | During steps 9–12 | Yes. SQLite either commits or rolls back per its own durability. Recovery item 4 re-applies DB idempotently from `canonical_records_path`. Idempotent re-replace is safe because §7 deletes-then-inserts on `(provider_name, session_id)`. |
| 7 — post-commit pre-journal-delete | After step 12, before step 13 | Yes. Recovery item 4 re-applies DB idempotently and deletes journal + canonical records file. |
| 8 — preimage-only | Hash matches `preimage_sha256` only | Yes. Recovery item 5 (`:477-479`) deletes journal + canonical records file; no DB mutation. |
| 9 — ambiguous | Hash matches neither, or transcript unparseable | Yes. Recovery item 6 (`:480-485`) moves journal to `replace_journal/quarantine/`, preserves canonical records file, logs warning, leaves transcript and DB untouched. |

Every crash state has a deterministic recovery branch. The
hash-domain consistency from F01 (journal hashes are canonical
export hashes; recovery rehashes through the canonical reader, not
raw provider bytes) still composes with the provider-native
renderer. **Race-free for the documented single-process threat
model.**

### 3.3 Concurrency × crash-state cross-product

A worth-checking interaction: process A acquires lock and crashes
mid-flow while process B is still pre-lock. After A's crash, A's
`SessionLock` lease eventually expires and B (or a fresh process)
can re-acquire. Recovery scans the journal directory and reconciles
A's pending entry as an orphan:

- If A crashed pre-rename (state 1): no per-session journal exists;
  recovery is a no-op for A. B's subsequent attempt under fresh
  lock proceeds normally.
- If A crashed under-lock pre-DB (state 2–6): per-session journal
  exists; recovery uses postimage-or-preimage branch to determine
  whether the rename landed; B (or any subsequent process) will
  see post-recovery state and proceed normally.

Recovery happens before normal session resolution work
(`proposals/06-import-replace.md:451-453`), so B's attempt cannot
race A's leftover state. The pending journal carries A's
`operation_uuid`, so even if recovery were re-entered concurrently
by two processes, idempotent unlink + idempotent DB re-replace
keep it safe.

### 3.4 Out of the documented threat model (residuals only)

- **Non-cooperating external writers** that bypass `SessionLock`
  remain a §12 residual. AIR-R1-F03 closure stands; not affected
  by Rev 4.
- **Concurrent recovery scans** by two simultaneously-starting
  processes. The proposal does not explicitly serialize recovery
  but idempotent unlink and idempotent DB re-replace keep it safe
  in practice. Phase 5 hookpoint should pin a recovery mutex or
  per-journal flock; flagged below as W6 informational.
- **Concurrent cleanup of stale `staging/` files** by age/UUID is
  not formally specified; Phase 5 hookpoint concern. Flagged as
  W6.

**Verdict on §3:** Rev 4's lock-before-journal reorder is
race-free for the documented threat model — including the
newly-documented two-process cooperative race that motivated
AIR-R3-F01.

---

## 4. Anti-scope and cross-feature constraints (no regression)

### Anti-scope (vs `06-session-override-contract.md:117-122` and harness)

| Anti-scope clause | Rev 4 stance | Compliance |
| --- | --- | --- |
| No auto-resume | §1 (`:88-91`); §11 (`:780-783`); §13 rows | yes |
| No provider spawn | §1; §11; §13 row | yes |
| No quota refresh | §1; §11; §13 row | yes |
| No config edits | §1; §11; §13 row | yes |
| No coupling to `migrate-config` | §1; §11; §13 row | yes |
| No GUI/Tauri/daemon/server | §1 (`:94`); §11.1 (`:763-764`) | yes |
| No provider-native JSONL as stable public input | §1 (`:97-98`); §3; §10 | yes |
| No manual recovery CLI in v1 | §6 last paragraph (`:487-489`); §12 #2 (`:810-812`); §13 row (`:849`) | yes |

Rev 4 does not introduce any new public surface. The new
`staging/` subdirectory under `<state-data-dir>/replace_journal/`,
the `operation_uuid` journal field, and the operation-unique
canonical staging path are all under
`<state-data-dir>/replace_journal/` and remain documented as
private implementation state
(`proposals/06-import-replace.md:379-381`,
`proposals/06-import-replace.md:797-800`).

### Cross-feature constraints (`06-session-override-contract.md:106-122`)

Every row in §13 still maps to its own numbered section. No row
loosened by Rev 4; the journal-related row is unchanged in
substance and the lock-related rows are tightened by virtue of the
lock-before-publish reorder:

| Constraint | Rev 4 | Notes |
| --- | --- | --- |
| Shared error-code namespace 10–15 | yes | §5 unchanged |
| Single ownership via `StateDb::resolve_resume` | yes | §4 step 7 (`:281-283`) unchanged |
| Lock observation once pause-handshake lands | yes within cooperative surface | F03 closure unchanged; lock now strictly precedes per-session journal artifacts (Rev 4) |
| Refuses if not exclusively owned | yes within cooperative lock surface | non-cooperating writers remain a §12 residual; cooperative two-process race is now closed |
| Read-only `StateDb` open / schema compatibility | yes | §4 step 5 → exit `14` |
| Reusable canonical reader from export | yes | A3, §3, §9 |
| Provider transcript receives provider-native bytes | yes | §3 / §6 renderer module unchanged |
| Lossy canonical-to-provider re-encoding refused | yes | §3 / §9 unsupported-record-class test |
| Two-phase atomic file replacement | yes | §4 / §8 unchanged |
| Durable journal closes post-rename/pre-DB crash recovery | yes (further hardened) | journal + deletion-after-verify + lock-before-publish — Rev 4 closure of AIR-R3-F01 |
| State consistency covers required rows | yes, with documented canonical-field loss | §7 D4a unchanged |
| No manual recovery CLI in v1 | yes | §6 / §12 |

---

## 5. Single-PR boundary

Re-evaluated against Rev 4's deltas (lock-before-journal reorder,
operation-unique `staging/` directory, `operation_uuid` journal
field, T-concurrent-import-replace test row). Same four split
candidates evaluated for Rev 3:

**Split A — `session_replace/render/` as a separate prereq PR.**
Unchanged from Rev 2/3. Renderer is private API consumed only by
`session_import_replace/`; splitting yields a private renderer
with no caller. **Rejected.**

**Split B — durable journal + recovery contract as a follow-up PR.**
Unchanged from Rev 2/3. Without the journal, both AIR-R1-F02 and
AIR-R2-F01 reopen. Now also AIR-R3-F01 reopens because the
lock-before-publish invariant only matters when there *is* a
journal. **Rejected.**

**Split C — DB consistency helper vs CLI surface.** Unchanged.
**Rejected.**

**Split D — recovery scanner as a prereq sibling PR.** Unchanged.
The startup-recovery contract only fires on journals written by
import-replace; without import-replace, the scanner has nothing to
recover. **Rejected.**

**Split E (new in Rev 4 — staging directory cleanup as a follow-up).**
The staging directory is intrinsic to the AIR-R3-F01 fix; deferring
its cleanup contract reopens the lock-before-publish closure.
Opportunistic cleanup is in scope and is referenced from §4 step 11
(transcript directory) and §8 crash state 1 (staging directory).
**Rejected.**

**Single-PR boundary: still justified.**

---

## 6. Scope-direction analysis (Rev 4 vs Rev 3)

| Surface vs Rev 3 | Direction | Reason |
| --- | --- | --- |
| Pre-lock window | scope-tightening | only operation-unique scratch state; per-session paths gone from pre-lock window |
| Per-session journal/canonical paths | scope-tightening | publication strictly under `SessionLock`; busy-contender forbidden from touching them |
| Journal payload | targeted addition | `operation_uuid` field added to associate journal with canonical-records file across rename; private state, no public surface impact |
| `staging/` subdirectory | additive (private state) | required so pre-lock writes are operation-unique; bounded to `<state-data-dir>/replace_journal/staging/` |
| §4 success-flow ordering | scope-tightening | lock acquisition moved before per-session publication; deletion-last invariant preserved |
| §8 crash states | additive (1 new state) | crash state 1 covers pre-lock staging window; recovery branch is "no per-session journal exists, opportunistic staging cleanup later" |
| §9 test track | additive coverage | T-concurrent-import-replace exercises two-process race on same session id |
| Anti-scope | unchanged | eight harness/initiative clauses still intact |
| Public surface | unchanged | §2 / §6 receipt JSON / §5 exit codes identical to Rev 3 |
| AIR-R1-F01..F04 closures | unchanged | held |
| AIR-R2-F01 closure | strengthened | axis 1 (resolved identity) gains `operation_uuid` association token |

Net direction: Rev 4 makes a targeted closure against the single
R3 audit finding without expanding the public surface and tightens
the lock surface (the previous concurrent-process race that Rev 3
had carried as W4'/outside-threat-model is now closed at the
contract level). The new internal state (`staging/` directory +
`operation_uuid` field) is the minimum work to satisfy the audit
and is bounded to v1 scope.

---

## 7. No-regression check (Rev 3 → Rev 4)

| Rev 3 scope item | Rev 4 status | Notes |
| --- | --- | --- |
| Anti-scope (8 clauses) | held | unchanged |
| Cross-feature constraints | held | "Durable journal closes …" row tightened by the lock-before-publish invariant |
| Coverage matrix (problem-map §1–§7) | held | additive coverage on AIR-R3-F01 closure surfaces |
| Single-PR boundary | held | re-justified against per-process staging directory |
| W1 stale-temp ordering (transcript dir) | unchanged | §4 step 11 same wording; still a Phase 5 hookpoint concern |
| W2 schema-probe flag flip | unchanged | §4 step 5 same; coordination with 06-schema-probe still pending |
| W3' renderer round-trip parity | unchanged | §9.1 postimage round-trip row carries it; load-bearing under §4 step 11 fresh-export verification |
| W4' journal/lock ordering invariant + side-file race | **closed by Rev 4** | the entire pre-lock per-session-path window that W4' described no longer exists |
| N1 exit-namespace 16/17 omission note | unchanged | §5 / §13 still do not carry the one-line note |
| N2 §7 conditional `source_file` hedge | unchanged | now §7 #5; same defensive language |
| N3 §1.1 "validated and narrowed" wording | unchanged | §1.1 line still says "validated and narrowed" |

**No regression.** Rev 4 was a targeted AIR-R3-F01 pass. The
previously-carried W4' is retired. One new informational
watch-flag (W6) is added to cover staging-directory cleanup
formalization (Phase 5 hookpoint, not scope).

---

## 8. Findings (severity ≥ MEDIUM)

**None.**

---

## 9. Watch-flags (informational; not findings)

### W1 — opportunistic stale-temp cleanup ordering (carry-over)

§4 step 11 ("Clean stale import-replace temp files in the target
transcript directory whose names match this feature's temp-file
convention and are not currently locked by another live replace
operation", `proposals/06-import-replace.md:289-291`) still runs
before lock acquire. Rev 4's `<jsonl_path>.tmp-import-replace-<operation_uuid>`
naming convention narrows the ambiguity (each in-flight operation
has a unique suffix), but the proposal does not specify how step
11 distinguishes a stale temp from a temp owned by another
in-flight import-replace at scan time. Phase 5 hookpoints should
pin the mechanism (per-temp flock sentinel, mtime threshold, or
post-lock cleanup). Not a scope issue.

### W2 — schema-probe `safe_for_import_replace` flag flip (carry-over)

Problem-map §3 #3 says 06-schema-probe returns
`safe_for_import_replace = false` until both `session_import_replace`
and `session_pause_handshake` features are present. The flip lands
in the schema-probe branch when this feature ships. §4 step 5
correctly defers the flip itself to schema-probe; this remains a
Phase 5 sequencing watch-flag, not a scope issue.

### W3' — renderer round-trip parity (carry-over)

§3 last paragraph of the renderer contract requires that "Every
supported rendered record must round-trip through export back to
the canonical input." §9.1 "Postimage round-trip" is the
end-to-end anchor. Phase 5 should sample real Claude / Codex
transcripts and verify renderer/export round-trip parity. Step 11
of §4 (fresh export verification under lock against
`canonical_records_path` before SQLite commit) is the load-bearing
runtime check; if renderer round-trip parity fails on a real-world
record class, the `1 operational-error` exit and quarantine branch
are the correct fail-closed behavior — but Phase 5 still needs to
pre-empt the common cases so quarantine is rare.

### W4' — RETIRED in Rev 4

Rev 3 carried W4' as the journal/lock ordering invariant + side-file
race (concurrent same-session import-replace racing the pre-lock
journal/side-file writes). Rev 4 closes it at the contract level by
reordering the success flow: pre-lock writes are now
operation-unique, per-session writes are under lock, busy-contender
cleanup is bounded to the contender's own staging file. The audit
promoted this from "outside threat model" (Rev 3) to "inside threat
model" (Round 3 audit) and then required it closed; Rev 4 closes
it. **Retired — not a watch-flag in Rev 4.**

### W6 — opportunistic `staging/` directory cleanup formalization (new in Rev 4)

§8 crash state 1 (`proposals/06-import-replace.md:637-640`) and the
§8 directory-fsync rule for `staging/` deletion on lock busy
(`:678-679`) imply opportunistic cleanup of stale staging files by
age and operation UUID, but the proposal does not formally define
*when* the cleanup runs. Two natural hookpoints exist:

1. As part of startup recovery (extend the `replace_journal/`
   scan to also walk `staging/` and unlink files past a TTL or
   without an active `flock`).
2. At the start of each fresh import-replace attempt (mirror the
   transcript-directory step 11 cleanup).

Neither is wrong; the choice belongs at Phase 5 hookpoint time.
Recovery does not depend on staging files (they are pre-lock
scratch, never the recovery signal), so deferring the
formalization does not affect crash-recovery determinism. Not a
scope issue; flagging so Phase 5 picks the resolution before
implementation.

---

## 10. Nits (severity LOW)

### N1 — exit-namespace 16/17 omission note (carry-over from Rev 1)

§5 lists exits `0` / `1` / `2` / `10`–`15`. Shared namespace at
`06-session-override-contract.md:106-111` also reserves `16`
(lock-token-invalid) and `17` (lock-expired). Import-replace
acquires its own lock under owner `"import-replace"` and does not
accept caller-supplied lock tokens, so 16/17 are not reachable on
this surface. Neither §5's preamble nor §13's row 1 says so. A
one-line note ("16 and 17 are pause/resume-handshake token
vocabulary; not reachable on this surface") would close the small
ambiguity. Drafting only.

### N2 — §7 #5 conditional `source_file` write (carry-over from Rev 1)

§7 #5 reads "Set `source_file` to the replaced `jsonl_path` when
the current schema/helper supports it; otherwise keep existing
ingest helper behavior if the column is not meaningful in this
branch." Given A1 commits to "earlier Initiative 06 surfaces land
before import-replace," the schema state at merge time should be
known: `session_turns.source_file` exists today. Recommended fix:
commit unconditionally to `source_file = jsonl_path` and remove
the hedge. Drafting.

### N3 — §1.1 "validated and narrowed" wording (carry-over from Rev 1)

§1.1 line reads "approved register validated and narrowed from
`research/06-import-replace-problem-map.md` §7." Counts match
(A1–A10 in both). "Narrowed" reads as a count reduction.
Recommended fix: "consolidated and re-themed from the problem-map
draft, with the same row count." Drafting.

---

## 11. Summary

- **Audit closure:** AIR-R3-F01 closed at the scope level on all
  three required-change axes — `SessionLock::acquire` moved
  strictly before per-session journal publication; pre-lock
  writes confined to operation-unique
  `staging/<operation_uuid>.canonical.jsonl` (with
  `operation_uuid` carried into the journal payload as the
  recovery association token); T-concurrent-import-replace pins
  the four behavioral invariants (winner unique, loser exits 13,
  loser leaves no per-session artifacts, winner mutation
  intact).
- **R1/R2 closures:** AIR-R1-F01..F04 + AIR-R2-F01 all still
  hold. AIR-R1-F02's hidden weakness (busy contender deleting a
  lock holder's pre-lock journal) is closed by the AIR-R3-F01
  fix. AIR-R2-F01 axis 1 strengthened by `operation_uuid` field.
- **Race-freeness:** Rev 4's lock-before-journal reorder is
  race-free for the documented threat model — including the
  newly-documented two-process cooperative race. All 9 §8 crash
  states have deterministic recovery branches; the
  concurrency × crash-state cross-product remains deterministic
  via idempotent unlink + idempotent DB re-replace + recovery
  before normal session-resolution work.
- **Anti-scope:** eight harness/initiative clauses still intact.
- **Cross-feature constraints:** all rows in §13 still
  satisfied; the lock-related rows are tightened.
- **Coverage:** complete; problem-map §1–§7 still maps with
  additive coverage on AIR-R3-F01 closure surfaces.
- **Single-PR boundary:** still justified after the
  lock-before-publish reorder, `staging/` directory, and
  T-concurrent-import-replace test row. Five split candidates
  (A/B/C/D/E) all produce dead intermediate state or reopen
  audit findings.
- **No regression:** Rev 3 anti-scope, constraints, coverage,
  single-PR boundary, three nits (N1/N2/N3), and three
  watch-flags (W1/W2/W3') all hold. W4' is **retired** because
  Rev 4 closes it at the contract level. One new informational
  watch-flag (W6) covers `staging/` cleanup formalization.
- **Findings:** none at MEDIUM or higher.
- **Watch-flags:** four total — W1 stale-temp cleanup ordering
  (carry; transcript dir), W2 schema-probe flag flip (carry),
  W3' renderer round-trip parity (carry; load-bearing under §4
  step 11), W6 staging-directory cleanup formalization (new in
  Rev 4; Phase 5 hookpoint).
- **Nits:** three carry-over drafting items (N1/N2/N3 from
  Rev 1).

**Verdict: LOW.** Rev 4 closes AIR-R3-F01 at the scope level
without expanding the public surface, regressing anti-scope, or
breaking the single-PR boundary. R1/R2 closures all still
standing; AIR-R1-F02 and AIR-R2-F01 are slightly hardened. The
lock-before-journal reorder is race-free for the documented
threat model — including the two-process cooperative race the
audit had explicitly required closed. Phase 5 dispatch can
proceed; W6 is the only new watch-flag worth pinning at
hookpoint time, and it does not block this gate.
