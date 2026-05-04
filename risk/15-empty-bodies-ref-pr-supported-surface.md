# WU-15-01 — Phase 8 PR Supported-Surface Risk

Scope: re-runs the supported-surface gate on `git diff main..HEAD`
(commits 242cb87, 8c35a6d, 5f14c22) on branch `impl/wu-15-01`,
not on the proposal text.

## 1. Termination signal

`NONE`.

### Assumption-invalidation check on the actual diff

- A1 (user override supersedes proposals 01 / 06 for body-storage
  purposes): not invalidated. The supersession is recorded in
  `DECISIONS.md` D-012 (and adjacent D-013 Phase 0 provenance,
  D-014 Phase 2.5 skip rationale) per the AC-8 obligation. Per
  Phase 8 constraints this is the WU's value statement, not
  termination grounds.
- A2 (idempotent column-presence migration; coexists with WU-13
  topology migration; no `user_version` bump): not invalidated.
  `src-tauri/src/state/db.rs:1042-1046` adds `body` only when
  `PRAGMA table_info(session_turns)` lacks it, beside the existing
  `is_compaction_boundary` and topology checks. New unit
  `session_turns_schema_migration_adds_nullable_body_to_legacy_db`
  asserts `body TEXT NULL` is added on a legacy v0 fixture and
  WU-13 quota topology columns
  (`topology_peak_live_window_count`, `last_topology_probe_at`)
  remain present. `schema_probe/mod.rs` is untouched and
  `CURRENT_SCHEMA_VERSION` remains `3`.
- A3 (canonical-record byte stability while JSONL is present;
  DB is fallback only): not invalidated.
  `src-tauri/src/session_export/mod.rs:90-96` keeps
  `fs::read(&metadata.jsonl_path)` as the first source and only
  enters `read_canonical_transcript_from_state_db` on `Err`. New
  test `read_canonical_transcript_keeps_jsonl_priority_when_db_body_exists`
  proves DB fallback is not used when JSONL is readable, even with
  a contradicting DB body row.
- A4 (one extra bound parameter inside the existing import-replace
  transaction): not invalidated.
  `src-tauri/src/session_replace/mod.rs:887-905` adds `body` to
  the same `tx.execute` insert column-set; receipt, lock, preimage,
  postimage, and journal flow are unchanged. New test
  `import_replace_round_trips_canonical_content_into_session_turn_bodies`
  exercises end-to-end. The schema preflight at
  `session_replace/mod.rs:378-387` adds `body` to the required
  column list; a new exit-14 test
  (`t_schema_incompatible_missing_body_column_exit_14`) confirms
  it fails closed before journal creation.
- A5 (TEXT/UTF-8 sufficient for v1): not invalidated. The DB
  column is `TEXT`; `scan_provider` serializes via
  `serde_json::to_string`. Edge-case test
  `scan_provider_persists_body_encoding_edge_cases` round-trips
  newlines, multibyte unicode, escaped JSON and control bytes.
- A6 (no consumer relies on `transcript: null`):
  not invalidated. `TraceNode.transcript` is now
  `Option<Vec<TraceTranscriptTurn>>`; per-turn
  `body_state ∈ { Stored, Missing, Invalid }` taxonomy lands in
  `src-tauri/src/trace/mod.rs:75-90` and is rendered in the
  expected JSON shape. README §`Inspecting a Run` was updated
  for the new shape (see Finding F2 below).
- A7 (raw JSON body normalizes to the existing `ContentChunk`
  shape without cross-CLI conversion): not invalidated.
  `scripts/claude-code-turns` and `scripts/codex-turns` both emit
  canonical chunk arrays via `extract_content_chunks`; ingest's
  new `is_canonical_body_shape` check at
  `src-tauri/src/sessions/mod.rs:58-69` rejects non-canonical
  shapes with a structured error and stores `NULL`, so only
  canonical chunk arrays land in the DB.
- A8 (no `schema_probe` bump): not invalidated.
  `src-tauri/src/schema_probe/mod.rs` is unchanged in the diff;
  `CURRENT_SCHEMA_VERSION = 3`. The migration is additive,
  nullable, and column-presence-driven inside `StateDb::open`,
  matching the Phase 4 commitment.

### RCA harness state on the actual diff

`tmp/scratch/wu-15-01/phase6/rc{1..4}-green-run.log` show all
four harnesses passed against the implementation. A fresh local
re-run on this Phase 8 worktree (`cargo test --test
empty_bodies_ref_rca -- --test-threads=1`) reproduces:

```
test rc1_schema_contract::session_turns_schema_has_direct_body_storage_column ... ok
test rc2_ingest_body_payload::turn_script_ingest_persists_body_payload_in_session_turns ... ok
test rc3_export_db_source::session_export_emits_db_stored_bodies_when_jsonl_is_missing ... ok
test rc4_trace_inline_transcript::trace_inline_transcript_embeds_db_stored_turn_bodies ... ok
```

All four RCA reproducers are RED→GREEN with the diff.

### Net-value check

Positive on the actual diff. The four current-state CLI failures
named in `research/12-empty-bodies-ref-rca.md` are closed:
RC-1 (schema lacks body column), RC-2 (ingest discards body
payload), RC-3 (export exits 1 when JSONL is missing), and RC-4
(`trace --inline-transcript` always serializes `null`).

## 2. Verdict

`LOW`.

## 3. Findings

### Migration footprint matches the proposal

`session_turns.body TEXT` is added by both the `CREATE TABLE`
batch (`src-tauri/src/state/db.rs:638-644`) and the column-
presence-checked `ALTER TABLE` in `ensure_session_turns_schema`
(`src-tauri/src/state/db.rs:1042-1046`). No `PRAGMA user_version`
bump and no schema-probe edit. Coexistence with the WU-13
topology migration is verified by the new legacy-DB unit test.
This matches the Phase 4 migration shape exactly.

### F1 — Export fallback widens "JSONL missing" to "JSONL Err"

`read_canonical_transcript` matches `Ok(bytes)` vs `Err(_)` and
falls back on any IO error, not strictly missing files
(`src-tauri/src/session_export/mod.rs:90-96`). The Phase 4 risk
gate framed the trigger as "absent or unreadable", so this is
within the proposal envelope. Side effect: a permissions or
filesystem error on the JSONL path now silently activates the DB
fallback instead of bubbling up the original IO error. If DB
bodies are present the user gets a successful export with
`source.storage_type = "state_db"`; if both fail, the operator
diagnostic comes from the DB-fallback path
(`src-tauri/src/session_export/mod.rs:171-179`). Bounded — same
exit-1 outcome on full failure, and no regression on the
JSONL-readable path (proven by the byte-priority test).

### F2 — README docs `body_state = "available"`, code emits `"stored"`

`README.md:453` documents `--inline-transcript` as emitting
`body_state` value `"available"`. The actual on-wire value in
`src-tauri/src/trace/mod.rs:84-90`
(`#[serde(rename_all = "snake_case")] enum { Stored, Missing,
Invalid }`) serializes as `"stored"`. The RC-4 harness and
trace integration tests both assert `"stored"`. This is a
documentation-only mismatch in user-facing prose; the on-wire
contract is consistently `"stored"|"missing"|"invalid"`. No
runtime behavior regression — but consumers who script against
the README literal will see no `"available"` rows. Worth a
follow-up doc patch; not a supported-surface block.

### F3 — Import-replace tightens preflight to require body column

`session_replace/mod.rs:378-387` adds `body` to the required
`session_turns` column list, and `t_schema_incompatible_missing_body_column_exit_14`
confirms exit-14 schema-incompatible before journal creation if
the column is missing. This is consistent with making body
authoritative and is symmetric with the migration that always
adds the column on `StateDb::open`. The Phase 4 rollback note
(nullable column survives binary revert) still holds: the only
downgrade path that breaks import-replace is one that destructively
drops the body column, which the proposal already named as an
operator action with understood data loss.

### F4 — Adjacent surfaces remain unchanged

Diff is bounded to body-bearing files plus mechanical
`body: None` field additions in pre-existing test fixtures
(initiative_05, initiative_06, pr_b/pr_f, routing_fanout_rca,
balancer tests). No `src/` frontend edits. No
`session_chains` / `session_chain_segments` schema edits. No
`session_metadata` / `session_locate` body-reader edits. No
routing, balancer, quota, lock, release-restore, or pause
behavior changes. `session_export` JSONL-priority preserves the
canonical-record wire shape from `proposals/06-export.md` for the
JSONL-readable path, and the DB-fallback records reuse the same
`canonical_jsonl_bytes` serializer with a deterministic
`source.storage_type = "state_db"` and `db://session_turns/<row_id>`
sentinel. README documents the new sentinel shape at
`README.md:516`.

### F5 — Adapter ingest path validates canonical shape

`is_canonical_body_shape` at `src-tauri/src/sessions/mod.rs:58-69`
rejects non-array bodies and non-text-string `text` chunks with a
structured `report.errors` entry; the row is still ingested but
with `body = NULL`. New test `scan_provider_rejects_non_canonical_body_shape`
verifies both reject paths land as `NULL` body and surface
`"invalid body shape"` errors. This protects export's
`Vec<ContentChunk>` parse on the DB-fallback path from poisoned
adapter input.

### F6 — Observability matches the proposal §3 commitment

Aggregate `report.errors` entries in `scan_provider` (no per-row
spam), distinct DB-fallback diagnostic in
`read_canonical_transcript_from_state_db` distinguishing
"missing body" (`Operational` error, exit 1) from "invalid body
JSON" (`Operational` error naming the row), and trace-warning
plumbing for invalid bodies in the inline transcript
(`src-tauri/src/trace/mod.rs:259-277`). Matches the Phase 4
observability commitment.

### Cargo-test parallelism caveat

R6-N01 (cargo-test parallelism interacting with `XDG_*` env
mutation) is preserved by the `test_support::env_lock` helper
extracted to `src-tauri/src/lib.rs:18-26` and used by every
new test. Single-threaded passes 100%. Per Phase 8 constraints
this is test infra, not a behavior regression — not a block.

## 4. LOW + NONE justification

The diff implements the Phase 4-cleared design exactly:
nullable additive `body TEXT` migrated by column-presence,
JSONL-first export with deterministic DB fallback, one extra
bound parameter in the existing import-replace transaction, and
a `body_state`-tagged trace shape — with all eight A1..A8
assumptions intact, RC1..RC4 harnesses GREEN on the current
diff, and adjacent supported surfaces (chains, segments, locate,
metadata, quota, routing, lock, frontend) untouched.
