# 1. Design + scope + architecture + tradeoffs

WU-15-01 changes the session-turn storage contract from metadata-only rows to rows that can carry the turn body directly. The change stays inside the ticket's code boundary: `src-tauri/src/state/db.rs`, `src-tauri/src/sessions/mod.rs`, `src-tauri/src/session_export/mod.rs`, `src-tauri/src/session_replace/mod.rs`, `src-tauri/src/session_metadata/mod.rs` verification only, `src-tauri/src/trace/mod.rs`, `scripts/claude-code-turns`, `scripts/codex-turns`, `scripts/README.md`, `README.md`, and `DECISIONS.md`.

## Schema migration shape

`src-tauri/src/state/db.rs` adds `session_turns.body TEXT NULL`. The name is `body`, not `content` or `payload`, because it matches the WU slug and ticket title, is short, and avoids confusing the DB storage field with export's structured `CanonicalRecord.content` field or Codex's native `payload` wrapper. The v1 type is `TEXT` because current Claude/Codex adapters read UTF-8 JSONL and D-002 defers multimodal/binary payload expansion. `BLOB` is deferred; if D-002 reopens, valid UTF-8 `TEXT` values can be migrated into BLOB storage without inventing a compatibility shim now.

The column is nullable with no default. Legacy rows remain `NULL`; new ingest/import-replace rows write `Some(compact_json_body)`. This avoids retroactive backfill and avoids implying that an empty body and an unknown legacy body are equivalent. The current `CREATE TABLE IF NOT EXISTS session_turns` statement at `src-tauri/src/state/db.rs:628-641` includes `body TEXT`. Existing DBs are upgraded by `ensure_session_turns_schema` after the table batch. That helper already uses column-presence checks at `src-tauri/src/state/db.rs:1017-1045`; the new body migration follows the same shape:

```sql
ALTER TABLE session_turns ADD COLUMN body TEXT
```

only when `PRAGMA table_info(session_turns)` does not include `body`. This coexists with the WU-13 topology migration at `src-tauri/src/state/db.rs:1099-1137`, which also uses column-presence checks. No `PRAGMA user_version` change is proposed because `StateDb::open` does not currently manage `user_version`, and this is an idempotent additive nullable column. `src-tauri/src/schema_probe/mod.rs` does not join the code boundary; `CURRENT_SCHEMA_VERSION` remains `3`.

## Ingest write path

`src-tauri/src/sessions/mod.rs` extends the turn-script contract and `ScriptTurn` at `src-tauri/src/sessions/mod.rs:32-45` with:

```rust
#[serde(default)]
pub body: Option<serde_json::Value>,
```

The adapter JSON field name is `body`. The field shape is a raw JSON value, not a stringified JSON blob. For supported text turns the value is the same shape as export's `CanonicalRecord.content`: an ordered array of content chunks such as `[{"type":"text","text":"..."}]`. `scan_provider` serializes the raw value to compact JSON text before constructing `SessionTurnIngest`; absent body stays `None`.

`src-tauri/src/state/db.rs` extends `SessionTurnIngest` at `src-tauri/src/state/db.rs:189-201` with `pub body: Option<String>`. The single-turn insert at `src-tauri/src/state/db.rs:2555-2583` gains a body parameter and includes `body` in the insert column-set. The bulk insert at `src-tauri/src/state/db.rs:2590-2647` includes `body` in the SQL column list and binds `turn.body.as_deref()`. Duplicate rows still collapse under `UNIQUE(provider_name, session_id, turn_id)`; WU-15-01 does not add update-on-conflict behavior because changing previously ingested rows would be a separate policy choice.

The write path logs at most one aggregate scan diagnostic when script output includes turns without `body`, e.g. a provider-level count in `ScanReport.errors` only for malformed body JSON or serialization failure. Missing body is accepted data, not an ingest error, because legacy/custom adapters may emit metadata-only rows; readers handle `NULL` explicitly.

## Export read path

`src-tauri/src/session_export/mod.rs` keeps provider JSONL as the first source. `read_canonical_transcript(&metadata)` first attempts `fs::read(&metadata.jsonl_path)` and uses the existing provider parsers when that succeeds. This preserves byte-identical canonical JSONL for the supported path when the provider transcript exists.

When the JSONL path is absent or unreadable, export falls back to DB-stored bodies. The fallback is a new reader that queries `session_turns` for the resolved `(provider_name, session_id)`, ordered by `timestamp, id`, and builds `CanonicalRecord` values from row metadata plus `body`. Rows with `body IS NULL` are not silently converted to empty content; export reports an operational/malformed transcript error naming the missing body because AC-3 only promises fallback when DB-stored bodies exist.

For DB-backed rows, `body` is parsed as the canonical `Vec<ContentChunk>` shape and then serialized through the existing `canonical_jsonl_bytes` path. This gives canonical-record byte stability for DB fallback itself: JSONL-derived records and DB-derived records use the same Rust `CanonicalRecord` structs and compact serializer. The remaining source-block detail is intentionally called out for Phase 5: the proposal requires a deterministic DB source block, likely `storage_type: "state_db"` with a DB-row sentinel path, but hookpoint research must verify whether the current `RecordSource.jsonl_path: PathBuf` can safely represent that without changing the public wire schema.

## Trace inline transcript

`src-tauri/src/trace/mod.rs` changes `TraceNode.transcript: Option<()>` at `src-tauri/src/trace/mod.rs:33-40` into a real serializable type. The placeholder at `src-tauri/src/trace/mod.rs:134-160` stops using `options.inline_transcript.then_some(())`.

The proposed JSON shape is an array of per-turn objects that reuses the export content shape:

```json
[
  {
    "turn_id": "turn-assistant",
    "role": "assistant",
    "timestamp": "2026-04-17T08:00:01Z",
    "body_state": "stored",
    "content": [{"type":"text","text":"db stored assistant body"}]
  },
  {
    "turn_id": "legacy-turn",
    "role": "assistant",
    "timestamp": "2026-04-17T08:00:02Z",
    "body_state": "missing",
    "content": null
  }
]
```

`body_state` is `"stored"` when `body` is non-null and parses, and `"missing"` for legacy rows. Parse errors should surface as trace warnings and mark the individual turn `"body_state": "invalid"` with `content: null`, rather than making the whole trace unusable. This is a new trace contract, not a metadata-only compatibility shim.

## Import-replace transaction

`src-tauri/src/session_replace/mod.rs:865-928` keeps the existing transaction shape: delete old rows, insert replacement rows, refresh `session_chain_segments.last_turn_id`, refresh `session_chains.last_used_at`, then commit. The insert column-set adds `body`, populated from `CanonicalRecord.content` serialized as compact JSON. This does not weaken the existing receipt, lock, preimage, provider-file replacement, postimage verification, or journal flow because it is one more bound value inside the same SQLite transaction after the provider transcript mutation point.

`session_replace` does not read legacy body rows for diff/preimage purposes. Canonical input and provider-native rendering remain the mutation source; the body column is updated to match the replacement records after the same validation that already gates row replacement.

## Turn-script adapters

`scripts/claude-code-turns` emits `"body": <raw-json-value>` for each supported turn. For Claude, the value is normalized to the canonical content array from `message.content`, `message` string, or top-level `content`, using the same text chunk convention as `session_export::extract_claude_content`.

`scripts/codex-turns` emits `"body": <raw-json-value>` for `response_item` message rows. For Codex, the value is normalized from `payload.content` into the canonical content array, mapping `input_text` and `output_text` to `"type": "text"` as export already does.

Both scripts keep summary fields unchanged and add `body` only when body content is available. They do not emit stringified JSON; the field is an actual JSON array/object value. `scripts/README.md` documents the required field name, raw JSON shape, and `NULL` behavior for adapters that cannot provide bodies.

## README and decisions

`README.md` updates AC-7 sections only: `§Session Ingestion`, `§Inspecting a Run`, `§Exporting a Session`, and `§Replacing a Session Transcript`. `§Load Balancing` is unchanged. The docs describe DB-stored bodies as the authoritative source for turn bodies, JSONL as the preferred export source while present and a provider-native artifact for replace, and `--inline-transcript` as a real DB-backed JSON transcript rather than `null in this version`.

`DECISIONS.md` records the WU-15-01 design intent override required by AC-8: bodies-in-DB supersedes proposals 01/06 for body-storage purposes while leaving the canonical-record wire shape from `proposals/06-export.md` authoritative.

# 2. Anti-scope

- No `session_chains` / `session_chain_segments` schema changes.
- No retroactive backfill of legacy rows from provider JSONL.
- No deletion of `session_turns.source_file` or other existing metadata columns.
- No body compression, encryption-at-rest, or deduplication.
- No multimodal payload expansion; D-002 stays in force.
- No BLOB storage in v1 and no deferred helper whose only purpose is future BLOB support.
- No backwards-compatibility shims for the metadata-only contract. The new column is `NULL` for legacy rows, and readers handle that data state explicitly.
- No cross-CLI canonical conversion. Bodies are captured and replayed through the existing per-provider parser/adapter semantics.
- No canonical-record wire schema change for `agents session export`; the source of body bytes changes, not the public record family.
- No routing, balancer, quota, release-restore, pause-handshake, or session-lock behavior changes.
- No provider spawning, account selection, quota refresh, or config mutation from export/trace body reads.
- No `src/` frontend changes and no E2E UI scope.

# 3. Supported-surface track

Deployment mode: Tauri desktop plus the `agents` / `oulipoly-agent-runner` CLI using the local SQLite `state.db`.

Customer cohort: current local users running Claude Code or Codex through or alongside agent-runner, especially users relying on `agents session export`, `agents session import-replace`, and `agents trace --json --inline-transcript` after provider JSONL files move or disappear.

Adjacent public/user-reachable paths: top-level successful execution and `agents resume` ingest session turns after provider completion; `agents session export <session-id>`; `agents session import-replace <session-id>`; `agents trace <uuid> --json --inline-transcript`; `scripts/claude-code-turns`; `scripts/codex-turns`; documented adapter contracts.

Blast-radius notes for unchanged adjacent paths: `session_chains` and `session_chain_segments` remain metadata/ownership tables; import-replace continues to update their existing recency fields only. `session locate` and `session_metadata` path resolution continue to report transcript availability and workspace roots but do not become body readers. Quota and routing surfaces keep counting assistant turns; they do not inspect `body`. Frontend surfaces do not change.

Migration path: in-place, idempotent, column-presence migration in `StateDb::open`. Fresh DBs create `session_turns.body TEXT`; existing DBs add `body TEXT` if missing. No `user_version` bump and no backfill. Legacy rows keep `NULL`, and new ingest/import-replace rows populate the column when body data is available.

Rollback path: because `body` is nullable, reverting read behavior restores metadata-only export/trace behavior for old binaries. Reverting writes stops populating `body`; the physical column remains in existing DBs unless a manual destructive migration drops it. Dropping the column on rollback would discard newly captured bodies, so the safer rollback tradeoff is to leave the nullable column unused. If a release explicitly reverts schema writes and wants to remove the column, that must be a destructive operator action with understood data loss.

Observability: add concise ingest diagnostics for malformed `body` values or body serialization failures, including provider name and script line number. Missing body is not logged per row to avoid noisy scans; aggregate counts in tests are enough. Export fallback should emit an operational diagnostic path that distinguishes "provider JSONL missing, DB fallback used" from "provider JSONL missing and DB body unavailable" in errors/logs.

# 4. Assumption register

| ID | Assumption | Evidence | Invalidator |
| --- | --- | --- | --- |
| A1 | User's bodies-in-DB design intent supersedes proposals 01/06 for body-storage purposes. | Ticket "Decision binding this WU" and `tmp/scratch/wu-15-01/ticket.md:38-49`. | A counter-statement from the user. |
| A2 | `provider_quotas.topology_peak_live_window_count` migration is idempotent and column-presence-driven; the new migration follows the same shape and they coexist without ordering constraints. | `src-tauri/src/state/db.rs:1099-1137`; `StateDb::open` uses ensure helpers and does not bump `user_version`. | Discovery of an ordering constraint or a `user_version` bump in `StateDb::open` we missed. |
| A3 | `agents session export` canonical-record bytes must remain byte-identical when JSONL is present; the DB fallback only activates when JSONL is missing or unreadable. | Ticket AC-3 and `proposals/06-export.md` canonical-record contract, still authoritative for wire shape. | A revised export contract that makes DB rows the preferred source even when JSONL is readable. |
| A4 | `agents session import-replace`'s atomic transaction can incorporate one additional column in its INSERT column-set without rearchitecting the receipt/lock/preimage/postimage flow. | `src-tauri/src/session_replace/mod.rs:865-928` owns the delete/insert/update transaction. | Discovery that the transaction interleaves with a read that depends on the missing-body-column shape. |
| A5 | TEXT (UTF-8) is sufficient for v1 body bytes. | Ticket Notes; D-002 defers multimodal; current export/import and adapter paths already parse UTF-8 JSONL. | Discovery that a current adapter emits binary content already. |
| A6 | The `--inline-transcript` flag's transcript shape is not yet contractually defined elsewhere; Phase 6 chooses the JSON shape per the approved contract. | `src-tauri/src/trace/mod.rs:33-40` uses `Option<()>`; `README.md:447-451` documents `null` as a placeholder. | A downstream consumer relying on `null` as the stable contract. |
| A7 | Adapter-emitted `body` as a raw JSON value can be normalized to the existing export `ContentChunk` shape without cross-CLI conversion. | `src-tauri/src/session_export/mod.rs:405-460` already normalizes Claude/Codex text chunks into `ContentChunk`. | Provider fixtures show required body data outside text/content chunks that cannot be represented under D-002. |
| A8 | `schema_probe` can remain schema-version-3-compatible because this migration is additive, nullable, and managed by `StateDb::open` column inspection. | Problem map notes `StateDb::open` does not read/write `PRAGMA user_version`; `schema_probe` owns `CURRENT_SCHEMA_VERSION = 3` separately. | A risk gate decides body storage must be a schema-probe feature flag or compatibility predicate. |

# 5. Test-intent track

| ID | Risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- | --- |
| T1 / RC-1 | Schema regression. | `session_turns_schema_has_direct_body_storage_column` finds direct `body` storage on a fresh DB. | particular-integration | `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs`; source `research/12-empty-bodies-ref-rca.md` RC-1. | A1, A2, A8 | Test passes and `PRAGMA table_info(session_turns)` includes `body`. | Does not prove migrated legacy DB shape. |
| T2 / RC-2 | Ingest regression. | `turn_script_ingest_persists_body_payload_in_session_turns` persists adapter-emitted body payload. | particular-integration | `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`; source RC-2. | A1, A5, A7 | Scan report has one new turn and DB query returns the stored body text chunk. | Does not cover all provider-native chunk variants. |
| T3 / RC-3 | Export regression. | `session_export_emits_db_stored_bodies_when_jsonl_is_missing` exits 0 and emits body content from DB when locator points to missing JSONL. | end-to-end | `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs`; source RC-3. | A1, A3 | CLI stdout contains canonical JSONL with `db stored assistant body`. | Does not fully prove source-block sentinel semantics. |
| T4 / RC-4 | Trace regression. | `trace_inline_transcript_embeds_db_stored_turn_bodies` returns non-null transcript content from DB. | particular-integration | `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`; source RC-4. | A1, A6 | JSON `root.transcript` is an array containing `db stored assistant body`. | Does not prove human trace footer behavior. |
| T5 | Schema migration on v0 DB. | Opening a DB with legacy `session_turns` adds nullable `body` and keeps WU-13 quota topology migration intact. | unit | `state::db` inline tests using legacy schema fixture. | A2, A8 | `body` present, old rows have NULL body, quota topology columns still present/backfilled. | Does not simulate every historical SQLite file. |
| T6 | Ingest body encoding edge cases. | Body containing newlines, unicode, escaped JSON, and representative edge bytes round-trips as compact UTF-8 JSON text. | particular-integration | `sessions::scan_provider` script fixture emitting `body` raw JSON. | A5, A7 | DB `body` parses and text values match exactly. | Does not verify non-UTF-8 binary because v1 excludes it. |
| T7 | Export priority regression. | When JSONL is present, export output remains byte-identical to JSONL-derived canonical bytes even if DB has matching body. | particular-integration | `session_export` fixture with readable provider JSONL plus seeded DB body. | A3 | `canonical_jsonl_bytes` output equals pre-WU provider JSONL-derived expected bytes. | Does not exercise missing JSONL fallback. |
| T8 | Export DB fallback. | When JSONL is absent/unreadable and DB body is present, DB body is the source and export succeeds. | particular-integration | `session_export` fixture with missing path and body-bearing `session_turns` rows. | A3 | Export returns records whose `content` matches DB `body`. | Source-block sentinel detail remains a Phase 5 verification point. |
| T9 | Import-replace round-trip with bodies. | Import-replace writes replacement `body` values in the same DB transaction while preserving existing receipt/lock/preimage/postimage flow. | particular-integration | `session_replace` fixture using canonical input and post-replace DB query. | A4 | Receipt still reports success; `session_turns.body` equals serialized `record.content`; chain recency fields update. | Does not prove crash recovery beyond existing import-replace tests unless extended separately. |
| T10 | Trace mixed legacy + new rows. | Inline trace reports stored rows with content and legacy rows with `body_state: "missing"` and `content: null`. | component | `trace` inline test with mixed `session_turns.body` values. | A6 | JSON shape has per-turn `body_state`; no root-level placeholder null. | Does not validate every invalid-body warning path. |
| T11 | Claude adapter body emission. | `scripts/claude-code-turns` emits `body` raw JSON content array for Claude user/assistant records. | particular-integration | Adapter fixture JSONL under temp Claude project tree. | A5, A7 | Script stdout JSON lines contain `body[0].text` from Claude content. | Does not cover future Claude non-text content under D-002. |
| T12 | Codex adapter body emission. | `scripts/codex-turns` emits `body` raw JSON content array for Codex `response_item` message records. | particular-integration | Adapter fixture rollout JSONL under temp Codex sessions tree. | A5, A7 | Script stdout JSON lines contain `body[0].text` from `payload.content`. | Does not cover future Codex multimodal payloads. |

If, during Phase 6b, any named change-risk or verification-risk listed above cannot be verified by the emitted test set, the test writer produces `risk/15-empty-bodies-ref-test-residuals.md` per `~/ai/workflows/implementation-pipeline.md` Phase 6b residual rule. The residual artifact records residual class (`combinatorial/path-state`, `bounded-model`, `integration-hidden`, `emergent-interaction`, `temporal/concurrency`, or `generator/search-budget`), technique attempted, scope, budget, result, remaining residual, invalidating inputs, and whether the residual changes the net-value case.

# 6. Qualitative net-value statement

Positive net value. The proposal directly reduces four concrete current-state risks on the supported CLI surface: RC-1 missing-body schema means SQLite cannot answer what a turn said; RC-2 ingest drops body bytes even if an adapter emits them; RC-3 export fails when provider JSONL is missing; RC-4 `trace --inline-transcript` always serializes `null`.

The added burden is a nullable `TEXT` column, narrow ingest/import insert changes, DB fallback readers, and adapter/documentation updates. Migration cost is low because the column add is idempotent and presence-checked. Rollback burden is also bounded: the nullable column can remain unused, and reverting reads restores metadata-only behavior for older binaries. That burden is clearly outweighed by restoring the ticket's bodies-in-DB contract for the current Tauri desktop and CLI surfaces.

# 7. Open questions left for Phase 5

- Does any risk gate require `src-tauri/src/schema_probe/mod.rs` to expose `body` as a required column or feature flag despite the no-bump proposal? Default answer here is no.
- Confirm the exact DB-fallback `RecordSource` representation. The likely shape is a deterministic `state_db`/`db://session_turns/<id>` sentinel, but Phase 5 must verify `PathBuf` serialization and the "no canonical wire schema change" constraint.
- Confirm whether export fallback should error on the first `NULL` body row or skip unsupported/legacy rows. This proposal chooses error for export and explicit `body_state` for trace.
- Confirm whether `session_replace` needs to read existing body values for diff/preimage displays. This proposal says no because canonical input/provider file remains the replace source.
- Confirm all existing tests/fixtures that manually insert into `session_turns` use explicit column lists or can tolerate the new nullable column.
- Confirm `scan_provider` should preserve duplicate-row behavior as `INSERT OR IGNORE` only, meaning a later adapter fix will not update pre-existing metadata-only duplicate turns.

## Round 2 changelog

- audit-F1: added residual-risk artifact obligation paragraph after test-intent table (proposal §5).
- audit-F2: pinned single test-level per row for T4, T11, T12 and normalized T3 to the allowed `end-to-end` level (proposal §5 table cells).
