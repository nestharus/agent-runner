# Contract — WU-15-01 empty-bodies-ref

Owner: implementation-pipeline-orchestrator (Phase 6a; orchestrator-authored)
Source:
- `proposals/15-empty-bodies-ref.md` (revised, Phase 4 LOW + NONE)
- `research/15-empty-bodies-ref-problem-map.md` (Phase 2.5)
- `research/15-empty-bodies-ref-hookpoints.md` (Phase 5)
- `research/12-empty-bodies-ref-rca.md` (Phase 0 RCA)
- `tmp/scratch/wu-15-01/ticket.md` /
  `tickets/phase-15:plans/tickets/phase-15/WU-15-01.md`

Inputs to Step 6b (test writer) and Step 6c (code writer).

This contract is the orchestrator's interface between the test agent
(Step 6b) and the code agent (Step 6c). The test agent does NOT see
the code agent's output. The code agent reads this contract, the
proposal, the hookpoints, the problem map, the RCA, and the Step 6b
output index — and only then writes product code.

---

## 1. Acceptance criteria (from ticket)

- **AC-1 / RC-1** — `session_turns` schema gains a `body TEXT NULL`
  column. The reproduction harness
  `session_turns_schema_has_direct_body_storage_column` at
  `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`
  flips RED → GREEN. A schema migration upgrades existing DBs
  in place; existing rows get `NULL`. Migration is idempotent
  and forward-compatible with the existing
  `provider_quotas.topology_peak_live_window_count` migration
  from WU-13-01.
- **AC-2 / RC-2** — Turn-script ingest writes the body bytes into
  the new column.
  `turn_script_ingest_persists_body_payload_in_session_turns` at
  `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`
  flips RED → GREEN. Adapters
  (`scripts/claude-code-turns`, `scripts/codex-turns`) emit a
  `body` raw JSON value (not stringified). The schema accepts
  NULL for legacy rows; ingest fills new rows with body content
  serialized to compact UTF-8 JSON text.
- **AC-3 / RC-3** — `agents session export` reads bodies from
  `state.db` when the on-disk JSONL is unavailable.
  `session_export_emits_db_stored_bodies_when_jsonl_is_missing`
  at `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`
  flips RED → GREEN. Export priority order: provider JSONL (if
  present) → DB-stored bodies (fallback). When JSONL is present,
  byte-identical canonical-record output is preserved.
- **AC-4 / RC-4** — `agents trace --json --inline-transcript`
  populates `transcript` per node from DB-stored bodies.
  `trace_inline_transcript_embeds_db_stored_turn_bodies` at
  `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`
  flips RED → GREEN. Legacy turns (no body) report
  `body_state: "missing"` with `content: null`. The `--inline-transcript`
  "placeholder for future" disclaimer is removed from `README.md`.
- **AC-5** — Existing tests stay green: routing-fanout, release-restore,
  session-migration, session-import-replace, session_lock_cross_platform.
  The migration is additive nullable; legacy rows with NULL body must
  not break readers (export errors with a named diagnostic when DB
  fallback is the source).
- **AC-6** — `cd src-tauri && cargo fmt --check && cargo clippy
  -- -D warnings && cargo test --no-fail-fast` all green.
  Frontend regression gates stay green: `bun run check && bunx tsc
  --noEmit && bun run test`.
- **AC-7** — `README.md` updated for bodies-in-DB:
  `§Session Ingestion`, `§Inspecting a Run`, `§Exporting a Session`,
  `§Replacing a Session Transcript` describe DB-stored bodies as
  source of truth. The `§Inspecting a Run` placeholder line for
  `--inline-transcript` is removed (`README.md:447-451`).
  `§Load Balancing` is unchanged.
- **AC-8** — `DECISIONS.md` gains three entries:
  - `D-NN — WU-15-01 design intent override`: bodies-in-DB is
    the authoritative contract; proposals 01-trace-inspection /
    06-export / 06-import-replace are superseded for body-storage
    purposes.
  - `D-NN — WU-15-01 Phase 0 done`: RCA performed pre-merge.
  - `D-NN — WU-15-01 Phase 2.5 human-gate skip`: per pre-approval
    policy.

## 2. Code surfaces (in-scope)

### Schema migration — `src-tauri/src/state/db.rs`

- Fresh-schema CREATE statement at `src-tauri/src/state/db.rs:628-641`:
  add `body TEXT` column.
- Migration helper at `ensure_session_turns_schema`
  (`src-tauri/src/state/db.rs:1017-1045`): extend the helper with a
  body-column-presence check. Use the existing
  `session_turns_columns` helper at `:1048-1061`.
- Migration is idempotent: only `ALTER TABLE session_turns ADD COLUMN
  body TEXT` when not present. No `PRAGMA user_version` change.
  Coexists with `ensure_provider_quotas_topology_schema` at
  `:1099-1137`; no ordering constraint required.
- `schema_probe::CURRENT_SCHEMA_VERSION` stays at `3`. Required-column
  list at `src-tauri/src/schema_probe/mod.rs:217-231` does NOT include
  `body`.

### Ingest — `src-tauri/src/sessions/mod.rs` + `state/db.rs`

- `ScriptTurn` (`src-tauri/src/sessions/mod.rs:32-45`) gains:
  ```rust
  #[serde(default)]
  pub body: Option<serde_json::Value>,
  ```
- Mapping site in `scan_provider` (`src-tauri/src/sessions/mod.rs:97-123`):
  serialize `turn.body` with `serde_json::to_string(&body)` to
  `Option<String>`. On serialization failure: log scan diagnostic
  naming provider/line; treat the row as having NULL body; do NOT
  error the whole scan.
- `SessionTurnIngest` (`src-tauri/src/state/db.rs:193-201`) gains:
  `pub body: Option<String>`.
- Single-turn insert (`StateDb::ingest_session_turn` at
  `src-tauri/src/state/db.rs:2555-2583`): add `body` to INSERT
  column list and binding.
- Bulk insert (`StateDb::ingest_session_turns_batch` at
  `src-tauri/src/state/db.rs:2590-2647`): add `body` to INSERT
  column list at `:2607-2620` and binding at `:2623-2639`.
- Preserve `INSERT OR IGNORE` semantics; duplicate-row test
  `duplicate_turns_are_idempotent_per_unique_constraint`
  (`src-tauri/src/sessions/mod.rs:363-375`) must remain green.
- Compile-surface sweep — add `body: None` to
  `SessionTurnIngest` constructors found by Phase 5:
  - `src-tauri/src/balancer/mod.rs:959-976` (test helper)
  - `src-tauri/tests/initiative_05_migration.rs:244-260`
  - `src-tauri/tests/routing_fanout_rca/mod.rs:59-70`
  - `src-tauri/tests/pr_f_resume_integration.rs:308-323`
  - `src-tauri/src/trace/mod.rs:616-650`, `:1244-1278`
  - inline tests in `src-tauri/src/state/db.rs:3473-3491`,
    `:5919-5933`, `:5956-6008`

### Export — `src-tauri/src/session_export/mod.rs` + `main.rs`

- `read_canonical_transcript` (`src-tauri/src/session_export/mod.rs:88-97`):
  branch around `fs::read(&metadata.jsonl_path)`. JSONL-first.
  On read failure (file missing OR unreadable), open the default
  state DB and read body-bearing rows for `(provider_name, session_id)`.
- DB fallback reader queries `session_turns` for `(provider_name,
  session_id)` ordered by `timestamp, id`, builds `CanonicalRecord`
  values from row metadata + parsed `body` JSON.
- Source block for DB-derived records (per Phase 5 R4-N03):
  - `RecordSource.storage_type = "state_db"`
  - `RecordSource.jsonl_path = PathBuf::from("db://session_turns/<row-id>")`
  - `RecordSource.line` and `RecordSource.byte_offset` reflect DB
    row position (use `id` as line, `0` as byte_offset).
  - `RecordSource.sha256` over the canonical-record bytes
    constructed from the body.
  This preserves the public canonical-record wire schema at
  `src-tauri/src/session_export/mod.rs:8-34`.
- `NULL` body handling: when DB fallback is the source AND any row
  has NULL body, return `ExportError::Operational` with a diagnostic
  naming the missing body. Do NOT silently skip legacy rows.
- CLI export at `src-tauri/src/main.rs:759-770` (per R4-N04): replace
  the manual serialization loop with `session_export::canonical_jsonl_bytes`
  so JSONL-present and DB-fallback both go through the canonical
  serializer.

### Import-replace — `src-tauri/src/session_replace/mod.rs`

- `replace_db_turns` transaction at
  `src-tauri/src/session_replace/mod.rs:865-928`:
  - Add `body` to the INSERT column-set at `:887-890`.
  - Bind `serde_json::to_string(&record.content)` for `body` at
    `:891-899`.
  - No diff against legacy rows; replacement is canonical-bytes-driven.
  - Existing receipt/lock/preimage/postimage flow unchanged.
- `probe_state_schema_compatible` at `:336-382` is a read-only
  preflight; it does NOT require `body` (schema-probe v3 unchanged).

### Trace inline transcript — `src-tauri/src/trace/mod.rs`

- Replace `TraceNode.transcript: Option<()>` at `:33-40` with
  `Option<Vec<TraceTranscriptTurn>>`.
- New struct `TraceTranscriptTurn` (in `src-tauri/src/trace/mod.rs`):
  ```rust
  #[derive(Debug, Serialize)]
  pub struct TraceTranscriptTurn {
      pub turn_id: String,
      pub role: String,
      pub timestamp: String,
      pub body_state: TraceBodyState,
      pub content: Option<Vec<ContentChunk>>,
  }

  #[derive(Debug, Serialize)]
  #[serde(rename_all = "snake_case")]
  pub enum TraceBodyState {
      Stored,
      Missing,
      Invalid,
  }
  ```
  Reuse `ContentChunk` from `src-tauri/src/session_export/mod.rs:20-24`.
- Population site at `src-tauri/src/trace/mod.rs:134-160`: replace
  `options.inline_transcript.then_some(())` with a DB read against
  `session_turns` for the node's `(provider_name, session_id)` ordered
  by `timestamp, id`. For each row:
  - body present and parses → `body_state: Stored`, `content: Some(parsed)`
  - body NULL → `body_state: Missing`, `content: None`
  - body present but parse fails → `body_state: Invalid`, `content: None`
  Trace must NOT error on parse failure; per-turn `body_state: invalid`
  is the failure mode.
- Update existing tests that assert `transcript: null`:
  - `src-tauri/src/trace/mod.rs:1046-1066`
  - `src-tauri/tests/pr_b_trace_integration.rs:181-193`

### Adapter scripts — `scripts/claude-code-turns`, `scripts/codex-turns`

- `scripts/claude-code-turns:57-86`: emit `"body": <raw-json-value>`
  for supported turns. Body shape MUST match canonical content array,
  e.g. `[{"type": "text", "text": "..."}]`. Use the same logic as
  `extract_claude_content` /`extract_content_chunks`
  (`src-tauri/src/session_export/mod.rs:405-448`) as guidance —
  Python-side normalization mirrors the Rust extractor.
- `scripts/codex-turns:56-87`: emit `"body": <raw-json-value>` for
  `response_item` message rows. Body shape from `payload.content`
  with `input_text` and `output_text` mapped to `{"type": "text",
  "text": "..."}` per `canonical_chunk_type` at
  `src-tauri/src/session_export/mod.rs:450-455`.
- Both adapters keep summary fields unchanged; `body` is added.
- Missing body (e.g., adapter cannot extract content) → omit the
  `body` key (NOT `null`); `ScriptTurn.body` `#[serde(default)]`
  handles the absence.

### Documentation — `scripts/README.md`, `README.md`

- `scripts/README.md` — document the new `body` field in the
  turn-script contract (raw JSON value matching canonical content
  shape; absent when adapter cannot extract).
- `README.md` per AC-7:
  - `§Session Ingestion` — add a sentence: turn-script adapters emit
    body bytes; `state.db` stores bodies directly.
  - `§Inspecting a Run` — replace the placeholder disclaimer at
    `README.md:447-451` with a sentence that `--inline-transcript`
    embeds DB-stored body bytes per turn; legacy turns without body
    are reported with `body_state: "missing"`.
  - `§Exporting a Session` — add: when provider JSONL is missing,
    export reads bodies from `state.db`.
  - `§Replacing a Session Transcript` — add: import-replace updates
    body bytes alongside metadata in the atomic transaction.
  - `§Load Balancing` — unchanged.

### DECISIONS.md — three new entries per AC-8

- Append `D-NN — WU-15-01 design intent override`: bodies-in-DB is
  the authoritative contract; proposals 01-trace-inspection /
  06-export / 06-import-replace are superseded for body-storage
  purposes only. Canonical-record wire shape from
  `proposals/06-export.md` remains authoritative for `agents session
  export` output.
- Append `D-NN — WU-15-01 Phase 0 done`: RCA performed pre-merge on
  `rca/empty-bodies-ref` at commit `242cb87`; reproduction harnesses
  shipped as RED on pre-fix HEAD `e9649a1`.
- Append `D-NN — WU-15-01 Phase 2.5 human-gate skip`: per the standing
  pre-approval policy from WU-11-01 / WU-13-01 / WU-14-01.

## 3. Out of scope (anti-scope; reaffirmed)

- No `session_chains` / `session_chain_segments` schema changes.
- No retroactive backfill of legacy rows from JSONL.
- No deletion of `session_turns.source_file` or other metadata columns.
- No body compression, encryption-at-rest, or deduplication.
- No multimodal payload expansion (D-002 stays).
- No BLOB storage in v1.
- No backwards-compatibility shims for metadata-only.
- No cross-CLI canonical conversion.
- No canonical-record wire schema change for `agents session export`.
- No routing/balancer/quota/release-restore/pause-handshake/session-lock
  behavior changes.
- No `src/` (frontend) changes.

## 4. Test plan (Step 6b feeds Step 6c)

The four RC harnesses are inputs to Step 6b (existing tests; flip
RED → GREEN). Step 6b carries them forward via the output index;
Step 6c does NOT modify them except to align expectations with the
new contract (e.g. assertion strings).

### T1 / RC-1 — schema contract
- File: `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`
- Risk: schema regression.
- Level: particular-integration.
- Source: `research/12-empty-bodies-ref-rca.md` RC-1.
- Acceptance: `PRAGMA table_info(session_turns)` includes `body`
  on a fresh DB AND on a legacy DB upgraded by the migration helper.
- Assumption links: A1 (override), A2 (coexistence), A8 (no bump).
- Expected signal: test passes; `body` column present.

### T2 / RC-2 — ingest body payload
- File: `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`
- Risk: ingest regression.
- Level: particular-integration.
- Source: RC-2.
- Acceptance: when turn-script emits `body: <chunk-array>`, the DB
  query returns matching body text.
- Assumption links: A1, A5 (TEXT), A7 (canonical-shape adapter).
- Expected signal: scan succeeds; `SELECT body FROM session_turns ...`
  returns the stored chunk array.

### T3 / RC-3 — export DB-source fallback
- File: `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`
- Risk: export regression.
- Level: end-to-end.
- Source: RC-3.
- Acceptance: when locator returns a non-existent JSONL path AND DB
  has body rows, `agents session export <id>` exits 0 with stdout
  containing canonical bytes including `db stored assistant body`.
- Assumption links: A1, A3 (byte-stable when JSONL present).
- Expected signal: CLI stdout contains the body content.

### T4 / RC-4 — trace inline transcript
- File: `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`
- Risk: trace regression.
- Level: particular-integration.
- Source: RC-4.
- Acceptance: `--inline-transcript` returns a non-null
  transcript array; row content matches DB body.
- Assumption links: A1, A6 (no consumer relies on null).
- Expected signal: `root.transcript[0].content[0].text == "db stored assistant body"`.

### T5 — schema migration on v0 DB
- File: extend `src-tauri/src/state/db.rs` inline tests.
- Risk: legacy-DB upgrade.
- Level: unit.
- Acceptance: opening a DB with legacy `session_turns` schema (no
  body column) adds nullable `body`; legacy rows have `NULL` body;
  WU-13-01 quota topology migration still works.
- Assumption links: A2, A8.
- Expected signal: column present after open; quota topology columns
  present and backfilled.

### T6 — ingest body encoding edge cases
- File: extend `src-tauri/src/sessions/mod.rs` inline tests OR add
  `src-tauri/tests/empty_bodies_ref_rca/` coverage.
- Risk: encoding regression.
- Level: particular-integration.
- Acceptance: body containing newlines, unicode, escaped JSON, and
  control characters round-trips as compact UTF-8 JSON text.
- Assumption links: A5, A7.
- Expected signal: `SELECT body` returns parseable JSON; chunks match
  input.

### T7 — export priority (JSONL present)
- File: extend `src-tauri/src/session_export/` inline tests OR add
  `src-tauri/tests/` coverage.
- Risk: byte-stability regression.
- Level: particular-integration.
- Acceptance: with provider JSONL readable AND DB body present,
  export output is byte-identical to JSONL-derived canonical bytes.
- Assumption links: A3.
- Expected signal: byte-equal output to a pre-WU baseline; DB body
  not consulted.

### T8 — export DB fallback (JSONL missing)
- File: extend `src-tauri/src/session_export/` inline tests OR add
  `src-tauri/tests/` coverage.
- Risk: fallback regression.
- Level: particular-integration.
- Acceptance: with JSONL absent AND DB body present, export reads
  from DB and emits canonical bytes whose `content` matches body.
- Assumption links: A3.
- Expected signal: stdout contains DB-sourced canonical record.
  `RecordSource.storage_type == "state_db"` and
  `RecordSource.jsonl_path` starts with `db://session_turns/`.

### T9 — import-replace round-trip with bodies
- File: extend
  `src-tauri/src/session_replace/` inline tests OR add to
  `src-tauri/tests/initiative_06_*.rs`.
- Risk: atomic-transaction regression.
- Level: particular-integration.
- Acceptance: import-replace writes `record.content` serialized as
  body; receipt success unchanged; chain recency fields update; body
  matches replacement records on post-replace `SELECT`.
- Assumption links: A4.
- Expected signal: receipt OK; `SELECT body FROM session_turns ...`
  matches input canonical content.

### T10 — trace mixed legacy + new rows
- File: extend `src-tauri/src/trace/mod.rs` inline tests.
- Risk: mixed-state regression.
- Level: component.
- Acceptance: rows with body have `body_state: "stored"` +
  `content: <chunks>`; legacy rows have `body_state: "missing"` +
  `content: null`.
- Assumption links: A6.
- Expected signal: per-turn JSON shape matches; no root-level
  null placeholder.

### T11 — Claude adapter body emission
- File: new test exercising `scripts/claude-code-turns` with a
  fixture Claude project tree.
- Risk: adapter regression.
- Level: particular-integration.
- Acceptance: script stdout includes `"body": [{"type":"text","text":"..."}]`
  for a Claude assistant turn.
- Assumption links: A5, A7.
- Expected signal: adapter JSON line has `body` key with a list of
  chunks.

### T12 — Codex adapter body emission
- File: new test exercising `scripts/codex-turns` with a fixture
  Codex rollout JSONL.
- Risk: adapter regression.
- Level: particular-integration.
- Acceptance: script stdout includes `"body": [{"type":"text","text":"..."}]`
  for a Codex `response_item` turn (input_text/output_text mapped to
  text).
- Assumption links: A5, A7.
- Expected signal: adapter JSON line has `body` key with a list of
  chunks.

### Residual-risk artifact

If, during Phase 6b, any named change-risk or verification-risk
listed above cannot be verified by the emitted test set, the test
writer produces `risk/15-empty-bodies-ref-test-residuals.md` per
`~/ai/workflows/implementation-pipeline.md` Phase 6b residual rule.

## 5. Fixture and infrastructure points

- `src-tauri/tests/empty_bodies_ref_rca/mod.rs` is the integration
  fixture for all four RC harnesses. The helper
  `RcaFixture::add_contract_body_column` at `:67-84` is a temporary
  test-only "shadow" body column added BEFORE the production migration
  existed. **Step 6c removes / replaces this helper** — the production
  migration provides the body column. The harnesses then use the
  production column directly.
- DB fallback in trace/export needs a default `state.db` path. The
  DB-fallback reader can use `StateDb::open_default()` (or equivalent)
  to find the active DB.
- Adapter integration tests for T11/T12 should run the actual scripts
  via `std::process::Command` against fixture provider JSONL trees,
  matching the existing `scripts/` test patterns.

## 6. Verification gates

Per AC-6:

- Rust:
  - `cd src-tauri && cargo fmt --check`
  - `cd src-tauri && cargo clippy -- -D warnings`
  - `cd src-tauri && cargo test --no-fail-fast`
- Frontend (regression check; no `src/` changes expected):
  - `bun run check`
  - `bunx tsc --noEmit`
  - `bun run test`

All gates green is the Step 6c completion bar.

## 7. Step 6b output index

Step 6b MUST produce `tmp/scratch/wu-15-01/phase6/step6b-output-index.md`
listing every test-intent item, named risk, selected level, source,
emitted test file path / test-or-test-group identifier, residual
entry path, documented non-applicability reason, and declared fixture
source. Step 6c log MUST echo the Step 6b output paths it consumed
before product-code changes.

## 8. Done definition

- Step 6b: tests written; output index produced; residual artifact
  produced if applicable.
- Step 6c: product code passes all six gates; the four RC harnesses
  flip RED → GREEN; Step 6b's output index paths are echoed at the
  top of the Step 6c log.
- Step 6c is NOT complete until `cargo test --no-fail-fast` is green
  AND `bun run test` is green.
