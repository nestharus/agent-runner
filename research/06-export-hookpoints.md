# Phase 5 Hookpoints — 06-export (`agents session export`)

> **Note (pre-change evidence):** This hookpoint map describes the current
> `06-export` worktree before any Phase 6 implementation. The risk gate cleared
> Rev 2 after R1-F01 pinned the allowed locator `STATE_DIR` mkdir side effect in
> the proposal (`risk/06-export-audit-history.md:1-14`). The approved action map
> is `proposals/06-export.md` Rev 2. Export is sequenced after 06-locate and
> 06-schema-probe, so implementation hookpoints intentionally reference
> 06-locate's `SessionMetadata` API and the future read-only state open variant
> rather than today's local `StateDb::open_default()` behavior.

## A. `session export` subcommand surface hookpoints (proposal §2)

- **Extend:** `SessionSubcommands` is introduced by 06-locate in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:175-185`.
  Add `Export { session_id: String, #[arg(long, default_value =
  "canonical-jsonl")] format: ExportFormat }` as a sibling to `Locate`, not a
  new top-level `Subcommands` variant.
- **Current local gap:** the local 06-export worktree has no `session` command
  group today; `Subcommands` currently contains `Trace`, `Repl`, `Resume`,
  hidden `ResumeList`, `MigrateDb`, and `MigrateConfig` in
  `src-tauri/src/main.rs:77-166`. Phase 6 must land on top of 06-locate or
  replay 06-locate's parent command shape first.
- **New enum:** define `ExportFormat` near `SessionSubcommands` in
  `src-tauri/src/main.rs`. It should derive `Clone`, `Copy`, and
  `clap::ValueEnum`; the only v1 value is `CanonicalJsonl`, serialized by clap
  as `canonical-jsonl`.
- **Bare parent behavior:** keep the nested `Session { #[command(subcommand)]
  command: SessionSubcommands }` shape from 06-locate. Bare `agents session`
  remains a clap usage error. Bare `agents session export` without a session id
  remains a clap usage error.
- **Unsupported format behavior:** do not implement an ad hoc format parser.
  Clap `ValueEnum` should reject any value other than `canonical-jsonl`, with
  exit `2`. The CLI wrapper only handles already-parsed `ExportFormat`.
- **Extend dispatch:** 06-locate dispatches `Subcommands::Session` in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:354-358`.
  Extend that nested match with `SessionSubcommands::Export { session_id,
  format } => run_session_export(&session_id, format)`.
- **New wrapper:** add `run_session_export` in `src-tauri/src/main.rs` near
  `run_session_locate` so the two session command wrappers share error style,
  config loading, and state-open conventions.
- **Stdout contract:** success writes compact JSONL only. Use
  `serde_json::to_string(&record)` per canonical record and `println!`, but
  only after `read_canonical_transcript` returns a fully validated `Vec`.
- **Stderr contract:** error exits write one compact JSON object to stderr with
  at least `code` and `message`. 06-locate's `emit_metadata_error` precedent is
  in `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:561-608`.
  Export can either add export-specific emitters or factor a small shared
  helper, but should not change locate's output shape.
- **No hookpoint — trace/resume:** do not route export through `trace --json`,
  `--inline-transcript`, `resume`, or `repl --resume`; those surfaces have
  different inputs, output shapes, and side effects (`src-tauri/src/main.rs:447-478`,
  `src-tauri/src/main.rs:1056-1263`).

## B. Reusable canonical-transcript reader API hookpoints (proposal §6)

- **New module:** create `src-tauri/src/session_export/` and expose it from
  `src-tauri/src/lib.rs` with `pub mod session_export;`, next to the
  06-locate `pub mod session_metadata` export.
- **Public consumer boundary:** the reader consumes
  `agent_runner_lib::session_metadata::SessionMetadata` from 06-locate. The
  current 06-locate API fields are `session_id`, `chain_id`, `provider_name`,
  `storage_type`, `jsonl_path`, `workspace_root`, `transcript_state`, and
  `mutable` in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:11-21`.
- **Public storage enum reuse:** use 06-locate's `SessionStorageType` directly
  for `RecordSource.storage_type`. Its serialized variants are `claude_code`,
  `codex_session`, and `other` in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-39`.
- **Public types:** define `CanonicalRecord`, `CanonicalRole`,
  `ContentChunk`, `RecordSource`, and `ExportError` in
  `src-tauri/src/session_export/mod.rs`, matching proposal §6. `CanonicalRecord`,
  `CanonicalRole`, `ContentChunk`, and `RecordSource` need
  `serde::Serialize`; `Deserialize` is useful for the later import/replace
  round-trip and should be included as proposed.
- **Serde shape:** put `#[serde(rename_all = "snake_case")]` on
  `CanonicalRole`. Put `#[serde(tag = "type", rename_all = "snake_case")]` on
  `ContentChunk` so chunk JSON is internally tagged as `text`, `tool_call`, and
  `tool_result`.
- **Public function:** expose exactly
  `pub fn read_canonical_transcript(metadata: &SessionMetadata) ->
  Result<Vec<CanonicalRecord>, ExportError>`. Do not expose a streaming public
  iterator in v1 because the CLI must prove no partial stdout on late parse
  errors.
- **Private modules:** use `jsonl.rs` for byte-preserving line scanning,
  `claude_code.rs` for Claude mapping and compaction, and `codex_session.rs`
  for Codex mapping. Keep parser helpers private until another feature needs
  them.
- **Dependency hookpoint:** add `sha2` as a direct dependency in
  `src-tauri/Cargo.toml`; today it is absent from `[dependencies]`
  (`src-tauri/Cargo.toml:10-25`) even though the proposal notes it exists
  transitively in the lockfile.
- **Import/replace compatibility:** keep the public record types stable enough
  for 06-import-replace to parse canonical JSONL and compare a post-replace
  export, but do not implement import parsing in this feature.

## C. Resolution flow hookpoints (proposal §4)

- **Step 1, UUID parse:** parse `<session-id>` as a full UUID before opening
  state or loading config. 06-locate's wrapper already does this in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:505-512`.
  Export should mirror it and emit `invalid-session-id` / exit `2`.
- **Step 2, read-only DB open:** use the read-only `StateDb` open variant
  supplied by 06-schema-probe. Do not use today's local `StateDb::open_default`
  in `src-tauri/src/state/db.rs:611-615`, because it creates directories,
  opens read/write, enables WAL, ensures schemas, and backfills chains
  (`src-tauri/src/state/db.rs:431-608`).
- **Stop trigger:** if 06-schema-probe has not provided a read-only open path
  by Phase 6, do not silently fall back to `StateDb::open_default`; return to
  the proposal gate because A2 would be invalidated.
- **Config load parity:** after read-only state open, load models, providers,
  and sessions with the same semantics as locate/resume. 06-locate currently
  loads `models_dir`, `providers.toml`, and `sessions.toml` in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:520-538`,
  with provider/session config using `unwrap_or_default`.
- **Metadata call:** call `locate_session_metadata(&state, &models,
  &providers_cfg, &sessions_cfg, session_id)` and inherit the ownership,
  ambiguity, path canonicalization, storage vocabulary, and workspace-root
  validation from 06-locate
  (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:81-132`).
- **Metadata error mapping:** map `MetadataError::InvalidSessionId` to exit `2`,
  `SessionNotFound` to `10`, `AmbiguousSession` to `11`,
  `UnsupportedStorage` to `12`, and `Operational` to `1`, matching 06-locate's
  `metadata_error_exit_code` in
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:561-570`.
- **Storage rejection:** after metadata success, reject
  `SessionStorageType::Other` with `unsupported-storage` / exit `12` before
  opening the JSONL file. V1 parser support is only `ClaudeCode` and
  `CodexSession`.
- **Reader dispatch:** call `read_canonical_transcript(&metadata)`. The reader
  dispatches on `metadata.storage_type`, not on file extension, provider name,
  `sessions.toml`, or `session_turns`.
- **Export error mapping:** map `ExportError::UnsupportedStorage` to exit `12`;
  map `MalformedTranscript` and `UnsupportedRecord` to exit `15`; map `Io`,
  serialization failures, and unexpected `Operational` errors to exit `1`.
- **No partial stdout:** `run_session_export` should collect the `Vec` first,
  then serialize all records into an in-memory `Vec<String>` or string buffer,
  and only then write stdout. This catches serialization errors before any
  canonical line is emitted.
- **No `session_turns` fallback:** do not query `session_turns` for content,
  ordering, source path, byte offsets, compaction cutoff, or parser dispatch.
  That table stores summaries only (`src-tauri/src/state/db.rs:559-572`).

## D. Source-metadata fields hookpoints (proposal §3 D1)

- **Scanner home:** implement source preimage scanning in
  `src-tauri/src/session_export/jsonl.rs`. The scanner should return a private
  source-line struct with `line`, `byte_start`, `byte_end`, `raw_bytes`, and
  parsed `serde_json::Value`.
- **Byte offsets:** track offsets over raw file bytes, not UTF-8 chars and not
  `String` lengths. `byte_start` is inclusive; `byte_end` is exclusive; both
  exclude line terminator bytes.
- **Line numbers:** count every physical line from 1. Empty or whitespace-only
  lines do not emit records but still advance line numbers and byte offsets.
- **Line terminators:** support LF and CRLF. For CRLF, `byte_end` stops before
  `\r\n`; for LF, it stops before `\n`. A final unterminated line is valid and
  ends at EOF.
- **Hash preimage:** compute `sha256` over exactly `raw_bytes[byte_start,
  byte_end)` after removing the terminator but before JSON parsing,
  normalization, or whitespace trimming.
- **Hash format:** output lowercase hex. If no hex crate is added, format bytes
  manually in `jsonl.rs`; do not use debug formatting.
- **Path/storage source:** set `source.jsonl_path` and `source.storage_type`
  from metadata. Do not re-run locators or emit `other`; `other` exits `12`
  before parsing.
- **Parse errors:** malformed UTF-8 or malformed JSON in a non-empty source
  line is `ExportError::MalformedTranscript { path, line: Some(line), ... }`
  and CLI exit `15`; existing adapter scripts silently skip such lines, but
  export must fail closed.
- **Record/source cardinality:** v1 assumes one emitted canonical record has
  one native JSONL source line. If a future parser needs to merge or split
  native lines, that invalidates A4 and should return to proposal design rather
  than faking a single source preimage.

## E. Storage-specific parsing hookpoints (Claude / Codex / unsupported)

- **Dispatcher:** in `read_canonical_transcript`, match
  `SessionStorageType::ClaudeCode` to `claude_code::read`, `CodexSession` to
  `codex_session::read`, and `Other` to `ExportError::UnsupportedStorage`.
- **Claude evidence:** the current Claude adapter reads top-level `type`,
  `uuid`, `timestamp`, `sessionId`, `parentUuid`, `isSidechain`, and
  `isCompactSummary` (`scripts/claude-code-turns:57-86`). Export should use
  this as fixture guidance but read directly from the JSONL file.
- **Claude session filter:** emit only records whose native `sessionId` matches
  `metadata.session_id`. If a Claude file located by filename contains a
  conflicting `sessionId` on transcript records, treat that as malformed rather
  than exporting the wrong session.
- **Claude id/timestamp:** prefer native `uuid` for `turn_id` and native
  `timestamp` for time. Missing id/timestamp on mappable transcript records is
  exit `15`; timestamps must validate as RFC3339.
- **Claude role/content:** map top-level `type: "user"` and `"assistant"` to
  roles. Extract text chunks from known message/content fields discovered in
  fixtures. Tool-use and tool-result shapes should map to `ToolCall` and
  `ToolResult` when the native payload exposes id/name/input or result text.
- **Claude bookkeeping:** skip provider bookkeeping lines that are not
  transcript records. Emit `unsupported_record: true` only for session-bound
  transcript records that satisfy the proposal's safe-placeholder conditions.
- **Claude compaction marker:** treat `isCompactSummary == true` as a
  compaction boundary. The boundary line itself must be mappable as the first
  emitted record after cutoff, usually as a summary text record.
- **Codex evidence:** the current Codex adapter reads one `session_meta` line
  for session id and then `response_item` lines whose `payload.type ==
  "message"` (`scripts/codex-turns:56-87`). It synthesizes ids from
  `<file>:<line_no>` because current payload ids may be null.
- **Codex session filter:** require an earlier or current `session_meta` line
  whose `payload.id` matches `metadata.session_id`. A located Codex file with
  no matching `session_meta` is malformed or unsupported storage, not an empty
  transcript success.
- **Codex id/timestamp:** prefer `payload.id` when present and non-null;
  otherwise synthesize from `metadata.jsonl_path` plus source line number. Use
  top-level `timestamp` from `response_item` records and fail on missing or
  non-RFC3339 values.
- **Codex role/content:** map `payload.role` values `user` and `assistant`.
  Use `payload.content` / message item fixture shapes for text chunks; map tool
  call/result payloads only when they are explicit and auditable.
- **Codex unsupported compaction:** no stable Codex compaction marker is known
  from current adapters or proposal evidence. Codex v1 emits the full mappable
  transcript unless Phase 5/6 fixture research proves a raw marker and the
  proposal is revised.
- **Unsupported storage:** providers with no `session_storage`, storage
  metadata `other`, non-UTF-8 located paths, or no parser contract fail with
  `unsupported-storage` / exit `12`. Do not attempt provider-native best effort.

## F. Compaction-aware emission hookpoints (proposal §4 D4)

- **Policy home:** storage parsers should return all candidate records plus an
  optional compaction cutoff index, or apply the cutoff internally before
  returning. Keep the policy in parser modules because markers are
  storage-specific.
- **Claude cutoff:** while scanning Claude source lines, remember the latest
  emitted record whose source line has `isCompactSummary == true`. After the
  scan validates all records, emit from that boundary record through the end.
- **Multiple boundaries:** latest boundary wins. Earlier boundaries and all
  records before the latest boundary are omitted from canonical output.
- **Boundary inclusion:** include the latest compaction boundary summary as the
  first emitted record. Do not emit only post-boundary turns; the boundary is
  the compacted-context preimage.
- **No boundary:** when a supported parser sees no compaction marker, emit the
  full mappable transcript in JSONL file order.
- **Codex v1:** emit full mappable transcript. Do not infer compaction from
  `session_turns.is_compaction_boundary` or timestamps because export's source
  of truth is the raw located JSONL.
- **Unsupported boundary line:** if a parser recognizes a compaction boundary
  but cannot represent the boundary safely as a canonical record, return
  `ExportError::UnsupportedRecord` and exit `15`; otherwise the live transcript
  would silently lose its compacted prefix.

## G. Ordering guarantee hookpoints (proposal §4 D5)

- **Primary order:** preserve provider JSONL file order after the compaction
  cutoff. Do not sort by timestamp, turn id, parent id, or `session_turns`
  ingestion order.
- **Validation:** validate every emitted record timestamp with
  `chrono::DateTime::parse_from_rfc3339`.
- **Regression rule:** track the previous emitted timestamp. Equal timestamps
  are allowed; decreasing timestamps are `MalformedTranscript` / exit `15`.
- **Native causal order:** current adapters walk files line-by-line, and Codex
  already synthesizes ids from source line (`scripts/codex-turns:56-87`).
  Export should keep that causal order instead of inventing a sorter.
- **No partial success:** ordering validation must complete before stdout
  emission. A late timestamp regression means zero stdout bytes.

## H. Read-only behavior hookpoints (proposal §8)

- **State DB:** use the 06-schema-probe read-only open variant. Export must not
  call DB write APIs such as invocation writers, session ingest, migration, or
  quota refresh paths.
- **Known mutating open:** today's `StateDb::open_default()` is mutating by
  design (`src-tauri/src/state/db.rs:431-615`). This is not acceptable for
  export after schema-probe lands.
- **Locator exception:** export may create the configured transcript locator's
  adapter `state_dir` directory through `locate_session_metadata`, matching the
  Rev 2 proposal and current `locate_transcript` behavior
  (`src-tauri/src/sessions/mod.rs:183-187`). No file inside that directory may
  be written by export.
- **No scans:** do not call `scan_provider` in `src-tauri/src/sessions/mod.rs:55-141`.
  Scans run turn scripts and write cursor/ingest state.
- **No adapter scripts for parsing:** do not run `turn_script`s to produce
  export output. The Rust reader opens the already located `jsonl_path`.
- **No provider launch:** do not invoke executor paths, provider commands,
  auth checks, model discovery, diagnostics, resume strategies, or REPL.
- **No transcript mutation:** open the transcript read-only. Do not rewrite,
  normalize, chmod, touch, copy to temp files, or create sidecar hashes.
- **Test snapshot point:** read-only tests should snapshot DB rows and relevant
  filesystem mtimes after any fixture setup and after permitted DB opening, then
  invoke export and compare snapshots.

## I. Test-intent track hookpoints (proposal §9)

- **General test home:** parser and scanner unit/component tests belong under
  `src-tauri/src/session_export/`. CLI integration tests belong in a new
  `src-tauri/tests/initiative_06_export.rs`, following the binary invocation
  style used by existing integration tests.
- **CLI surface:** clap/parser tests in `src-tauri/src/main.rs` should cover
  default `canonical-jsonl`, explicit `--format canonical-jsonl`, invalid
  format exit `2`, missing session id, and bare `agents session`.
- **Locate pass-through:** integration fixture should seed chain/segment state
  and provider/session config, use a locator script that returns a temp JSONL
  path, and assert exported records carry `metadata.session_id` and
  `provider_name`.
- **Metadata error mapping:** tests should force not-found, ambiguous, and
  unsupported-storage cases through `locate_session_metadata` and assert exits
  `10`, `11`, and `12` with empty stdout.
- **Source preimage:** unit tests for `jsonl.rs` should cover LF, CRLF,
  final-line-without-newline, empty lines, whitespace-only lines, multibyte
  UTF-8, byte offsets, and SHA-256 over exact non-terminator bytes.
- **Malformed JSON/UTF-8:** parser tests should include a valid first record
  followed by malformed content; CLI integration should assert exit `15` and
  zero stdout bytes.
- **Claude mapping:** component fixtures should cover user text, assistant
  text, system-like/native summary records if present, tool call, tool result,
  skipped bookkeeping, safe unsupported transcript record, and unsafe
  unsupported record.
- **Claude compaction:** fixture with pre-boundary turns, two
  `isCompactSummary` records, and post-boundary turns should assert output
  starts at the latest boundary line and includes that boundary.
- **Codex mapping:** component fixtures should cover `session_meta`, user and
  assistant `response_item` messages, null `payload.id` synthesized turn ids,
  present `payload.id`, skipped non-message response items, and missing or
  mismatched `session_meta`.
- **Ordering:** parser tests should cover increasing timestamps, equal
  timestamps, and decreasing timestamps. Decreasing timestamp returns exit `15`
  and no stdout.
- **No `session_turns` reconstruction:** integration fixture should seed
  summary rows with empty `source_file` and content that conflicts with the raw
  JSONL; exported content must match raw JSONL only.
- **Read-only:** integration test should snapshot DB row counts, transcript
  mtime/size/hash, adapter `state_dir` listing, and temp directory listing
  before/after export. Account for the allowed locator directory creation only
  if locate creates it during the test.
- **Serialization shape:** component test should serialize one
  `CanonicalRecord` and assert compact JSON field names, snake_case enums,
  typed `content`, mandatory `source`, and `unsupported_record`.
- **README/doc check:** add documentation assertions or a manual residual that
  README mentions synopsis, `canonical-jsonl`, field schema, source hash
  preimage, compaction policy, exit codes, and no partial stdout.
- **Residual tracking:** Phase 6b should record parser drift gaps, especially
  real Claude/Codex tool payload variants and unknown Codex compaction markers,
  in `risk/06-export-test-residuals.md` if not fully covered.

## J. Implementation surface summary

| Proposal action | Hookpoint | Reuse / extend / new |
| --- | --- | --- |
| `export` child command | 06-locate `SessionSubcommands` in `src-tauri/src/main.rs` | extend |
| Export format parser | new `ExportFormat` `clap::ValueEnum` | new |
| Nested session dispatch | 06-locate `Subcommands::Session` match arm | extend |
| Export CLI wrapper | new `run_session_export` near `run_session_locate` | new |
| Invalid UUID handling | 06-locate `run_session_locate` parse-before-open pattern | reuse |
| Read-only state open | 06-schema-probe `StateDb` read-only variant | reuse |
| Config load | 06-locate/resume model + provider/session config load | reuse |
| Ownership/path metadata | 06-locate `locate_session_metadata` | reuse |
| Metadata error exit mapping | 06-locate `MetadataError` mapping | reuse/extend |
| Stderr JSON / exit mapping | metadata + export error helpers | extend/new |
| Canonical reader module | `src-tauri/src/session_export/` | new |
| Library export | `src-tauri/src/lib.rs` `pub mod session_export;` | extend |
| Public canonical types | `CanonicalRecord`, `CanonicalRole`, `ContentChunk`, `RecordSource`, `ExportError` | new |
| Public reader | `read_canonical_transcript(&SessionMetadata)` | new |
| Byte JSONL scanner | `src-tauri/src/session_export/jsonl.rs` | new |
| SHA-256 dependency | `src-tauri/Cargo.toml` direct `sha2` dependency | extend |
| Claude parser | `src-tauri/src/session_export/claude_code.rs`; evidence from `scripts/claude-code-turns` | new + reference |
| Codex parser | `src-tauri/src/session_export/codex_session.rs`; evidence from `scripts/codex-turns` | new + reference |
| Unsupported storage refusal | `SessionStorageType::Other` before parser dispatch | reuse |
| Compaction cutoff | Claude parser latest `isCompactSummary == true` | new |
| Ordering validation | parser-level timestamp validation in source order | new |
| No partial stdout | CLI buffers validated records/serialized lines before printing | new |
| No `session_turns` fallback | avoid `src-tauri/src/state/db.rs:559-572` as content source | retain boundary |
| Read-only tests | new `src-tauri/tests/initiative_06_export.rs` | new |
| Parser tests | tests under `src-tauri/src/session_export/` | new |
| README synopsis/schema | `README.md` CLI/session sections | extend |
| Keep trace output | `src-tauri/src/trace/mod.rs` and `run_trace_command` unchanged | retain |
| Keep resume/repl behavior | `src-tauri/src/main.rs:1056-1263` unchanged | retain |

## What this hookpoint research deliberately does NOT cover

1. 06-schema-probe implementation details beyond the required read-only
   `StateDb` open variant consumed by export.
2. 06-locate implementation changes except where export consumes
   `SessionMetadata`, `SessionStorageType`, and error semantics.
3. 06-pause-handshake locks; export is read-only and does not observe or create
   pause leases in v1.
4. 06-import-replace write, preimage, crash-recovery, and lock composition,
   except that it should reuse the canonical record family later.
5. Alternate export formats such as pretty JSON, Markdown, provider-native
   JSONL, archives, or trace inline transcript compatibility.
6. Provider-private payload preservation beyond the canonical schema.
7. GUI/Tauri frontend surfaces, HomeView/StatusView/PoolsView, and Ollie/design
   work.
8. Quota correctness, provider selection, auth refresh, diagnostics, setup,
   discovery, and model discovery behavior.
9. Cross-CLI migration policy, Claude-to-Codex transcript conversion, and Codex
   migration support.
10. Performance budgets for very large transcripts beyond the explicit v1
   buffering requirement needed to prevent partial stdout.

deliberately does NOT cover
