# 06-import-replace — Phase 4 Shortcut Risk Assessment (Rev 3)

## Verdict: LOW

Rev 3 closes Round 2 finding AIR-R2-F01 with concrete journal-schema and
flow-ordering changes that line up exactly with the audit's three required
revisions. Round 1 closures (AIR-R1-F01..F04) carry forward unchanged.
The reordered success flow is race-free against the documented
single-instance crash threat model in `proposals/06-import-replace.md:587-614`
(§8 crash states #1–#8). Two Round 1 watchpoints (W1, W2) remain retired.
Three watchpoints (W3 Codex two-track, W4 concurrent-invocation journal
race, W5 downstream NULL-tolerance) persist for audit-/scope-track and
Phase 6 specification precision; none rises to a shortcut finding under
this rubric.

No new finding rises to MEDIUM or HIGH.

## Round 2 closure check (AIR-R2-F01)

The Rev 2 audit (`risk/06-import-replace-audit.md:90-145`) required three
proposal changes for AIR-R2-F01 to clear:

1. Persist resolved recovery identity in the journal before transcript
   mutation (provider/storage/chain/segment/path/hashes + canonical
   postimage material).
2. Move fresh postimage export verification before journal deletion, or
   state that any post-DB verification failure leaves/quarantines the
   journal instead of deleting it.
3. Add a recovery test that simulates stale or ambiguous resolver-visible
   DB rows after rename and proves startup recovery uses journal identity
   rather than rediscovery through the broken state.

### Required change 1 — resolved identity persisted — DONE

Rev 3 §4 step 1 of success flow (`proposals/06-import-replace.md:282-286`)
writes `<state-data-dir>/replace_journal/session-<id>.canonical.jsonl`
**plus** the pending journal entry, fsyncing both files and the
`replace_journal` directory.

Rev 3 §4 pre-mutation step 8 (`:264-265`) freezes the resolved identity:
"`session_id`, `chain_id`, `active_segment_id`, `provider_name`,
`storage_type`, and `jsonl_path`."

Rev 3 §4 journal format (`:328-347`) makes those frozen fields explicit
in the on-disk schema:

```
"session_id", "chain_id", "active_segment_id",
"provider_name", "storage_type", "jsonl_path",
"preimage_sha256", "postimage_sha256",
"canonical_records_path", "db_state_pending",
"expected_turn_count"
```

§4 closing paragraph (`:352-358`) makes the no-stale-rediscovery contract
explicit: "recovery must not re-read the postimage transcript and infer
DB rows from provider-rendered bytes." `canonical_records_path` is the
recovery source of truth; the journal carries identity, not just hashes.

§6 startup recovery contract (`:420-446`) reads identity from the
journal: "extract resolved identity (`chain_id`, `active_segment_id`,
`provider_name`, `storage_type`, `jsonl_path`), hashes, and
`canonical_records_path`." Step 4 (`:432-437`) re-applies DB updates
"idempotently from `canonical_records_path`: replace `session_turns` rows
for `(provider_name, session_id)` and update **the frozen active
segment's** `last_used_at` / `last_turn_id`" — i.e., the recovery routine
does not re-resolve through `StateDb::resolve_resume` and cannot be
fooled by stale resolver-visible rows.

§8 side-effect contract (`:567-586`) repeats the journal field set and
binds the `replace_journal/quarantine/` directory to ambiguous-hash
recovery. §13 row "Durable journal closes post-rename/pre-DB crash
recovery" (`:779`) records YES with the new "resolved identity and
`canonical_records_path`" addendum.

The audit-required identity payload is met:

| Audit-required field | Rev 3 location |
|---|---|
| `provider_name` | journal `:339`; recovery `:425`; success-flow `:264-265` |
| `storage_type` | journal `:340`; recovery `:425` |
| `chain_id` | journal `:336`; recovery `:425` |
| active `segment_id` | journal `:337` (`active_segment_id`); recovery `:425, :436` |
| `session_id` | journal `:335`; receipt `:472`; recovery `:424` |
| canonical `jsonl_path` | journal `:341`; recovery `:425` |
| preimage/postimage hashes | journal `:342-343`; recovery `:432-440` |
| canonical postimage material | `canonical_records_path` JSONL file (`:282-286`, `:344`); used as recovery DB source `:435-437` |

### Required change 2 — verification before deletion — DONE

Rev 3 §4 success flow reorders the durable cleanup so deletion is the
**last** step, and is gated on **two** verifications. Concretely
(`:280-321`):

- Step 6: begin SQLite transaction with new rows from
  `canonical_records_path`. **Do not commit.**
- Step 7: compute `postimage_sha256` from the renamed transcript.
  Mismatch → "roll back the SQLite transaction, exit `1
  operational-error`, and **leave the journal plus canonical records
  file in place for operator inspection**."
- Step 8: fresh export verification — parse new transcript through
  canonical reader, compare bytes to `canonical_records_path`. Mismatch
  → "roll back the SQLite transaction, exit `1 operational-error` with
  a specific fresh-export verification error, and **leave the journal
  plus canonical records file in place**."
- Step 9: only after step 8 succeeds, commit SQLite.
- Step 10: delete journal entry and canonical records file (idempotent
  unlink), fsync `replace_journal/`. "This is the last durable cleanup
  step."

§4 closing paragraph (`:322-326`) reinforces the contract: "Any failure
in success-flow steps 3-9 leaves the journal plus canonical records file
in place; that journal is the recovery signal."

§8 side-effect contract (`:573-580`) repeats the no-delete-before-verify
ordering: "Failures in the protected flow after lock acquisition and
before SQLite commit, including under-lock preimage mismatch, postimage
hash mismatch, and fresh export verification mismatch, leave the journal
entry and canonical records file in place. After postimage verification,
fresh export verification, and SQLite commit all succeed, import-replace
deletes the journal entry and canonical records file and fsyncs the
`replace_journal` directory before emitting the receipt."

§13 row "Durable journal closes post-rename/pre-DB crash recovery"
(`:779`) explicitly cites "deletion happens only after verification plus
DB commit." This is the audit-required ordering, not paraphrased.

### Required change 3 — recovery test using journal identity — DONE

Rev 3 §9.1 adds four new T-rows that pin the post-rename recovery
contract end-to-end (`:644-650`):

- **T-recovery-rename-only** (`:645`): "Kill process between rename and
  DB commit; restart recovers derived state from the journal-attached
  canonical records file." Fixture seeds **stale `session_turns`**;
  startup scan must find postimage hash, replace `session_turns` from
  `canonical_records_path`, refresh the **frozen** segment, then delete
  journal+canonical. This is the audit-required "stale or ambiguous
  resolver-visible DB rows after rename" test.
- **T-recovery-ambiguous-hash** (`:648`): manual transcript corruption
  → quarantine.
- **T-recovery-canonical-records-preserved** (`:649`): post-crash
  canonical file is byte-for-byte equal to normalized input — i.e., the
  recovery source of truth survives the crash.
- **T-no-deletion-before-verify** (`:650`): postimage hash mismatch
  after rename leaves both journal and canonical_records_path; SQLite
  is not committed.

Combined with the existing "Journal post-rename recovery" (`:644`) and
"Journal pre-rename recovery" (`:646`) and "Journal ambiguous recovery"
(`:647`), the recovery contract has injection points and expected steady
states for **every** crash state listed in §8 (`:587-614`). T-rows
explicitly assert that the **frozen segment** is updated, not the
currently-resolved one — pinning the no-rediscovery contract from §4.

Closure verdict: **AIR-R2-F01 CLOSED**. All three audit-required
proposal changes are present, located in §4 / §6 / §8 / §9 / §13, and
each is bound to a typed exit, a specific journal field, or a specific
T-row.

## Round 1 closures still standing

Rev 3 deltas focus on §4 / §6 / §8 / §9 / §13 to expand the journal
schema and reorder cleanup. The §3 renderer surface, §7 DB-update field
set, and the F03 lock-retrofit prose are unchanged.

### AIR-R1-F01 (renderer / canonical-bytes-on-disk) — STILL CLOSED

§3 (`:209-238`) keeps the renderer contract: provider-native bytes on
disk, lossy classes refused with `15 invalid-input-transcript`, anti-
scope listed (multi-modal, tool-use), `Other → UnsupportedStorage`. §6
(`:393-418`) keeps `CanonicalToProviderRenderer` as the typed module.
§13 rows "Provider transcript file receives provider-native bytes, not
canonical bytes" and "Lossy canonical-to-provider re-encoding is
refused" still record YES (`:776-777`). §9.1 "Postimage round-trip" row
still pins "Export hash equals receipt postimage_sha256 even though
on-disk bytes are provider-native" (`:654`).

The Rev 3 success flow reinforces the renderer contract: the DB update
in step 6 (`:303-304`) reads from `canonical_records_path` (canonical
bytes) and the fresh export verification in step 8 (`:310-314`) is the
round-trip oracle that catches a renderer that produces non-round-
trippable native bytes. The renderer-vs-disk contract is now under
**two** orthogonal verifications (postimage hash + fresh export) before
commit.

### AIR-R1-F02 (post-rename/pre-DB-commit recovery) — STRENGTHENED

Rev 2 closed F02 with the journal mechanism; Rev 3 strengthens the
closure by:

- Persisting frozen identity instead of forcing recovery through stale
  resolver state (`:328-347`).
- Adding `canonical_records_path` as a separate file so recovery rebuilds
  rows without re-reading the (provider-native) transcript through a
  parser (`:344, :352-358, :432-437`).
- Moving deletion to after both verifications and commit (`:280-321`).
- Pinning four new recovery T-rows (`:644-650`).

§8 crash states #1–#8 (`:587-614`) cover every single-instance crash
window:

| Crash state | Recovery action |
|---|---|
| #1 before temp write | no durable mutation |
| #2 after temp before rename | stale temp cleanup (prefix-scoped) |
| #3 after fsync before rename | same as #2 |
| #4 after rename before DB | postimage match → re-apply DB from `canonical_records_path`, refresh frozen segment, delete artifacts |
| #5 during DB transaction | same as #4 (idempotent re-apply) |
| #6 after DB commit before journal deletion | same as #4 (idempotent re-apply) |
| #7 preimage match | rename never landed → delete journal+canonical |
| #8 neither hash matches | quarantine journal, preserve `canonical_records_path` for inspection |

Each crash state binds to a deterministic action with a specific T-row.
No silent drift, no rebuild-from-stale-state. **CLOSED, strengthened.**

### AIR-R1-F03 (lock observation / cooperative-surface scope) — STILL CLOSED

§13 row "Lock observation for import-replace once pause-handshake
lands" (`:772-773`) still cites 06-pause-handshake's PR #17 as the
lock-primitive dependency and still records the writer-path retrofit
(`run_repl`, `run_resume`, balanced one-shot, `migrate_chain_segment`)
as a sibling-PR concern with explicit "advisory until full retrofit
lands" framing for v1 harness consumers. §12 residual #3 (`:752-753`)
still names the cooperative-surface limit as a documented residual.
Rev 3 did not touch this surface; the closure carries forward.

### AIR-R1-F04 (canonical-record field-loss model) — STILL CLOSED

§6 (`:407-409`) still declares the loss explicitly: "Fields not present
in `CanonicalRecord` (`parent_turn_id`, `is_sidechain`,
`is_compaction_boundary`) are intentionally written as `NULL` or schema
defaults in `session_turns`."

§7 step 4 (`:506-510`) still warns consumers: "downstream features such
as resume and trace should not rely on these fields after a replace."
§7 (`:536-538`) still names canonical-schema extension as the future
fix point.

§9.1 row "DB metadata loss is explicit" (`:652`) still pins the test
expectation. §12 residual (`:756-758`) and §13 row "State consistency
covers required rows" (`:780`) preserve the documented loss.

The W5 watchpoint about asserted-not-verified downstream NULL tolerance
persists (re-confirmed against current branch below).

## Race-freeness for the documented threat model

The proposal's threat model is single-instance crash recovery: the
journal is "the durable recovery signal" if the process exits between
file rename and journal deletion (§8 `:585-586`). Concurrent-invocation
races are a known Round 2 watchpoint (W4) that the proposal does not
explicitly include in its threat model.

### Single-instance crash recovery — RACE-FREE

The reordered success flow (`:280-321`) executes durable side effects
in this order:

```
T0  write canonical_records_path, fsync
T1  write journal pending entry, fsync file + dir
T2  acquire SessionLock
T3  read existing transcript, compute under-lock preimage
T4  (optional) rewrite + fsync journal if preimage shifted
T5  write provider-native temp file, fsync
T6  atomic rename to jsonl_path, fsync parent dir
T7  begin SQLite transaction (uncommitted)
T8  compute postimage_sha256 from renamed transcript
T9  verify postimage matches journal; mismatch → rollback + exit + leave artifacts
T10 fresh export round-trip vs canonical_records_path; mismatch → rollback + exit + leave artifacts
T11 commit SQLite
T12 unlink journal + canonical_records_path, fsync replace_journal dir
T13 release lock; emit receipt
```

For a single instance, every crash boundary maps deterministically:

- **Crash before T1**: nothing on disk; no recovery needed.
- **Crash T1..T5**: journal+canonical exist; transcript hash = preimage
  → §8 #7 path, recovery deletes artifacts.
- **Crash T6..T11**: journal+canonical exist; transcript hash =
  postimage → §8 #4–#6 paths, recovery uses **frozen identity** from
  the journal and **canonical bytes** from `canonical_records_path` to
  re-apply the DB transaction idempotently, refresh the frozen segment,
  then unlink artifacts. SQLite's transactional commit guarantees that
  T11 is all-or-nothing, so #5 (mid-transaction) reduces to either #4
  or #6 on disk.
- **Crash after T12**: clean state.

Every action between T0 and T12 is either fsynced or transactional, so
under fail-stop semantics the on-disk state always matches one of the
documented hash states. The new T-no-deletion-before-verify (`:650`)
specifically tests that T9/T10 mismatches do **not** trigger T12 — i.e.,
the ordering is bound by a test, not just by prose.

The recovery routine never calls `StateDb::resolve_resume` to rediscover
the segment. It reads `chain_id` and `active_segment_id` from the
journal, replays canonical records into `session_turns` via the same DB
helper used in the forward path, and updates the frozen segment's
`last_turn_id` / `last_used_at`. This is the audit's "rebuild without
relying on stale resolver output" requirement.

**Verdict: race-free for the documented single-instance crash model.**

### Concurrent-invocation race — W4 PERSISTS (not a shortcut)

Rev 3 step 1 (`:282-286`) still writes `session-<X>.pending` and
`session-<X>.canonical.jsonl` on a single per-session path **before**
step 2's lock acquisition. Step 2 (`:287-291`) still says "this handled
busy failure **may** unlink the journal and canonical records file
idempotently before exit."

The W4 race (Rev 2) carries forward in Rev 3, with the canonical_records
file added as a second collision point:

1. Instance A writes journal + canonical at T0 (per-session paths).
2. Instance B writes journal + canonical at T0+ε (overwrites A's
   content; OS write-order semantics determine final bytes per file).
3. A acquires lock at T1; B is busy at T1+ε.
4. B's busy-cleanup unlinks journal + canonical at T2.
5. A continues under the lock. Step 6 writes a temp from in-memory
   canonical records (A's content). Step 7 renames → A's transcript on
   disk.
6. Step 7 (DB transaction) **reads `canonical_records_path` from disk**
   (`:303-304`) — file is gone. SQLite read returns no rows / I/O error.
   Step 8 postimage check is moot if rollback already triggered.
7. A exits with `1 operational-error`. Transcript = A's bytes (already
   renamed). DB = unchanged. Journal = gone. **Recovery has no signal.**

§4's "may unlink" is permissive language that allows but does not
mandate this behavior. A Phase 6 implementer who chooses to skip the
unlink leaves orphan journal/canonical pairs from racing-but-busy
instances; a Phase 6 implementer who chooses to unlink reopens F02 for
the concurrent case.

Two clean fixes remain available (same as W4 in Rev 2):

- **Fix A (reorder)**: acquire `SessionLock` first; only the lock holder
  writes / rewrites / deletes the journal + canonical_records_path. The
  busy path never touches durable state because it never wrote any.
- **Fix B (per-attempt name)**:
  `session-<X>.<attempt-uuid>.pending` and `session-<X>.<attempt-uuid>.canonical.jsonl`
  so each attempt owns its own pair. Busy path deletes only its own
  attempt; recovery scans the `session-<X>.*.pending` prefix and
  resolves precedence by `started_at`.

Why this is not a shortcut finding:

- Rev 3 prose does not silently mask the race; it under-specifies an
  ordering/naming detail.
- The lock-busy path is documented and typed (`13 session-busy`).
- The proposal does not commit to a single-name-per-session contract
  that would make Fix B a breaking change.
- The threat model in §8 is single-instance fail-stop.

W4 thus persists at the Rev 2 severity tier (audit-/scope-track for
Phase 6 specification precision), not as a shortcut violation.

## Watchpoints carried forward from Round 1 / Round 2

### W3 (Rev 1) — Codex two-track in §9.1 — STILL NARROWED-BUT-PERSISTS

A5 (`:101`) still declares `claude_code` and `codex_session` supported.
§3 renderer (`:226`) still names `codex_session` as a first-class
storage type. §6 receipt enum (`:475`) still names `codex_session`. But
§9.1 last row (`:654`) still hedges: "If Codex renderer deferred, Codex
test becomes explicit unsupported-storage test."

Not a shortcut because the deferral fallback is typed (exit `12`), but
audit-track should pin which fork is binding before Phase 6 begins.
Unchanged from Rev 2.

### W4 (Rev 2) — Concurrent-invocation journal race — PERSISTS

Detailed above. Rev 3 added a second collision file
(`canonical_records_path`) on the same per-session path, which expands
the W4 surface but does not change its severity tier. Phase 6 must
resolve via reorder (Fix A) or per-attempt naming (Fix B).

### W5 (Rev 2) — Downstream NULL-tolerance asserted, not verified — PERSISTS

§7 step 4 (`:510`) still asserts "downstream features such as resume
and trace should not rely on these fields after a replace." Re-verified
against the current branch in this worktree:

- `latest_compaction_boundary` (`src-tauri/src/state/db.rs:2510-2536`)
  filters `WHERE is_compaction_boundary = 1`. Post-replace rows default
  to `0`/NULL → resume's compaction-boundary handling regresses for any
  session whose transcript was replaced.
- `parent_turn_id` and `is_sidechain` are referenced at
  `src-tauri/src/state/db.rs:109-125, :877` and in `balancer/mod.rs`,
  `sessions/mod.rs`, `trace/mod.rs`. Per-consumer NULL-tolerance is not
  enumerated.

Same disposition as Rev 2: not a shortcut because the loss model is
explicit and tested, but the contract claim ("downstream should not
rely") is currently false for `latest_compaction_boundary`. Phase 5
hookpoints or Phase 6 implementation should either (a) harden consumer
paths, (b) extend canonical schema to carry the three fields before
import-replace ships, or (c) narrow §7 step 4's prose to enumerate which
downstream behaviors are accepted to regress in v1.

## LOW-severity nits

### N1 — Stale-temp cleanup scope (carries from Rev 1)

§4 step 10 (`:270-272`) is unchanged: "Clean stale import-replace temp
files in the target transcript directory whose names match this
feature's temp-file convention and are not currently locked by another
live replace operation." Per N1 in Rev 2, Claude project directories
and Codex session directories host multiple sessions per directory, so
cleanup must filter by `<jsonl_path>` prefix, not just by feature
suffix. Phase 6 should specify the predicate.

### N2 — `source_file` conditional (carries from Rev 1)

§7 step 5 (`:511-513`) is unchanged: "Set `source_file` to the replaced
`jsonl_path` when the current schema/helper supports it; otherwise keep
existing ingest helper behavior if the column is not meaningful in this
branch." Phase 5 hookpoints should declare which branch state is
binding.

### N3 — Quarantine-marker shape (carries from Rev 2)

§6 step 6 (`:441-446`) and §8 crash state #8 (`:608-614`) still
instruct recovery to move the journal to `replace_journal/quarantine/`.
Rev 3 names the directory but does not pin the per-file shape inside
it (e.g., whether the original `session-<id>.pending` filename is
preserved, suffixed with timestamp, or renamed to `.quarantined`).
Combined with §6 / §12 #2's anti-scope on a manual-recovery CLI, Phase
6 should pin the shape and exclude it from the on-startup scan filter.

### N4 — `canonical_records_path` lifetime in quarantine (new in Rev 3)

§8 crash state #8 (`:608-614`) and §6 step 6 (`:441-446`) say recovery
"preserves the canonical records file for inspection" / "leave the
canonical records file in place" while moving the journal entry to
`quarantine/`. This means quarantined journals (in `quarantine/`) point
at canonical_records files that remain in `replace_journal/` (the
active directory). Phase 6 should clarify whether `canonical_records_path`
is also moved into `quarantine/`, or whether the on-startup scan filter
deliberately ignores `*.canonical.jsonl` files. Otherwise an operator's
manual cleanup of `replace_journal/` could orphan canonical material
from quarantined journals, or recovery could misread an orphan canonical
file. Not a shortcut (quarantine is typed and named); flag for Phase 6
specification precision.

## Per-pattern shortcut audit (Rev 3 deltas focus)

Eight canonical shortcut patterns re-checked against Rev 3's three new
surfaces (expanded journal schema, `canonical_records_path` companion
file, postimage+fresh-export-before-commit ordering). Round 2 PASS
results carry forward unchanged where Rev 3 did not touch the surface.

### 1. Hidden silent fallback

Rev 3 deltas:
- §4 step 7 (`:305-309`): postimage hash mismatch → rollback + exit + leave artifacts. No silent re-render.
- §4 step 8 (`:310-314`): fresh export round-trip mismatch → rollback + exit + leave artifacts. No silent re-encode.
- §6 step 6 (`:441-446`): ambiguous-hash recovery quarantines; does not silently rewrite transcript or DB.
- §6 step 4 (`:432-437`): re-applies DB update **idempotently** from `canonical_records_path` using **frozen** identity; idempotency is explicit, not silent.

PASS.

### 2. Dual-write / compat shim / backward-compat alias

`canonical_records_path` is not a dual-write of state. It is the
canonical-input source-of-truth for both the forward DB transaction
(step 6 `:303-304`) and the recovery DB rebuild (§6 `:435-437`) — the
same file feeds both paths so they cannot diverge. The journal is the
recovery-signal primitive, not a compat shim.

Grep `compat|shim|backward|legacy|transitional|dual-write|alias` over
the Rev 3 proposal returns matches only on `compatibility` (schema-probe
gate) and "schema-compatible JSON" (rejection criteria). PASS.

### 3. Deferred stubs without typed errors

Rev 3 deferrals re-checked against `~/ai/conventions/no-deferred-stubs.md`:

| Deferred surface | Typed error / refusal | Test pin |
|---|---|---|
| `Other` storage rendering | `12 unsupported-storage` (§3, §4 step 7, §5); residual §12 #4 | §9.1 "Unsupported storage" |
| Lossy canonical record classes | `15 invalid-input-transcript` with `unsupported-record-class:<class>` (§3 step 11) | §9.1 "Unsupported record class" |
| Manual recovery CLI | anti-scope explicit (§6 last paragraph, §12 #2); on-startup auto-recovery delivered (§6 startup recovery contract) | §9.1 four "Journal *recovery" + "T-recovery-*" rows |
| Quarantine cleanup | typed quarantine directory `replace_journal/quarantine/` (§6 step 6, §8 crash #8); no silent self-heal | §9.1 "Journal ambiguous recovery" + "T-recovery-ambiguous-hash" |
| Codex renderer (if Phase 6 finds blockers) | `12 unsupported-storage` fork (§9.1 Postimage round-trip residual) | covered |
| Canonical-schema extension for absent fields | NULL/default writes (§6, §7 step 4); residual §12; §13 row | §9.1 "DB metadata loss is explicit" |
| In-binary writer-path lock observation | `13 session-busy` for cooperative observers; residual §12 #3; sibling-PR retrofit (§13 `:772-773`) | §9.1 "Lock busy" |

Each Rev 3 deferral has a typed exit and a named follow-up. None
silently returns success. PASS.

### 4. Hardcoded constants / magic numbers

Grep `hardcode|hard-code|magic|placeholder` over Rev 3 returns zero
hits. New literals introduced in Rev 3 — `replace_journal/quarantine/`,
`session-<id>.canonical.jsonl`, `expected_turn_count` — are namespaced
data, not magic. SHA-256 is the harness-named digest. `started_at`,
`schema_version`, `db_state_pending` are journal-schema discriminators.
PASS.

### 5. TODO/FIXME-gated rollout

Grep `TODO|FIXME|for now|in the future|temporary|workaround` over Rev 3
returns matches only on "future" / "later" framings (anti-scope
sentences in §6 / §7 / §13). No new in-mainline TODOs in Rev 3. PASS.

### 6. Symptom-masking heuristic

Rev 3 ordering surfaces:

- §4 step 4 (`:296-299`): rewrite journal under lock if preimage shifted
  — closes the journal/lock TOCTOU rather than trusting the pre-lock
  hash. Inverse of symptom-masking.
- §4 step 7 (`:305-309`): postimage verification before commit — does
  not paper over a non-round-trippable rendering.
- §4 step 8 (`:310-314`): fresh export verification before commit —
  catches renderer drift even when the hash matches.
- §4 step 10 (`:316-318`): journal deletion is the **last** durable
  step. Cleanup never runs ahead of correctness checks.
- §6 step 4 (`:432-437`): recovery uses **frozen** identity from the
  journal, not re-resolution. Closes the rediscovery-through-stale-
  state pattern that AIR-R2-F01 flagged.

PASS.

### 7. Feature-flag rollout

Grep `feature flag` over Rev 3 returns matches only on schema-probe
feature flags consumed as input gates (A1, §9.1). The proposal does not
introduce a new feature flag for itself. PASS.

### 8. Atomicity bypass / sed-style rewrite

Rev 3 atomicity surfaces:

- §4 D2 (`:248-250`) still commits to "two-phase replace with same-
  directory temp file, fsync, atomic rename, and a durable replace
  journal."
- §8 fsync ordering (`:621-626`) preserved: temp fsync, rename, parent
  dir fsync.
- §4 steps 1, 4, 10 (`:282-286, :296-299, :316-318`) wrap the journal
  in fsync ordering: write-fsync-pre, rewrite-fsync-during,
  delete-fsync-post.
- §4 step 1 introduces an additional fsync on `canonical_records_path`
  before journal write — strengthens, does not weaken, atomicity.
- No in-place edit, no `sed`-style byte rewrite, no append-only
  amendment.

The W4 watchpoint (concurrent-invocation race on per-session journal
+ canonical paths with busy-cleanup deletion) is a Phase 6 ordering/
naming defect, not a deliberate atomicity bypass. PASS in mainline;
W4 carries forward.

## Per-pattern grep summary (Rev 3)

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
| `journal` | many | Durable journal mechanism; fsync ordering and recovery contract; new `canonical_records_path` companion file. |
| `frozen\|freeze` | several | Resolved identity is frozen at §4 step 8 and used by recovery; no rediscovery. |
| `verification\|verify` | several | Postimage hash verification + fresh export verification before journal deletion. |
| `idempotent` | 2 | Recovery DB re-application; busy-cleanup unlink. |
| `quarantine` | several | Ambiguous-hash recovery directory and crash state #8. |

## Patterns followed correctly (Rev 3)

- **Hard refusal of provider-native input** preserved (§1, §3, §10, §13).
- **Provider-native bytes on disk** (§3, §6, §13): renderer is the dual
  of the export parser with a round-trip oracle.
- **Lossy renderer refusal** (§3, §9.1): typed `15
  invalid-input-transcript` with named error-code shape.
- **Two-phase atomic rename + fsync + parent-dir fsync** (§4 D2, §8):
  unchanged.
- **Durable pending-op journal with frozen identity and canonical
  postimage material** (§4, §6, §8): closes Rev 2's AIR-R2-F01 for the
  single-instance crash case. Recovery is deterministic across all five
  hash-state cases without consulting `StateDb::resolve_resume`.
- **No-delete-before-verify ordering** (§4 steps 7–10, §8 side-effect
  contract): journal deletion is the last durable step and is gated on
  postimage hash match + fresh export round-trip + SQLite commit.
- **Idempotent recovery DB re-application** from `canonical_records_path`
  using frozen segment identity (§6 step 4, §9.1 T-recovery-rename-only).
- **Double preimage check across the lock boundary** (§4 step 11
  preflight + step 3 under-lock): unchanged.
- **Typed exit-codes mirroring the harness namespace** (§5, §13):
  `10`–`15` mapped, no proceed-anyway path.
- **Explicit named residuals in §12**: each enumerated with a recovery
  story rather than masked.
- **Codex two-track via typed exit `12`** (§9.1 last row): preserved
  (W3 narrows but persists).
- **No second ownership path** (§13 row, A2): preserved.
- **No second lock format** (§4 D1, §8): preserved; sibling-PR retrofit
  for in-binary writers named.
- **Receipt as the durable observability surface** (§6, §11):
  preserved; lost-receipt recovery via export+hash documented.
- **Documented canonical-field loss** (§6, §7, §9, §12, §13):
  preserved; W5 unchanged.

## Specific shortcut traps (re-validated against Rev 3)

- **Migration-style temp without fsync.** §8 (`:621-626`) preserves
  fsync ordering. PASS.
- **Migration-style temp filename collision.** Temp uses
  `<jsonl_path>.tmp-import-replace-<uuid>` (§8 `:556-558`). PASS.
- **Per-session journal filename collision.** Rev 3 keeps
  `session-<id>.pending` and adds `session-<id>.canonical.jsonl`.
  Both are single-name-per-session. Under concurrent invocation this
  races with §4 step 2's busy-cleanup unlink. See **W4**. Not a hidden
  shortcut. PASS-with-watchpoint.
- **Running invocation as session-busy lock.** A6 and §12 #3 refuse;
  supported signal is `SessionLock`. Sibling-PR retrofit named (§13
  `:772-773`). PASS.
- **Preimage over DB summary rows.** A4 explicitly hashes the canonical
  export byte stream. §4 closing paragraph (`:349-352`) reaffirms "Its
  hashes are canonical export hashes for recovery comparison, not raw
  provider-native file-byte hashes." PASS.
- **`session_turns` reconstruction from canonical input.** §7 step 1–3
  preserves canonical fields; step 4 NULLs absent canonical fields;
  recovery (§6 step 4) uses the **same** canonical input file the
  forward path used (`canonical_records_path`), not a re-derivation
  from provider-native disk bytes. PASS.
- **Auto-resume after replace.** §1, §11, §13 refuse. PASS.
- **Auto-`migrate-db` after replace.** §11, §12, §13 refuse. PASS.
- **Cross-provider migration coupling.** §11 keeps `migration::
  migrate_chain_segment` UNCOUPLED. PASS.
- **Renderer round-trip silence.** §3 (`:227-228`) and §9.1
  "Postimage round-trip" (`:654`) bind the renderer to a round-trip-
  through-export oracle. Step 8 (`:310-314`) elevates the oracle from
  test-only to a runtime gate before commit. PASS.
- **Quarantine self-heal.** §6 step 6 (`:441-446`) quarantines on
  ambiguous hash; does not auto-rewrite. Anti-scope on manual recovery
  CLI is explicit (§12 #2 `:749-751`). PASS-with-nits (N3 marker shape,
  N4 canonical_records_path lifetime).
- **Field-loss silent reconstruction.** §6 / §7 / §9 / §12 / §13
  enumerate the loss; §9.1 "DB metadata loss is explicit" pins the
  expected NULL/default state. PASS-with-watchpoint (W5).
- **No-delete-before-verify (new in Rev 3).** §4 steps 7–10
  (`:305-318`) and §8 (`:573-580`) place journal deletion strictly
  after postimage hash verification, fresh export round-trip
  verification, and SQLite commit. §9.1 T-no-deletion-before-verify
  (`:650`) tests the ordering. PASS.
- **Recovery via stale resolver state (new in Rev 3).** §4 closing
  paragraph (`:352-358`), §6 step 4 (`:432-437`), and §9.1
  T-recovery-rename-only (`:645`) bind recovery to **frozen** identity
  from the journal, not to current `StateDb::resolve_resume` output.
  PASS.

## Conclusion

Verdict: **LOW**.

Rev 3 closes Round 2's AIR-R2-F01 cleanly. The audit's three required
proposal changes — persisted resolved identity, verification-before-
deletion ordering, and a recovery test that uses journal identity
against stale DB state — each map to a specific journal field, success-
flow step ordering, and §9.1 T-row. The expanded journal + reordered
flow is race-free for the documented single-instance crash threat
model: every crash boundary T0..T13 maps to one of §8's eight
deterministic recovery actions, recovery operates on **frozen** identity
and **canonical** bytes from `canonical_records_path` rather than
re-resolution, and journal deletion is gated on two orthogonal post-
rename verifications plus SQLite commit.

Round 1 closures (AIR-R1-F01..F04) carry forward; the renderer contract,
cooperative-lock retrofit citation, and explicit field-loss model are
unchanged. Eight canonical shortcut patterns and fourteen specific
shortcut traps pass. Two new traps (no-delete-before-verify, recovery-
via-stale-resolver-state) are added by Rev 3 and pass.

Three watchpoints persist for audit-/scope-track:

- **W3** — A5 / §6 receipt enum commit to `codex_session`; §9.1
  "Postimage round-trip" residual still hedges. Audit-track should pin
  which fork is binding before Phase 6.
- **W4** — Concurrent-invocation race on the single per-session journal
  + canonical_records_path pair with permissive busy-cleanup unlink.
  Phase 6 must resolve via reorder (acquire-lock-then-write-journal) or
  per-attempt naming. Rev 3 expanded the surface (added
  canonical_records_path) but did not change the ordering or naming.
- **W5** — Asserted downstream NULL-tolerance is false for
  `latest_compaction_boundary` (`src-tauri/src/state/db.rs:2510-2536`).
  Phase 5/Phase 6 should harden consumers, extend the canonical schema,
  or narrow §7 step 4's prose.

Four LOW-severity nits (N1 stale-temp cleanup scope, N2 `source_file`
conditional, N3 quarantine-marker shape, N4 canonical_records_path
lifetime in quarantine) are filed for Phase 6 specification precision.
N1 and N2 carry from Round 1; N3 carries from Round 2; N4 is new in
Rev 3.

No regression from Round 2: every Rev 2 protection (atomic rename,
double preimage check, typed exit namespace, no second ownership path,
no second lock format, durable journal, receipt-as-observability,
field-loss documented) is preserved or strengthened. No new finding
rises to MEDIUM or HIGH.
