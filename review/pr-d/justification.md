# PR-D Justification Review

**Verdict: TIGHT.** Three small, explicable deviations from the
literal contract — each traces to a value in `VALUES.md` and each
is defensible. No scope creep of consequence.

The PR is the smallest of the four and the diff reflects that: one
schema delta, one adapter-contract widening, one script passthrough,
and the wiring to populate the sidechain count field PR-B stubbed.

## Hunk-by-hunk classification

### `scripts/claude-code-turns` — `parentUuid` / `isSidechain` passthrough
**In scope.** Contract §"`claude-code-turns` script update" specifies
exactly this (`parentUuid` → `parent_turn_id`, `isSidechain` →
`is_sidechain`). Two added lines plus a docstring edit. Tight.

### `src-tauri/src/sessions/mod.rs` — `ScriptTurn` + batch call update
**In scope.** `ScriptTurn` gets the two optional fields per contract
§"ScriptTurn widening"; `#[serde(default)]` preserves V4 additive
backwards-compat (four-field adapters still parse). The batch
collection switches from `Vec<(String, String, DateTime<Utc>, String)>`
to `Vec<SessionTurnIngest>` — mechanical consequence of the named
struct in `state/db.rs` (see below). Two new deserialization tests
exercise both the legacy and full JSON shapes, which is the right
pair to pin.

### `src-tauri/src/state/db.rs` — named `SessionTurnIngest` struct (vs tuple)
**Principled contract deviation.** Contract §"Batch ingest signature
change" literally says:

> `StateDb::ingest_session_turns_batch` widens its tuple to include
> parent_turn_id + is_sidechain.

The implementation introduces a named struct instead of widening the
tuple to six fields. Inline comment justifies it:

> Named struct instead of a tuple so callers can't accidentally swap
> positional fields (the role / parent_turn_id pair is otherwise easy
> to mix up).

This is a judgment call the contract didn't pre-authorize, but it's
the right one: a 6-tuple of `(String, String, DateTime, String,
Option<String>, bool)` puts two `String`-typed positions (`role`,
`parent_turn_id` via `.into()` paths at call sites) adjacent to each
other with no compiler help against a swap. V14 ("no backwards-compat
shims for internal code") actively encourages clean internal
rewrites; nothing in V-space argues for preserving the tuple shape.
The cost is ~10 LoC of struct definition + field names at call sites.
Keep.

### `src-tauri/src/state/db.rs` — `SessionTurnCounts` struct + `count_session_turns`
**In scope.** Contract §"DB method addition" specifies both the
struct shape and the method signature verbatim; the SQL matches
contract §"Trace integration" exactly. Two DB tests (persist
round-trip, counts with mixed data spanning two sessions and two
providers) pin the cross-axis filtering the query depends on.

### `src-tauri/src/state/db.rs` — schema + migration order fix
**Necessary, not scope creep.** Commit message flags this:

> legacy session_turns DBs would fail if SQLite tried to create the
> new parent index before the ALTER TABLE added parent_turn_id. Index
> creation moved to after migration.

Concretely: proposal §2 requires `idx_session_turns_parent
(provider_name, session_id, parent_turn_id, timestamp)`. On a legacy
DB where `session_turns` already exists without `parent_turn_id`,
the `CREATE TABLE IF NOT EXISTS` is a no-op, so the subsequent
`CREATE INDEX … parent_turn_id` would reference a nonexistent column
and fail. The fix extracts the three indexes into
`session_turns_index_sql()` and runs it inside
`ensure_session_turns_schema` *after* the ALTERs land. The existing
`idx_session_turns_provider_ts` is carried along for uniform
placement — keeping it in the old pre-ensure location would work but
split the index definitions across two code paths for no reason.

The `legacy_session_turns_db()` test fixture plus
`session_turns_schema_migration_adds_parent_and_sidechain_columns`
exercise this exact path. The migration order fix is correctness,
not decoration.

### `src-tauri/src/state/db.rs` — two new indexes (`session_ts`, `parent`)
**In scope per proposal §2, adjacent per the contract.** The PR-D
contract's "Schema additions" section only lists the two ALTER TABLE
statements, not the indexes. But proposal §2 specifies both new
indexes as part of the session_turns schema, and `count_session_turns`'
filter `WHERE provider_name = ? AND session_id = ?` is precisely
what `idx_session_turns_session_ts` serves. Adding them while the
migration infrastructure is already being touched is the efficient
call. If the contract had forbidden index additions this would be
creep; as written, it's silent and the proposal covers it.

### `src-tauri/src/state/db.rs` — `SessionTurnRecord` field additions
**In scope.** The read-side struct gains the same two fields so that
any future reader sees the full column set. No read-path consumer in
this diff changes behavior (the trace path reaches counts via
`count_session_turns`, not `SessionTurnRecord`), but widening the
record in lockstep with the schema is the normal shape.

### `src-tauri/src/state/mod.rs` — re-exports
**In scope.** Mechanical re-export of `SessionTurnCounts` and
`SessionTurnIngest`.

### `src-tauri/src/trace/mod.rs` — `build_trace_session` signature + graceful-degradation
**In scope + principled V10 expansion.** The signature change
(adding `db: &StateDb`, returning `Result`) is forced by the contract:
`count_session_turns` takes a DB handle and can return `Err`. The
counts propagate to all five `TraceSession` return paths (null
session, missing provider, no-locator, locator-available,
locator-missing, locator-error).

The interesting judgment call is the error-handling shape. Contract
§"Trace integration" says counts populate the three fields; it's
silent on what happens when `count_session_turns` itself errors. The
implementation pushes a warning and falls back to `None` counts
rather than aborting the whole trace:

> Per V10 (failures observable, never silent): a DB error counting
> turns shouldn't abort the entire trace — push a warning, fall back
> to None counts, and let the caller render the rest of the tree.
> This mirrors how locate_transcript failures are handled below.

This is the right call and is well-anchored:
- V10 requires degraded modes be observable; the warning delivers that.
- V15 (surface choice belongs to the caller) argues against aborting
  the whole trace when one session's counts fail.
- There's file-local prior art: `locate_transcript` errors already
  degrade to a warning + `TranscriptState::Missing`. Counts failing
  catastrophically while transcript-locator errors degrade gracefully
  would be inconsistent.

**Required by V10 even outside the explicit contract.** Keep.

### `src-tauri/src/trace/mod.rs` — integration test
**In scope.** `json_output_populates_sidechain_turn_count_from_session_turns`
asserts all three count fields on the JSON output, using a fixture
with one non-sidechain assistant turn and one sidechain assistant
turn — which is the minimal shape that distinguishes
`assistant_turn_count` from `sidechain_turn_count`. Good test.

### `src-tauri/tests/pr_d_claude_code_turns.rs`
**In scope.** Contract test-item 6 ("extends emission to include
parentUuid + isSidechain; parses Claude raw JSONL with sidechain
markers correctly"). End-to-end: writes a two-row JSONL fixture,
invokes the Python adapter via `python3`, deserializes emitted lines
through `ScriptTurn`, and asserts the child row's two new fields.
Exactly the integration seam worth covering with a process boundary.

## Answers to the three explicit questions

1. **Named `SessionTurnIngest` struct vs the tuple** — contract
   deviation, but the right one. The tuple would have put two
   String-typed fields adjacent with no compiler help against a
   positional swap. V14 encourages clean internal changes. Keep,
   note for the record.

2. **`count_session_turns` graceful-degradation** — required by V10
   even though the contract is silent. Aborting the entire trace on
   a DB count error would make one bad session poison the whole
   tree, contradicting V10 (observable-not-silent) and V15
   (caller chooses). The chosen pattern also matches the existing
   `locate_transcript` fallback in the same function — consistency
   alone would justify it.

3. **Migration order fix (indexes moved post-ALTER)** — necessary,
   not scope creep. Without the fix, `idx_session_turns_parent` on
   a column that doesn't yet exist in a legacy DB would fail the
   migration. Explicitly exercised by the legacy-schema migration
   test. Belongs in PR-D because PR-D is the one adding
   `parent_turn_id`.

## Things explicitly checked and clean

- Anti-pattern "Do NOT make the new columns required in ScriptTurn"
  — honored (`Option<_>` + `#[serde(default)]`).
- Anti-pattern "Do NOT change codex-turns" — honored (script
  untouched; `git diff` confirms).
- Anti-pattern "Do NOT add `is_sidechain` as a separate table" —
  honored (single `session_turns` column).
- Anti-pattern "Do NOT add new Cargo deps" — honored (no Cargo.toml
  change in the diff).
- PR-B's `sidechain_turn_count` field is now populated; `turn_count`
  and `assistant_turn_count`, which PR-B also stubbed as `None`, are
  populated in the same commit — that's consistent with how the
  underlying SQL returns all three counts together, not creep.
- No `match provider_name { "claude" => ... }` anywhere — V1/V3
  honored. The adapter-contract widening is declarative (optional
  fields) per V4.
- `source_file` and `ingested_at` positional args in the INSERT are
  preserved; no drive-by refactors of unrelated columns.

## Summary

PR-D is a tight, on-spec implementation of the smallest PR in the
trace-inspection sequence. The three deviations from the literal
contract (struct-vs-tuple, count-error graceful degradation, index
relocation) are each load-bearing and each traceable to a
`VALUES.md` value or to correctness. Nothing should block merge on
scope grounds.

Verdict: **TIGHT.**
