# WU-15-01 Phase 4 Shortcut Risk

## 1. Verdict

**LOW.**

The proposal at `proposals/15-empty-bodies-ref.md` directly addresses each
of RC-1..RC-4 at the mechanism level, names a specific test for each
failure mode, and explicitly forbids the shortcuts the convention files
target (back-compat shims, deferred helpers, feature flags). The
deferrals it does take (BLOB, source-block sentinel, schema-probe bump)
are named, bounded, and have defensible architectural rationale. None of
them silently leave the new contract incomplete.

## 2. Findings

### 2.1 Each root cause has a direct mechanism fix, not a symptom patch

- **RC-1 (no body column).** Proposal §1 adds
  `session_turns.body TEXT NULL` to the fresh schema at
  `src-tauri/src/state/db.rs:628-641` and to existing DBs via column-
  presence `ALTER TABLE … ADD COLUMN body TEXT` in
  `ensure_session_turns_schema` at `src-tauri/src/state/db.rs:1017-1045`.
  This is the column the RC-1 harness asserts must exist
  (`research/12-empty-bodies-ref-rca.md:256-277`). T1 maps to this with
  a specific failure mode named ("schema regression";
  `proposals/15-empty-bodies-ref.md` §5 row T1).

- **RC-2 (ingest drops body).** Proposal §"Ingest write path" extends
  both boundaries the RCA flagged: `ScriptTurn` at
  `src-tauri/src/sessions/mod.rs:32-45` and `SessionTurnIngest` at
  `src-tauri/src/state/db.rs:189-201`, and binds `body` in the bulk
  insert at `src-tauri/src/state/db.rs:2590-2647`. T2 names this with
  failure mode "ingest regression". This is the actual mechanism the
  RCA called out (`research/12-empty-bodies-ref-rca.md:149-168`), not a
  proxy fix.

- **RC-3 (export errors on missing JSONL).** Proposal §"Export read
  path" adds a DB fallback inside `read_canonical_transcript` rather
  than relaxing the existing JSONL-first contract. T3 names this with
  failure mode "export regression" and is explicitly an end-to-end test
  whose oracle is CLI exit 0 + canonical JSONL containing the DB body
  (`proposals/15-empty-bodies-ref.md` §5 row T3).

- **RC-4 (trace inline transcript is null).** Proposal §"Trace inline
  transcript" replaces `TraceNode.transcript: Option<()>` at
  `src-tauri/src/trace/mod.rs:33-40` with a real serializable shape and
  removes the `then_some(())` placeholder at
  `src-tauri/src/trace/mod.rs:134-160`. T4 names this with failure mode
  "trace regression".

Each test row in §5 is keyed to its RC id, names the failure mode in
the Risk column, and points at a concrete fixture/application file.
This satisfies the "name the failure mode each test exercises" check.

### 2.2 Anti-scope explicitly forbids each forbidden pattern

`proposals/15-empty-bodies-ref.md` §2 lists items that match the two
convention files almost line-for-line:

- "No backwards-compatibility shims for the metadata-only contract"
  (mirrors `~/ai/conventions/no-backwards-compatibility.md` "transitional
  adapter layers", "dual implementations", "deprecated aliases").
- "No deferred helper whose only purpose is future BLOB support"
  (mirrors `~/ai/conventions/no-deferred-stubs.md` empty-stub rule).
- "No body compression, encryption-at-rest, or deduplication" — keeps
  scope from sneaking in a "we'll wire this later" hook.
- "No retroactive backfill" — explicitly chooses the small fix over a
  symptom-papering legacy-rewrite that would create a half-implemented
  body store.

The proposal also explicitly says "the new column is `NULL` for legacy
rows, and readers handle that data state explicitly" (§1, §2). That is
a real semantic decision with reader handling, not a shim that lets
the new code "work" against old data while pretending the rows are
populated.

### 2.3 TEXT-vs-BLOB deferral is defensible, not a hidden shortcut

§1 ties TEXT to D-002: current Claude/Codex adapters read UTF-8 JSONL
(`src-tauri/src/session_export/mod.rs:327-332`,
`src-tauri/src/session_replace/mod.rs:425-429`,
`scripts/claude-code-turns:57-60`, `scripts/codex-turns:56-60`), and
D-002 in `DECISIONS.md:34-57` defers multimodal payload expansion. So
TEXT covers the entire body-bearing surface that v1 actually carries.

The deferral is named: "if D-002 reopens, valid UTF-8 TEXT values can
be migrated into BLOB storage without inventing a compatibility shim
now". That is a follow-up tied to a real decision id, not a vague "we
might add it later". It also avoids the
`no-deferred-stubs.md` failure mode of an empty BLOB-supporting helper
checked in today.

### 2.4 Body column name is justified, not cosmetic

§1 names three reasons `body` beats `content`/`payload`:

1. matches the WU slug (`empty-bodies-ref`) and ticket title;
2. avoids collision with export's `CanonicalRecord.content` at
   `src-tauri/src/session_export/mod.rs:8-18`;
3. avoids collision with Codex's native `payload` wrapper documented
   at `scripts/codex-turns:7-14`.

The RC-1 harness accepts any of `body`/`content`/`payload`
(`src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs:10-19`),
so picking `body` is a real choice with collision arguments behind it,
not an arbitrary cosmetic call.

### 2.5 Schema-probe no-bump decision is hedged but defensible

The proposal claims `schema_probe` does not need to change, citing
that `StateDb::open` runs the new migration as an idempotent column-
presence check (precedent: WU-13 topology migration at
`src-tauri/src/state/db.rs:1099-1137` and the existing
parent/sidechain/compaction migration at
`src-tauri/src/state/db.rs:1017-1045`).

Verified: `src-tauri/src/schema_probe/mod.rs:217-249` already lists
`parent_turn_id`, `is_sidechain`, `is_compaction_boundary` as required
columns despite those being added by the same column-presence
migration. So the precedent is split: column-presence migrations do
flow into schema_probe required columns. This proposal explicitly
calls that out as Open Question 1 ("Does any risk gate require
schema_probe to expose `body` as a required column…? Default answer
here is no.") and as Assumption A8 with a named invalidator.

This is borderline shortcut territory but lands on the defensible side
because (a) the deferral is explicit and surfaced as an open question
back to risk gates, not buried, and (b) the new column is nullable, so
reporting "compatible" against an old DB whose `body` column is all-
NULL still gives consumers the contract they were promised (legacy
rows are recoverable via JSONL when present, error-on-missing
otherwise). If risk gates push back on this, A8/Q1 forces the
decision; the proposal does not silently lock it in.

### 2.6 Export DB-fallback NULL-error semantics is defensible

The proposal says export errors on the first `NULL` body row when
JSONL is missing (§"Export read path", §7 Open Question 3). This is
the more conservative behavior: the alternative — silently treating
NULL as empty content — would create the failure mode where a mixed
DB (legacy NULLs + new bodies) silently exports as a partial
transcript. That is the kind of hidden-shortcut symptom-paper-over the
RCA is trying to eliminate. AC-3 says fallback applies "when DB-stored
bodies exist", and §1 makes "empty body" and "unknown legacy body"
explicitly distinct, with the trace `body_state` axis carrying the
distinction onward.

This is a real contract decision with consumer-visible semantics, not
a shortcut.

### 2.7 Import-replace not reading existing bodies is defensible

The current replace transaction at
`src-tauri/src/session_replace/mod.rs:865-928` already operates as
delete-old / insert-new with the canonical input + provider-native
rendering as the source of truth, behind receipt/lock/preimage/
postimage gates that hash provider transcript bytes
(`research/06-import-replace-problem-map.md:54-60`,
`proposals/06-import-replace.md:111-115`). The body column slots in as
one more bound value in the existing INSERT, after the same gates
already ran. Adding a "read existing body for diff/preimage" branch
would be net new behavior outside the receipt contract, so omitting it
is conservative, not a shortcut. §5 row T9 names this directly.

### 2.8 Trace transcript shape reuses the export content shape

§"Trace inline transcript" defines per-turn objects whose `content`
field is "the same shape as export's `CanonicalRecord.content`: an
ordered array of content chunks such as `[{"type":"text","text":…}]`"
(also restated in §"Ingest write path"). This is the same
`Vec<ContentChunk>` shape produced by the existing normalizer at
`src-tauri/src/session_export/mod.rs:405-460`, not an invented parallel
schema.

The new fields the trace shape adds (`turn_id`, `role`, `timestamp`,
`body_state`) are summary metadata already present in `session_turns`,
plus a 3-valued state axis (`stored`/`missing`/`invalid`) that exists
because trace must explicitly distinguish legacy rows from stored-but-
unparseable rows from real bodies. That distinction is the inverse of
the RC-4 placeholder: the proposal explicitly calls it "a new trace
contract, not a metadata-only compatibility shim". Since the prior
contract was `transcript: null`, there is no parallel-evolution risk
versus an existing trace transcript shape.

### 2.9 Adapter contract — partial shortcut concern, but bounded

§"Turn-script adapters" says `scripts/claude-code-turns` and
`scripts/codex-turns` emit `body` as a raw JSON content array using
"the same text chunk convention as `session_export::extract_claude_
content`" and "mapping `input_text` and `output_text` to `'type':
'text'` as export already does"
(`src-tauri/src/session_export/mod.rs:405-460`, `:450-455`).

This is logically reuse of the canonical chunk convention but
mechanically a duplicated implementation: the Python adapter and the
Rust normalizer must agree on chunk shape. That is a real
duplication-risk surface. However:

- The duplication does not introduce a backwards-compatibility shim;
  it introduces parallel adapters that target the same contract.
- The adapters are already parsing Claude/Codex provider-native JSONL
  into summary fields today (`scripts/claude-code-turns:57-86`,
  `scripts/codex-turns:56-87`), so they already encode provider
  knowledge. Adding body extraction extends an existing parser rather
  than seeding a new one.
- T11/T12 in §5 explicitly assert the emitted shape matches the
  expected canonical chunks, which converts the duplication into a
  testable invariant.

This is a duplication trade-off, not a shortcut that lets the v1
ship without solving the body-extraction problem. Calling out for
risk-gate awareness, not as a verdict driver.

### 2.10 Source-block sentinel for DB-fallback exports is openly deferred

§"Export read path" and §7 Open Question 2 explicitly leave the exact
`RecordSource` representation for DB-derived records to Phase 5
hookpoint research, citing the constraint that
`RecordSource.jsonl_path: PathBuf` cannot change wire schema. This is
flagged as a Phase-5 verification task, not deferred to "later".
Within Phase 4 scope (design + scope + architecture + tradeoffs), this
is an honest carry-forward, not a stub.

T8 in §5 captures this as a residual ("Source-block sentinel detail
remains a Phase 5 verification point"), so the unknown is named and
testable rather than papered over.

### 2.11 No design-intent override mistaken for a shortcut

The proposal contradicts `proposals/01-trace-inspection.md:300-306`,
`proposals/06-export.md:48-51`, and
`proposals/06-import-replace.md:111-115`, all of which say SQLite
holds metadata while JSONL is the body source of truth. Per the
ticket's "Decision binding this WU" clause and per §"README and
decisions" requiring a `DECISIONS.md` AC-8 entry, this contradiction
is the binding contract for body storage and not a shortcut versus
those superseded proposals. This finding is explicitly excluded from
the verdict by the prompt's constraints.

## 3. Justification

LOW: each RC has a direct mechanism fix in the named files, each test
row names its failure mode and RC id, the proposal explicitly forbids
the convention-file shortcut patterns in its anti-scope, and the
deferrals it does take (BLOB via D-002, source-block sentinel via
Phase 5, schema-probe bump via Q1/A8) are bounded with named follow-up
triggers rather than vague "we'll add it later" stubs.
