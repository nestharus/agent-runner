# Phase 6 Step 6a — Contract for `agents session export`

This contract bridges `proposals/06-export.md` (Rev 2) and Phase 6
implementation. Step 6b (test writer) and Step 6c (code writer)
read this contract; neither needs the proposal.

## 1. CLI surface

```text
agents session export <session-id> [--format canonical-jsonl]
```

`--format canonical-jsonl` is the only accepted format and the
default. Bare `agents session` exits with clap usage error
code `2`.

Clap shape — extend `SessionSubcommands`:

```rust
enum SessionSubcommands {
    Locate { ... },
    SchemaProbe,
    Export { session_id: String, #[arg(long, default_value = "canonical-jsonl")] format: String },
}
```

Stdout output: line-delimited JSON. Each line is one canonical
record (see §3). Stderr error: single JSON line on failure.

## 2. Public types (new module `src-tauri/src/session_export/mod.rs`)

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CanonicalRecord {
    pub session_id: String,
    pub provider_name: String,
    pub turn_id: String,
    pub role: String,                     // "user" | "assistant" | "system" | "tool" | "tool_result"
    pub timestamp: String,                // ISO-8601
    pub content: Vec<ContentChunk>,
    pub source: RecordSource,
    pub unsupported_record: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ContentChunk {
    pub r#type: String,                   // "text" | "tool_use" | "tool_result" | etc.
    pub text: Option<String>,
    // future fields optional
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecordSource {
    pub storage_type: String,             // "claude_code" | "codex_session" | "other"
    pub jsonl_path: PathBuf,
    pub line: u64,                        // 1-indexed
    pub byte_start: u64,
    pub byte_end: u64,
    pub sha256: String,                   // sha256 of the JSONL line bytes
}

pub enum ExportError {
    InvalidSessionId { input: String },
    SessionNotFound { input: String },
    AmbiguousSession { input: String },
    UnsupportedStorage { provider_name: String, reason: String },
    MalformedTranscript { path: PathBuf, line: u64, reason: String },
    Operational { message: String },
}

pub fn read_canonical_transcript(
    metadata: &SessionMetadata,  // from session_metadata module
) -> Result<impl Iterator<Item = Result<CanonicalRecord, ExportError>>, ExportError>;
```

The CLI wrapper owns clap parsing, calling
`locate_session_metadata` for ownership, then calling
`read_canonical_transcript` and emitting JSONL.

## 3. Canonical record JSON shape (per line)

Each stdout line is a compact JSON object with all 8 fields above.
Trailing newline after each record. Empty stdout for failures.

`source.byte_start`/`byte_end` are 0-indexed file byte offsets of
the line's contents (excluding the trailing newline).
`source.sha256` is the SHA-256 of the line bytes (lowercase hex).
`source.line` is 1-indexed line number within the JSONL file.

`unsupported_record: true` when the provider-native record cannot
be canonicalized: emit a placeholder canonical record with empty
`content`, set `unsupported_record: true`, and continue. Do NOT
exit.

If a record is so malformed that `source.byte_start`/`byte_end`/
`sha256` cannot be computed (parse error), the entire command
exits `15 malformed-provider-transcript` with stderr JSON before
any stdout output (per harness §Exit codes 15).

## 4. Resolution flow

1. Parse `<session-id>` as UUID; on failure exit `2`.
2. Open `StateDb::open_default()`.
3. Load configs via `unwrap_or_default` (matching resume).
4. Call `session_metadata::locate_session_metadata(...)`. Map
   `MetadataError` → `ExportError`:
   - `InvalidSessionId` → exit `2`
   - `SessionNotFound` → exit `10`
   - `AmbiguousSession` → exit `11`
   - `UnsupportedStorage` → exit `12`
   - `Operational` → exit `1`
5. Call `read_canonical_transcript(&metadata)`.
6. Stream records to stdout as JSONL. Buffer-and-validate first
   line OR streaming with rollback on early error (D1 from
   proposal — verify the choice).
7. On `MalformedTranscript`: exit `15`, no partial stdout (per
   D1 buffer-and-validate).

## 5. Exit codes

| Exit | Trigger |
| --- | --- |
| `0` | Success; complete JSONL on stdout. |
| `1` | DB open / config load / IO failure. |
| `2` | Clap usage error or invalid session-id. |
| `10` | `session-not-found`. |
| `11` | `ambiguous-session`. |
| `12` | `unsupported-storage` (no canonical reader for storage). |
| `15` | `malformed-provider-transcript` (parser failure or unsupported_record=true count exceeds threshold per proposal). |

Stderr format: `{"error": {"code": "<code>", "message": "..."}}`.

## 6. Storage-specific parsing

Per proposal §6 / D3: parsers live in `src-tauri/src/session_export/`.

- `parse_claude_code_jsonl(path: &Path) -> impl Iterator<Result<CanonicalRecord, ExportError>>`:
  reads Claude Code JSONL, maps each `user`/`assistant` record
  to a `CanonicalRecord`. Record types other than user/assistant
  emit `unsupported_record: true`.
- `parse_codex_rollout_jsonl(path: &Path) -> impl Iterator<Result<CanonicalRecord, ExportError>>`:
  reads Codex rollout JSONL, walks records, maps each message
  record. Skip `session_meta` (not a turn).

Both parsers compute `source.byte_start`, `byte_end`, `sha256`
during the line read.

## 7. Side-effect contract

`agents session export`:

**Permitted:**
- Read state DB.
- Read provider transcript JSONL files.
- Run configured `transcript_locator` (already part of trace
  contract; may create `STATE_DIR` per §8 of proposal).

**Forbidden:**
- INSERT/UPDATE/DELETE on any table.
- Provider commands, quota refresh, auth flow.
- Migration, scan.
- Config edits.
- Telemetry, invocation rows.
- Mutating the original transcript file.

## 8. Test-intent track

Per proposal §9.1. T1-T9:

| ID | Risk | Level | Fixture |
| --- | --- | --- | --- |
| T1 | Resolver pass-through; Claude Code session emits canonical JSONL with all 8 fields per line | particular-integration | Temp DB + Claude Code JSONL fixture; binary spawn |
| T2 | Codex session emits canonical JSONL (same shape) | particular-integration | Temp DB + Codex rollout JSONL fixture |
| T3 | Source preimage: line/byte_start/byte_end/sha256 correct | component | Direct parser call; verify each line's source matches file content |
| T4 | Stable chronological order within canonical transcript | component | Multi-record fixture; verify emission order |
| T5 | `unsupported_record: true` for provider-native records that don't map | component | Fixture with system/tool record |
| T6 | Malformed provider transcript exits 15 with no partial stdout | particular-integration | JSONL with garbled mid-stream record |
| T7 | Read-only: command does not mutate state.db rows / cursors / transcript / config | particular-integration | Snapshot row counts + transcript mtime; run; re-snapshot |
| T8 | Compaction-aware emission per D4 (post-compaction or pre-compaction per proposal choice) | component | Temp DB with `is_compaction_boundary` row; verify emission |
| T9 | Resolver-error mapping: missing/ambiguous/unsupported propagate to exit 10/11/12 | particular-integration | Temp DB variants |

## 9. Process-tree audit obligations

Step 6b and Step 6c are separate agent invocations. Step 6c MUST
write `.tmp/phase6/step6c-reads.md` BEFORE any product-code change.

Output index at `.tmp/phase6/step6b-output-index.md`.

After Step 6c, run `process-tree-auditor` on the Phase 6 subtree.

## 10. References

- Proposal: `proposals/06-export.md` (Rev 2).
- Hookpoints: `research/06-export-hookpoints.md`.
- 06-locate's `SessionMetadata` (consumed): `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs`.
- 06-locate's contract pattern: `/home/nes/projects/agent-runner/worktrees/06-locate/research/06-locate-contract.md`.

## 11. Cross-feature dependency

Crucially: 06-export depends on 06-locate's `SessionMetadata` API.
The `read_canonical_transcript` function takes a `&SessionMetadata`
parameter. Step 6c needs to either:

- (a) Reproduce the `SessionMetadata` types in 06-export's tree
  (since 06-export branches off `main`, not `06-locate`), OR
- (b) Stack 06-export on top of 06-locate, OR
- (c) Define a minimal local equivalent (`ExportSessionMetadata`
  with the fields export needs: `provider_name`, `chain_id`,
  `session_id`, `jsonl_path`, `storage_type`).

**Choose (c) for v1**: define a minimal `read_canonical_transcript`
input shape in 06-export. When 06-locate merges, a follow-up PR
unifies the type.
