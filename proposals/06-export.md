# 1. Scope statement

06-export adds one read-only CLI surface:

```bash
agents session export <session-id> [--format canonical-jsonl]
```

It emits canonical transcript JSONL to stdout: one compact JSON object
per line, one canonical transcript record per object. This is the third
Initiative 06 feature and builds on the preceding `06-locate` and
`06-schema-probe` surfaces: `locate` supplies `SessionMetadata` and the
`session` command group; `schema-probe` supplies the read-only state open
variant that export must use for its no-side-effect contract
(`initiatives/06-session-override-contract.md:41-56`,
`initiatives/06-session-override-contract.md:83-89`,
`initiatives/06-session-override-contract.md:118-120`).

This proposal does not implement code. It defines the command shape,
canonical JSONL record schema, provider parser policy, compaction policy,
ordering guarantee, reusable canonical-transcript reader API, test-intent
track, supported-surface track, side-effect contract, and anti-scope for
the later Phase 6 implementation. It consumes the approved current-state
map at `research/06-export-problem-map.md`; this proposal's §1.1
register replaces the draft register in that map.

The implementation scope is additive: extend `agents session` with
`export`, add `src-tauri/src/session_export/`, add `claude_code` and
`codex_session` canonicalizers, and document the contract. Existing
resume/repl/trace/migration/locate behavior remains unchanged. Export does
not reconstruct content from `session_turns`, scan provider stores, refresh
cursors, run turn scripts, migrate transcripts, launch providers, implement
write paths, add GUI surface, or add formats beyond `canonical-jsonl`.

**Rev 2 changes** (in response to Phase 4 Round 1 audit):

- §8: explicit `STATE_DIR` mkdir clause matching 06-locate's §8.
  Closes R1-F01 by pinning the contract; removes Phase 5 deferral
  language.

# 1.1 Assumption register

This is the approved register narrowed from
`research/06-export-problem-map.md` §7. Do not keep a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | `06-locate` lands before export and supplies `SessionMetadata`, `SessionStorageType`, `TranscriptState`, and the `session` command group. | Initiative sequence puts locate first and export third; locate Rev 3 defines `src-tauri/src/session_metadata/` and `SessionMetadata` (`initiatives/06-session-override-contract.md:41-50`, `proposals/06-locate.md:1-38`, `proposals/06-locate.md:177-216`). | Export is merged independently before locate, or locate's public type names/fields materially change. | §2 subcommand; §4 resolution; §6 API; §13 constraints. |
| A2 | `06-schema-probe` lands before export and provides a read-only `StateDb` open path for session commands. | Initiative sequencing assigns read-only open to schema-probe before export (`initiatives/06-session-override-contract.md:44-50`, `initiatives/06-session-override-contract.md:118-120`). | Export starts from today's mutating `StateDb::open_default()` without an accepted exception. | §4 step 2; §8 side effects; §9 tests. |
| A3 | The canonical transcript source for export is the provider JSONL path in `SessionMetadata`, not `session_turns`. | Harness says each output line is a canonical transcript record, not a `session_turns` quota row; `session_turns` stores no content, offsets, line numbers, or hashes (`02-session-export.md:20-41`, `src-tauri/src/state/db.rs:559-572`). | Harness accepts summary-row reconstruction or agent-runner starts persisting full canonical content in state before export. | D3 / §3 source; §4 steps 3-5; §7 anti-scope. |
| A4 | Per-record source metadata can be computed at read time by scanning the raw JSONL file as bytes. | The requested metadata is path/line/byte/hash over source records; no DB columns are required in principle (`02-session-export.md:22-41`, `research/06-export-problem-map.md` §6 #3). | A future canonical record merges or splits multiple native source records and needs multi-source provenance. | D1 / §3 `source`; §6 `RecordSource`; §9 tests. |
| A5 | `SessionStorageType` is sufficient to choose the v1 parser family. | Locate emits `claude_code`, `codex_session`, and `other`; current Claude/Codex adapters show materially different native line shapes (`proposals/06-locate.md:113-121`, `scripts/claude-code-turns:57-86`, `scripts/codex-turns:56-87`). | A provider declares one storage type while its locator returns another format, or a new storage type is required by the harness in v1. | D3 / §4 step 5; §6 parser dispatch; §11 supported surface. |
| A6 | Provider JSONL line order is the stable conversation order for supported storage types. | Existing adapters walk files line-by-line; Codex synthesizes current turn ids from `<file>:<line_no>` because payload ids may be null (`scripts/claude-code-turns:57-86`, `scripts/codex-turns:56-87`). | Real provider transcripts rely on timestamp sorting rather than append order, or contain valid causal records with regressing timestamps. | D5 / §4 step 8; §9 ordering tests. |
| A7 | Claude Code compaction boundaries are detectable in raw JSONL through `isCompactSummary == true`; Codex compaction is not supported in v1 unless a parser-visible marker is found during Phase 5. | Current Claude adapter and compaction backfill inspect `isCompactSummary`; existing Codex adapter ignores compaction state and prior migration notes defer Codex compaction detection (`scripts/claude-code-turns:69`, `src-tauri/src/main.rs:1988`, `research/05-session-migration-answers.md:152`). | Codex compaction must be live-state accurate in v1, or Claude changes compaction marker shape. | D4 / §4 step 7; §9 compaction tests; §12 residuals. |
| A8 | `sha2` can be added as a direct Rust dependency for line SHA-256. | `sha2` already exists transitively in `src-tauri/Cargo.lock`, but `src-tauri/Cargo.toml` has no direct dependency (`src-tauri/Cargo.toml:10-25`, `src-tauri/Cargo.lock:3142-3149`). | Dependency policy rejects a direct hash crate or mandates another existing hashing implementation. | D1 / §3 `source.sha256`; §12 residuals. |

# 1.2 Net-value statement

Yes: this reduces a concrete current-state risk on the supported CLI
surface. Today the harness must parse private provider JSONL layouts or
read private SQLite summaries that do not contain transcript content
(`research/06-export-problem-map.md` §6 #1-5). Export moves that
provider-specific parsing behind one stable CLI contract and one reusable
Rust reader while preserving source preimage metadata for audit checks.

The added blast radius is bounded: one additive subcommand and one
read-only parser module. The migration cost is none for user state because
export reads the currently located JSONL. Rollback is also low cost: avoid
the subcommand or revert the binary. The main implementation burden is
provider JSONL drift handling, but that burden already exists in the
harness; centralizing it in agent-runner is a positive value trade.

# 2. Subcommand surface

Extend the `SessionSubcommands` enum introduced by 06-locate:

```text
session export <session-id> [--format canonical-jsonl]
```

Clap shape:

```rust
enum SessionSubcommands {
    Locate { ... },
    Export {
        session_id: String,
        #[arg(long, default_value = "canonical-jsonl")]
        format: ExportFormat,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ExportFormat {
    CanonicalJsonl,
}
```

`canonical-jsonl` is the default and the only supported v1 format.
Unknown `--format` values are clap usage errors and exit `2`.
Bare `agents session` remains a clap usage error. `agents session export`
without a session id is a clap usage error.

Success stdout is line-delimited compact JSON. Error stderr is one compact
JSON object with at least `code` and `message`, matching locate's stable
stderr style. Export buffers/validates records before writing stdout so
malformed transcripts cannot produce partial canonical JSONL.

# 3. JSON output schema (per record)

Each stdout line is one compact JSON object with this required schema:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `session_id` | string UUID | yes | Active provider session id from `SessionMetadata.session_id`. |
| `provider_name` | string | yes | Active provider/account name from `SessionMetadata.provider_name`. |
| `turn_id` | string | yes | Stable canonical turn identifier for this record. Native ids are preferred; otherwise synthesize from `jsonl_path:line`. |
| `role` | string enum | yes | `system`, `user`, `assistant`, `tool`, or `unknown`. `unknown` is allowed only when `unsupported_record: true`. |
| `timestamp` | RFC3339 string | yes | Native record timestamp normalized as a string; malformed/missing timestamps on supported records are exit `15`. |
| `content` | array | yes | Ordered canonical content chunks. Empty only when `unsupported_record: true`. |
| `source` | object | yes | Exact source preimage metadata for the native JSONL line consumed. |
| `unsupported_record` | boolean | yes | `true` when the native record belongs to the transcript but cannot be mapped into supported canonical content. |

D2 decision: `content` is an array of typed objects, not a string. This
keeps text, tool calls, and tool results in one field without flattening
non-text content into lossy prose. V1 chunk variants:

| Chunk shape | Meaning |
| --- | --- |
| `{ "type": "text", "text": "..." }` | Text content from user, assistant, system, or tool-result records. |
| `{ "type": "tool_call", "id": "...", "name": "...", "input": <json> }` | A model-requested tool/function call when the provider record exposes call id/name/input. |
| `{ "type": "tool_result", "tool_call_id": "...", "text": "...", "is_error": false }` | A tool/function result that can be represented as text. |

System messages use `role: "system"` with `text` chunks. Tool calls stay
on `role: "assistant"` because they are assistant actions. Tool results
use `role: "tool"`. Provider-native thinking/reasoning, event messages,
session metadata, usage records, and other non-conversation bookkeeping
are skipped when they do not represent transcript turns. They are emitted
as `unsupported_record: true` only when the parser can prove all of the
following:

1. The native line belongs to the requested session.
2. It is a transcript/conversation record, not provider bookkeeping.
3. It has a usable timestamp or a parser-defined safe timestamp fallback.
4. It has a stable native id or source-line synthesized id.
5. Emitting an unsupported placeholder is less lossy than silently
   skipping it.

If those conditions are not all true, malformed or unsafe native records
produce `ExportError::MalformedTranscript` and CLI exit `15`.

D1 decision: all `source` fields are mandatory on every emitted record.
The source object is:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `storage_type` | string enum | yes | `claude_code` or `codex_session` in v1. `other` exits `12`. |
| `jsonl_path` | string path | yes | Canonical absolute UTF-8 transcript path from `SessionMetadata.jsonl_path`. |
| `line` | integer | yes | 1-based source line number in the JSONL file. |
| `byte_start` | integer | yes | 0-based inclusive byte offset of the native JSON record, excluding line terminator bytes. |
| `byte_end` | integer | yes | 0-based exclusive byte offset of the native JSON record, excluding line terminator bytes. |
| `sha256` | lowercase hex string | yes | SHA-256 of the exact byte slice `[byte_start, byte_end)`. |

The hash is of the native source record bytes, not the parsed JSON value
and not the whole file. A CRLF-terminated file hashes the bytes before
`\r\n`; an LF-terminated file hashes the bytes before `\n`. Empty/whitespace
lines are ignored and do not produce records, but their bytes still count
for subsequent offsets.

Implementation cost for D1: export needs a byte-preserving JSONL scanner
instead of `BufRead::lines()`. The scanner tracks `(line_no, byte_start,
byte_end, raw_bytes_without_terminator)`, computes SHA-256, then parses the
same byte slice as UTF-8 JSON. This is moderate cost and belongs in the new
Rust module; it is not reconstructible from existing DB rows.

# 4. Resolution flow

1. Clap parses `session export <session-id> [--format canonical-jsonl]`.
   Invalid flags or unsupported formats exit `2`.
2. Parse `<session-id>` as a full UUID before opening state. Invalid parse
   exits `2` with stderr JSON error code `invalid-session-id`, matching
   locate's normalization.
3. Open the CLI default state DB through the read-only open variant supplied
   by 06-schema-probe. Export must not use today's mutating
   `StateDb::open_default()` unless Phase 4 explicitly revises this
   dependency.
4. Load model/provider/session config with the same semantics as locate.
   Provider/session config load failures follow locate/resume defaulting and
   normally fail closed later as `unsupported-storage`; model load failures
   are operational errors.
5. Call `locate_session_metadata(...)`. Export inherits ownership
   resolution, ambiguity semantics, storage vocabulary, transcript path
   canonicalization, and workspace-root validation from 06-locate.
6. Reject `SessionStorageType::Other` with exit `12 unsupported-storage`.
   V1 parser support is only `claude_code` and `codex_session`.
7. Dispatch to the storage parser in `src-tauri/src/session_export/`.
   D3 decision: parsers live in Rust, not in `scripts/`, because export
   needs byte offsets, hashes, canonical content, no cursor writes, and a
   reusable library API for import/replace. Existing adapters remain
   summary-ingestion helpers only.
8. Apply D4 compaction policy. Export emits the live canonical transcript:
   if the storage parser can identify one or more compaction boundaries,
   records before the latest boundary are omitted and the boundary summary
   record is included as the first emitted record. If no supported boundary
   is present, export emits the full transcript. Claude v1 identifies
   boundaries with `isCompactSummary == true`. Codex v1 has no supported
   compaction marker; it emits the full transcript unless Phase 5 proves a
   stable raw marker and this proposal is revised.
9. Apply D5 ordering policy. Emission order is provider JSONL file order
   after the compaction cutoff. This is the stable chronological transcript
   order for supported storage. Parsers validate that emitted records have
   parseable timestamps and that timestamps do not regress; timestamp
   regression is exit `15` rather than sorting records into a possibly
   different causal order.
10. Build the complete `Vec<CanonicalRecord>` in memory, validating every
    record and source hash before writing stdout. Only after successful
    validation does the CLI write JSONL records and exit `0`.

`session_turns` rows are not used for content, source metadata, ordering,
or compaction cutoff in v1. D3 rejects `session_turns` ordering because
those rows lose content and usually lose `source_file` provenance. The
raw JSONL file is the transcript source of truth.

# 5. Exit codes

| Exit | Error code | Producing condition | Notes |
| --- | --- | --- | --- |
| `0` | none | Transcript emitted successfully. | Valid canonical JSONL on stdout; stderr empty except non-contract warnings are forbidden in v1. |
| `1` | `operational-error` | Read-only DB open/read failure, model-load failure, file I/O error not classified as unsupported storage, JSON serialization failure, or unexpected internal error. | No partial stdout. |
| `2` | `invalid-session-id` or clap usage | Invalid UUID, missing args, or unsupported `--format`. | Invalid UUID should use stderr JSON; clap structural errors may use clap formatting. |
| `10` | `session-not-found` | `locate_session_metadata` returns session not found. | Inherits resolver semantics; no fallback to `session_turns`. |
| `11` | `ambiguous-session` | `locate_session_metadata` returns ambiguous session. | Inherits `StateDb::resolve_resume` ambiguity semantics. |
| `12` | `unsupported-storage` | Located session has unsupported storage type, no available canonical JSONL path, non-UTF-8 path, or no v1 parser. | `other` storage exits here. |
| `15` | `malformed-transcript` or `unsupported-record` | Native JSONL is malformed, a required supported record field is missing, timestamps regress, or a provider-native transcript record cannot be represented safely. | No partial stdout. |

D6 decision: use the exit-code table above. The initiative-wide namespace
says `15` may cover invalid input or preimage mismatch for sibling
features; export uses `15` only for malformed provider transcripts or unsafe
unsupported records, matching the harness export request.

# 6. Reusable canonical-transcript reader API

Create a new library module:

```text
src-tauri/src/session_export/
```

Expose it from `src-tauri/src/lib.rs`:

```rust
pub mod session_export;
```

Public types:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanonicalRecord {
    pub session_id: String, pub provider_name: String, pub turn_id: String,
    pub role: CanonicalRole, pub timestamp: String,
    pub content: Vec<ContentChunk>,
    pub source: RecordSource, pub unsupported_record: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CanonicalRole { System, User, Assistant, Tool, Unknown }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentChunk {
    Text { text: String },
    ToolCall { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_call_id: String, text: String, is_error: bool },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordSource {
    pub storage_type: SessionStorageType, pub jsonl_path: PathBuf,
    pub line: u64, pub byte_start: u64, pub byte_end: u64, pub sha256: String,
}

#[derive(Debug, Clone)]
pub enum ExportError {
    UnsupportedStorage { storage_type: SessionStorageType, reason: String },
    MalformedTranscript { path: PathBuf, line: Option<u64>, reason: String },
    UnsupportedRecord { path: PathBuf, line: u64, reason: String },
    Io { path: PathBuf, message: String },
    Operational { message: String },
}
```

D7 decision: the public reader shape is:

```rust
pub fn read_canonical_transcript(
    metadata: &SessionMetadata,
) -> Result<Vec<CanonicalRecord>, ExportError>
```

This chooses a `Vec` rather than returning a streaming iterator because
the CLI must guarantee no partial stdout on parse error. Later internal
helpers may stream source lines into parser state, but the public API
returns a fully validated transcript. `06-import-replace` can use the same
types to parse replacement input and compare post-replace export output.

Private modules are `mod.rs`, `jsonl.rs`, `claude_code.rs`, and
`codex_session.rs`. `jsonl.rs` owns byte tracking and SHA-256; storage
modules own native mapping and compaction detection.

# 7. Anti-scope

- No provider spawn, auto-resume, provider login/auth refresh, quota
  refresh, provider selection, diagnostics, or model discovery.
- No DB writes, transcript writes, temp files, adapter cursor writes, state
  repair, scans, turn scripts, migrations, or pause/resume lock commands.
- No fallback to `session_turns` for content, source metadata, ordering, or
  parser dispatch; no `SessionStorageType::Other` parser in v1.
- No alternate formats such as pretty JSON, Markdown, provider-native JSONL,
  or packed archives.
- No guarantee that canonical JSONL is byte-for-byte provider-native JSONL.
- No import/replace, append, truncate, rewrite, normalization-in-place, or
  transcript migration.
- No GUI/Tauri frontend surface.
- No preservation of provider-private metadata beyond the canonical schema.

# 8. Side-effect contract

`agents session export` does not:

- Insert/update/delete rows in `state.db`, write cursors, or emit durable
  telemetry/invocation/trace/cache state.
- Modify transcript bytes, permissions, mtimes, parent directories, temp
  files, or replacement files.
- Launch providers, provider maintenance commands, turn scripts, scans, or
  quota refresh jobs.

The command may run the configured transcript locator only through
`locate_session_metadata`, because that is already part of the current
trace/session contract and 06-locate API. Export depends on the
06-schema-probe read-only state open. `agents session export` may create
the locator adapter `state_dir` directory
(`src-tauri/src/sessions/mod.rs:184-185`) when `locate_transcript` is
invoked. This directory creation is the same behavior `trace --json` and
`agents session locate` already exhibit and is part of the existing
transcript-locator contract that the harness anti-scope explicitly permits
("Running configured transcript locators is allowed only if already part of
the current trace/session contract"). No file inside the directory is
written by `export`.

The CLI writes stdout only after the complete canonical transcript has been
validated. Error exits write no canonical records to stdout.

# 9. Test-intent track

| Change risk or verification risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| CLI surface and format parsing | `agents session export <uuid>` defaults to `canonical-jsonl`; `--format canonical-jsonl` succeeds; other formats exit `2`. | unit + end-to-end | Clap parser tests plus CLI invocation fixture. | A1 | Valid forms parse; invalid format produces usage failure; no stdout records. | Clap structural formatting may differ from JSON stderr. |
| Locate reuse and resolver pass-through | Known session resolves through `locate_session_metadata` and exports records for that active provider/session. | particular-integration | Temp read-only DB/config fixture from locate tests plus located JSONL. | A1, A2, A3 | Exit `0`; records carry expected `session_id` and `provider_name`. | Does not prove every locate failure mapping. |
| Not found / ambiguous / unsupported storage mapping | Locate errors map to exits `10`, `11`, and `12` with no partial stdout. | component + particular-integration | Metadata API stubs or seeded DB/config cases from locate fixtures. | A1, A5 | Exit codes and stderr `code` match §5. | Stubbed metadata errors may miss config-load edge cases. |
| D1 source offsets and SHA-256 | Every emitted record includes line, byte range, and SHA-256 of exact source bytes excluding line terminator. | unit | `jsonl.rs` fixtures with LF, CRLF, whitespace lines, and multibyte UTF-8. | A4, A8 | Byte offsets and hashes match independently computed fixtures. | Does not fuzz every malformed UTF-8 boundary. |
| D2 text/system/tool shape | Text, system, tool call, and tool result records map to typed content chunks without flattening. | component | Claude and Codex provider JSONL fixtures with representative native message shapes. | A5 | JSONL chunks match schema; roles are expected. | Real provider schema drift can invalidate fixtures. |
| Unsupported native record policy | Safe unsupported transcript records emit `unsupported_record: true`; unsafe/malformed records exit `15`. | component | Parser fixtures for known unsupported-but-session-bound records and malformed required fields. | A5 | Safe case emits placeholder; unsafe case returns `ExportError` and CLI exit `15`. | Boundary between bookkeeping and transcript records may need Phase 5 refinement. |
| D3 no `session_turns` reconstruction | Export succeeds from raw JSONL even when `session_turns.source_file` is empty; content is not read from DB rows. | particular-integration | Seed chain/segment metadata and summary rows with empty source_file; provide located JSONL. | A3 | Exported content matches JSONL, not summary table. | Does not prove absence of every incidental DB query. |
| D4 Claude compaction live-state export | With multiple Claude compaction boundaries, output starts at latest boundary and includes the boundary summary. | component | Claude JSONL fixture with pre-boundary turns, two `isCompactSummary` lines, and post-boundary turns. | A7 | First emitted `source.line` equals latest boundary line; earlier records omitted. | Claude marker drift remains a residual. |
| D4 no boundary full export | When no supported compaction boundary exists, export emits all mappable records. | component | Claude and Codex fixtures without compaction markers. | A7 | First emitted record is first transcript record. | Codex unknown compaction remains a residual. |
| D5 ordering and timestamp validation | Emission follows JSONL file order; timestamp regressions fail with exit `15`. | component | Parser fixtures with equal timestamps, increasing timestamps, and regressing timestamps. | A6 | Equal/increasing order preserved; regression returns malformed transcript. | Real transcripts with benign clock skew would be rejected. |
| No partial stdout on parser error | A malformed later line causes exit `15` and zero stdout bytes. | end-to-end | CLI fixture with first valid record then malformed transcript record. | A4, A5 | stdout empty; stderr JSON code `malformed-transcript`. | Requires test harness to capture stdout exactly. |
| Read-only behavior | Export does not mutate DB rows, transcript mtimes, adapter state, or temp dirs. | particular-integration | Snapshot DB row counts, file mtimes, and directory listings before/after export. | A2 | Snapshots unchanged after command. | Filesystem timestamp resolution can hide very small changes. |
| README examples remain truthful | README documents synopsis, JSONL schema, source hash semantics, and exit codes. | documentation check | README grep/snapshot or Phase 6b manual residual if no doc-test convention. | none | Docs mention `session export`, `canonical-jsonl`, source offsets/hash, and no partial stdout. | Examples may not execute against real CLI. |

New fixture infrastructure expected: byte-level JSONL fixtures, parser-level
Claude/Codex transcript fixtures, and a CLI fixture that can route
`locate_session_metadata` to temp config/state without touching real user
state. Phase 6b should record any unverified parser-drift cases in
`risk/06-export-test-residuals.md`.

# 10. README updates

Update `README.md` in the CLI sections:

- Add `session export <session-id> [--format canonical-jsonl]` near the
  existing `session locate` documentation.
- Explain JSONL output, empty stdout on error, `canonical-jsonl` as the only
  v1 format, and every required field from §3.
- Document source conventions: SHA-256 of exact native JSONL line bytes
  excluding the terminator, 1-based line, 0-based inclusive/exclusive bytes.
- Document compaction behavior: live transcript from latest supported
  boundary; full transcript when no supported boundary exists.
- Document exit codes `0`, `1`, `2`, `10`, `11`, `12`, and `15`.
- Clarify that `trace --json` and `session locate` do not emit transcript
  content; `session export` is the supported harness transcript reader.

# 11. Supported-surface track

## 11.1 Supported-surface track

Deployment mode: local CLI binary only. No GUI command, no Tauri frontend
surface, no daemon, and no server.

Customer cohort: `agent-harness` is the primary consumer, replacing direct
provider JSONL parsing in `SessionOverrideContract.read_transcript`.
Secondary consumers are local scripts that need auditable canonical
transcript records by session id.

Adjacent public/user-reachable paths and blast-radius notes: `session
locate` remains metadata-only; resume/repl paths keep launching providers
through existing code; `trace --json` remains invocation-tree scoped and
placeholder-only for inline transcripts; `migrate-db` and `migrate-config`
are not called or coupled; direct CLI ingestion remains adapter-script
based; `06-import-replace` later consumes this record family but is not
implemented here.

Migration path: no user state migration is required. Existing sessions are
exportable only if locate can resolve them and their current provider JSONL
matches a v1 parser. Unsupported or drifted transcripts fail closed.

Rollback path: uninstall/revert the binary or avoid the new subcommand. The
command is additive and writes no durable state, so rollback has no data
cleanup step.

Observability: success JSONL and stderr JSON errors are the whole observable
surface. Export emits no telemetry, invocation rows, state markers, or
durable logs in v1.

# 12. Implementation residuals

Known residuals Phase 4 should evaluate rather than treat as omissions:

- Parser drift is the largest residual. Claude Code and Codex JSONL are
  private upstream formats; v1 fixtures reduce but do not eliminate drift
  risk.
- Codex compaction is not live-state aware in v1 because no stable marker is
  currently known. Codex exports full mappable transcript unless Phase 5
  proves a marker and this proposal is revised.
- Timestamp regression is fail-closed. A real provider transcript with valid
  causal order but regressing timestamps would exit `15`.
- Export buffers the whole canonical transcript to avoid partial stdout.
  Very large transcripts pay memory cost proportional to exported records.
- `sha2` must become a direct dependency unless Phase 5 identifies an
  existing project-approved SHA-256 helper.
- `SessionStorageType::Other` is rejected in v1 even if a locator can return
  a file path, because there is no parser contract for unknown native JSONL.
- The proposal does not add provider-native payload preservation for
  unsupported records. It emits canonical placeholders only when safe.

# 13. Cross-feature constraint compliance

| Constraint | Compliance | Citation / note |
| --- | --- | --- |
| Shared error namespace uses `10`, `11`, `12`, and `15` for export-relevant cases. | Yes | Mapping in §5; initiative namespace at `initiatives/06-session-override-contract.md:106-111`. |
| Ownership resolution reuses `StateDb::resolve_resume`; no second ownership path. | Yes | Export calls `locate_session_metadata`; no direct `session_turns` fallback (§4 steps 5 and 10). |
| Read-only `StateDb` open variant lands in 06-schema-probe. | Yes | Export depends on that variant (§4 step 3, §8). |
| No auto-resume. | Yes | §7 and §8 forbid provider launch/resume. |
| No provider spawn. | Yes | §7 and §8. |
| No quota refresh. | Yes | §7 and §8. |
| No config edits. | Yes | §7 and §8. |
| No coupling to `migrate-config`. | Yes | §7 and §11. |
| Export establishes canonical reader for import-replace round-trip. | Yes | `CanonicalRecord`, `RecordSource`, `ExportError`, and `read_canonical_transcript` are defined in §6. |
| Harness receives canonical JSONL rather than provider-native JSONL. | Yes | §3 schema and §7 anti-scope. |
