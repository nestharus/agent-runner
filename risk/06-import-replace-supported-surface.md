# 06-import-replace — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

This is the Round 2 supported-surface review of `proposals/06-import-replace.md`
Rev 2. Round 1 verdict was LOW with two non-terminal findings (R1-F01
cooperative-lock contract clarification, R1-F02 cosmetic temp-cleanup scoping);
the audit track was HIGH with four findings (AIR-R1-F01 canonical-vs-native
bytes mismatch, AIR-R1-F02 missing crash recovery for post-rename/pre-DB-commit,
AIR-R1-F03 lock observation outside cooperative surface, AIR-R1-F04 canonical
record field-loss ambiguity). Rev 2's announced scope is to close all four
audit findings while preserving Round 1's supported-surface verdict. This
review confirms that closure from the supported-surface lens, runs the no-
regression check on adjacent paths and cohorts, and registers the carryover
of R1-F01 / R1-F02 prose plus three new bounded blast-radius items intrinsic
to the Rev 2 closures. Net value remains positive on the supported surface;
no termination signal fires.

## Concern 1 — Closure of AIR-R1-F01..F04 from the supported-surface lens

This concern is closure-only audit on the four audit findings. It does not
re-derive the audit verdict; it asks whether each closure is real on the
public-CLI surface and whether each closure introduces an unbounded blast-
radius item.

### AIR-R1-F01 — canonical bytes vs provider-native bytes mismatch — CLOSED

Rev 2 §3 / §6 / §10 / §13 introduce `CanonicalToProviderRenderer` under
`src-tauri/src/session_replace/render/`, with `claude_code` and
`codex_session` implementations and an `UnsupportedStorage` return for
`other`. §3's renderer contract requires every supported rendered record to
round-trip through 06-export back to the canonical input, and §1 states
plainly that "the replacement transcript file does not store canonical JSONL
in v1. It stores provider-native bytes rendered from canonical input for the
resolved storage type."

Supported-surface effects of the closure:

- `agents resume`, `agents repl --resume`, top-level `--resume`, and direct
  CLI `claude` / `codex` continue reading their own native transcript shapes
  unchanged after a replace. Round 1's R1 concern that Rev 1 might silently
  break provider CLIs is now eliminated by construction (provider files
  receive provider-native bytes).
- `agents session export <id>` after replace remains the round-trip oracle:
  §6 step 4 keeps `postimage_sha256` defined as the canonical export stream
  hash, not raw native file bytes. The receipt continues to be byte-for-byte
  comparable to the canonical input the harness shipped in.
- `other` storage continues to refuse with exit `12` (§3 / §4 step 7 / §5);
  it never guesses a native layout. This preserves Round 1's bounded
  unsupported-storage handling.
- New refusal mode: lossy re-encoding (multi-modal blocks, tool-use records,
  or any provider-specific record class without a clean native representation)
  exits `15 invalid-input-transcript` with `unsupported-record-class:<class>`
  before mutation (§3, §13 row "Lossy canonical-to-provider re-encoding is
  refused"). This is loud refusal in place of Rev 1's silent provider-CLI
  breakage; it is a strict positive even though it narrows effective coverage.
  See R2-F02 for the cohort-coverage callout.

Closure verdict: **real and complete on the supported surface.** No
adjacent-path regression. One bounded new blast-radius item (renderer record-
class scope) that is captured under R2-F02.

### AIR-R1-F02 — missing crash recovery for post-rename/pre-DB-commit — CLOSED

Rev 2 §4 / §6 / §8 / §9 / §13 introduce a durable replace journal at
`<state-data-dir>/replace_journal/session-<session_id>.pending`. §4 sequences
journal write before lock acquire, journal-with-fsync before transcript
mutation, and journal delete with directory fsync after DB transaction
commit. §6 startup recovery scans `<state-data-dir>/replace_journal/`,
compares the canonical export hash of the on-disk transcript to the
journal's `preimage_sha256` / `postimage_sha256`, and reconciles
deterministically (re-apply DB idempotently if postimage; clear journal if
preimage; quarantine if neither). §8 crash states 4–8 enumerate the recovery
behavior for the post-rename/pre-DB window that Round 1 audit flagged.

Supported-surface effects of the closure:

- New private filesystem path: `<state-data-dir>/replace_journal/`. This is
  a sibling directory to existing state-DB layout (`state.db`, `locks/`).
  No public CLI flag exposes it; §6 calls it private implementation state.
  Cohort A / B do not gain or need to read this directory.
- Receipt JSON shape is unchanged from Round 1; the journal does not leak
  into stdout. Cohort A consumers parse the same fields.
- The per-startup recovery scan changes the no-op latency profile of any
  command that triggers it. §6 says "scans `<state-data-dir>/replace_journal/`
  on startup and reconciles pending replace operations before normal session
  resolution work relies on derived rows." The scope of "on startup" is not
  pinned to a specific entry point — it could mean "every `agents` binary
  invocation" or "every command that performs session resolution" or "only
  `agents session import-replace` startup." This ambiguity is non-terminal
  but is a Phase 6 hookpoint question; see R2-F01.
- Manual recovery commands (`agents migrate-db --recover`,
  `agents session import-replace --recover`) remain anti-scope (§6 / §12 /
  §13). UNCOUPLED status of `agents migrate-db` from Round 1 is preserved.
- Quarantine behavior on hash-mismatch (§6 step 6, §8 crash state 8) is a
  fail-safe leave-alone with a renamed journal marker. Cohort A does not get
  silent overwrite of its transcript on ambiguous state. Round 1's Concern 4
  rollback story is unchanged.

Closure verdict: **real and complete on the supported surface.** Recovery
is now deterministic for the audit-flagged window. One bounded ambiguity
(startup-recovery scope) captured under R2-F01.

### AIR-R1-F03 — lock observation outside cooperative surface — CLOSED

Rev 2 §13 row "Lock observation for import-replace once pause-handshake
lands" now reads: "06-pause-handshake's PR #17 supplies the lock primitive
dependency. Lock observation by writer paths (`run_repl`, `run_resume`,
balanced one-shot, `migrate_chain_segment`) is a sibling-PR concern per
06-pause-handshake's PR #17 narrowed harness acceptance. v1 import-replace
observes locks; concurrent runner writers observe per their own retrofit
timeline. The harness consumer of v1 should treat `session-busy` as advisory
until full retrofit lands."

This is the contract-prose tightening Round 1 R1-F01 recommended for the
§13 row. The in-binary writers are now named explicitly, the deferral is
attributed to PR #17, and the harness is told to treat `session-busy` as
advisory. From the supported-surface lens this is the correct evolution
shape: cooperative-lock contract is bounded by sibling-PR retrofits, and
the harness as orchestrator absorbs the v1 caveat.

Carryover gap: §12 residual #3 still reads "Running invocation rows are not
treated as authoritative busy locks. The supported cross-process signal is
`SessionLock`; non-cooperating external provider processes remain outside
this contract." Round 1 R1-F01 specifically asked for §12 prose to name the
in-binary writers. §13 names them; §12 still does not. This is non-terminal
because §13 carries the explicit contract — but it is the same prose
inconsistency Round 1 raised. Carried as R2-F03.

Cohort-A orchestrator note also recommended by R1-F01 was not added to
§11.1 in Rev 2. §11.1 cohort prose still treats `agent-harness` as "primary
consumer" without a sentence that the harness is expected to be the sole
orchestrator of `agents` invocations against any session it is actively
replacing. Same severity as the §12 prose gap; carried as R2-F03.

Closure verdict: **closed at the cross-feature constraint table (§13)** but
the §12 / §11.1 prose carryover persists. Non-terminal — the harness reads
§13 as the contract and the orchestrator role is implied by "advisory until
full retrofit lands."

### AIR-R1-F04 — canonical record field-loss ambiguity — CLOSED

Rev 2 §6 (DB update API), §7 step 4 (DB consistency update), §12 (residual),
and §13 (compliance row) explicitly enumerate the lost fields:
`parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` are written as
`NULL` or schema defaults in `session_turns` after a replace. §7 adds:
"This is documented data loss in v1; downstream features such as resume and
trace should not rely on these fields after a replace." §13 row carries
"Yes, with documented canonical-field loss." A future canonical-schema
extension is named as the path to preserve the fields (§6, §12).

Supported-surface effects of the closure:

- The contract for cohort A / B is explicit: a session that has been
  import-replaced will not have parent/sidechain/compaction metadata in
  `session_turns`. Callers can plan around this rather than discovering it
  empirically.
- `agents resume`, `repl --resume`, `--resume`: the resolver
  (`StateDb::resolve_resume`) reads `session_chain_segments` for active
  segment selection, not the three lost fields directly, so resume continues
  to find the active segment. Behavior that depends on parent_turn_id,
  is_sidechain, or is_compaction_boundary in turn-level traversal will see
  defaults instead. This is partial DEGRADED behavior on replaced sessions
  only; non-replaced sessions retain full metadata.
- `agents trace --json`: trace reads invocation rows, not these
  `session_turns` fields directly, so trace remains PRESERVED for
  invocation-tree shape. Any future trace features that walk turn parentage
  on a replaced session would see defaults.
- Cross-provider migration (`migration::migrate_chain_segment`) remains
  UNCOUPLED.

§11.1 cohort C ("existing `agents repl` / `agents resume` users not using
import-replace") remains PRESERVED unconditionally — they cannot be import-
replaced without a caller invoking the new subcommand. Cohort C users
*whose sessions are import-replaced by the harness* see the partial
DEGRADED state above. This conditional partial degradation is documented in
§7 / §12 prose; §11.1 cohort discussion does not enumerate it. Carried as
R2-F05.

Closure verdict: **real and complete on the supported surface.** The data
loss is documented in three places (§6, §7, §12) and called out in the §13
compliance row. One bounded cohort-discussion gap (R2-F05).

### AIR-R1 closure summary

| Audit finding | Closure status | Supported-surface residual |
| --- | --- | --- |
| AIR-R1-F01 (HIGH, native bytes) | Closed | New record-class refusal mode (R2-F02). |
| AIR-R1-F02 (HIGH, crash recovery) | Closed | Startup-recovery scope ambiguity (R2-F01). |
| AIR-R1-F03 (MEDIUM, lock observation) | Closed at §13 | §12 / §11.1 prose carryover (R2-F03). |
| AIR-R1-F04 (MEDIUM, field loss) | Closed | Cohort-discussion gap (R2-F05). |

All four closures are real and bounded on the supported surface. No
termination signal fires from the closure check.

## Concern 2 — Fresh assessment of Rev 2 changes (assumption / net-value)

### Assumption register (Rev 2)

Rev 2 §1.1 republishes A1–A10 with two material changes vs Round 1:

- A3's evidence row is rewritten: "The on-disk replacement bytes are
  provider-native renderings of that canonical input." Round 1 A3 said
  canonical input is the export `CanonicalRecord` family; Rev 2 A3 keeps
  that and additionally pins the on-disk byte family to provider-native.
  This is the explicit byte-family clarification AIR-R1-F01 asked for and
  Round 1 A3 already implicitly held.
- A8's invariant tightens from "two-phase replace ordering" to "must use a
  durable pending-operation journal to make startup recovery deterministic."
  This makes A8 a load-bearing claim for AIR-R1-F02's closure and shifts
  A8's invalidator to "a prior feature lands an equivalent durable
  transcript-replace journal used by import-replace."

A1, A2, A4, A5, A6, A7, A9, A10 are unchanged in their substantive content.
All ten **HOLD** under the same evidence Round 1 cited.

The Round 1 A5 fail-stop hedge ("If Phase 5 hookpoints prove provider-native
renderers cannot consume that canonical byte stream directly, stop and
revise") is no longer needed because Rev 2 has chosen the renderer path
and committed to the provider-native disk format. A5 in Rev 2 reads cleanly:
"`claude_code` and `codex_session` are supported; `other` is refused in
v1," with the renderer module supplying the conversion.

**Termination signal #1 (`assumption_invalidated`) does not fire.**

### Net value (Rev 2 vs Rev 1 vs current state)

Round 1 retired ten distinct problem-map entries; Rev 2 retains all ten and
adds two more closures from problem-map §2 / §3:

| Additional problem-map entry | Retired by Rev 2 |
| --- | --- |
| §2 #7 / §2 #10 No durable transaction marker / pending-op table for replace recovery | §4 / §6 / §8 durable replace journal at `<state-data-dir>/replace_journal/`. |
| §2 #11 No two-phase sequence with temp write + fsync + rename + DB update under one recovery boundary | §4 / §6 / §8 sequence with journal as the recovery boundary. |

Twelve problem-map entries retired total.

Blast-radius items vs Round 1:

| Blast-radius item | Round 1 status | Rev 2 status |
| --- | --- | --- |
| Wrong canonical bytes written under a valid lock | Bounded | Bounded (§3 / §5 / §6 unchanged). |
| Caller-supplied preimage stale by acquisition time | Bounded | Bounded (§4 step 14 second under-lock check). |
| Crash after rename before DB commit | Residual (recovered via next ingestion / migrate-db / repeat) | **Closed by durable journal + startup recovery** (§4 / §6 / §8). |
| Stale temp files in transcript dir | Bounded (R1-F02 cosmetic) | Bounded (carryover R2-F04). |
| Codex canonical→canonical writeback round-trip | Phase-5 fail-stop | Replaced by Codex renderer scope; deferral becomes "explicit unsupported-storage refusal if renderer absent" (§9.1 last row). |
| In-binary writers not honoring `SessionLock` | Residual (R1-F01) | **Tightened at §13** ("advisory until full retrofit lands"); §12 / §11.1 prose carryover (R2-F03). |
| Receipt lost after commit | Bounded (export+hash recovery) | Bounded (§12 residual #6 unchanged). |
| `migrate-db` / `migrate_chain_segment` adjacency | UNCOUPLED | UNCOUPLED unchanged. |
| **NEW** Provider-native renderer record-class scope | n/a | Bounded by `15 invalid-input-transcript` + `unsupported-record-class:<class>` (R2-F02 cohort-A note). |
| **NEW** Startup-recovery scope on every `agents` invocation | n/a | Bounded but ambiguous (R2-F01 hookpoint question). |
| **NEW** Replaced-session metadata loss on resume / trace | n/a | Bounded by §6 / §7 / §12 explicit field-loss prose (R2-F05 cohort gap). |

Twelve problem-map entries retired; eight existing blast-radius items
preserved or tightened; three new bounded blast-radius items added. Net
value remains unambiguously positive: the closures of AIR-R1-F01 and
AIR-R1-F02 each retire a HIGH-severity audit gap, and the new items are
bounded by structured exit codes (`12`, `15`), explicit deferral language,
or non-public filesystem state.

**Termination signal #2 (`non-positive-value`) does not fire.**

## Concern 3 — Adjacent-path no-regression check (Rev 2)

Round 1 classified twelve adjacent paths PRESERVED, PRESERVED + REUSED, or
UNCOUPLED. Rev 2 changes that affect adjacency are:

1. **Provider-native renderer** changes nothing about how `agents resume`,
   `repl --resume`, top-level `--resume`, direct `claude`, or direct `codex`
   read transcript files. They continue to see native bytes. Verdict
   unchanged: PRESERVED.
2. **Durable replace journal** under `<state-data-dir>/replace_journal/` is
   a new sibling to `state.db` and `locks/`. No existing CLI command reads
   or writes this path. `agents migrate-db` does not consume the journal in
   v1 (§13 anti-scope). Verdict for `migrate-db`: UNCOUPLED unchanged.
3. **Startup recovery scan** is the one Rev 2 change with potential adjacent
   effect: any `agents` command path that triggers the scan absorbs the
   scan's latency and may see the journal's idempotent DB update applied
   before its own session resolution. For non-pending journals (the common
   case) this is an empty directory listing — negligible cost. For pending
   journals, recovery is the correct behavior even from cohort C's
   perspective: a stale `session_turns` view that has not been reconciled
   would otherwise mislead `agents resume`. Net: this is a corrective
   adjacency, not a regression. Phase 6 should still pin the trigger scope
   (R2-F01).
4. **`session_turns` field-loss on replaced sessions** is a conditional
   DEGRADED state for `agents resume`, `repl --resume`, `--resume`, and
   `trace --json` — but only on sessions that have been import-replaced.
   Sessions that have never been replaced retain full metadata. Verdict for
   resume / repl / trace: PRESERVED for non-replaced sessions; partial
   DEGRADED for replaced sessions. Round 1 row updated below.
5. **`migration::migrate_chain_segment`** is unchanged by Rev 2; does not
   call the journal, does not consume the renderer, does not observe
   import-replace's lock. Verdict: UNCOUPLED unchanged.
6. **GUI / Tauri** is unchanged by Rev 2; `<state-data-dir>/replace_journal/`
   sits under the same default state root as 06-pause-handshake's `locks/`,
   so the existing GUI/CLI state-DB-location divergence (problem map §4 #12)
   is unchanged in scope.

Updated adjacent-path table for Rev 2:

| Path | Verdict | Evidence |
| --- | --- | --- |
| `agents resume`, `repl --resume`, top-level `--resume` | PRESERVED for non-replaced sessions; partial DEGRADED for replaced sessions on parent_turn_id / is_sidechain / is_compaction_boundary | §1 / §11.1; §6 / §7 explicit field-loss; R2-F05 cohort gap. |
| `agents trace --json` | PRESERVED for invocation-tree; partial DEGRADED for any future per-turn parentage feature on replaced sessions | §11.1; §6 / §7. |
| `agents migrate-config` | UNCOUPLED | §1 / §11.1. |
| `agents migrate-db` | UNCOUPLED | §11.1; not auto-called; no consumer of the journal in v1. |
| Hidden `agents resume-list` | PRESERVED | Not referenced by import-replace. |
| Direct CLI `claude` / `codex` | PRESERVED | §1; provider files receive provider-native bytes via renderer. |
| `agents session locate` | PRESERVED + REUSED | A1 / §4 step 6. |
| `agents session schema-probe` | PRESERVED + REUSED | §4 step 5. |
| `agents session export` | PRESERVED + REUSED | A3 / §3 / §6 round-trip oracle still holds for `postimage_sha256`. |
| `agents session pause-handshake` / `resume-handshake` | PRESERVED + REUSED | §4 D1; same `SessionLock` lock-dir convention. |
| `migration::migrate_chain_segment` | UNCOUPLED | §11.1. |
| GUI / Tauri command surface | UNCOUPLED | §1 / §11.1; no GUI surface added in v1. |

Zero BROKEN paths. The two paths that move from "PRESERVED" to "PRESERVED
for non-replaced sessions; partial DEGRADED for replaced sessions" do so
because of an opt-in mutation cohort A explicitly invokes. This is not a
regression of the pre-PR surface — it is a documented bounded effect of the
new mutation surface.

## Concern 4 — Migration / rollback / observability (Rev 2 deltas)

**No user state one-shot.** Rev 2 §11.1 unchanged: "no user state one-shot
is required before using this command when schema-probe reports
compatibility." The new `<state-data-dir>/replace_journal/` directory is
created on demand by import-replace; existing installs without that
directory are not affected by its absence. Existing partial DBs still
return not-found (`10`).

**Rollback.** Two paths from Round 1 are preserved and one is added:

1. PR-level rollback: Rev 2 adds new modules (`session_replace/render/`,
   `replace_journal/` private directory under state-data-dir) but no DB
   schema, so `git revert` is still clean at the binary level. A
   `replace_journal/` directory left on disk after revert is benign — it
   contains only `.pending` JSON files that nothing else reads.
2. Operation-level rollback: re-import the prior canonical transcript with
   the current postimage as preimage. Unchanged from Round 1.
3. **NEW** Crash-window rollback: if the binary crashes between
   `agents session import-replace` rename and DB commit, the next
   `agents` startup runs recovery (§6) and either (a) re-applies the DB
   update idempotently and clears the journal, (b) clears the journal if
   the on-disk transcript still hashes to the preimage, or (c) quarantines
   the journal entry for operator action. Cohort A no longer needs to run
   manual recovery for the audit-flagged window.

**Observability.** Receipt JSON shape is unchanged from Round 1; cohort A
parsers do not need to update. The new journal file is private (§4 prose:
"The journal is private implementation state, not a public receipt log.").
Stderr structured JSON still covers every domain failure (§5). `committed_at`
remains a post-DB-commit timestamp.

## Concern 5 — Harness acceptance criteria coverage (Rev 2)

Round 1's eight bullet → §9.1 row mapping is preserved. Rev 2 §9.1 adds
three new rows that match three Rev 2 capabilities:

| Rev 2 capability | §9.1 row added | Closure |
| --- | --- | --- |
| Provider-native rendering | "Unsupported record class" + "Postimage round-trip" updated for native-on-disk + canonical-export-hash receipt | AIR-R1-F01. |
| Durable journal recovery | "Journal post-rename recovery" + "Journal pre-rename recovery" + "Journal ambiguous recovery" | AIR-R1-F02. |
| Field-loss documentation | "DB metadata loss is explicit" | AIR-R1-F04. |

The Round 1 caveat "in-flight sessions return exit `13`" remains covered for
cooperative observers. Rev 2 §13 row tightens the contract prose to make
the cooperative-only scope explicit ("advisory until full retrofit lands"),
which is the contract clarification AIR-R1-F03 / R1-F01 asked for. Coverage
is unchanged; clarity is improved.

All eleven test-intent rows (§9.1) map to declared behaviors in §3 / §4 /
§5 / §6 / §7 / §8. No bullet is orphaned.

## Concern 6 — Initiative-06 sequencing forward-compat (Rev 2)

Import-replace is still the **last** Initiative-06 feature; there is no
downstream sibling consumer of its surface. Rev 2 changes that touch
forward-compat:

- **Receipt JSON evolution.** §6 fields are unchanged; the journal is
  private and does not enter the receipt. Future fields can still be added
  additively. Stable consumer pin remains `operation: "import-replace"`.
- **Reserved exit codes 16 / 17.** Unchanged: import-replace owns its own
  lock and does not expose token-handling.
- **Cross-provider migration adjacency.** UNCOUPLED unchanged. A future
  refactor that lifts the renderer + atomic-replace primitive +
  replace_journal into `migration::migrate_chain_segment` is allowed but
  not required.
- **Future canonical-schema extension** (parent_turn_id, is_sidechain,
  is_compaction_boundary). §6 / §12 explicitly leave room for this; a later
  feature can extend `CanonicalRecord` and `session_turns` storage without
  changing the import-replace public CLI shape.
- **Future manual recovery CLI.** Round 1 noted §8 D5 left space for a
  follow-up to add a journal table; Rev 2 has now added the journal but
  kept manual recovery commands (`agents migrate-db --recover`,
  `agents session import-replace --recover`) as anti-scope. A subsequent
  feature can layer the manual recovery flag without changing the v1 CLI
  surface.
- **Provider renderer scope expansion.** A future feature can add a
  renderer for an additional storage type or for additional record classes
  within Claude / Codex without changing the v1 CLI surface; existing
  callers still see `15 invalid-input-transcript` for currently-unsupported
  classes.

No forward-compat hazard. Five additive evolution paths are open.

## Concern 7 — Cohort-specific concerns (Rev 2)

**Cohort A: `agent-harness` (primary consumer).** §11.1 unchanged. Rev 2
benefits cohort A in three ways: (a) provider-native disk format means the
harness no longer needs to run the canonical-bytes-on-disk experiment with
`agents resume` and risk silent breakage; (b) durable journal closes the
post-rename/pre-DB recovery window the harness would otherwise have had to
detect manually; (c) §13 explicit "advisory until full retrofit lands"
bounds the cooperative-lock contract for the harness's orchestrator role.
Rev 2 narrows cohort A in one way: any harness session whose canonical
transcript contains record classes that the renderer cannot represent
losslessly will now refuse with `15 invalid-input-transcript`. Cohort A
discovers this at first refusal rather than via silent data corruption,
which is a strict positive even though it narrows effective coverage.

**Cohort B: local automation scripts using `agents session export`.**
§11.1 unchanged. Same surface as cohort A; same renderer-scope caveat.

**Cohort C: existing `agents repl` / `agents resume` / `agents -m <model>
<prompt>` users not using import-replace.** PRESERVED for any session never
import-replaced. Partial DEGRADED for any session that the harness has
import-replaced (parent / sidechain / compaction metadata defaults). This
is an opt-in cohort transition, not a regression: cohort C cannot be
import-replaced without a caller invoking the new subcommand. R2-F05 asks
that §11.1 cohort C prose explicitly enumerate this conditional state.

**Cohort D: GUI / Tauri users.** PRESERVED unchanged. No GUI surface added.

**Cohort E: direct CLI `claude` / `codex` users.** PRESERVED unchanged.
Rev 2's renderer choice strictly improves cohort E: their tools continue to
recognize their own native transcript format on disk after replace, which
was implicit at best in Rev 1.

No cohort regressed. One cohort (C) gains a documented partial DEGRADED
state for opt-in replaced sessions.

## Verdict rationale

**Termination signal #1** (`assumption_invalidated`) does not fire — A1–A10
all hold under Rev 2 evidence; A3 and A8 are tightened in ways that match
the AIR-R1-F01 / AIR-R1-F02 closures.

**Termination signal #2** (`non-positive-value`) does not fire — twelve
problem-map entries retired (two more than Round 1); two HIGH audit findings
closed; two MEDIUM audit findings closed (one with §12 / §11.1 prose
carryover); three new bounded blast-radius items added, each guarded by a
structured exit, an explicit deferral, or private filesystem state. Net
value is unambiguously positive against (a) the v1 adapter the harness uses
today and (b) the Rev 1 supported surface.

**Standard verdict: LOW.** Adjacent-path blast-radius is bounded — twelve
adjacent paths, zero BROKEN, with two paths now carrying conditional partial
DEGRADED for opt-in replaced sessions only (Concern 3). Migration / rollback
mechanized: no schema added; uninstall is clean; operation-level rollback
documented; crash-window rollback now closed via durable journal (Concern
4). All eleven harness acceptance bullets covered, including three new
journal-recovery and field-loss rows (Concern 5). Forward-compat preserved
on receipt JSON, exit-code reservation, migration uncoupling, canonical-
schema extensibility, manual-recovery layering, and renderer scope
expansion (Concern 6). All five cohorts non-regressed; cohort C gains a
documented opt-in conditional partial DEGRADED state (Concern 7).

**Recommendation:** Phase 5 (hookpoints) and Phase 6 (implementation) may
proceed. Five non-terminal findings below; none fires a termination
signal. R2-F01 and R2-F02 are Phase 5 / Phase 6 hookpoint questions to pin
during scope freeze. R2-F03 and R2-F04 are Round-1 prose carryovers
already covered elsewhere in the document. R2-F05 is a §11.1 cohort-prose
addition.

## Findings

- **R2-F01 (startup-recovery scope ambiguity, LOW, non-terminal)** — §6
  step "scans `<state-data-dir>/replace_journal/` on startup and reconciles
  pending replace operations before normal session resolution work relies
  on derived rows" does not pin the trigger scope. This could mean (a)
  every `agents` binary invocation, (b) every command path that performs
  session resolution, or (c) only `agents session import-replace` itself.
  Each interpretation has a different supported-surface profile: (a) adds
  no-op directory-listing latency to fast read commands like `trace`; (b)
  scopes the cost to commands that already pay session-resolution cost; (c)
  leaves stale post-rename/pre-DB state visible to non-import-replace
  commands until the next import-replace runs. Recommendation: Phase 5
  hookpoint research should pin the trigger to (b) — every command path
  whose correctness depends on `session_turns` consistency for the resolved
  session — and Phase 6 should add a guard that no-ops the scan if
  `<state-data-dir>/replace_journal/` is missing or empty. This is a
  hookpoint question, not a contract question; the §6 / §8 recovery
  semantics are correct under any of (a) / (b) / (c).

- **R2-F02 (renderer record-class coverage, LOW, non-terminal)** — §3's
  renderer contract refuses lossy record classes with
  `15 invalid-input-transcript` and `unsupported-record-class:<class>`.
  Multi-modal blocks and tool-use are listed as examples. The proposal
  does not enumerate the exact set of record classes the v1 renderer
  supports, leaving cohort A's effective coverage as a Phase 6 implementation
  detail. Recommendation: §11.1 add a cohort-A note that "import-replace's
  effective session coverage in v1 is bounded by the
  `CanonicalToProviderRenderer` implementations for `claude_code` and
  `codex_session`; harness consumers should be prepared for
  `15 invalid-input-transcript` on sessions whose canonical transcripts
  contain record classes outside the v1 renderer scope, and Phase 6 should
  publish the supported record-class list at PR time." Non-terminal because
  the refusal is loud (exit 15 with a structured error code), but cohort A
  needs to know the effective coverage at scope-freeze rather than discover
  it at first refusal.

- **R2-F03 (R1-F01 prose carryover, cosmetic, non-terminal)** — Round 1
  R1-F01 asked for two prose changes: (i) tighten §12 residual #3 to name
  the in-binary writers (`run_resume`, `run_repl`, balanced one-shot,
  `migration::migrate_chain_segment`) explicitly, and (ii) add a §11.1
  cohort-A note that the harness is expected to be the sole orchestrator of
  `agents` invocations against any session it is actively replacing. Rev 2
  delivers an equivalent contract clarification at §13 row "Lock
  observation" ("advisory until full retrofit lands"), but the §12 prose
  and §11.1 cohort-A note were not updated. The contract is not ambiguous
  to a cohort A reader who reads §13, but the §12 / §11.1 prose drift makes
  the document internally inconsistent. Recommendation: a one-paragraph
  edit to §12 residual #3 and one sentence in §11.1 cohort A. Non-terminal
  because §13 carries the contract.

- **R2-F04 (R1-F02 cosmetic carryover, cosmetic, non-terminal)** — §4 step
  9 still reads "Clean stale import-replace temp files in the target
  transcript directory whose names match this feature's temp-file convention
  and are not currently locked by another live replace operation." The §8
  convention is `<jsonl_path>.tmp-import-replace-<uuid>` (per-jsonl-path),
  and Claude / Codex place many sessions' JSONLs in shared directories.
  Phase 5 / Phase 6 implementer should scope cleanup to
  `<resolved.jsonl_path>.tmp-import-replace-*` rather than a directory-wide
  sweep matching the feature prefix. Cosmetic — §9.1 atomic-temp/rename
  test bound this in code; the prose is the only ambiguity.

- **R2-F05 (cohort-C partial-degraded prose gap, cosmetic, non-terminal)**
  — AIR-R1-F04's closure documents parent_turn_id / is_sidechain /
  is_compaction_boundary loss in §6 / §7 / §12 and a §13 compliance row.
  §11.1 cohort prose does not enumerate the resulting partial DEGRADED
  state for `agents resume` / `repl --resume` / `--resume` / `trace --json`
  on replaced sessions. Recommendation: §11.1 cohort C add a sentence:
  "Sessions that have been import-replaced will have parent_turn_id,
  is_sidechain, and is_compaction_boundary set to NULL or schema defaults;
  resume / repl / trace continue to function but downstream features that
  walk per-turn parentage on a replaced session will see defaults until the
  canonical schema extends." Non-terminal because the contract is
  documented in §6 / §7 / §12 prose; the cohort discussion is the only
  place that does not echo it.
