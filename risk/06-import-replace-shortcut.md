# 06-import-replace — Phase 4 Shortcut Risk Assessment (Rev 2)

## Verdict: LOW

Rev 2 closes Round 1 findings AIR-R1-F01..F04 without introducing new
hidden shortcuts. The four Round 1 closures are real:

- **F01** (canonical-bytes-on-disk vs provider-native readers) is closed
  by an explicit `CanonicalToProviderRenderer` module, with `claude_code`
  / `codex_session` mapping contracts, an `Other → UnsupportedStorage`
  refusal, a typed `15 invalid-input-transcript` exit for lossy records,
  and a round-trip-through-export oracle (`proposals/06-import-replace.md:150-216`,
  `:340-353`, `:387-415`, `:691-692`).
- **F02** (post-rename/pre-DB-commit recovery) is closed by a durable
  `<state-data-dir>/replace_journal/session-<id>.pending` journal with
  fsync ordering, on-startup auto-recovery that reconciles transcript-vs-
  postimage/preimage hashes, and an explicit quarantine path on
  ambiguous hash (`:260-286`, `:362-379`, `:492-538`, `:562-565`,
  `:694`).
- **F03** (lock observation outside cooperative surface) is addressed
  by citing 06-pause-handshake's PR #17 as the lock-primitive dependency
  and by documenting writer-path retrofit ("`run_repl`, `run_resume`,
  balanced one-shot, `migrate_chain_segment`") as a sibling-PR concern
  on its own timeline (`:687-688`).
- **F04** (canonical-record field-loss ambiguity) is closed by an
  explicit data-loss model: `parent_turn_id`, `is_sidechain`, and
  `is_compaction_boundary` are written as `NULL`/defaults, named in
  §6 / §7 / §9 / §12 / §13, and pinned by a "DB metadata loss is
  explicit" test row (`:355-358`, `:436-444`, `:567`, `:671-673`,
  `:695`).

The Rev 2 deltas — renderer module, durable journal, writer-path
retrofit citation, field-loss model — each ship with typed exits or
typed test rows rather than silent defaults. The renderer's lossy-record
refusal carries a named code shape (`unsupported-record-class:tool-use`,
`:215-216`); the journal's ambiguous-hash branch quarantines rather
than auto-fixes (`:378-379`); the field loss is enumerated in three
places with a §9.1 row that pins the exact NULL/default expectation
(`:567`).

Two Round 1 watchpoints retire under Rev 2:

- **W1 retired** — Rev 1's canonical-on-disk question is resolved by §3
  / §6 committing to provider-native bytes on disk and canonical bytes
  through the export reader for hashing.
- **W2 retired** — Rev 1's postimage-hashing-path mismatch is resolved
  for the same reason: both preimage and postimage now read provider-
  native bytes through the export reader (`:417-422`).

One Round 1 watchpoint narrows but persists:

- **W3 narrows** — A5 (`:84`) declares `claude_code` and `codex_session`
  supported, and §6 receipt enum and §3 renderer contract both name
  `codex_session` as a first-class storage type. §9.1 still hedges
  with "If Codex renderer deferred, Codex test becomes explicit
  unsupported-storage test" (`:569`). Inconsistency between the
  contract commitment and the test fork remains; not a shortcut
  because the deferral fallback is typed (exit `12`), but audit-track
  should pin which fork is binding before Phase 6 begins.

Two Round 2 watchpoints are filed for audit-/scope-track. Two Round 1
nits (N1, N2) remain unchanged in Rev 2 prose. One new minor nit (N3) is
filed.

No finding rises to MEDIUM or HIGH.

## Round 1 closure check (F01–F04)

### AIR-R1-F01 (canonical-bytes-on-disk vs provider-native readers) — CLOSED

Round 1 evidence: §4 step 13 in Rev 1 wrote canonical export bytes to
`jsonl_path`; provider readers (Claude `sessionId/type/uuid`, Codex
`session_meta/response_item.payload`) consume native shapes that
canonical records do not carry.

Rev 2 evidence:

- §3 last paragraph (`:191-216`) commits to provider-native bytes on
  disk: "The transcript file write contract is narrower: v1 writes
  provider-native bytes, not canonical bytes. Import-replace renders
  canonical input into the resolved storage type through a new renderer
  module." A new module path is named: `src-tauri/src/session_replace/render/`.
- §3 step 11 (`:176-180`) gates input on lossless renderer support and
  exits `15` on lossy classes with named error codes.
- §6 (`:340-353`) names `CanonicalToProviderRenderer`, requires
  `claude_code` / `codex_session` to round-trip through export, and
  refuses `Other` with `UnsupportedStorage`.
- §6 receipt schema (`:387-415`) preserves canonical-export hashes for
  `preimage_sha256` / `postimage_sha256` while the on-disk bytes are
  provider-native; "Hashing never uses session_turns summaries" remains
  authoritative.
- §13 row "Provider transcript file receives provider-native bytes,
  not canonical bytes" and row "Lossy canonical-to-provider re-encoding
  is refused" both record YES (`:691-692`).
- §9.1 row "Postimage round-trip" (`:569`) pins "Export hash equals
  receipt postimage_sha256 even though on-disk bytes are provider-
  native" — i.e., the hash-vs-disk-format split is a contract claim
  with a test row.

Closure verdict: **CLOSED**. The renderer is a typed module with a
round-trip oracle, not a stub. `Other` is a typed `12` refusal. Lossy
records are typed `15` refusals with named error-code prefixes.

### AIR-R1-F02 (post-rename/pre-DB-commit recovery missing) — CLOSED with watchpoint W4

Round 1 evidence: §8 documented the rename/DB gap as a residual
recoverable only via "next ingestion scan, future migrate-db, or a
repeated import-replace with the correct preimage" — none of which
are deterministic on-startup.

Rev 2 evidence:

- §4 steps 12, 13, 14, 15, 21 (`:260-285`) wire a durable journal:
  write-then-fsync the pending entry, fsync parent dir, acquire lock,
  re-hash under lock, rewrite-then-fsync the journal if preimage shifts,
  delete-then-fsync after DB commit.
- §6 startup-recovery contract (`:362-379`) reconciles deterministically:
  postimage match → re-apply DB update idempotently and delete journal;
  preimage match → delete journal only; neither match → quarantine.
- §8 crash states #4–#8 (`:516-533`) enumerate every post-rename / pre-
  DB-commit / mid-DB-transaction / post-DB-commit / preimage-rollback /
  ambiguous case and bind each to a deterministic recovery action.
- §9.1 rows "Atomic temp/rename", "Journal post-rename recovery",
  "Journal pre-rename recovery", "Journal ambiguous recovery" pin the
  injection points and expected steady states (`:562-565`).
- §13 row "Durable journal closes post-rename/pre-DB crash recovery"
  records YES with citations (`:694`).

Closure verdict: **CLOSED for the single-instance crash case**. See
**W4** below for a concurrent-invocation race that arises from the
specific journal-write / lock-acquire ordering Rev 2 chose.

### AIR-R1-F03 (lock observation outside cooperative surface) — ADDRESSED (residual)

Round 1 evidence: §12 residual #3 in Rev 1 stated "running invocation
rows are not authoritative busy locks; non-cooperating external
provider processes remain outside this contract" but understated that
in-binary writers (`run_resume`, `run_repl`, balanced one-shot,
`migrate_chain_segment`) likewise do not consult `SessionLock` in v1.

Rev 2 evidence:

- §13 row "Lock observation for import-replace once pause-handshake
  lands" (`:687-688`) cites 06-pause-handshake PR #17 as the lock-
  primitive dependency and documents the writer-path retrofit:
  "Lock observation by writer paths (run_repl, run_resume, balanced
  one-shot, migrate_chain_segment) is a sibling-PR concern per
  06-pause-handshake's PR #17 narrowed harness acceptance. v1
  import-replace observes locks; concurrent runner writers observe per
  their own retrofit timeline. The harness consumer of v1 should treat
  session-busy as advisory until full retrofit lands."
- The retrofit-timeline narrowing matches the
  `~/ai/conventions/no-deferred-stubs.md` shape: typed exit (`13
  session-busy`), named follow-up (sibling PRs per the cross-feature
  constraint at `initiatives/06-session-override-contract.md:106-111`),
  and an explicit advisory-only contract claim for v1.

Closure verdict: **ADDRESSED**. F03 was a contract-prose tightening
finding, not a behavior gap; Rev 2 prose now names the in-binary
retrofit dependency explicitly. Residual remains in §12 #3 (`:667-668`)
under the same "advisory until full retrofit lands" framing.

### AIR-R1-F04 (canonical-record field-loss ambiguity) — CLOSED with watchpoint W5

Round 1 evidence: Rev 1 wrote `session_turns` rows from canonical
records but did not define what happens to fields not carried by
`CanonicalRecord` (`parent_turn_id`, `is_sidechain`,
`is_compaction_boundary`). The 06-export `CanonicalRecord` definition
(`06-export/src-tauri/src/session_export/mod.rs:8-18`) confirms these
three fields are absent from the canonical schema.

Rev 2 evidence:

- §6 (`:355-358`) declares the loss explicitly: "Fields not present in
  CanonicalRecord (parent_turn_id, is_sidechain, is_compaction_boundary)
  are intentionally written as NULL or schema defaults in session_turns."
- §7 step 4 (`:439-444`) repeats this in the DB transaction contract and
  warns consumers: "downstream features such as resume and trace should
  not rely on these fields after a replace."
- §9.1 row "DB metadata loss is explicit" (`:567`) pins the test
  expectation: "Reinserted rows have canonical fields populated and
  absent fields set to NULL or defaults."
- §12 residual (`:671-673`) and §13 row "State consistency covers
  required rows. Yes, with documented canonical-field loss" (`:695`)
  both name the loss as documented and tag canonical-schema extension
  as the future fix point.

Closure verdict: **CLOSED for the typed data-loss model**. The loss is
explicit, typed (NULL/default), tested, and bound to a named follow-up
(canonical-schema extension). See **W5** for an unverified contract
claim about downstream NULL-tolerance.

## Watchpoints (audit-/scope-track, not shortcut violations)

### W4 — Journal busy-delete race under concurrent invocation (Rev 2)

§4 step 12 (`:260-262`) writes the pending journal at
`<state-data-dir>/replace_journal/session-<session_id>.pending` **before**
step 13's lock acquisition. §4 step 13 (`:263-265`) instructs the busy
path to delete that same per-session journal entry: "Busy deletes the
pre-mutation journal entry, fsyncs the journal directory, and exits 13."

Two concurrent `agents session import-replace <X>` invocations against
the same session race on a single per-session path:

1. Instance A writes `session-<X>.pending` at T0.
2. Instance B writes `session-<X>.pending` at T0+ε (the OS write
   semantics determine which content wins).
3. Instance A acquires `SessionLock` at T1; B is busy at T1+ε.
4. B's busy-cleanup deletes `session-<X>.pending` at T2.
5. A is mid-replace under the lock. A's step 14–15 may have rewritten
   the journal (A's authoritative content); B's T2 delete then erases
   that authoritative content.
6. A crashes after rename in step 18 but before step 21's journal
   deletion. Startup recovery has nothing to find. **F02 reopens for
   the concurrent-invocation case.**

This is not a shortcut violation — the journal logic itself is explicit
and the busy path is documented. It is, however, an ordering/naming
defect introduced by Rev 2's specific closure of F02 that audit-track
should pin before Phase 6. Two clean fixes are available:

- **Fix A (reorder)**: acquire `SessionLock` first; only the lock holder
  writes / rewrites / deletes the journal. The busy path never touches
  the journal because it never wrote one.
- **Fix B (per-attempt name)**: write `session-<X>.<attempt-uuid>.pending`
  so each attempt owns its own file; the busy path deletes only its own
  attempt; recovery scans the `session-<X>.*.pending` prefix.

This watchpoint is filed at the same severity tier as Rev 1's W1/W2
(unresolved contract mechanics) rather than as a shortcut finding,
because Rev 2's prose does not silently mask the race; it just under-
specifies the ordering. Phase 6 implementer can resolve in either
direction.

### W5 — Downstream NULL-tolerance asserted, not verified (Rev 2)

§7 step 4 (`:443-444`) asserts "downstream features such as resume and
trace should not rely on these fields after a replace." This is a
contract claim about consumer behavior, not a verified property of the
current branch.

Verified in this worktree (`src-tauri/src/state/db.rs`):

- `latest_compaction_boundary` (`:2510-2536`) reads
  `is_compaction_boundary = 1` to drive resume's compaction decision.
  Post-replace rows default to `0`, so resume after a replace returns
  `None` for the latest boundary — a real semantic regression for the
  resume path's compaction handling.
- `parent_turn_id` and `is_sidechain` are referenced in the
  resolver-adjacent path (`src-tauri/src/state/db.rs:109-125, :877`)
  and appear in `balancer/mod.rs`, `sessions/mod.rs`, `trace/mod.rs`.
  Whether each consumer tolerates NULL on the post-replace branch is
  not asserted by Rev 2.

This is not a shortcut violation — F04's typed data-loss model is
explicit, test-pinned, and bound to a named extension. The shortcut
review's question is "does Rev 2 mask the loss?" and the answer is
no. The audit-track question is "is the contract claim ('downstream
should not rely') accurate against the current branch?" and the
answer is "not for `latest_compaction_boundary`, and unverified for
the rest." Phase 5 hookpoints or Phase 6 implementation should either
(a) add NULL-tolerance handling on the consumer paths, (b) extend
canonical schema to carry the three fields before import-replace
ships, or (c) narrow §7 step 4's prose to enumerate which downstream
behaviors are accepted to regress in v1.

W3 (Rev 1) — A5 vs §9 Codex-renderer two-track — narrows but persists.
A5 (`:84`) and §6 receipt enum (`:408`) commit to `codex_session`;
§9.1 still hedges (`:569`). Audit-track should pin which fork is
binding before Phase 6.

W1 (Rev 1) — RETIRED. Provider-native bytes on disk per §3 / §6.
W2 (Rev 1) — RETIRED. Preimage and postimage both go through the
export reader on provider-native bytes (`:417-420`).

## LOW-severity observations / nits

### N1 — Stale-temp cleanup scope still underspecified (carries from Rev 1)

§4 step 9 (`:233-235`) is unchanged from Rev 1: "Clean stale import-
replace temp files in the target transcript directory whose names
match this feature's temp-file convention and are not currently locked
by another live replace operation." The temp-file convention
`<jsonl_path>.tmp-import-replace-<uuid>` (§8 `:488-490`) ties each temp
to a specific path, but Claude project directories
(`projects/<workspace>/`) and Codex session directories
(`sessions/<yyyy>/<mm>/<dd>/`) host multiple sessions per directory.
Cleanup must filter by `<jsonl_path>` prefix, not just feature suffix.
The "not currently locked by another live replace operation" predicate
is also unspecified — `SessionLock` is per resolved active session id,
not per temp file. Phase 6 should specify the predicate. Not a
shortcut; cleanup is a recovery convenience.

### N2 — `source_file` conditional unchanged (carries from Rev 1)

§7 step 5 (`:445-447`) is unchanged: "Set `source_file` to the replaced
`jsonl_path` when the current schema/helper supports it; otherwise
keep existing ingest helper behavior if the column is not meaningful in
this branch." Today batch ingestion writes `source_file = ''`. The
conditional is reasonable layering against the stacked schema-probe /
06-export branches but reads as a soft fallback. Phase 5 hookpoints
should declare which branch state is binding so this becomes a single
sentence, not a switch.

### N3 — Quarantine-marker shape unspecified (new in Rev 2)

§6 step 6 (`:378-379`) and §8 crash state #8 (`:529-533`) instruct
recovery to "rename the journal to a quarantined marker" on ambiguous-
hash transcripts, but the marker name shape is not pinned. Without a
shape (e.g., `session-<id>.quarantined-<timestamp>` or
`.pending → .quarantined`), Phase 6 has freedom to pick something that
collides with the active-pending namespace or that recovery later
mistakes for a still-pending entry. Combined with §12 #2's anti-scope
on a manual-recovery CLI (`:664-665`), quarantined entries pile up
without an in-binary cleanup command, and operators must know the
shape to remove them by hand. Recommendation: pin the shape and
explicitly exclude it from the on-startup scan filter. Not a shortcut
(quarantine is typed and named); flag for Phase 6 specification
precision.

## Per-pattern shortcut audit (Rev 2 deltas focus)

Eight canonical shortcut patterns re-checked against Rev 2's new
surfaces (renderer module, durable journal, field-loss model, F03
prose tightening). Round 1 PASS results carry forward unchanged where
Rev 2 did not touch the surface.

### 1. Hidden silent fallback

Rev 2 deltas:
- §3 step 11 (`:176-180`) hard-exits `15` on lossy renderer cases with
  named error codes; no proceed-anyway path.
- §3 (`:208-216`) lists the renderer's anti-scope explicitly (multi-
  modal blocks, tool-use records); refusal is typed.
- §6 step 6 of recovery contract (`:378-379`) quarantines on ambiguous
  hash; does not silently rewrite transcript or DB.
- §6 step 4 (`:373-375`) re-applies DB update idempotently from
  transcript rows; idempotency is explicit, not silent.
- §7 step 4 (`:439-444`) NULLs absent canonical fields; documented as
  data loss, not papered over.

PASS.

### 2. Dual-write / compat shim / backward-compat alias

Rev 2 deltas:
- The renderer is the dual of the export parser, not a compat shim;
  §3 (`:212-213`) makes round-trip-through-export the contract.
- The journal is a recovery primitive, not a dual-write of state.
- The field-loss model writes one row per turn from one source
  (canonical records); no dual-source.

Grep `compat|shim|backward|legacy|transitional|dual-write|alias` over
the proposal returns matches only on `compatibility` (schema-probe
gate) and "schema-compatible JSON" (rejection criteria). PASS.

### 3. Deferred stubs without typed errors

Per `~/ai/conventions/no-deferred-stubs.md`, deferred work needs a
typed error and a named follow-up. Rev 2's new deferrals:

| Deferred surface | Typed error / refusal | Test pin |
|---|---|---|
| `Other` storage rendering | `12 unsupported-storage` (§3 `:209`, §4 step 7 `:243`, §5 `:332`); residual §12 #4 `:669-670` | §9.1 "Unsupported storage" `:559` |
| Lossy canonical record classes (multi-modal, tool-use) | `15 invalid-input-transcript` with `unsupported-record-class:<class>` (§3 `:179-180, :213-216`) | §9.1 "Unsupported record class" `:560` |
| Manual recovery CLI (`migrate-db --recover`, `import-replace --recover`) | anti-scope explicit (§6 `:381-383`, §12 #2 `:664-666`); on-startup auto-recovery delivered (§6 `:362-379`) | §9.1 three "Journal *recovery" rows `:563-565` |
| Quarantined-journal cleanup | typed quarantine path (§6 step 6 `:378-379`, §8 crash #8 `:529-533`); operator manual cleanup; no silent self-heal | §9.1 "Journal ambiguous recovery" `:565` |
| Codex renderer (if Phase 6 finds blockers) | `12 unsupported-storage` fork (§9.1 last row `:569`) | covered by §9.1 last row |
| Canonical-schema extension for `parent_turn_id`/`is_sidechain`/`is_compaction_boundary` | NULL/default writes (§6 `:355-358`, §7 step 4 `:439-444`); residual §12 `:671-673`; §13 row `:695` | §9.1 "DB metadata loss is explicit" `:567` |
| In-binary writer-path lock observation | `13 session-busy` for cooperative observers (§5 `:333`); residual §12 #3 `:667-668`; sibling-PR retrofit per `initiatives/06-session-override-contract.md:106-111` (`:687-688`) | §9.1 "Lock busy" `:556` |

Each Rev 2 deferral has a typed exit and a named follow-up. None
silently returns success. PASS.

### 4. Hardcoded constants / magic numbers

Grep `hardcode|hard-code|magic|placeholder` over Rev 2 returns zero
hits. The journal directory `replace_journal/`, file shape
`session-<id>.pending`, and renderer module path
`src-tauri/src/session_replace/render/` are namespaced literals, not
magic constants. Owner string `"import-replace"` is a stable
discriminator, not a placeholder. SHA-256 is the harness-named digest.
PASS.

### 5. TODO/FIXME-gated rollout

Grep `TODO|FIXME|for now|in the future|temporary|workaround` returns
matches only on "future" / "later" framings:

- §1 `:39-40, :41-50` future-tense scope statements;
- §6 `:381-383` "future agents migrate-db --recover" framed as
  anti-scope, not as a TODO embedded in mainline;
- §7 `:469-471` "Future canonical-record schema extensions can preserve
  parent_turn_id / is_sidechain / is_compaction_boundary" framed as
  named follow-up;
- §13 row `:702` "agents migrate-db --recover / agents session
  import-replace --recover marked as anti-scope" — anti-scope, not a
  TODO.

No TODO-gated rollout. PASS.

### 6. Symptom-masking heuristic

Rev 2's symptom-masking surfaces:

- §4 step 14 (`:266-269`) re-hashes under the lock — inverse of
  symptom-masking; closes TOCTOU rather than trusting the preflight
  hash.
- §4 step 15 (`:270-273`) rewrites the journal under the lock if
  preimage shifted between pre-lock and under-lock; this is journal
  correctness, not silent rewrite.
- §6 step 6 (`:378-379`) quarantines on ambiguous hash; does not
  guess-and-write.
- §7 step 7 (`:451-452`) uses commit-time fallback for chain
  `last_used_at` only when no usable turn timestamp exists — a single
  named default for a non-null column, documented behavior.
- §8 fsync fallback (`:543-545`) "On platforms where directory fsync
  is unavailable, use the strongest local equivalent and document the
  platform caveat in code comments and tests" — explicit docstring +
  test obligation, not silent weakening.

PASS.

### 7. Feature-flag rollout

Grep `feature flag` returns matches only on schema-probe feature flags
consumed as input gates (A1 `:80`, §9.1 "Schema incompatible" row
`:561`). The proposal does not introduce a new feature flag for itself.
PASS.

### 8. Atomicity bypass / sed-style rewrite

Rev 2 atomicity surfaces:

- §4 D2 (`:230-232`) commits to "two-phase replace with same-directory
  temp file, fsync, atomic rename, and a durable replace journal."
- §8 (`:539-545`) preserves the fsync ordering: temp fsync, rename,
  parent dir fsync.
- §4 steps 12, 21 (`:260-262, :280-283`) wrap the journal in fsync
  ordering: write-fsync-pre, delete-fsync-post.
- No in-place edit, no `sed`-style byte rewrite, no append-only
  amendment.

The W4 watchpoint (concurrent-invocation race on per-session journal
path with busy-cleanup deletion) is a Phase-6 ordering/naming defect
in Rev 2, not a deliberate atomicity bypass. The proposal does not
silently weaken atomicity; the prose just under-specifies an ordering
that a careful Phase 6 implementer must resolve. PASS in mainline;
flagged as W4.

## Per-pattern grep summary (Rev 2)

| Pattern | Hits | Disposition |
|---|---|---|
| `compat\|compatibility` | several | Schema-probe / "schema-compatible" axes only. No compat shim introduced. |
| `shim\|backward\|legacy\|transitional\|alias` | 0 | None. |
| `dual-write` | 0 | None. |
| `TODO\|FIXME` | 0 | None. |
| `for now\|temporary\|workaround\|hack\|magic` | 0 | None. |
| `hardcode\|hard-code\|placeholder` | 0 | None. |
| `feature flag` | 2 | Schema-probe inputs (A1, §9), not a new flag. |
| `defer\|deferred\|future` | several | Future-tense framings around extension points and anti-scope; §9.1 Codex two-track; §7 canonical-schema extension. None mask current behavior. |
| `silent\|silently` | 0 | None. |
| `fallback` | 1 | §8 platform-fsync fallback, documented + tested. |
| `stub` | 0 | None. |
| `renderer` | several | Discriminator language; renderer module is a typed contract with round-trip oracle. |
| `journal` | many | Durable journal mechanism for F02 closure; explicit fsync ordering and recovery contract. |
| `residual` | several | §1, §12, §13; explicit named residuals. |
| `null\|defaults` | several | §6 / §7 / §12 / §13 documented data-loss model for absent canonical fields. |

## Patterns followed correctly (Rev 2)

- **Hard refusal of provider-native input** preserved (§1, §3, §10,
  §13). Provider-native is not a stable public input format; canonical
  JSONL from export's record family is the only stable input.
- **Provider-native bytes on disk** (§3, §6, §13): closes Rev 1's W1.
  Renderer is the dual of the export parser with a round-trip oracle.
- **Lossy renderer refusal** (§3, §9.1): typed `15
  invalid-input-transcript` with named error-code shape; multi-modal
  and tool-use are named anti-scope.
- **Two-phase atomic rename + fsync + parent-dir fsync** (§4 D2, §8):
  unchanged from Rev 1.
- **Durable pending-op journal** (§4, §6, §8): closes Rev 1's F02 for
  the single-instance case. Recovery contract is deterministic across
  five hash-state cases.
- **Double preimage check across the lock boundary** (§4 step 10 +
  step 14): unchanged from Rev 1; closes TOCTOU.
- **Typed exit-codes mirroring the harness namespace** (§5, §13):
  `10`–`15` mapped, no proceed-anyway path on supported-surface
  failures.
- **Explicit named residuals in §12**: rename/DB gap (now mitigated by
  journal), no manual recovery CLI, cooperative-lock-only surface,
  `Other` storage refusal, canonical-schema field loss, platform fsync
  caveat, no receipt log, GUI state-DB divergence — each enumerated
  with a recovery story rather than masked.
- **Codex two-track via typed exit `12`** (§9.1 last row): preserved.
- **No second ownership path** (§13 row, A2): preserved.
- **No second lock format** (§4 D1, §8): preserved; sibling-PR retrofit
  for in-binary writers is named (`:687-688`).
- **Receipt as the durable observability surface** (§6, §11): preserved;
  lost-receipt recovery via export+hash documented.
- **Documented canonical-field loss** (§6, §7, §9, §12, §13): closes
  Rev 1's F04 with NULL/default semantics and named extension follow-up.

## Specific shortcut traps (re-validated against Rev 2)

- **Migration-style temp without fsync.** §8 (`:539-545`) preserves
  fsync ordering. PASS.
- **Migration-style temp filename collision.** Temp uses
  `<jsonl_path>.tmp-import-replace-<uuid>` (§8 `:488-490`). PASS.
- **Per-session journal filename collision.** §4 step 12 uses
  `session-<session_id>.pending` — single name per session. Under
  concurrent invocation this races with §4 step 13's busy-cleanup.
  See **W4**. Not a hidden shortcut; an ordering/naming defect Phase
  6 must resolve. PASS-with-watchpoint.
- **Running invocation as session-busy lock.** A6 and §12 #3 refuse
  this; supported signal is `SessionLock`. Sibling-PR retrofit for
  in-binary writers named (§13 `:687-688`). PASS.
- **Preimage over DB summary rows.** A4 explicitly hashes the
  canonical export byte stream. §6 reaffirms hashes are canonical-
  export hashes even when on-disk bytes are provider-native. PASS.
- **`session_turns` reconstruction from canonical input.** §7 step 3
  preserves canonical fields; step 4 NULLs absent canonical fields;
  step 1 deletes-then-inserts with the unique constraint forcing full
  replace. All-unsupported input exits `15` before mutation (§3
  `:166-168`). PASS.
- **Auto-resume after replace.** §1, §11, §13 refuse. PASS.
- **Auto-`migrate-db` after replace.** §11, §12, §13 refuse. PASS.
- **Cross-provider migration coupling.** §11.1 keeps `migration::
  migrate_chain_segment` UNCOUPLED. PASS.
- **Renderer round-trip silence.** §3 (`:212`) and §9.1
  "Postimage round-trip" (`:569`) bind the renderer to a round-trip-
  through-export oracle. No silent partial encode. PASS.
- **Quarantine self-heal.** §6 step 6 (`:378-379`) quarantines on
  ambiguous hash; does not auto-rewrite. Anti-scope on manual recovery
  CLI is explicit (§12 #2 `:664-666`). PASS-with-nit (N3 quarantine-
  marker shape).
- **Field-loss silent reconstruction.** §6 / §7 / §9 / §12 / §13
  enumerate the loss; §9.1 "DB metadata loss is explicit" pins the
  expected NULL/default state. No silent inference from provider-
  native payloads. PASS-with-watchpoint (W5 downstream NULL-tolerance).

## Conclusion

Verdict: **LOW**.

Rev 2 closes Round 1 findings AIR-R1-F01..F04 with typed deferrals,
explicit residuals, and round-trip oracles where applicable. Eight
canonical shortcut patterns and twelve specific shortcut traps pass.
Two Round 1 watchpoints (W1 canonical-on-disk, W2 postimage-hashing)
retire under Rev 2; one Round 1 watchpoint (W3 Codex two-track)
narrows but persists.

Two Round 2 watchpoints are filed for audit-/scope-track:

- **W4** — Rev 2's specific F02 closure (per-session journal path with
  pre-lock write and busy-cleanup deletion) races under concurrent
  invocation. Phase 6 must resolve via reorder (acquire-lock-then-
  write-journal) or per-attempt naming.
- **W5** — Rev 2's F04 closure asserts downstream features (resume,
  trace) "should not rely on" the lost canonical fields, but
  `latest_compaction_boundary` (`src-tauri/src/state/db.rs:2510-2536`)
  does rely on `is_compaction_boundary = 1`. Phase 5 / Phase 6 should
  either harden consumers, extend canonical schema, or narrow the
  contract claim.

Three LOW-severity nits (N1 stale-temp cleanup scope, N2 `source_file`
conditional, N3 quarantine-marker shape) are filed for Phase 6
specification precision. N1 and N2 carry from Round 1; N3 is new in
Rev 2.

No regression from Round 1: every Rev 1 protection (atomic rename,
double preimage check, typed exit namespace, no second ownership path,
no second lock format, receipt-as-observability) is preserved or
strengthened. No new finding rises to MEDIUM or HIGH.
