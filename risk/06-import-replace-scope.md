# 06-import-replace — Phase 4 Scope Risk Assessment (Rev 2)

**Assessor:** scope reviewer
**Verdict:** **LOW.** Rev 2 is a targeted closing pass against the
four Round 1 audit findings (AIR-R1-F01..F04). All four are closed
at the scope level: F01 by introducing a `CanonicalToProviderRenderer`
that writes provider-native bytes to `jsonl_path` (with `other`
refused and lossy records exit-`15`), F02 by adding a durable
`replace_journal/` and a deterministic startup-recovery contract,
F03 by citing 06-pause-handshake PR #17 as the lock-primitive
dependency and explicitly carving runner-writer retrofit out as a
sibling-PR concern with `session-busy` documented as advisory in
v1, and F04 by making `parent_turn_id` / `is_sidechain` /
`is_compaction_boundary` an explicit data-loss model written as
`NULL`/defaults in `session_turns`. Rev 2 anti-scope holds, the
single-PR boundary is still justified after two new internal
modules (`session_import_replace/` and `session_replace/render/`)
and a durable journal directory are added, and every cross-feature
constraint in `06-session-override-contract.md:106-122` still
maps to a numbered section. No findings at MEDIUM or higher. One
new informational watch-flag (W4 — journal/lock ordering invariant
relevant to F02 closure) and three carry-over watch-flags
(W1/W2/W3) for Phase 5. Three carry-over nits (N1/N2/N3 from
Rev 1, untouched in Rev 2's targeted pass).

---

## 1. Closure check on AIR-R1-F01..F04

Audit-only closure: each finding is matched against the Rev 2 text
that resolves it. No new audit work is performed here.

### AIR-R1-F01 — Canonical-bytes-vs-provider-native (HIGH → CLOSED)

Rev 1 wrote canonical JSONL bytes verbatim to provider transcript
paths. Rev 2 replaces that with a renderer:

- §1 "What does not change" line: "The replacement transcript file
  does not store canonical JSONL in v1. It stores provider-native
  bytes rendered from canonical input for the resolved storage
  type." (`proposals/06-import-replace.md:68-70`)
- §3 step 11 introduces `CanonicalToProviderRenderer` and exits
  `15 invalid-input-transcript` with a sub-code such as
  `unsupported-record-class:tool-use` when a record class cannot be
  rendered losslessly (`proposals/06-import-replace.md:176-216`).
- §6 adds `src-tauri/src/session_replace/render/` with explicit
  per-storage implementations: `claude_code` maps to native
  `sessionId` / `type` / `uuid` / `message`; `codex_session` maps
  to `response_item.payload`; `other` returns `UnsupportedStorage`
  (`proposals/06-import-replace.md:202-216`, `:344-349`).
- §3 last paragraph: "Rendering is the dual of 06-export's provider
  parser. Every supported rendered record must round-trip through
  export back to the canonical input." (`:210-211`)
- §13 row "Provider transcript file receives provider-native
  bytes, not canonical bytes" = Yes; row "Lossy canonical-to-
  provider re-encoding is refused" = Yes (`:691-692`).
- §9.1 adds an "Unsupported record class" component test that exits
  `15` and names the class (`:560`).
- §10 README updates document the rendering contract and
  unsupported-record-class refusal (`:587-596`).

Rev 1 W3 ("canonical-byte stream as on-disk representation, Phase 5
verify-or-revise") is therefore retired and replaced by W3' below
(round-trip parity for the new renderer). **Closed.**

### AIR-R1-F02 — Crash recovery (HIGH → CLOSED)

Rev 1 documented the post-rename / pre-DB gap as a residual. Rev 2
adds a durable journal and a deterministic startup-recovery contract:

- §4 step 12 writes `<state-data-dir>/replace_journal/session-<id>.pending`
  with `operation`, `session_id`, `jsonl_path`, `preimage_sha256`,
  `postimage_sha256`, `db_state_pending`, `started_at`, then
  fsyncs the file and the directory (`:261-263`, `:294-304`).
- §4 step 21 deletes the journal entry only after the SQLite DB
  transaction commits, then fsyncs the journal directory before
  the receipt is emitted (`:281-283`).
- §6 startup-recovery contract walks `replace_journal/`, parses
  each entry, hashes the on-disk transcript through the canonical
  export path, and reconciles deterministically: postimage match →
  reapply DB updates idempotently; preimage match → delete journal;
  neither match → quarantine (`:362-379`).
- §8 enumerates 8 deterministic crash states (pre-temp, post-temp,
  post-fsync, post-rename / pre-DB, mid-DB, post-commit /
  pre-journal-delete, preimage-only, ambiguous) (`:511-533`).
- §9.1 adds three recovery test rows (post-rename, pre-rename,
  ambiguous) (`:563-565`).
- §13 row "Durable journal closes post-rename/pre-DB crash recovery"
  = Yes (`:694`).

The journal hashes are canonical export hashes even though the
transcript file is provider-native; recovery uses the same export
parser the receipt uses, which keeps the hash domain consistent
with F01's renderer choice. **Closed**, with W4 below as a
follow-on Phase 5 verify item on the journal/lock ordering
invariant the recovery contract depends on.

### AIR-R1-F03 — Cooperative-lock surface (MEDIUM → CLOSED)

Rev 1 acquired its own `SessionLock` but did not pin who else
observes it. Rev 2 cites pause-handshake's PR #17 as the
lock-primitive dependency and carves runner-writer retrofit out
of this proposal:

- §1 "Rev 2 changes" bullet: "cite 06-pause-handshake's PR #17 as
  lock-primitive dependency; document that runner writers retrofit
  observation per their own timeline (AIR-R1-F03)." (`:34-36`)
- §13 row "Lock observation for import-replace once pause-handshake
  lands": "06-pause-handshake PR #17 supplies the lock primitive
  dependency. Lock observation by writer paths (`run_repl`,
  `run_resume`, balanced one-shot, `migrate_chain_segment`) is a
  sibling-PR concern per 06-pause-handshake's PR #17 narrowed
  harness acceptance. v1 import-replace observes locks; concurrent
  runner writers observe per their own retrofit timeline. The
  harness consumer of v1 should treat `session-busy` as advisory
  until full retrofit lands." (`:687-688`)
- §12 residual #3 keeps the same scope-correct disclosure: "Running
  invocation rows are not treated as authoritative busy locks. The
  supported cross-process signal is `SessionLock`; non-cooperating
  external provider processes remain outside this contract." (`:666-668`)

This is the right closure shape for a scope review: it does not
expand import-replace's responsibility to retrofit other writers,
and it explicitly makes `session-busy` advisory rather than a hard
exclusion claim. **Closed.**

### AIR-R1-F04 — Canonical-record field-loss (MEDIUM → CLOSED)

Rev 1 said "preserve fields where represented", which made silent
loss of `parent_turn_id` / `is_sidechain` / `is_compaction_boundary`
a possible Phase 6 outcome. Rev 2 makes the loss explicit:

- §6 reusable-API bullet: "Fields not present in `CanonicalRecord`
  (`parent_turn_id`, `is_sidechain`, `is_compaction_boundary`) are
  intentionally written as `NULL` or schema defaults in
  `session_turns`." (`:355-358`)
- §7 #4: "Intentionally drop fields not present in
  `CanonicalRecord` … This is documented data loss in v1;
  downstream features such as resume and trace should not rely on
  these fields after a replace." (`:439-443`)
- §7 last paragraph: "Future canonical-record schema extensions
  can preserve `parent_turn_id`, `is_sidechain`, and
  `is_compaction_boundary`; v1 does not infer them from
  provider-native payloads during import-replace." (`:469-471`)
- §9.1 adds a "DB metadata loss is explicit" component test row
  (`:567`).
- §12 residual #5 records the same loss as a v1 caveat (`:670-672`).
- §13 "State consistency" row updated to "Yes, with documented
  canonical-field loss" (`:695`).

**Closed.**

### Audit-history reconciliation

Rev 2's "Rev 2 changes" bullets in §1 (`:24-39`) tag each closure
to its finding ID (F01, F02, F03, F04). The `risk/06-import-replace-audit-history.md`
"Decision: continue. Rev 2 closes all 4." is consistent with the
text above.

---

## 2. Fresh assessment of Rev 2 changes

Walked the full proposal §1–§13 against the Initiative 06 contract,
the harness ask (`03-session-import-replace.md`), and the
Phase 2.5 problem map (`research/06-import-replace-problem-map.md`).
Rev 2 deltas relative to Rev 1:

| Surface | Rev 1 | Rev 2 | Direction |
| --- | --- | --- | --- |
| Bytes written to `jsonl_path` | canonical export JSONL | provider-native via `CanonicalToProviderRenderer` | targeted contract correction (closes F01) |
| Internal module count | one (`session_import_replace/`) | two (`session_import_replace/`, `session_replace/render/`) | additive; renderer module is dead code without import-replace, so the boundary is still right |
| Crash-recovery surface | residual + opportunistic temp cleanup | durable `replace_journal/` + startup-recovery contract | additive durable state in `<state-data-dir>` (closes F02) |
| Recovery startup hook | none | scan + reconcile before resolver-derived rows are read | new always-on cost on every binary startup; magnitude is one directory stat in the empty case |
| Lock-observation claim | "Yes" without writer citation | "Yes" with PR #17 citation and explicit advisory carve-out | clarification only (closes F03) |
| Canonical-field loss | "where represented" | explicit `NULL`/defaults; named fields | scope-tightening (closes F04) |
| Error sub-vocabulary | preimage-mismatch, invalid-input-transcript | adds `unsupported-record-class:<class>` under exit `15` | additive sub-code under existing exit; not a new exit-namespace entry |
| Test track | 12 rows | 16 rows (renderer round-trip, unsupported class, three recovery rows, explicit field-loss) | additive coverage |

### Anti-scope (vs `06-session-override-contract.md:117-122` and harness)

| Anti-scope clause | Rev 2 stance | Compliance |
| --- | --- | --- |
| No auto-resume | §1 (`:62-64`); §11 (`:633-635`); §13 row | yes |
| No provider spawn | §1 (`:62-64`); §11; §13 row | yes |
| No quota refresh | §1; §11; §13 row | yes |
| No config edits | §1; §11; §13 row | yes |
| No coupling to `migrate-config` | §1; §11; §13 row | yes |
| No GUI/Tauri/daemon/server | §1 (`:64`); §11.1 (`:617-618`) | yes |
| No provider-native JSONL as stable public input | §1 (`:66-67`); §3 (`:185-220`); §10 (`:584-585`); §13 row | yes — and tightened: §1 now also explicitly states the on-disk file is provider-native bytes rendered from canonical input, which closes the F01 round-trip ambiguity without inventing a second public input format |
| No manual recovery CLI in v1 | §6 last paragraph (`:381-383`); §12 #2 (`:663-664`); §13 row | yes |

Anti-scope is intact. Rev 2's renderer module and journal directory
are internal — they do not introduce a new public surface and do
not promise a public recovery command.

### Cross-feature constraints (`06-session-override-contract.md:106-122`)

Every row in §13 maps to its own numbered section. New rows added
in Rev 2:

- "Provider transcript file receives provider-native bytes, not
  canonical bytes" (§3, §6 renderer module).
- "Lossy canonical-to-provider re-encoding is refused" (§3, §9
  unsupported-record-class test).
- "Durable journal closes post-rename/pre-DB crash recovery"
  (§4, §6, §8, §9 recovery rows).
- "State consistency covers required rows. Yes, with documented
  canonical-field loss" (§7).
- "No manual recovery CLI in v1" (§6, §12).

Existing rows still hold:

| Constraint | Compliance | Notes |
| --- | --- | --- |
| Shared error-code namespace 10–15 | yes | §5 unchanged; sub-codes under exit `15` remain inside the namespace |
| Single ownership via `StateDb::resolve_resume` | yes | §4 steps 4–7 unchanged |
| Lock observation once pause-handshake lands | yes within cooperative surface | F03 closure adds PR #17 citation and advisory carve-out |
| Refuses if not exclusively owned | yes within cooperative lock surface | non-cooperating writers remain a §12 residual |
| Read-only `StateDb` open / schema compatibility | yes | §4 step 5 → exit `14` |
| Reusable canonical reader from export | yes | A3, §3, §9 |

### Coverage matrix — problem-map → Rev 2

Rev 1 coverage was complete; Rev 2 changes do not regress it. New
coverage is additive:

| Problem-map item | Rev 2 location | Notes |
| --- | --- | --- |
| §1 #7-10 canonical record family + sha2 | A3, §3 step 5, §6 hash details | unchanged from Rev 1 |
| §2 #1-15 migration replace shortfalls + crash gaps | §4 steps 9, 12, 21; §6 startup recovery; §8 crash states 1–8; §9 recovery rows | F02 closure now meets "performs crash recovery" harness acceptance |
| §3 #4-5 `session_turns` summary-only / `source_file = ''` | §7 #5 (carry-over hedge — see N2 below) | not changed in Rev 2 |
| §3 #13-15 Claude/Codex JSONL shape divergence | §3 step 11 + §6 `session_replace/render/` per-storage renderers | F01 closure replaces W3 with W3' |
| §3 #6-7 ambiguity recency / multi-active-segment | §4 step 6 → exit `11`; §7 D4a "Do not close or reopen" | unchanged |
| §3 #21-25 SessionLock vs lock-blind writers | A6, D1 in §4, §13 PR #17 citation, §12 residual #3 | F03 closure |
| §6 #1-16 migration / preexisting-state implications | §4 step 9 cleanup, §7 D4a, §8 crash states | unchanged |
| §7 draft register A1–A10 | §1.1 A1–A10 | A3, A5, A8 wording adjusted to match Rev 2 renderer + journal |

Coverage remains complete. The three problem-map items the original
proposal flagged as Phase 5 verify items have evolved:

- W1 stale-temp cleanup ordering — unchanged in Rev 2 (still a
  Phase 5 hookpoint concern). Carry.
- W2 schema-probe `safe_for_import_replace` flag flip
  coordination — unchanged in Rev 2. Carry.
- W3 canonical-as-on-disk → W3' renderer round-trip parity for
  real Claude/Codex transcripts. Same anchor (§9 postimage
  round-trip), different verify item.

### Single-PR boundary

Re-evaluated against Rev 2's two internal modules and the durable
journal directory. Three split candidates considered:

**Split A — `session_replace/render/` as a separate prereq PR.**
The renderer is private API consumed only by `session_import_replace/`;
`other` returns `UnsupportedStorage` and is never called by anything
else in v1. Splitting yields a private renderer with no caller. The
test plan (§9.1 "Postimage round-trip") cannot exercise the renderer
without the import-replace command. **Rejected.**

**Split B — durable journal + recovery contract as a follow-up PR.**
The journal exists to close F02 specifically. A v1 import-replace
without the journal is exactly Rev 1, which the audit blocked.
Landing import-replace first would re-introduce the F02 HIGH gap.
**Rejected.**

**Split C — DB consistency helper vs CLI surface.**
Already evaluated and rejected in Rev 1. Rev 2 does not change the
helper boundary; helper still has no caller other than the new CLI.
**Rejected.**

**Single-PR boundary: still justified.** The two new internal
modules and the journal directory are tightly coupled to the
import-replace CLI; any split produces dead intermediate state or
re-opens an audit finding.

### Scope-direction analysis (Rev 2 vs Rev 1)

| Surface vs Rev 1 | Direction | Reason |
| --- | --- | --- |
| Renderer module | targeted addition | required to close F01; does not extend public surface |
| Journal directory + recovery | targeted addition | required to close F02; durable state lives in `<state-data-dir>` and is documented as private implementation state |
| Lock-retrofit citation | clarification | F03 closure shifts retrofit work to PR #17, with `session-busy` documented as advisory in v1 — explicit reduction with citation, not a silent loosening |
| Canonical-field loss | scope-tightening | F04 closure narrows §7 from "where represented" to explicit `NULL`/defaults |
| Receipt JSON | unchanged | §6 schema and field semantics identical to Rev 1 |
| CLI surface | unchanged | §2 subcommand shape, flags, exit codes unchanged |
| Anti-scope | unchanged | seven harness/initiative clauses still intact |

Net direction: Rev 2 makes targeted closures against four audit
findings without expanding the public surface. The new internal
modules and durable journal are the minimum work to satisfy the
audit and are bounded to v1 scope.

---

## 3. No-regression check

Walked Rev 1 scope verdict items, watch-flags, and nits against
Rev 2 to confirm none regressed.

| Rev 1 scope item | Rev 2 status | Notes |
| --- | --- | --- |
| Anti-scope (7 clauses) | held | §1, §11, §13 unchanged on the seven clauses |
| Cross-feature constraints (6 rows + new rows) | held + extended | new rows tighten the contract; do not loosen any existing row |
| Coverage matrix (problem-map §1–§7) | held | additive coverage; no item dropped |
| Single-PR boundary | held | re-justified against the new internal modules and journal |
| W1 stale-temp ordering | unchanged | §4 step 9 same wording; still a Phase 5 hookpoint concern |
| W2 schema-probe flag flip | unchanged | §4 step 5 same; coordination with 06-schema-probe still pending |
| W3 canonical-as-on-disk verify | superseded | replaced by W3' below — verify renderer round-trip parity |
| N1 exit-namespace 16/17 omission note | unchanged | §5 / §13 still do not carry the one-line note |
| N2 §7 conditional `source_file` hedge | unchanged | now §7 #5; same defensive language |
| N3 §1.1 "validated and narrowed" wording | unchanged | §1.1 line still says "validated and narrowed" |

**No regression.** Three open Rev 1 nits and two open Rev 1
watch-flags are carry-over (Rev 2 was a targeted F01–F04 pass and
did not retouch §1.1 wording, §5 namespace prose, §7 #5 hedge,
or §4 step 9 cleanup ordering). Rev 1 W3 is properly retired —
Rev 2's renderer choice is the resolution path that Rev 1 W3
predicted.

---

## 4. Findings (severity ≥ MEDIUM)

**None.**

---

## 5. Watch-flags (informational; not findings)

### W1 — opportunistic stale-temp cleanup ordering (carry-over)

§4 step 9 ("Clean stale import-replace temp files in the target
transcript directory whose names match this feature's temp-file
convention and are not currently locked by another live replace
operation") still runs before `SessionLock::acquire` at step 13.
The proposal does not specify how step 9 distinguishes a stale
temp from a temp owned by another in-flight import-replace. Phase 5
hookpoints should pin the mechanism (per-temp flock sentinel,
mtime threshold, or post-lock cleanup). Not a scope issue.

### W2 — schema-probe `safe_for_import_replace` flag flip (carry-over)

Problem-map §3 #3 says 06-schema-probe returns
`safe_for_import_replace = false` until both `session_import_replace`
and `session_pause_handshake` features are present. The flip lands
in the schema-probe branch when this feature ships. §4 step 5
correctly defers the flip itself to schema-probe; this remains a
Phase 5 sequencing watch-flag, not a scope issue.

### W3' — renderer round-trip parity (replaces Rev 1 W3)

§3 last paragraph of the renderer contract requires that "Every
supported rendered record must round-trip through export back to
the canonical input." §6 §9.1 "Postimage round-trip" is the
end-to-end anchor. Phase 5 should sample real Claude / Codex
transcripts and verify:

- Claude renderer emits `sessionId` / `type` / `uuid` / `message`
  records that the export Claude parser maps back to the same
  `CanonicalRecord` set.
- Codex renderer emits `response_item.payload` records that the
  export Codex parser maps back to the same `CanonicalRecord` set.
- Compaction-boundary truncation semantics in the export Claude
  parser (`06-export/src-tauri/src/session_export/mod.rs:108-163`)
  do not asymmetrically drop records between import and export.
- The unsupported-record-class taxonomy (§3 step 11) is exhaustive
  enough for the fixtures seen in the wild; if a multi-modal /
  tool-use class can be rendered losslessly, the implementation
  should support it rather than refusing with a sub-code.

This is the load-bearing Rev 2 verify item. It carries the
weight Rev 1's W3 used to carry, but on a correct foundation
(provider-native bytes, not canonical bytes on disk).

### W4 — journal/lock ordering invariant (new, F02-related)

§4 step 12 writes `<state-data-dir>/replace_journal/session-<id>.pending`
**before** §4 step 13 acquires `SessionLock`, and §4 step 13's
busy branch ("deletes the pre-mutation journal entry, fsyncs the
journal directory, and exits 13") deletes the same path the lock
holder has just written. Two import-replace processes against the
same `<id>` use the same journal path. If process B writes the
journal (overwriting A's), A acquires the lock, then B exits busy
and deletes "the pre-mutation journal entry," A is now mid-flight
with no on-disk journal. A subsequent crash between rename and DB
commit would no longer be recoverable by the §6 startup-recovery
contract that closed F02.

Step 15 partly mitigates this ("If the under-lock preimage differs
from the journal's `preimage_sha256`, atomically rewrite and fsync
the journal before writing a transcript temp file"), but the rewrite
path assumes the journal still exists on disk. If B has deleted it,
A's "rewrite" is a fresh write — semantically the same, but the
ordering invariant the proposal documents at §8 ("Before acquiring
the session lock and before writing the transcript temp file …")
is no longer the protective invariant the recovery contract reads
as.

Two clean fixes are available at Phase 5 hookpoint time, neither
requiring scope change:

- Move the journal write to **after** lock acquisition (the only
  crash window F02 cares about is post-rename, which can only
  happen under the lock anyway).
- Keep the pre-lock write but use a per-attempt unique journal
  filename (e.g., `session-<id>-<uuid>.pending`), so a busy
  process never deletes the lock holder's journal.

This is informational — it does not change the proposal's promised
contract, only how the contract is met. Audit may re-raise as a
contract-level finding in Round 2; flagging here so Phase 5 picks
the resolution before implementation.

---

## 6. Nits (severity LOW)

### N1 — exit-namespace 16/17 omission note (carry-over from Rev 1)

§5 lists exits `0` / `1` / `2` / `10`–`15`. Shared namespace at
`06-session-override-contract.md:106-111` also reserves `16`
(lock-token-invalid) and `17` (lock-expired). Import-replace
acquires its own lock under owner `"import-replace"` (§4 step 13)
and does not accept caller-supplied lock tokens, so 16/17 are not
reachable on this surface. Neither §5's preamble nor §13's row 1
says so. A one-line note ("16 and 17 are pause/resume-handshake
token vocabulary; not reachable on this surface") would close the
small ambiguity. Drafting only.

### N2 — §7 #5 conditional `source_file` write (carry-over from Rev 1)

§7 #5 reads "Set `source_file` to the replaced `jsonl_path` when
the current schema/helper supports it; otherwise keep existing
ingest helper behavior if the column is not meaningful in this
branch." Given A1 commits to "earlier Initiative 06 surfaces land
before import-replace," the schema state at merge time should be
known: `session_turns.source_file` exists today (problem-map §3 #5
records `source_file = ''`). Recommended fix: commit unconditionally
to `source_file = jsonl_path` and remove the hedge. Drafting.

### N3 — §1.1 "validated and narrowed" wording (carry-over from Rev 1)

§1.1 line 75 reads "approved register validated and narrowed from
`research/06-import-replace-problem-map.md` §7." Counts match
(A1–A10 in both). "Narrowed" reads as a count reduction.
Recommended fix: "consolidated and re-themed from the problem-map
draft, with the same row count." Drafting.

---

## 7. Summary

- **Audit closure:** AIR-R1-F01 (canonical-vs-native bytes), F02
  (post-rename/pre-DB crash gap), F03 (cooperative-lock retrofit
  scope), F04 (canonical-record field loss) all closed at the
  scope level by Rev 2's renderer module, durable journal +
  startup-recovery contract, PR #17 citation with advisory carve-out,
  and explicit canonical-field loss model.
- **Anti-scope:** seven harness/initiative clauses still intact,
  with the on-disk-bytes rule tightened to provider-native bytes
  rendered from canonical input.
- **Cross-feature constraints:** all rows in §13 still satisfied;
  five new rows added (renderer, lossy refusal, durable journal,
  canonical-field loss, no manual recovery CLI in v1).
- **Coverage:** complete; problem-map §1–§7 still maps, with
  additive coverage on F01–F04 closure surfaces.
- **Single-PR boundary:** still justified after the addition of
  `session_replace/render/` and `replace_journal/`. Three split
  candidates all produce dead intermediate state or reopen audit
  findings.
- **No regression:** Rev 1 anti-scope, constraints, coverage, and
  single-PR boundary all hold. Three Rev 1 nits and two Rev 1
  watch-flags are carry-over (Rev 2 was a targeted F01–F04 pass).
  Rev 1 W3 is properly retired.
- **Findings:** none at MEDIUM or higher.
- **Watch-flags:** four total — W1 stale-temp cleanup ordering
  (carry), W2 schema-probe flag flip (carry), W3' renderer
  round-trip parity (replaces W3), W4 journal/lock ordering
  invariant (new; relevant to F02 closure).
- **Nits:** three carry-over drafting items (N1, N2, N3 from Rev 1).

**Verdict: LOW.** Rev 2 closes all four Round 1 audit findings at
the scope level without expanding the public surface, regressing
anti-scope, or breaking the single-PR boundary. Phase 5 dispatch
can proceed; W4 is the only new item worth pinning at hookpoint
time.
