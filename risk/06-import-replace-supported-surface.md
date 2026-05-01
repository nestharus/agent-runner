# 06-import-replace — Phase 4 Supported-Surface Risk Report (Rev 3)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

This is the Round 3 supported-surface review of `proposals/06-import-replace.md`
Rev 3. Round 1 verdict was LOW with two non-terminal findings; the audit
track was HIGH (AIR-R1-F01..F04). Rev 2 closed AIR-R1-F01, F03, F04 and was
re-reviewed at Round 2 supported-surface as LOW with five carryover findings
(R2-F01..R2-F05). Round 2 audit re-opened HIGH on AIR-R2-F01 (journal
recovery underspecified and cleared too early). Rev 3's announced scope is to
close AIR-R2-F01 by (a) expanding the journal schema with frozen resolved
identity plus `canonical_records_path`, (b) reordering the success flow so
journal deletion is the last step (after postimage_sha256 verification, fresh
export verification, and SQLite commit), (c) specifying recovery for both
postimage-matching and preimage-matching cases plus quarantining the ambiguous
case, and (d) adding four T-rows for recovery scenarios. This review confirms
that closure from the supported-surface lens, runs the no-regression check on
adjacent paths and cohorts under the documented threat model, registers the
five Round 2 prose carryovers (still untouched in Rev 3), and adds one new
non-terminal finding tied to the Rev 3 ordering of journal write versus lock
acquisition. Net value remains positive on the supported surface; no
termination signal fires.

The originally referenced `risk/06-import-replace-audit-history.md` is not
present at HEAD; the Round 1 history exists at git commit `4a598ac` and is
treated as authoritative for prior rounds.

## Concern 1 — Closure of AIR-R2-F01 from the supported-surface lens

This concern is closure-only audit on the single Round 2 audit finding. It
asks whether the closure is real on the public-CLI surface and whether it
introduces an unbounded blast-radius item.

### AIR-R2-F01 — journal recovery underspecified and cleared too early — CLOSED

Round 2 audit had three required changes:

1. Persist the resolved recovery identity in the journal before transcript
   mutation, with enough canonical postimage material or parser metadata to
   rebuild `session_turns` without relying on stale resolver output.
2. Move fresh postimage export verification before journal deletion, or state
   that any post-DB verification failure leaves/quarantines the journal
   instead of deleting it.
3. Add a recovery test that simulates stale or ambiguous resolver-visible DB
   rows after rename and proves startup recovery uses journal identity rather
   than rediscovery through the broken state.

Rev 3 delivers all three.

**Frozen resolved identity in the journal.** §4 step 8 now says: "Freeze the
resolved identity for the operation: `session_id`, `chain_id`,
`active_segment_id`, `provider_name`, `storage_type`, and `jsonl_path`." The
journal format (§4 lines 330-347) records `session_id`, `chain_id`,
`active_segment_id`, `provider_name`, `storage_type`, `jsonl_path`,
`preimage_sha256`, `postimage_sha256`, `canonical_records_path`,
`db_state_pending`, and `expected_turn_count`. The §4 closing prose makes the
authority explicit: "`chain_id`, `active_segment_id`, `provider_name`, and
`storage_type` are resolved before transcript mutation and frozen in the
journal. The `canonical_records_path` file is written before the transcript
rename and is the recovery source of truth for rebuilding `session_turns`;
recovery must not re-read the postimage transcript and infer DB rows from
provider-rendered bytes."

**Canonical postimage material persisted before mutation.** §4 step 1 now
requires the validator to write normalized canonical records to
`<state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl`,
fsync that file plus the journal plus the `replace_journal` directory, all
before lock acquisition or transcript mutation. §6 startup recovery step 4
explicitly says recovery rebuilds `session_turns` rows from
`canonical_records_path` rather than re-parsing the post-rename provider
transcript and inferring rows from native bytes. This is exactly the
"persist enough canonical postimage material" requirement Round 2 asked for.

**Verification-before-deletion ordering.** Rev 2 deleted the journal
immediately after the SQLite transaction committed and then computed
`postimage_sha256` against the committed transcript. Rev 3 §4 success flow
inverts the dependency:

| Rev 3 success-flow step | Verification gate |
| --- | --- |
| 6 | Begin SQLite transaction; replace `session_turns` from `canonical_records_path`; do not commit. |
| 7 | Compute `postimage_sha256` from new transcript through canonical reader; compare to journal's recorded hash. Mismatch → SQLite rollback, exit `1`, journal + canonical records preserved. |
| 8 | Run fresh export verification (parse new transcript, compare bytes to `canonical_records_path`). Mismatch → SQLite rollback, exit `1`, journal + canonical records preserved. |
| 9 | Commit SQLite transaction. |
| 10 | Idempotent unlink of journal and canonical records file; fsync `replace_journal` directory. |

This satisfies "verification before deletion" and the "leaves/quarantines on
failure" alternative. The §4 closing prose is now consistent with §6 / §8
quarantine: "Any failure in success-flow steps 3-9 leaves the journal plus
canonical records file in place; that journal is the recovery signal."

**T-rows for recovery scenarios.** §9.1 adds four explicit T-rows that match
the Round 2 required test:

- `T-recovery-rename-only` — kill between rename and DB commit; restart
  recovers `session_turns` from `canonical_records_path` against frozen
  segment identity, not via fresh resolver output. This is the Round 2
  required test almost verbatim.
- `T-recovery-ambiguous-hash` — corrupt transcript so it matches neither
  hash; recovery quarantines the journal, preserves the canonical records
  file, leaves transcript and DB untouched.
- `T-recovery-canonical-records-preserved` — canonical records file survives
  crash byte-for-byte equal to normalized input.
- `T-no-deletion-before-verify` — postimage hash mismatch after rename
  exits operationally without deleting recovery artifacts; SQLite is not
  committed.

Supported-surface effects of the closure:

- Receipt JSON shape is unchanged from Rev 2 (`session_id`, `provider_name`,
  `storage_type`, `operation`, `preimage_sha256`, `postimage_sha256`,
  `jsonl_path`, `state_updated`, `committed_at`); cohort A parsers do not need
  to update.
- Two new private filesystem surfaces are introduced under the existing
  `<state-data-dir>/replace_journal/`: the per-session canonical records file
  (`session-<id>.canonical.jsonl`) and the quarantine subdirectory
  (`replace_journal/quarantine/`). Both are documented as private
  implementation state in §4 / §8 / §11.1; no public CLI flag exposes them.
  Cohort A / B do not gain or need to read these paths.
- Quarantine semantics on hash-mismatch (§6 step 6, §8 crash state 8) is a
  fail-safe leave-alone: journal moved aside, canonical records preserved
  for inspection, transcript and DB left untouched. Cohort A still has no
  silent-overwrite risk on ambiguous state.
- The recovery contract now explicitly preserves DB-recovery determinism
  even when resolver-visible DB rows are stale (the AIR-R2-F01 specific
  failure mode), because identity comes from the journal, not from a
  re-resolution that would itself depend on the rows recovery is meant to
  fix.

Closure verdict: **real and complete on the supported surface.** The audit
HIGH retires; no new unbounded blast-radius item is introduced. The two new
private filesystem surfaces (`canonical_records_path`, quarantine directory)
are scoped, fsynced, and documented as private. One new bounded ordering
concern (journal write happens before lock acquire) is captured under R3-F01.

## Concern 2 — Closures of AIR-R1-F01..F04 still standing

Round 2 supported-surface review confirmed all four R1 closures real and
complete on the supported surface. Rev 3 changes are restricted to the
journal/recovery contract (§4, §6, §8, §9) plus the §1 "Rev 3 changes" log;
no Rev 3 change touches the rendering contract, the lock contract, the
field-loss contract, or the cooperative-lock prose. Each prior closure is
re-verified below.

### AIR-R1-F01 — provider-native rendering — STILL CLOSED

§3 renderer contract is unchanged in Rev 3 (`CanonicalToProviderRenderer`,
`claude_code` and `codex_session` implementations, `UnsupportedStorage` for
`other`, `15 invalid-input-transcript` with
`unsupported-record-class:<class>` for lossy classes,
round-trip-through-export requirement). §1 still states: "The replacement
transcript file does not store canonical JSONL in v1. It stores
provider-native bytes rendered from canonical input for the resolved storage
type." §13 compliance row "Provider transcript file receives provider-native
bytes, not canonical bytes" is unchanged. Round 2 supported-surface evidence
holds.

### AIR-R1-F02 — durable journal recovery — STILL CLOSED (and tightened)

Rev 3 expansion of the journal and the verification-before-deletion ordering
are themselves the AIR-R2-F01 closure. The R1-F02 mechanism (durable
journal, startup recovery, deterministic reconciliation) is preserved and
strengthened, not retracted. Crash states 4–8 in §8 now branch through the
expanded recovery contract; the post-rename/pre-DB window is closed and the
post-DB/pre-deletion window is also covered (DB rollback + leave artifacts
on verification failure). Round 2 supported-surface evidence holds; the
closure is now stronger, not weaker.

### AIR-R1-F03 — cooperative-lock contract — STILL CLOSED at §13

§13 row "Lock observation for import-replace once pause-handshake lands"
prose is unchanged in Rev 3: "v1 import-replace observes locks; concurrent
runner writers observe per their own retrofit timeline. The harness consumer
of v1 should treat `session-busy` as advisory until full retrofit lands."
Round 2 noted that §12 residual #3 and §11.1 cohort-A prose did not echo
this; Rev 3 also did not update those two locations. The contract is still
unambiguous to a §13 reader (the supported-surface contract source of
truth), so the closure stands; the prose carryover is re-registered as
R3-F03 below (same content as R2-F03).

### AIR-R1-F04 — canonical record field-loss — STILL CLOSED

§6 DB update API, §7 step 4, §12 residual, and §13 compliance row are
unchanged in Rev 3. Replaced sessions still write `parent_turn_id`,
`is_sidechain`, and `is_compaction_boundary` as `NULL` or schema defaults;
§9.1 T-row "DB metadata loss is explicit" is unchanged. The R2-F05 cohort-C
prose gap (§11.1 does not enumerate the partial DEGRADED state) is also
unchanged in Rev 3 and is re-registered as R3-F05 below.

### AIR-R1 closure summary (Rev 3)

| Audit finding | Round 2 status | Rev 3 status | Supported-surface residual |
| --- | --- | --- | --- |
| AIR-R1-F01 (HIGH, native bytes) | Closed | Still closed | Renderer record-class scope (R3-F02). |
| AIR-R1-F02 (HIGH, crash recovery) | Closed | Tightened by AIR-R2-F01 closure | Startup-recovery scope ambiguity (R3-F01b carry of R2-F01). |
| AIR-R1-F03 (MEDIUM, lock observation) | Closed at §13 | Still closed at §13 | §12 / §11.1 prose carryover (R3-F03). |
| AIR-R1-F04 (MEDIUM, field loss) | Closed | Still closed | Cohort-C prose gap (R3-F05). |
| AIR-R2-F01 (HIGH, journal underspecified / cleared early) | n/a (Round 2) | Closed | New ordering note: journal write before lock (R3-F01a). |

All five closures are real and bounded on the supported surface. No
termination signal fires from the closure check.

## Concern 3 — Race-free check on the Rev 3 expanded journal + reordered flow

The Round 3 obligation specifically asks whether the Rev 3 expanded journal
and reordered flow are race-free for the documented threat model. The
documented threat model (R1-F03 / §13 / §11.1) is:

- Cooperative-lock surface keyed by `SessionLock` for the resolved active
  provider session id.
- v1 in-binary writer paths (`run_repl`, `run_resume`, balanced one-shot,
  `migration::migrate_chain_segment`) retrofit on PR #17's timeline; until
  retrofit, `session-busy` is advisory.
- Cohort A (`agent-harness`) is the primary consumer and is expected to be
  the sole orchestrator of `agents` invocations against any session it is
  actively replacing.

Under that threat model, the Rev 3 sequencing is race-free. The argument is
walked through both the success path and each crash window:

### Success path under documented threat model

1. **Pre-mutation hashing window (steps 1-12 of pre-mutation).** Pre-image
   hashing occurs both before lock acquire (to fail fast on caller-supplied
   `--preimage-sha256`) and after lock acquire (success-flow step 3, the
   under-lock TOCTOU re-check). Round 1 noted the second check closes the
   stale-preimage gap; Rev 3 preserves it. No new race surface introduced.
2. **Journal write under expected-postimage hash.** §4 success-flow step 1
   computes the expected `postimage_sha256` over the normalized canonical
   input stream and stores it in the journal before mutation. This means
   recovery can detect both "rename landed" (transcript hash equals stored
   postimage) and "rename did not land or rolled back" (transcript hash
   equals stored preimage) without re-parsing intent from on-disk bytes.
3. **Renamed-but-not-committed window.** Steps 5-9 hold the lock. The
   SQLite transaction is begun but uncommitted until both verifications
   pass. A concurrent same-session caller is excluded by `SessionLock`
   under the cooperative threat model; non-cooperating external writers
   are residual per §12.
4. **Post-commit-pre-deletion window.** Step 10 deletes the journal and
   canonical records file only after step 9 commits the SQLite transaction.
   A crash here leaves both the durable post-DB state on disk and the
   journal pointing at a postimage-matching transcript; recovery (§6 step
   4) re-applies the DB update idempotently from `canonical_records_path`
   and deletes the artifacts, leaving identical durable state.
5. **Lock release and receipt.** Steps 11-12 release the lock and emit the
   receipt; the durable contract ends at step 10.

No verification step happens after deletion. No deletion happens before
verification. The verification + deletion chain is monotonic and audit-
visible.

### Crash windows under documented threat model

| Crash point | Recovery action | Determinism guarantee |
| --- | --- | --- |
| Before journal + canonical records write | None needed (no journal exists) | Fine. |
| After journal write, before lock acquire | Recovery sees journal; transcript hash equals preimage; recovery deletes journal + canonical records file | §6 step 5. |
| After lock acquire, before temp write | Same as above; under-lock preimage matches; no transcript mutation | §6 step 5. |
| After temp write before rename | Same as above; rename did not land | §6 step 5. |
| After rename, before SQLite begin | Transcript hash equals postimage; recovery rebuilds `session_turns` from `canonical_records_path` against frozen segment identity, refreshes chain/segment recency, deletes artifacts | §6 step 4 / §8 crash state 4. |
| During SQLite txn (uncommitted) | SQLite rolls back; same as "after rename, before SQLite begin" on next start | §8 crash state 5. |
| After SQLite commit, before deletion | Recovery re-applies idempotently; deletes artifacts | §6 step 4 / §8 crash state 6. |
| After deletion, before lock release | No journal; lock leases expire per `SessionLock`; durable state equals post-commit | Fine. |
| Ambiguous hash (transcript mutated externally between rename and recovery) | Recovery moves journal to quarantine, preserves canonical records, leaves DB untouched | §6 step 6 / §8 crash state 8 / §9.1 `T-recovery-ambiguous-hash`. |
| Postimage hash mismatch under lock (verification step 7 fails) | SQLite rollback; journal + canonical records preserved; exit `1`; next-start recovery either re-applies or quarantines depending on disk hash | §4 step 7 / §9.1 `T-no-deletion-before-verify`. |
| Fresh export verification mismatch under lock (step 8 fails) | Same as above; this is the AIR-R2-F01 specific window now closed | §4 step 8. |

Every crash window has a deterministic recovery rule, and every recovery
rule reads identity from the journal rather than rediscovering it through
DB state that may itself be the artifact recovery is meant to fix.

### Boundary call-out: outside the documented threat model

The Rev 3 ordering writes the journal and canonical records file **before**
acquiring `SessionLock`. Under the documented threat model (single-
orchestrator cohort A, advisory busy), this ordering is race-free. Outside
that threat model — specifically, two concurrent invocations of
`agents session import-replace <same-session-id>` from a non-cooperating
caller without orchestration — the journal-before-lock ordering admits a
narrow race window:

- Process A writes journal + canonical records (fsync), starts lock acquire.
- Process B writes journal + canonical records (fsync), overwriting A's
  pending journal and canonical records at the same per-session paths,
  starts lock acquire.
- Process A wins the lock; B fails with busy and per §4 step 2 "may unlink
  the journal and canonical records file idempotently before exit."
- A continues mutation against in-memory state, but the on-disk journal /
  canonical records file now reflect B's intent, not A's.
- If A then crashes after rename and before deletion, recovery reads B's
  journal hashes and B's canonical records file, applies B's DB rows
  against A's transcript, producing inconsistent state.

This is bounded by the documented threat model: cohort A is the sole
orchestrator, advisory busy is acceptable for v1, and concurrent same-
session attempts are not in the contract. The cohort-A note R2-F03 already
asks for §11.1 to make this orchestrator role explicit. Re-registered as
R3-F01 below; non-terminal because (a) it is outside the documented threat
model, (b) `SessionLock` is the supported cross-process serialization
primitive, and (c) the cleaner fix (acquire lock before journal write,
write journal under lock, idempotent unlink remains for handled in-flow
errors) is a Phase 6 implementation choice, not a contract change.

**Verdict for Concern 3: race-free under the documented threat model.** The
expanded journal carries enough frozen identity for deterministic recovery;
the reordered verification-before-deletion sequence preserves the journal
through every failure mode the proposal documents. One non-terminal
ordering note (R3-F01) is registered for Phase 6.

## Concern 4 — Fresh assessment of Rev 3 changes (assumption / net-value)

### Assumption register (Rev 3)

Rev 3 §1.1 republishes A1–A10. A1, A2, A3, A4, A5, A6, A7, A9, A10 are
unchanged in their substantive content. A8's invariant in Rev 3 reads:
"Crash recovery cannot make filesystem rename and SQLite update one
physical transaction, so import-replace v1 must use a durable
pending-operation journal to make startup recovery deterministic." This is
verbatim Rev 2's tightening; Rev 3 adds the canonical_records_path and
verification-before-deletion specifics to satisfy A8 rather than restate
it. A8 still **HOLDS** under the same evidence (current migration code does
not have a pending-op table; no equivalent durable transcript-replace
journal landed in another sibling feature).

All ten **HOLD** under Rev 3 evidence.

**Termination signal #1 (`assumption_invalidated`) does not fire.**

### Net value (Rev 3 vs Rev 2 vs Rev 1 vs current state)

Round 2 retired twelve problem-map entries; Rev 3 retains all twelve and
additionally retires the problem-map entry that AIR-R2-F01 surfaced as
unfinished business (deterministic post-rename DB recovery from journal
identity rather than from potentially stale resolver state):

| Additional retirement | Retired by Rev 3 |
| --- | --- |
| AIR-R2-F01 underlying gap: post-rename DB recovery cannot rely on stale resolver context | §4 step 8 frozen identity + `canonical_records_path` (§6 step 4 recovery from canonical records, §9.1 `T-recovery-rename-only`). |

Thirteen problem-map / audit entries retired total against pre-Rev-1 state.

Blast-radius items vs Round 2:

| Blast-radius item | Round 2 status | Rev 3 status |
| --- | --- | --- |
| Wrong canonical bytes written under a valid lock | Bounded | Bounded (§3 / §6 unchanged). |
| Caller-supplied preimage stale by acquisition time | Bounded | Bounded (§4 success-flow step 3 second under-lock check). |
| Crash after rename before DB commit | Closed by durable journal + startup recovery | **Closed deterministically** via frozen identity in journal + canonical_records_path (§4, §6 step 4). |
| Crash after DB commit before journal deletion | Bounded (re-apply idempotent) | **Closed**: §4 step 10 unchanged but verification-before-deletion ordering means a hash mismatch leaves recovery artifacts (§9.1 `T-no-deletion-before-verify`). |
| Postimage hash mismatch under lock | Implicit operational error | **Explicit**: §4 step 7 names the rollback / leave-artifacts behavior. |
| Fresh export verification mismatch | Not addressed | **Explicit**: §4 step 8 names the rollback / leave-artifacts behavior. |
| Stale temp files in transcript dir | Bounded (R2-F04 cosmetic) | Bounded (R3-F04 carryover). |
| In-binary writers not honoring `SessionLock` | Tightened at §13 | Tightened at §13 (unchanged); §12 / §11.1 prose carryover (R3-F03). |
| Provider-native renderer record-class scope | Bounded by `15 invalid-input-transcript` (R2-F02 cohort-A note) | Bounded (R3-F02 carryover). |
| Startup-recovery scope on every `agents` invocation | Bounded but ambiguous (R2-F01 hookpoint) | Bounded but ambiguous (R3-F01b carryover). |
| Replaced-session metadata loss on resume / trace | Bounded by §6 / §7 / §12 (R2-F05 cohort gap) | Bounded (R3-F05 carryover). |
| **NEW** Journal write before lock acquire | n/a | Bounded by cohort-A orchestrator threat model (R3-F01a). |
| **NEW** Quarantine subdirectory | n/a | Bounded private filesystem state under existing data-dir (§6 step 6 / §8). |
| Receipt lost after commit | Bounded (export+hash recovery) | Bounded (§12 residual #6 unchanged). |
| `migrate-db` / `migrate_chain_segment` adjacency | UNCOUPLED | UNCOUPLED unchanged. |

Thirteen problem-map / audit entries retired; nine existing blast-radius
items preserved or tightened; three new blast-radius items added (two of
which are tighter specifications of behaviors that were implicit in Rev 2,
plus the journal-before-lock ordering note). Net value is unambiguously
positive against (a) the v1 adapter the harness uses today, (b) the Rev 1
supported surface, and (c) the Rev 2 supported surface.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 5 — Adjacent-path no-regression check (Rev 3)

Rev 3 changes are restricted to §4 (success flow + journal format), §6
(startup recovery), §8 (side-effect contract / crash states), and §9.1
(four new T-rows). No change touches §1 scope, §2 CLI surface, §3 input
validation / rendering, §5 exit codes, §7 DB consistency contract, §10
README, §11 supported-surface customer cohort prose, §12 residuals, or §13
constraint compliance. The adjacent-path table from Round 2 is unchanged.

| Path | Verdict | Evidence (Rev 3 delta) |
| --- | --- | --- |
| `agents resume`, `repl --resume`, top-level `--resume` | PRESERVED for non-replaced sessions; partial DEGRADED for replaced sessions on parent_turn_id / is_sidechain / is_compaction_boundary | Rev 3 unchanged. |
| `agents trace --json` | PRESERVED for invocation-tree; partial DEGRADED for any future per-turn parentage feature on replaced sessions | Rev 3 unchanged. |
| `agents migrate-config` | UNCOUPLED | Rev 3 unchanged. |
| `agents migrate-db` | UNCOUPLED | Rev 3 unchanged; journal still not consumed by `migrate-db`; manual-recovery flag still anti-scope (§13). |
| Hidden `agents resume-list` | PRESERVED | Rev 3 unchanged. |
| Direct CLI `claude` / `codex` | PRESERVED | Rev 3 unchanged; provider files still receive provider-native bytes. |
| `agents session locate` | PRESERVED + REUSED | Rev 3 unchanged. |
| `agents session schema-probe` | PRESERVED + REUSED | Rev 3 unchanged. |
| `agents session export` | PRESERVED + REUSED | Rev 3 strengthens use as round-trip oracle: §4 step 8 fresh-export verification before commit means callers get more reliable round-trip semantics post-replace. |
| `agents session pause-handshake` / `resume-handshake` | PRESERVED + REUSED | Rev 3 unchanged. |
| `migration::migrate_chain_segment` | UNCOUPLED | Rev 3 unchanged. |
| GUI / Tauri command surface | UNCOUPLED | Rev 3 unchanged; new private `quarantine/` subdirectory is under same default state root, no GUI surface added. |

Zero BROKEN paths. The two paths carrying conditional partial DEGRADED for
replaced sessions only (R1 / R2 / R3 unchanged) remain bounded by opt-in.
The new private `replace_journal/quarantine/` subdirectory introduces no
public-CLI adjacency; cohort A / B do not need to read it. `agents export`
adjacency is materially strengthened by the Rev 3 fresh-export verification
gate before SQLite commit (§4 step 8): the round-trip oracle now blocks the
SQLite commit on its own success.

## Concern 6 — Migration / rollback / observability (Rev 3 deltas)

**No user state one-shot.** Rev 3 §11.1 unchanged. The new
`<state-data-dir>/replace_journal/quarantine/` subdirectory is created on
demand by recovery; existing installs without it are not affected by its
absence. The new `canonical_records_path` file lives inside the existing
`<state-data-dir>/replace_journal/` and follows the same on-demand creation
pattern as the pending journal.

**Rollback.** Three paths from Round 2 are preserved:

1. PR-level rollback: Rev 3 adds no DB schema and no public-CLI surface
   delta. `git revert` remains clean at the binary level. Leftover
   `replace_journal/` and `replace_journal/quarantine/` directories on disk
   after revert are benign — they contain only `.pending` JSON and
   `.canonical.jsonl` files that nothing else reads.
2. Operation-level rollback: re-import the prior canonical transcript with
   the current postimage as preimage. Unchanged from Round 2.
3. Crash-window rollback: identical to Round 2 in shape, strengthened by
   Rev 3's verification-before-deletion ordering. Cohort A no longer needs
   to run manual recovery for the audit-flagged windows; ambiguous-hash
   cases land in `quarantine/` for explicit operator inspection rather
   than touching transcript or DB.

**Observability.** Receipt JSON shape is unchanged from Round 2; cohort A
parsers do not need to update. The journal file remains private (§4
unchanged). Stderr structured JSON still covers every domain failure (§5).
`committed_at` remains a post-DB-commit timestamp. Two new private
filesystem signals are introduced: `canonical_records_path` (recovery
source of truth for DB rebuild) and `replace_journal/quarantine/` (operator
inspection target on hash-mismatch). Neither is a public observability
surface; both are documented as private implementation state in §4 / §6 /
§8 / §11.1.

## Concern 7 — Harness acceptance criteria coverage (Rev 3)

Round 2's eight bullet → §9.1 row mapping is preserved. Rev 3 §9.1 adds
four new rows that match the four AIR-R2-F01 closure capabilities:

| Rev 3 capability | §9.1 row added | Closure |
| --- | --- | --- |
| Frozen-identity journal recovery | `T-recovery-rename-only` | AIR-R2-F01 (1) + (3). |
| Quarantine on ambiguous hash | `T-recovery-ambiguous-hash` | AIR-R2-F01 (2) ambiguous arm. |
| Canonical records preserved across crash | `T-recovery-canonical-records-preserved` | AIR-R2-F01 (1) durability. |
| Verification-before-deletion enforcement | `T-no-deletion-before-verify` | AIR-R2-F01 (2) verify-first. |

All fifteen test-intent rows in Rev 3 §9.1 map to declared behaviors in
§3 / §4 / §5 / §6 / §7 / §8. No bullet is orphaned. The Round 1 / Round 2
caveat "in-flight sessions return exit `13`" remains covered for
cooperative observers; §13 prose remains the contract authority.

## Concern 8 — Initiative-06 sequencing forward-compat (Rev 3)

Import-replace is still the **last** Initiative-06 feature; there is no
downstream sibling consumer of its surface. Rev 3 changes that touch
forward-compat:

- **Receipt JSON evolution.** §6 fields are unchanged. Stable consumer pin
  remains `operation: "import-replace"`. The expanded journal is private
  and does not enter the receipt; future fields can still be added
  additively.
- **Reserved exit codes 16 / 17.** Unchanged.
- **Cross-provider migration adjacency.** UNCOUPLED unchanged. A future
  refactor that lifts the renderer + atomic-replace primitive +
  replace_journal + quarantine into `migration::migrate_chain_segment` is
  allowed but not required.
- **Future canonical-schema extension** (parent_turn_id, is_sidechain,
  is_compaction_boundary). §6 / §12 explicitly leave room for this.
- **Future manual recovery CLI.** Anti-scope confirmed at §12 / §13.
  `agents migrate-db --recover` and `agents session import-replace
  --recover` can be layered without changing v1 CLI shape. The Rev 3
  quarantine path gives a future CLI a stable input directory to drain.
- **Provider renderer scope expansion.** Unchanged.
- **Journal schema versioning.** §4 journal format pins
  `schema_version: 1`; §6 step 2 says recovery should "ignore files whose
  `operation` is not `"import-replace"` or whose `schema_version` is
  unsupported." This gives forward extension room (e.g. adding optional
  fields for fcntl-style cross-process exclusion or future renderer
  record-class metadata) without breaking existing recovery code.

No forward-compat hazard. Six additive evolution paths are open.

## Concern 9 — Cohort-specific concerns (Rev 3)

**Cohort A: `agent-harness` (primary consumer).** §11.1 cohort prose
unchanged. Rev 3 strengthens cohort A in three ways: (a) crash recovery is
now deterministic against stale resolver state, removing the AIR-R2-F01
risk that the harness would have had to reconcile manually; (b) postimage
verification before SQLite commit means a successful exit `0` carries a
stronger round-trip guarantee than Rev 2; (c) ambiguous-hash quarantine
gives the harness a clear operator-visible signal for the rare external-
mutation case rather than silent DB drift.

Rev 3 narrows cohort A in zero new ways relative to Rev 2 — the renderer
record-class refusal scope (R3-F02) is unchanged, and the journal-before-
lock ordering (R3-F01) only matters under non-orchestrated concurrent
calls, which the cohort-A contract already excludes.

**Cohort B: local automation scripts using `agents session export`.**
§11.1 unchanged. Same surface as cohort A; same renderer-scope caveat
(R3-F02). The journal-before-lock ordering (R3-F01) is more relevant here
in principle since cohort B does not have a single-orchestrator
expectation; in practice the supported-surface contract is "treat
`session-busy` as advisory until full retrofit lands," which means cohort
B is already advised that `SessionLock` is the supported serialization
primitive and concurrent same-session attempts are out of scope.

**Cohort C: existing `agents repl` / `agents resume` / `agents -m <model>
<prompt>` users not using import-replace.** PRESERVED for any session
never import-replaced. Partial DEGRADED for any session import-replaced by
an authorized caller (R3-F05 carryover prose gap).

**Cohort D: GUI / Tauri users.** PRESERVED unchanged.

**Cohort E: direct CLI `claude` / `codex` users.** PRESERVED unchanged.
Rev 3 strengthens this cohort indirectly: the fresh-export verification
gate before SQLite commit (§4 step 8) means a session that would not
round-trip through canonical reading after replace fails the import
explicitly rather than corrupting the on-disk file.

No cohort regressed. Cohort A and Cohort E are strengthened by the Rev 3
verification gates.

## Verdict rationale

**Termination signal #1** (`assumption_invalidated`) does not fire — A1–A10
all hold under Rev 3 evidence; A8's content unchanged from Rev 2 but its
closure mechanism is now stronger.

**Termination signal #2** (`non-positive-value`) does not fire — thirteen
problem-map / audit entries retired (one more than Round 2); one HIGH audit
finding closed (AIR-R2-F01); two implicit blast-radius items now explicit
(postimage hash mismatch, fresh-export verification mismatch); three new
items registered (journal-before-lock ordering, quarantine subdirectory,
expanded private filesystem footprint), each guarded by frozen identity,
private filesystem state, or the documented threat model.

**Standard verdict: LOW.** Adjacent-path blast-radius is bounded — twelve
adjacent paths, zero BROKEN, two paths still carrying conditional partial
DEGRADED for opt-in replaced sessions only (Concern 5). Migration /
rollback mechanized: no schema added; uninstall is clean; operation-level
rollback documented; crash-window rollback strengthened by verification-
before-deletion (Concern 6). All fifteen harness acceptance bullets covered,
including four new journal-recovery rows (Concern 7). Forward-compat
preserved on receipt JSON, exit-code reservation, migration uncoupling,
canonical-schema extensibility, manual-recovery layering, renderer scope
expansion, and journal schema versioning (Concern 8). All five cohorts
non-regressed; cohorts A and E strengthened (Concern 9). The Rev 3
expanded journal + reordered flow is race-free under the documented threat
model, with one non-terminal ordering note (Concern 3 / R3-F01).

**Recommendation:** Phase 5 (hookpoints) and Phase 6 (implementation) may
proceed. Five non-terminal findings below; none fires a termination signal.
R3-F01 is a Rev 3-specific ordering observation that Phase 6 should weigh
against the recovery-determinism reason for journal-before-lock. R3-F02,
R3-F03, R3-F04, R3-F05 are Round 2 carryovers (R2-F02..R2-F05 prose
issues that Rev 3 did not touch).

## Findings

- **R3-F01 (journal-write-before-lock ordering, LOW, non-terminal)** —
  Rev 3 §4 success-flow step 1 writes the journal and canonical records
  file, fsyncs both, and then step 2 acquires `SessionLock`. Under the
  documented threat model (cohort-A single-orchestrator, advisory busy),
  this is race-free. Outside that threat model — two concurrent
  non-cooperating callers invoking import-replace against the same
  session id — process B's pre-lock journal write can overwrite process
  A's pending journal at the same per-session paths
  (`session-<id>.pending`, `session-<id>.canonical.jsonl`); B then fails
  the lock and per §4 step 2 "may unlink the journal and canonical records
  file idempotently before exit," after which an A crash before step 10
  produces a recovery from B's identity rather than A's. Recommendation:
  Phase 6 should consider acquiring `SessionLock` first and writing the
  journal under lock; the lock provides cross-process exclusion for the
  same per-session paths and removes the need for step 2's "may unlink"
  clause. Non-terminal because the contract is explicit that
  `session-busy` is advisory in v1 and the supported cross-process
  serialization primitive is `SessionLock`; non-cooperating concurrent
  callers are already documented as out-of-scope (§12 residual #3, §13
  "advisory until full retrofit lands").

- **R3-F02 (renderer record-class coverage, LOW, non-terminal — carryover
  of R2-F02)** — §3's renderer contract refuses lossy record classes with
  `15 invalid-input-transcript` and `unsupported-record-class:<class>`.
  Multi-modal blocks and tool-use are listed as examples; the proposal
  does not enumerate the exact set of record classes the v1 renderer
  supports, leaving cohort A's effective coverage as a Phase 6
  implementation detail. Recommendation: §11.1 should add a cohort-A note
  bounding effective coverage to the v1 `CanonicalToProviderRenderer`
  scope for `claude_code` and `codex_session`, and Phase 6 should publish
  the supported record-class list at PR time. Non-terminal because the
  refusal is loud (exit `15` with a structured error code).

- **R3-F03 (R1-F01 prose carryover, cosmetic, non-terminal — carryover of
  R2-F03)** — Rev 3 did not update §12 residual #3 to name in-binary
  writers (`run_resume`, `run_repl`, balanced one-shot,
  `migration::migrate_chain_segment`), nor add the §11.1 cohort-A
  orchestrator-role sentence. The §13 row prose remains the contract
  authority. Recommendation unchanged from R2-F03: a one-paragraph edit
  to §12 residual #3 and one sentence in §11.1 cohort A. Non-terminal
  because §13 carries the contract.

- **R3-F04 (stale-temp cleanup scoping, cosmetic, non-terminal —
  carryover of R2-F04)** — Rev 3 §4 pre-mutation step 10 still reads
  "Clean stale import-replace temp files in the target transcript
  directory whose names match this feature's temp-file convention and are
  not currently locked by another live replace operation." The §8
  convention is `<jsonl_path>.tmp-import-replace-<uuid>` (per-jsonl-path),
  and Claude / Codex place many sessions' JSONLs in shared directories.
  Phase 5 / Phase 6 implementer should scope cleanup to
  `<resolved.jsonl_path>.tmp-import-replace-*` rather than a directory-
  wide sweep matching the feature prefix. Cosmetic — §9.1 atomic-temp/
  rename test bound this in code; the prose is the only ambiguity.

- **R3-F05 (cohort-C partial-degraded prose gap, cosmetic, non-terminal —
  carryover of R2-F05)** — AIR-R1-F04's closure documents
  `parent_turn_id` / `is_sidechain` / `is_compaction_boundary` loss in
  §6 / §7 / §12 and a §13 compliance row. Rev 3 §11.1 cohort-C prose
  still does not enumerate the resulting partial DEGRADED state for
  `agents resume` / `repl --resume` / `--resume` / `trace --json` on
  replaced sessions. Recommendation unchanged from R2-F05. Non-terminal
  because the contract is documented in §6 / §7 / §12 prose.

## Audit-history note

This is a Phase 4 supported-surface gate only. I did not review or change
an implementation; Rev 3 remains a proposal artifact. Termination signal
is `none`; verdict is LOW; Phase 5 and Phase 6 may proceed once the audit
track also clears AIR-R2-F01 closure (verified in Concern 1 of this
report). The five non-terminal findings above are recommendations for
Phase 6 prose / scoping rather than blockers.
