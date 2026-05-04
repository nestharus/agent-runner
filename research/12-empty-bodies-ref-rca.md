# RCA: state.db lost direct body storage / JSONL reference fallback

## Symptom

Observed live state, per the orchestrator snapshot in the Phase 0 brief:

- `session_turns` has 900,868 rows and ingest is still active.
- The current schema has metadata columns only: provider, session, turn id,
  timestamp, role, `source_file`, parent/sidechain/compaction flags, and
  ingestion timestamp.
- There is no `body`, `content`, or `payload` column.
- Recent Codex rows encode the JSONL reference in `turn_id` as
  `<jsonl_path>:<line>` while `source_file` is empty.

The intended design source for this RCA is the user's Phase 0 framing: the DB
was supposed to store message bodies directly for every turn, plus metadata
for provider/session/turn, parentage, sidechains, compaction boundaries, and
chain segments. The current README is not treated as the contract because the
brief explicitly says it reflects the regressed state.

What is broken now:

- SQLite cannot answer "what was said in this turn?" without dereferencing a
  provider JSONL file.
- `source_file` is commonly empty on batch ingestion, so even the metadata-only
  model relies on encoded `turn_id` conventions for Codex-like rows.
- `trace --json --inline-transcript` serializes `transcript: null`, not turn
  bodies.
- `session export` and `session import-replace` use provider JSONL as the body
  source and maintain `session_turns` as derived metadata.

What appears never shipped:

- I found no commit where `session_turns` or a sibling state table stored full
  turn bodies directly.
- I found no migration that added a direct body/content/payload column.
- The prior repo proposals I found mostly documented the opposite design: raw
  provider logs were the transcript-content source of truth, and SQLite held
  metadata.

## Design Intent

Primary design-intent source for this RCA:

- User Phase 0 brief: `state.db` should store message bodies directly and must
  not point at provider JSONL files as the primary mechanism.

Repo/global artifacts found during archaeology:

- `initiatives/05-session-migration.md:11-12` records the user framing, "We
  have all of the turns at this point in the database, right?" This supports
  the user's memory that DB-backed turns were assumed, but it does not specify
  a body column.
- `initiatives/05-session-migration.md:60-68` records the compaction intent:
  compacted sessions should preserve the original pre-compaction form so logs
  can be searched accurately. Without DB bodies, the current state can only
  preserve metadata and relies on provider JSONL for pre-compaction content.
- `initiatives/05-session-migration.md:93-97` scopes
  `session_turns.is_compaction_boundary`, but not direct body storage.
- `~/ai/initiatives/06-agent-runner-session-resume.md:74` requires resume
  lookup not to depend solely on raw provider transcript files when
  `session_turns` has the needed mapping. That is metadata/ownership language,
  not a direct body-storage contract.

Contradicting repo artifacts:

- `proposals/01-trace-inspection.md:1-4` says trace "dereferences transcripts
  lazily from raw CLI logs" and keeps transcript content out of SQLite.
- `proposals/01-trace-inspection.md:221-224` says trace uses SQLite only for
  structure and pointer-first JSON.
- `proposals/01-trace-inspection.md:300-306` says SQLite holds IDs, edges,
  timestamps, and capture metadata while raw logs remain the source of truth
  for transcript content.
- `proposals/06-export.md:27-33` explicitly says export does not reconstruct
  content from `session_turns`.
- `proposals/06-export.md:48-51` assumption A3 states the canonical transcript
  source is the provider JSONL path, not `session_turns`.
- `research/06-export-problem-map.md:55-56` says requested export content
  cannot be reconstructed from `session_turns` and that batch-ingested turns
  usually lose `source_file`.
- `research/06-export-problem-map.md:106` says no current migration adds
  content, line numbers, byte offsets, byte ranges, or hashes.
- `proposals/06-import-replace.md:113-117` says canonical input is the export
  record family, not `session_turns`, and that hashes are over canonical
  transcript bytes, not summary rows.

Finding: I could not find a prior checked-in design artifact that makes DB
body storage the authoritative contract. The strongest checked-in evidence
before this RCA points to a metadata-only implementation being intentional in
the trace/export/import-replace initiatives. This RCA therefore treats the
Phase 0 brief as the new authoritative design-intent source and classifies the
code/history state as `Hypothesis: feature-never-shipped`, not as a confirmed
post-ship deletion of a body column.

## Regression Window

Earliest shipped `session_turns` schema found:

- `3775d6f` (`feat: density-based multi-window balancing + CLI session
  ingestion`) introduced `session_turns` with `provider_name`, `session_id`,
  `turn_id`, `timestamp`, `role`, `source_file`, and `ingested_at`. No body,
  content, or payload column was present.

Later relevant commits:

- `3c6923c` added invocation IDs; `session_turns` remained metadata/source-file
  only.
- `9a96268` added `parent_turn_id` and `is_sidechain`.
- `91403a0` added chain tables and `is_compaction_boundary`.
- `21c67f7` added live-QA hotfixes and compaction backfill; no body storage.
- `8635dd1` added `session export`; the contract and implementation read
  provider JSONL.
- `941e6e8` added `session import-replace`; it rewrites provider JSONL and
  replaces derived `session_turns` metadata.
- `39ed3f5` (Initiative B, later reverted) added repository abstractions.
  `src-tauri/src/state/repository.rs` had `SessionTurnReplacement` with
  `source_file` and turn metadata, and its insert wrote `provider_name`,
  `session_id`, `turn_id`, `timestamp`, `role`, lineage flags, `source_file`,
  and `ingested_at`; no body storage.
- `b324175` reverted `39ed3f5`. It removed repository abstractions but did not
  remove a body column or body table because Initiative B did not add one.

Earliest commit where bodies were stored in DB: none found.

Latest commit where bodies were stored in DB: none found.

Removal SHA: none found. `b324175` is not the body-storage removal commit based
on the inspected history.

## Root Causes

### RC-1 — Feature-never-shipped: schema has no direct body column

Mechanism: `session_turns` schema has no `body`, `content`, or `payload`
column, and historical schema walks show that was true from the first
`session_turns` commit.

Evidence:

- Current fresh schema: `src-tauri/src/state/db.rs:624-637`.
- Current ingest structs: `src-tauri/src/state/db.rs:175-199`.
- Historical introduction: `3775d6f:src-tauri/src/state/db.rs` created
  `session_turns` with metadata/source-file columns only.
- Later migrations added parent/sidechain/compaction columns, not bodies:
  `src-tauri/src/state/db.rs:1012-1038`.

Classification: unshipped feature relative to the Phase 0 design intent.

### RC-2 — Ingestion boundary drops body payloads before SQLite

Mechanism: the turn-script contract and structs model only normalized summary
fields. Serde ignores extra JSON fields such as `content`, so even if an
adapter emits bodies today, they are discarded before `SessionTurnIngest`.
The batch insert then writes only metadata and `source_file = ''`.

Evidence:

- Script contract lists summary fields only:
  `src-tauri/src/sessions/mod.rs:8-18`.
- `ScriptTurn` has no body/content/payload field:
  `src-tauri/src/sessions/mod.rs:32-45`.
- Parse-to-ingest mapping omits extra payloads:
  `src-tauri/src/sessions/mod.rs:88-123`.
- `SessionTurnIngest` has no body/content/payload field:
  `src-tauri/src/state/db.rs:187-199`.
- Batch insert hard-codes an empty `source_file` and no body column:
  `src-tauri/src/state/db.rs:2503-2533`.

Classification: unshipped ingestion contract for direct bodies.

### RC-3 — Export consumes provider JSONL as the body source

Mechanism: `agents session export` resolves provider/session ownership from
state, calls the transcript locator to get a JSONL path, then
`read_canonical_transcript` reads that file from disk. It does not read body
payloads from SQLite.

Evidence:

- Export command calls `read_canonical_transcript(&metadata)`:
  `src-tauri/src/main.rs:725-756`.
- Metadata resolution calls `locate_transcript`:
  `src-tauri/src/main.rs:833-871`.
- Reader starts with `fs::read(&metadata.jsonl_path)`:
  `src-tauri/src/session_export/mod.rs:88-98`.
- Canonical records contain `content`, but that content is extracted from the
  provider line, not DB: `src-tauri/src/session_export/mod.rs:169-186`.

Classification: shipped JSONL-reference consumer behavior conflicting with
the Phase 0 design intent.

### RC-4 — Trace inline transcript is a placeholder, not a DB body reader

Mechanism: `TraceNode.transcript` is `Option<()>`; inline mode serializes
`Some(())` as JSON `null`. No body reader exists behind
`--inline-transcript`.

Evidence:

- `TraceNode.transcript: Option<()>`: `src-tauri/src/trace/mod.rs:33-38`.
- Inline mode sets `options.inline_transcript.then_some(())`:
  `src-tauri/src/trace/mod.rs:152-160`.
- Existing test asserts null placeholder:
  `src-tauri/src/trace/mod.rs:1046-1062`.

Classification: shipped placeholder consumer behavior conflicting with the
Phase 0 design intent.

## Files Involved

Source evidence:

- `src-tauri/src/state/db.rs`
- `src-tauri/src/sessions/mod.rs`
- `src-tauri/src/session_export/mod.rs`
- `src-tauri/src/session_replace/mod.rs`
- `src-tauri/src/trace/mod.rs`
- `src-tauri/src/main.rs`
- `scripts/claude-code-turns`
- `scripts/codex-turns`
- `proposals/01-trace-inspection.md`
- `proposals/05-session-migration.md`
- `proposals/06-export.md`
- `proposals/06-import-replace.md`
- `research/06-export-problem-map.md`
- `research/06-import-replace-hookpoints.md`
- `~/ai/initiatives/04-agent-questions-and-resumption.md`
- `~/ai/initiatives/06-agent-runner-session-resume.md`

Reproduction harnesses:

- `src-tauri/tests/empty_bodies_ref_rca.rs`
- `src-tauri/tests/empty_bodies_ref_rca/mod.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`

## Reproduction

Each harness asserts the expected post-design behavior and runs RED at
`b324175` / branch `rca/empty-bodies-ref`.

### RC-1 harness

Path:
`src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`

Command:

```bash
cd src-tauri && cargo test --test empty_bodies_ref_rca session_turns_schema_has_direct_body_storage_column 2>&1 \
  | tee ../.tmp/rc1-red-run.log
```

Verbatim red-run failure block:

```text
running 1 test
test empty_bodies_ref_rca::rc1_schema_contract::session_turns_schema_has_direct_body_storage_column ... FAILED

failures:

---- empty_bodies_ref_rca::rc1_schema_contract::session_turns_schema_has_direct_body_storage_column stdout ----

thread 'empty_bodies_ref_rca::rc1_schema_contract::session_turns_schema_has_direct_body_storage_column' (88488) panicked at tests/empty_bodies_ref_rca/rc1_schema_contract.rs:14:5:
session_turns must include a direct turn body column named body/content/payload; actual columns: ["id", "provider_name", "session_id", "turn_id", "timestamp", "role", "parent_turn_id", "is_sidechain", "is_compaction_boundary", "source_file", "ingested_at"]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    empty_bodies_ref_rca::rc1_schema_contract::session_turns_schema_has_direct_body_storage_column

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.16s

error: test failed, to rerun pass `--test empty_bodies_ref_rca`
```

### RC-2 harness

Path:
`src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`

Command:

```bash
cd src-tauri && cargo test --test empty_bodies_ref_rca turn_script_ingest_persists_body_payload_in_session_turns 2>&1 \
  | tee ../.tmp/rc2-red-run.log
```

Verbatim red-run output:

```text
running 1 test
test empty_bodies_ref_rca::rc2_ingest_body_payload::turn_script_ingest_persists_body_payload_in_session_turns ... FAILED

failures:

---- empty_bodies_ref_rca::rc2_ingest_body_payload::turn_script_ingest_persists_body_payload_in_session_turns stdout ----

thread 'empty_bodies_ref_rca::rc2_ingest_body_payload::turn_script_ingest_persists_body_payload_in_session_turns' (89692) panicked at tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs:43:10:
ingested turn body must be stored in state.db: "body column must be queryable from session_turns: no such column: content in SELECT content FROM session_turns\n                 WHERE provider_name = ?1 AND session_id = ?2 AND turn_id = ?3 at offset 7"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    empty_bodies_ref_rca::rc2_ingest_body_payload::turn_script_ingest_persists_body_payload_in_session_turns

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.22s

error: test failed, to rerun pass `--test empty_bodies_ref_rca`
```

### RC-3 harness

Path:
`src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`

Command:

```bash
cd src-tauri && cargo test --test empty_bodies_ref_rca session_export_emits_db_stored_bodies_when_jsonl_is_missing 2>&1 \
  | tee ../.tmp/rc3-red-run.log
```

Verbatim red-run output:

```text
running 1 test
test empty_bodies_ref_rca::rc3_export_db_source::session_export_emits_db_stored_bodies_when_jsonl_is_missing ... FAILED

failures:

---- empty_bodies_ref_rca::rc3_export_db_source::session_export_emits_db_stored_bodies_when_jsonl_is_missing stdout ----

thread 'empty_bodies_ref_rca::rc3_export_db_source::session_export_emits_db_stored_bodies_when_jsonl_is_missing' (90405) panicked at tests/empty_bodies_ref_rca/rc3_export_db_source.rs:21:5:
assertion `left == right` failed: Output { status: ExitStatus(unix_wait_status(256)), stdout: "", stderr: "{\"error\":{\"code\":\"operational-error\",\"message\":\"failed to read transcript /tmp/.tmpUnSih3/missing-rollout.jsonl: No such file or directory (os error 2)\"}}\n" }
  left: Some(1)
 right: Some(0)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    empty_bodies_ref_rca::rc3_export_db_source::session_export_emits_db_stored_bodies_when_jsonl_is_missing

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.26s

error: test failed, to rerun pass `--test empty_bodies_ref_rca`
```

### RC-4 harness

Path:
`src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`

Command:

```bash
cd src-tauri && cargo test --test empty_bodies_ref_rca trace_inline_transcript_embeds_db_stored_turn_bodies 2>&1 \
  | tee ../.tmp/rc4-red-run.log
```

Verbatim red-run output:

```text
running 1 test
test empty_bodies_ref_rca::rc4_trace_inline_transcript::trace_inline_transcript_embeds_db_stored_turn_bodies ... FAILED

failures:

---- empty_bodies_ref_rca::rc4_trace_inline_transcript::trace_inline_transcript_embeds_db_stored_turn_bodies stdout ----

thread 'empty_bodies_ref_rca::rc4_trace_inline_transcript::trace_inline_transcript_embeds_db_stored_turn_bodies' (91112) panicked at tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs:45:5:
inline transcript must embed DB-stored turn bodies; got null
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    empty_bodies_ref_rca::rc4_trace_inline_transcript::trace_inline_transcript_embeds_db_stored_turn_bodies

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.19s

error: test failed, to rerun pass `--test empty_bodies_ref_rca`
```

## Consumer Impact

### Resume

Current `agents resume` / `repl --resume` primarily needs ownership metadata:
session id, active provider, model inference, and provider resume strategy.
That path can route today without bodies as long as the upstream provider's
local store still has the session. If the provider JSONL/local store is gone,
the runner has no DB-stored bodies from which to reconstruct the provider
session, so the provider resume can reject the id even though `state.db` still
contains metadata.

### Export

Current export is directly broken relative to intended DB-source design. It
must locate and read provider JSONL. If the JSONL path is missing, the command
exits with an operational error even when a hypothetical DB body column has
the content, as reproduced by RC-3.

### Import-replace

Current import-replace treats canonical JSONL input as the stable public input,
renders provider-native JSONL to disk, and then replaces `session_turns` rows
as derived metadata. The current DB update explicitly discards parentage,
sidechain, and compaction metadata for imported canonical records and writes
`source_file`, not body content (`src-tauri/src/session_replace/mod.rs:883-900`).
Under the intended design, this means import-replace is mutating the wrong
source of truth.

### Trace

Current trace can show invocation/session/tree metadata and transcript
availability states. Inline transcript is a placeholder `null`, so trace
cannot answer content questions from `state.db`. RC-4 reproduces that gap.

### Compaction

Current compaction handling stores boundary flags in `session_turns` and the
export parser drops pre-summary records by scanning provider JSONL. That can
support "post-compaction export" while JSONL exists, but it cannot support the
user-stated goal of retaining/searching original pre-compaction bodies in DB.
The DB contains boundary markers without the bodies on either side of the
boundary.

### Chains

`session_chains` and `session_chain_segments` preserve logical ownership and
provider/session transitions. They do not contain body snippets or canonical
records. Ambiguous-chain diagnostics can count turns and show metadata, but
cannot disambiguate by recent content without JSONL or a future DB body store.

## JSONL Availability

I performed a narrow read-only availability spot-check of 12 recent
JSONL-encoded `turn_id` values from the live DB. `sqlite3` CLI was unavailable
in the environment, so the check used Python's stdlib sqlite module in read-only
URI mode and did not mutate the DB.

Observed result in `.tmp/live-jsonl-availability.txt`: all 12 sampled recent
Codex JSONL paths were readable. Examples included:

```text
readable	44	/home/nes/.codex/sessions/2026/05/02/rollout-2026-05-02T19-58-42-019debc6-1bcd-7a22-9c3e-ca706f1ceb9c.jsonl
readable	77	/home/nes/.codex/sessions/2026/05/02/rollout-2026-05-02T19-59-07-019debc6-7d52-7923-a044-121092715c82.jsonl
readable	67	/home/nes/.codex/sessions/2026/05/02/rollout-2026-05-02T19-59-07-019debc6-7d52-7923-a044-121092715c82.jsonl
```

Failure mode when JSONL is missing: metadata still resolves, but content
consumers fail or degrade. RC-3 demonstrates export returning exit 1 with
`failed to read transcript ... No such file or directory`. Trace can report
missing/no-locator states but cannot supply bodies. Provider resume depends on
the provider's own local store and can reject the session id.

## Open Questions

- No prior checked-in design doc was found that explicitly says
  `state.db` must store turn bodies directly. The checked-in trace/export docs
  say the opposite. Because the Phase 0 brief declares the intended design, I
  did not open NEEDS_INPUT; downstream phases should treat the brief as the
  authoritative design source unless the user points to an older artifact.
- The exact DB shape for body storage is not determined by Phase 0. The
  harnesses accept a `body`, `content`, or `payload` column at the schema
  invariant level, and use `content` JSON in consumer-level tests to express
  the intended retrievable payload.
- Historical live rows currently have no body data in DB. This RCA does not
  determine whether old bodies should be backfilled from available JSONL,
  marked unrecoverable when JSONL is missing, or handled by another migration
  policy.
- The live JSONL spot-check sampled recent Codex rows only. It did not prove
  older rows, Claude rows, rotated stores, archived logs, or compacted
  transcripts are available.
