# WU-15-01 Phase 5 Hookpoint Research

## 1. Reuse points

### Migration shape

- Reuse `StateDb::ensure_session_turns_schema` in `src-tauri/src/state/db.rs:1017-1045`. It already owns additive `session_turns` column migrations and index re-ensuring.
- Reuse the existing column-presence helper `StateDb::session_turns_columns` in `src-tauri/src/state/db.rs:1048-1061`; it reads `PRAGMA table_info(session_turns)` and returns column names. The `body` migration should follow the same `if !columns.iter().any(...) { ALTER TABLE ... }` shape as `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary`.
- Coexistence partner: `StateDb::ensure_provider_quotas_topology_schema` in `src-tauri/src/state/db.rs:1099-1137` uses the same column-presence pattern for `provider_quotas.topology_peak_live_window_count` and `last_topology_probe_at`, then backfills. The new `session_turns.body` helper does not require ordering relative to this quota migration beyond staying in `StateDb::open`'s existing ensure-helper sequence.

### Schema location

- Fresh DB path is `CREATE TABLE IF NOT EXISTS session_turns` in `src-tauri/src/state/db.rs:628-641`.
- Add `body TEXT` to that statement. Otherwise fresh DBs would rely on a post-create `ALTER TABLE`, which is avoidable and diverges from the approved fresh-schema contract.
- Keep `source_file TEXT NOT NULL`, `ingested_at TEXT NOT NULL`, and the `UNIQUE (provider_name, session_id, turn_id)` constraint in place.

### Ingest pipeline

- `sessions::scan_provider` in `src-tauri/src/sessions/mod.rs:60-141` is the ingest entry point.
- `ScriptTurn` in `src-tauri/src/sessions/mod.rs:32-45` is the wire record. Extend it with `#[serde(default)] body: Option<serde_json::Value>` per proposal.
- The mapping site is `src-tauri/src/sessions/mod.rs:97-123`, where parsed `ScriptTurn` becomes `SessionTurnIngest`. Serialize adapter-emitted `body` to compact JSON text here before constructing `SessionTurnIngest`.
- `SessionTurnIngest` is defined in `src-tauri/src/state/db.rs:193-201`; add `pub body: Option<String>`.
- SQL write sites:
  - Single-turn insert: `StateDb::ingest_session_turn` at `src-tauri/src/state/db.rs:2555-2583`.
  - Bulk insert: `StateDb::ingest_session_turns_batch` at `src-tauri/src/state/db.rs:2590-2647`.
- Preserve both sites' `INSERT OR IGNORE` behavior. Do not convert duplicates into updates.

### Export normalizers

- Canonical record types already exist in `src-tauri/src/session_export/mod.rs:8-34`:
  - `CanonicalRecord.content: Vec<ContentChunk>` at `:15`.
  - `ContentChunk { type, text }` at `:20-24`.
  - `RecordSource` at `:26-34`.
- Claude extractor: `extract_claude_content` in `src-tauri/src/session_export/mod.rs:405-415`, backed by `extract_content_chunks` in `:417-448`.
- Codex mapping: `parse_codex_rollout_jsonl_bytes` maps `payload.content` through `extract_content_chunks(payload.get("content"))` at `src-tauri/src/session_export/mod.rs:240-266`; `canonical_chunk_type` maps `input_text` / `output_text` to `text` at `:450-455`.
- Reference adapter scripts can produce the same canonical chunk shape directly:
  - `scripts/claude-code-turns` already parses Claude JSONL in Python at `:57-86`.
  - `scripts/codex-turns` already parses Codex rollout JSONL in Python at `:56-87`.
- Recommendation for Phase 6: adapter scripts normalize provider-native content into canonical chunk arrays; Rust ingest serializes and stores the raw `body` JSON value, and may validate parseability, but should not grow a second provider-native normalization pipeline inside `scan_provider`.

### Canonical record serializer

- `canonical_jsonl_bytes` is in `src-tauri/src/session_export/mod.rs:114-124`.
- Import-replace already funnels canonical bytes through this function via `session_replace::canonical_jsonl_bytes` in `src-tauri/src/session_replace/mod.rs:996-997`, and uses it for export/preimage/postimage hashes at `:723-729`, `:943-953`.
- `run_session_export` currently duplicates serialization manually in `src-tauri/src/main.rs:759-770`. The DB-fallback export path should use `session_export::canonical_jsonl_bytes` for stdout too, so JSONL-present and DB-fallback records share one byte-stability path.

### Trace transcript shape

- `CanonicalRecord` and `ContentChunk` are public in `src-tauri/src/session_export/mod.rs:8-24`.
- `src-tauri/src/trace/mod.rs` does not currently import or expose those types. It only imports `TranscriptState`, `locate_transcript`, and state types at `src-tauri/src/trace/mod.rs:7-10`.
- Trace can either import `ContentChunk` from `session_export` or define a trace-local serializable transcript struct containing `content: Option<Vec<ContentChunk>>`. Do not duplicate the chunk schema itself; reuse `ContentChunk` for the actual content array.

## 2. Extension points

### Schema migration

- Host the migration in `StateDb::ensure_session_turns_schema` at `src-tauri/src/state/db.rs:1017-1045`.
- Best shape: extend this helper with a `body` column presence check, not a new top-level `StateDb::open` call. This keeps all `session_turns` additive migrations beside the existing `PRAGMA table_info(session_turns)` helper.
- Add `body TEXT` to the fresh schema in `src-tauri/src/state/db.rs:628-641`.

### Ingest body field flow

- Add `ScriptTurn.body` in `src-tauri/src/sessions/mod.rs:32-45`.
- At `scan_provider` mapping in `src-tauri/src/sessions/mod.rs:97-123`, convert `turn.body` with `serde_json::to_string(&body_value)` into `Option<String>`.
- On serialization failure, add a scan error naming provider/line and skip or null that row per the approved proposal's malformed-body diagnostics; missing `body` remains accepted data.
- Add `body` binding to the bulk SQL at `src-tauri/src/state/db.rs:2607-2620` and the params at `:2623-2639`.

### Export DB fallback

- Branch in `read_canonical_transcript` immediately around `fs::read(&metadata.jsonl_path)` in `src-tauri/src/session_export/mod.rs:88-97`.
- Current behavior returns `ExportError::Operational` on any read failure. New behavior should keep JSONL-first semantics when read succeeds, and only fall back to DB rows on read failure.
- `metadata.jsonl_path` is not guaranteed absolute in the current export path. `resolve_export_session_metadata` calls `sessions::locate_transcript` directly in `src-tauri/src/main.rs:855-864`; `locate_transcript` returns `PathBuf::from(line)` at `src-tauri/src/sessions/mod.rs:194-197` with no absolute/canonical check. It cannot be empty because empty lines are filtered and empty stdout errors, but it can be relative and is not a sentinel today.
- DB fallback query key is already available: `ExportSessionMetadata` is defined in `src-tauri/src/session_export/mod.rs:54-61` and carries `session_id` and `provider_name`. `resolve_export_session_metadata` constructs those at `src-tauri/src/main.rs:866-872`.
- Missing piece: `ExportSessionMetadata` does not carry a DB connection/path. The least invasive implementation can open the default state DB from inside the fallback reader, matching CLI export use. Component tests that call `read_canonical_transcript(&metadata)` directly will need fixture setup through the default data root or a helper if Phase 6 chooses not to add a new parameter shape.

### Export metadata struct

- The struct is actually in `src-tauri/src/session_export/mod.rs:54-61`; `src-tauri/src/main.rs:774-872` is the resolver and construction site.
- It carries `(provider_name, session_id)` already. No new provider/session threading is needed for the DB fallback query.
- It does not carry `chain_id`-active segment row id, but DB fallback only needs provider/session unless the implementation chooses source sentinels keyed to chain/segment. Do not require that.

### Trace `transcript` field

- Serialization boundary is the `#[derive(Serialize)]` on `TraceReport` / `TraceNode` in `src-tauri/src/trace/mod.rs:24-40`, consumed by `serde_json::to_string_pretty(&report)` in `src-tauri/src/main.rs` trace output path and by tests that serialize `TraceReport` directly.
- Change `TraceNode.transcript: Option<()>` at `src-tauri/src/trace/mod.rs:33-40` to a typed field such as `Option<Vec<TraceTranscriptTurn>>`.
- Populate it at `src-tauri/src/trace/mod.rs:134-160`, replacing `options.inline_transcript.then_some(())`.
- Keep this isolated to `src-tauri/src/trace/mod.rs`; no frontend consumer was found.

### Import-replace transaction

- Existing transaction is `replace_db_turns` in `src-tauri/src/session_replace/mod.rs:865-928`.
- Replacement source is `CanonicalRecord`, re-exported at `src-tauri/src/session_replace/mod.rs:20` and parsed from canonical JSONL at `:430-434` / `:732-745`. There is no separate `ReplaceRecord`.
- `CanonicalRecord` carries body bytes as `content: Vec<ContentChunk>` in `src-tauri/src/session_export/mod.rs:15`, so replacement inserts can serialize `record.content` to compact JSON for `body`.
- Add `body` to the insert column-set at `src-tauri/src/session_replace/mod.rs:887-890` and bind serialized `record.content` in `:891-899`.

## 3. Conflicting systems

### Inline `state::db` legacy schema tests

- `legacy_session_turns_db` in `src-tauri/src/state/db.rs:4057-4075` creates a legacy `session_turns` schema without parent/sidechain/compaction/body.
  - Recommendation: leave the fixture legacy-shaped; update the migration assertion test to expect `body TEXT` added by the helper.
- `pre_chain_db_with_turns` in `src-tauri/src/state/db.rs:6709-6725` creates another pre-chain legacy schema, with inserts at `:6745-6752`.
  - Recommendation: leave it legacy-shaped so migration coverage remains valuable; only adjust post-open expectations if tests inspect the full schema.
- Fresh-schema test `session_turns_schema_creation_includes_sidechain_columns` at `src-tauri/src/state/db.rs:4600-4614` should be extended to assert `body TEXT`.
- Tests constructing `SessionTurnIngest` in `src-tauri/src/state/db.rs:3473-3491`, `:5919-5933`, `:5956-6008` need `body: None` unless they are testing body persistence.

### Required external test fixtures

- `src-tauri/tests/fixtures/initiative_06.rs:292-306` seeds `session_turns` without `body`.
  - Explicit column list means this remains valid with a nullable column. No update needed unless a new assertion depends on body content.
- `src-tauri/tests/fixtures/initiative_06_export.rs:23-26` defines Claude JSONL lines, not DB rows.
  - No body DB fixture update needed for JSONL-first tests. New DB-fallback export tests should seed `body`.
- `src-tauri/tests/fixtures/initiative_06_import_replace.rs:59-69` defines `TurnRow` without `body`, and `:291-317` seeds old rows without `body`.
  - Existing mutation/read-only snapshots can ignore `body`; nullable `NULL` works. Add `body` to `TurnRow` only for tests that verify replacement body persistence.
- `src-tauri/tests/fixtures/initiative_06_import_replace.rs:444-452` selects turn snapshot columns and should remain metadata-only unless a new body-specific assertion needs it.

### Schema-probe fixture

- `src-tauri/tests/fixtures/initiative_06_schema_probe.rs:226-255` creates a v3 schema-probe DB with `PRAGMA user_version = 3` and metadata-only `session_turns`.
- `required_columns` in the same fixture lists only `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` for `session_turns` at `src-tauri/tests/fixtures/initiative_06_schema_probe.rs:311-325`.
- Production `schema_probe::CURRENT_SCHEMA_VERSION` remains `3` at `src-tauri/src/schema_probe/mod.rs:7`; production required columns likewise omit `body` at `src-tauri/src/schema_probe/mod.rs:217-231`.
- Confirmed: the no-bump decision keeps this fixture valid. Do not add `body` to schema-probe fixture/required columns unless the approved proposal is revised to make `body` a schema-probe compatibility feature.

### `source_file` and `ingested_at`

- These stay. Fresh schema has both at `src-tauri/src/state/db.rs:638-639`.
- Single-turn insert includes both at `src-tauri/src/state/db.rs:2569-2580`.
- Bulk insert includes both, with `source_file = ''`, at `src-tauri/src/state/db.rs:2607-2620`.
- Import-replace insert includes both at `src-tauri/src/session_replace/mod.rs:887-899`.
- Implementation should add `body` to the column sets, not rewrite them to a reduced body-only shape.

### `scan_provider` duplicate semantics

- Duplicate behavior is currently enforced by `INSERT OR IGNORE` at `src-tauri/src/state/db.rs:2607-2620` plus the unique constraint at `:640`.
- Existing test `duplicate_turns_are_idempotent_per_unique_constraint` in `src-tauri/src/sessions/mod.rs:363-375` depends on that behavior.
- The proposal's decision means a later duplicate with a body must not update a pre-existing metadata-only row. Add a body-aware test for this if Phase 6 wants to lock the decision.

### `agents trace` consumers

- No frontend/tool consumer was found that relies on `transcript: null`.
- Existing tests do rely on the placeholder and must be updated:
  - `src-tauri/src/trace/mod.rs:1046-1066` asserts every inline transcript field is null.
  - `src-tauri/tests/pr_b_trace_integration.rs:181-193` asserts CLI `--json --inline-transcript` returns null payloads.
- README also documents null placeholder at `README.md:447-451`.

### Compile-surface delta

- Adding a required `body` field to `SessionTurnIngest` touches constructors the problem map did not list:
  - `src-tauri/src/balancer/mod.rs:959-976` test helper.
  - `src-tauri/tests/initiative_05_migration.rs:244-260`.
  - `src-tauri/tests/routing_fanout_rca/mod.rs:59-70`.
  - `src-tauri/tests/pr_f_resume_integration.rs:308-323`.
  - `src-tauri/src/trace/mod.rs:616-650` and `:1244-1278`.
- These should receive `body: None`. This is a compile/test-fixture sweep, not a routing/balancer behavior change.

## 4. Deletion candidates

- Delete/replace `options.inline_transcript.then_some(())` in `src-tauri/src/trace/mod.rs:159`; it is the core null placeholder.
- Replace the README placeholder line at `README.md:450`, which says `--inline-transcript` is "null in this version".
- Replace the null-placeholder tests rather than preserving compatibility:
  - `src-tauri/src/trace/mod.rs:1046-1066`.
  - `src-tauri/tests/pr_b_trace_integration.rs:181-193`.
- Remove duplicate CLI export serialization loop in `src-tauri/src/main.rs:759-770` in favor of `session_export::canonical_jsonl_bytes`. This is not dead code today, but once DB fallback exists it is the wrong parallel serializer.
- RCA harness helper `RcaFixture::add_contract_body_column` in `src-tauri/tests/empty_bodies_ref_rca/mod.rs:67-84` manually adds a `content` column. After the real `body` migration lands, update or remove this helper rather than carrying a parallel test-only body schema.
- No additional product dead code was found that body storage obviously obsoletes.

## 5. Open questions answered

### Source-block sentinel

Answer: use existing fields with a DB sentinel, not a new field.

- Recommended shape: `RecordSource.storage_type = "state_db"` and `RecordSource.jsonl_path = PathBuf::from("db://session_turns/<row-id>")`.
- This is closest to option (a), with the existing `storage_type` string used as intended. It preserves the canonical-record wire schema in `src-tauri/src/session_export/mod.rs:26-34`.
- Avoid option (b): an empty path plus a new enum/field changes the wire schema and makes provenance less deterministic.

### Schema version bump

Confirmed no bump.

- `StateDb::open` does not read/write `PRAGMA user_version`; it runs idempotent schema helpers in `src-tauri/src/state/db.rs:506-676`.
- Production schema probe owns `CURRENT_SCHEMA_VERSION = 3` at `src-tauri/src/schema_probe/mod.rs:7` and compatibility is based on `PRAGMA user_version` plus required table/column/index maps at `src-tauri/src/schema_probe/mod.rs:95-151`.
- Required `session_turns` columns in schema probe are currently `parent_turn_id`, `is_sidechain`, and `is_compaction_boundary` at `src-tauri/src/schema_probe/mod.rs:217-231`.
- No production caller requires body to be a schema-version-4 feature. `session_replace` has its own read-only column check at `src-tauri/src/session_replace/mod.rs:336-382`, then later opens `StateDb` through metadata resolution, which can run the body migration before replacement insert.

### `session_replace` body diff

Confirmed no-diff.

- Replace parses canonical input at `src-tauri/src/session_replace/mod.rs:430-434` and validates/render-checks that input before mutation at `:431-434`, `:557-568`.
- Preimage mismatch is checked by canonical hash before transcript mutation at `src-tauri/src/session_replace/mod.rs:527-533`.
- Provider-file canonical hashes use `canonical_jsonl_bytes` at `src-tauri/src/session_replace/mod.rs:943-953`.
- Existing DB replacement intentionally resets metadata from canonical v1 records at `src-tauri/src/session_replace/mod.rs:883-884`; it does not diff old DB rows. Adding body as `record.content` is consistent with that receipt/preimage model.

### Adapter body normalization

Answer: normalize in adapter scripts; Rust ingest serializes/validates.

- Existing Rust normalizers are private to `session_export` and provider-file parsing.
- Existing Python adapters already parse provider-native records for summary fields.
- Keeping provider-native body extraction in the adapters avoids creating a second provider parser inside `scan_provider` and matches the approved raw-JSON `body` wire contract.

### Export NULL-body behavior

Confirmed error on `NULL` body when JSONL is missing.

- AC-3 is DB fallback when DB-stored bodies exist, not partial export from legacy unknown bodies.
- Existing export error semantics are explicit operational/malformed failures on missing/unreadable/invalid transcript at `src-tauri/src/session_export/mod.rs:88-97` and malformed provider lines at `:311-355`.
- Erroring on `NULL` avoids silently exporting an incomplete canonical transcript. Trace can represent mixed legacy/new rows with `body_state`; export should not skip legacy rows.

## 6. Touched-surface delta vs problem map

- The problem map is materially correct for the approved implementation. No return to Phase 2.5 is needed.
- Missed compile/test touch: `SessionTurnIngest` constructors outside the listed body path need `body: None`, especially `src-tauri/src/balancer/mod.rs:959-976` and older test fixtures. This does not alter the approved behavior or assumptions.
- Slight overstatement/mislocation: the prompt refers to `ExportSessionMetadata` at `src-tauri/src/main.rs:774-872`; the type is actually defined in `src-tauri/src/session_export/mod.rs:54-61`, while main constructs it at `src-tauri/src/main.rs:866-872`.
- Additional reuse point: CLI export currently bypasses `canonical_jsonl_bytes` with a manual serializer in `src-tauri/src/main.rs:759-770`. The problem map noted canonical serialization generally but did not flag this duplicate serializer as a deletion/reuse candidate.
- Additional conflict checkpoint: `session_replace::probe_state_schema_compatible` at `src-tauri/src/session_replace/mod.rs:336-382` is a read-only preflight that runs before `StateDb::open_default` in replacement metadata resolution. This is compatible with the no-bump decision because it need not require `body`; the later `StateDb` open can migrate before insert.
