# 06-export — Phase 8 Supported-Surface Verification (PR)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Re-capture against branch tip `fc59558` (Phase 6 process-tree audit
PASS-WITH-ADVISORY). The PR adds one additive subcommand
(`agents session export`), one new library module
(`src-tauri/src/session_export/`), and one new direct dependency
(`sha2`). No adjacent module is touched: `git diff main..06-export`
shows zero edits in `src-tauri/src/state/`, `src-tauri/src/sessions/`,
`src-tauri/src/trace/`, `src-tauri/src/migration/`, or
`src-tauri/src/config/`. The Phase 4 Rev 2 LOW verdict carries
forward; A1–A8 hold against the diff (with A1 reading per the Step
6a contract option (c) carve-out, see "Cross-feature note"
below). All ten enumerated adjacent paths are PRESERVED or UNCOUPLED.
§11.1's three load-bearing claims (no user state migration,
uninstall/avoid rollback, observability = success JSONL + stderr
JSON) are verifiable from the diff. Termination signals do not fire.
One advisory finding on README documentation deferral is recorded
below (matches 06-locate's pattern of landing the README addition
in a separate follow-up commit).

## A1-A8 verification against diff

| ID | Verdict | Diff evidence |
| --- | --- | --- |
| A1 (06-locate lands before export and supplies `SessionMetadata` + `session` command group) | HOLDS via Step 6a option (c) | The implementation chose Step 6a contract §11 option (c) — define a minimal local equivalent — rather than block on 06-locate. `src-tauri/src/session_export/mod.rs:53-61` declares `ExportSessionMetadata { session_id, chain_id, provider_name, storage_type, jsonl_path }`; `src-tauri/src/main.rs:319-324` adds `Subcommands::Session { command: SessionSubcommands }` directly. The Phase 4 Rev 2 invalidator wording ("Export is merged independently before locate") is therefore not a contract violation — the contract pre-approved option (c) and named "follow-up PR unifies the type" as the merge-time path. Forward-compat preserved: `ExportSessionMetadata`'s field set is a strict subset of locate's `SessionMetadata` (locate adds `workspace_root`, `transcript_state`, `mutable`), so a later switch to `&SessionMetadata` is a non-breaking signature change. |
| A2 (`06-schema-probe` lands before export and provides read-only `StateDb` open) | HOLDS as known residual | `src-tauri/src/main.rs:524` calls `StateDb::open_default()` (today's mutating open: WAL, schema-ensure, backfill). This is the same situation 06-locate was in at PR time — locate's PR review marked A6 as HOLDS with the explicit note that `open_default`'s physical write cost is "deferred to 06-schema-probe per §12". Proposal §12 lists schema-probe as a known residual; §11.1's "no user state migration" claim is preserved because the `open_default` mutations are pre-existing and not specific to export. No new mutation is introduced by export. |
| A3 (canonical source is provider JSONL path, not `session_turns`) | HOLDS | `read_canonical_transcript` (`src-tauri/src/session_export/mod.rs:88-99`) dispatches to `parse_claude_code_jsonl` or `parse_codex_rollout_jsonl`; both call `scan_jsonl(metadata.jsonl_path)` (lines 104, 169) and read raw file bytes only. No reference to `session_turns`, `session_chains`, `session_chain_segments`, or `latest_compaction_boundary` exists in `session_export/`. The CLI wrapper at `src-tauri/src/main.rs:539-561` likewise reads only `resolved.active_provider`/`active_session_id` from `resolve_resume` for ownership; content always comes from the JSONL file. |
| A4 (storage type is enough to choose v1 parser) | HOLDS | `read_canonical_transcript` matches on `metadata.storage_type` only (`src-tauri/src/session_export/mod.rs:91-98`). `resolve_export_session_metadata` derives `storage_type` from `provider_entry.session_storage` directly (`src-tauri/src/main.rs:601-617`): `Some(ClaudeCode { .. })` → `ClaudeCode`; `Some(Codex { .. })` → `CodexSession`; `None` → fail-closed `UnsupportedStorage`. Storage vocabulary serializes as `claude_code`, `codex_session`, `other` (`src-tauri/src/session_export/mod.rs:38-52`), matching locate's vocabulary. |
| A5 (per-record source metadata can be computed at read time) | HOLDS | `scan_jsonl` (`src-tauri/src/session_export/mod.rs:260-308`) tracks `(line_no, byte_start, byte_end, sha256, value)` per record: byte-level scanner (not `BufRead::lines()`); LF and CRLF terminator handling at line 274; final-unterminated-line valid; whitespace-only lines skipped at line 278; `byte_start`/`byte_end` exclude the terminator; `sha256` is over the exact `&bytes[start..end]` slice via `Sha256::digest` (line 416). No DB column, no provider command, no locator script reuse for source preimage. |
| A6 (provider JSONL line order is the stable conversation order) | HOLDS | `parse_claude_code_jsonl` and `parse_codex_rollout_jsonl` walk `scan_jsonl(...)`'s output in source order; emission order is preserved into the returned `Vec<CanonicalRecord>`. `validate_timestamp_order` (`src-tauri/src/session_export/mod.rs:333-355`) enforces non-decreasing RFC3339 timestamps; equal timestamps are accepted; regression returns `MalformedTranscript` → exit `15`. No timestamp sort, no causal reordering. |
| A7 (Claude compaction via `isCompactSummary == true`; Codex none in v1) | HOLDS | `parse_claude_code_jsonl` (`src-tauri/src/session_export/mod.rs:151-161`) tracks `latest_compaction_boundary` as the latest record where `isCompactSummary == true`; on completion, all earlier records are skipped via `records.into_iter().skip(index)`; the boundary record itself is the first emitted, matching proposal §4 D4. `parse_codex_rollout_jsonl` (lines 166-236) has no compaction handling and emits the full mappable transcript — matching A7 and proposal §12's residual on Codex compaction. |
| A8 (`sha2` available as direct dependency) | HOLDS | `src-tauri/Cargo.toml` diff adds `sha2 = "0.10"` as a direct dependency. `sha2_hex` at `src-tauri/src/session_export/mod.rs:416-419` uses `Sha256::digest`. CodeRabbit Pass 1 R2-F05 already replaced an earlier handwritten implementation (per `risk/06-export-audit-history.md:23-24`). |

**Termination signal #1 (`invalidated-assumption`) does not fire.**
A1's "merged independently" branch was the contract-sanctioned path,
not a contract violation. A2's deferral matches locate's accepted
residual. A3-A8 hold strictly.

## Adjacent-path blast-radius

`git diff main..06-export` touches only `proposals/`, `research/`,
`risk/`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`,
`src-tauri/src/lib.rs` (one `pub mod session_export;` line),
`src-tauri/src/main.rs` (additive `Session` subcommand + dispatch +
helpers), the new `src-tauri/src/session_export/mod.rs`, and new
test files under `src-tauri/tests/`. No source file under
`src-tauri/src/state/`, `src-tauri/src/sessions/`,
`src-tauri/src/trace/`, `src-tauri/src/migration/`, or
`src-tauri/src/config/` is modified.

| Path | Verdict | Diff evidence |
| --- | --- | --- |
| `agents resume` | PRESERVED | The `Resume` arm and `run_resume`/`resume_execution_target` paths in `src-tauri/src/main.rs` are not modified by this diff. Only the new `Session` arm is added at line 319-324 and the new dispatch arm at line 320-324. |
| `agents repl --resume` | PRESERVED | The `Repl` subcommand and `run_repl` wrapper are not touched. |
| Top-level `--resume` | PRESERVED | Top-level dispatch (the existing flag-based resume) is not modified. |
| `agents trace --json` | PRESERVED | `src-tauri/src/trace/mod.rs` is bit-for-bit untouched (`git diff` shows no changes). `TraceSession`'s serialized JSON shape is bit-stable. |
| `agents migrate-config` | UNCOUPLED | `MigrateConfig` arm and `run_migrate_config` are not touched. |
| `agents migrate-db` | UNCOUPLED | `MigrateDb` arm and `run_migrate_db` are not touched. |
| Hidden `agents resume-list` | PRESERVED | `ResumeList` arm and `run_resume_list` are not touched. |
| Direct CLI ingestion (`scan_provider`, adapter scripts) | PRESERVED | `src-tauri/src/sessions/mod.rs` is not touched. Export consumes `locate_transcript` only via `agent_runner_lib::sessions::locate_transcript` (`src-tauri/src/main.rs:619-625`); the function itself is unchanged. The `STATE_DIR` mkdir behavior at `sessions/mod.rs:184-185` continues to apply, matching the §8 carve-out the proposal explicitly permits. |
| Future `agents session locate` (06-locate stacked) | FORWARD-COMPAT-WITH-MERGE-CONFLICT | The PR adds `Subcommands::Session { command: SessionSubcommands }` and `SessionSubcommands::Export` here; 06-locate adds the same `Session` parent variant and `SessionSubcommands::Locate`. At merge time these will conflict structurally (clap enum membership), but both branches add to the same enum non-divergently; resolution is mechanical (union the variants). The supported-surface contract is not violated — both branches converge on the same parent group. |
| Future `06-import-replace` consumer surface | FORWARD-COMPAT | `CanonicalRecord`, `ContentChunk`, `RecordSource`, and `ExportError` are `Serialize + Deserialize` (`src-tauri/src/session_export/mod.rs:8-86`). The `read_canonical_transcript(&ExportSessionMetadata)` signature is the documented v1 reader API. Field set matches proposal §6. |

No path is BROKEN or DEGRADED.

## §11.1 claims verification

- **Deployment mode (local CLI binary only)**: VERIFIED. The diff
  adds zero Tauri/GUI surface, zero daemon, zero server. The new
  module is exposed only via `pub mod session_export;` for in-crate
  consumption and the `agents session export` clap subcommand for
  user-reachable invocation.
- **No user state migration**: VERIFIED. The diff adds zero schema
  migration, zero new DB column, zero config rewrite, zero state
  rewrite. The mutating writes that occur during
  `StateDb::open_default()` (WAL, schema-ensure, chain backfill)
  pre-exist export and are deferred to 06-schema-probe per §12.
- **Existing sessions are exportable only if locate can resolve them
  and their current provider JSONL matches a v1 parser**: VERIFIED.
  `resolve_export_session_metadata` (`src-tauri/src/main.rs:521-637`)
  inlines locate's resolution flow (UUID parse → resolve_resume →
  storage_type from provider_entry.session_storage → locate_transcript)
  and fails closed at every step: invalid UUID → exit `2`, no chain
  → exit `10`, ambiguous → exit `11`, missing/unconfigured storage
  or no locator → exit `12`. `SessionStorageType::Other` would also
  fail closed at the parser dispatch (`session_export/mod.rs:94-97`).
- **Rollback path: uninstall/revert binary or avoid the new
  subcommand**: VERIFIED. The new module file
  (`src-tauri/src/session_export/mod.rs`), the `pub mod` line in
  `src-tauri/src/lib.rs`, the `Session` clap variant, the
  `SessionSubcommands::Export` variant, the `Session` dispatch arm,
  the `run_session_export` and helpers, and the `sha2` Cargo
  dependency form a strictly additive set; reverting them leaves the
  rest of the binary functionally unchanged. No durable user-state
  cleanup is required.
- **Observability: success JSONL on stdout, stderr JSON errors;
  no telemetry / invocation rows / state markers / durable logs**:
  VERIFIED. The implementation paths in `run_session_export`
  (`src-tauri/src/main.rs:503-547`) are: `Uuid::parse_str`,
  `StateDb::open_default`, `load_models`, `ProvidersConfig::load`,
  `SessionsConfig::load`, `state.resolve_resume`, `locate_transcript`,
  `fs::read`, `serde_json::from_str`, `serde_json::to_string`,
  `print!`, `eprintln!`. No invocation row writer, no trace record
  writer, no `auth_refresh_command`, no provider spawn, no quota
  refresh, no JSONL copy/create/rename/truncate. The T7 test
  (`src-tauri/tests/initiative_06_export.rs:206-219`) snapshots
  table counts across `invocations`, `session_turns`, `session_chains`,
  `session_chain_segments`, `provider_quotas`, `provider_quota_windows`,
  plus transcript bytes/mtime and config bytes, and asserts strict
  equality before/after.
- **No partial stdout on parse error**: VERIFIED. `run_session_export`
  builds a complete `output: String` (`src-tauri/src/main.rs:539-547`)
  by serializing every `CanonicalRecord` first; only after the loop
  exits without `?`-propagating an error does it call `print!`.
  `read_canonical_transcript` itself returns a fully validated
  `Vec<CanonicalRecord>` (`session_export/mod.rs:88-99`) — any
  malformed line, timestamp regression, or required-field miss
  surfaces as `Err(ExportError)` before record collection completes.
  T6 (`src-tauri/tests/initiative_06_export.rs:184-198`) asserts
  exit `15` with empty stdout for a malformed mid-stream record.

## Side-effect contract verification

§7 (anti-scope) and §8 (forbidden side effects) hold against the
diff:

- **No INSERT/UPDATE/DELETE on state.db**: `session_export/mod.rs`
  contains zero SQL. `run_session_export` and its helpers do not
  call `INSERT`, `UPDATE`, `DELETE`, `scan_provider`,
  `migrate_chain_segment`, `record_*`, or any quota/auth refresh
  path.
- **No transcript mutation**: `scan_jsonl` calls `fs::read` only
  (`session_export/mod.rs:261`). No `OpenOptions::new().write(true)`,
  no `fs::write`, no `fs::rename`, no `fs::create`, no `fs::set_permissions`,
  no temp file under the transcript path.
- **No provider spawn / auth refresh / quota refresh / migration /
  scan / config edit**: the dispatch arm calls `run_session_export`
  only; `run_session_export`'s call graph terminates at I/O reads,
  serde, and println.
- **`STATE_DIR` mkdir carve-out**: `locate_transcript`
  (`sessions/mod.rs:184-185`) does create the configured `state_dir`
  directory before invoking the locator script. This is the proposal
  §8 carve-out, lifted verbatim from 06-locate's already-Phase-8-LOW
  language. The T7 read-only test (`tests/initiative_06_export.rs:206-219`)
  snapshots transcript bytes/mtime and config bytes, but
  intentionally does not snapshot the locator state directory
  itself, matching the documented residual at `tests/...:204` ("the
  contract permits the transcript locator to create STATE_DIR").

## Cross-feature note: contract option (c) carve-out

The Step 6a contract (`research/06-export-contract.md` §11)
explicitly authorized the option (c) implementation chosen here:
"Define a minimal local equivalent (`ExportSessionMetadata` with the
fields export needs: `provider_name`, `chain_id`, `session_id`,
`jsonl_path`, `storage_type`)" with a follow-up unification PR after
06-locate merges. The actual `ExportSessionMetadata` structure
(`session_export/mod.rs:54-61`) matches that authorized field set
exactly. This is not a deviation from Phase 4 Rev 2 — Phase 4 Rev 2's
A1 invalidator named "Export is merged independently before locate"
as a fork point that would have to be addressed; the Step 6a
contract is the document that resolves it. The Phase 4 supported-
surface report's prose was written assuming the locate-stacked path;
the PR is on the merge-independent path that the contract approved.

Two behavioral notes follow from the option (c) choice that future
reviewers should be aware of (none of which break the supported
surface in v1):

1. The implementation does not perform locate's path canonicalization
   (`fs::canonicalize`) on `jsonl_path`. The path emitted in
   `RecordSource.jsonl_path` is exactly what `locate_transcript`
   returned. If the locator script returns a non-canonical path
   (e.g., contains `..` or a symlink), it is preserved verbatim.
   This is a strict subset of locate's stricter contract; it does
   not break the export schema (`jsonl_path` is typed as `PathBuf`
   without a "canonical" qualifier in §3).
2. The implementation does not perform locate's workspace_root
   derivation/validation. A located JSONL whose `session_meta.payload.cwd`
   is missing/non-existent (Codex) — a case locate would reject
   with `unsupported-storage` — is exportable here as long as a
   matching `session_meta.payload.id` line exists. Workspace_root is
   not in the export output schema, so this only affects which
   sessions are exportable. Wider export surface, narrower locate
   surface; both fail closed where their respective contracts
   require.

When the locate→export unification PR lands, both behaviors will
align with locate's stricter shape automatically.

## Findings

- **F01 (advisory; carries through; non-blocking)** — README does
  not yet document `agents session export`. `Grep "session export"
  README.md` returns no matches. Proposal §10 lists README updates
  as part of the deliverable; the Phase 4 Rev 2 §9 test row noted
  this could land as a "Phase 6b manual residual if no doc-test
  convention" — and indeed 06-locate's branch tip in the locate
  worktree does include a README addition in a follow-up commit
  (`2605b37`). Recommend a follow-up commit on `06-export` mirroring
  06-locate's pattern: synopsis, JSONL schema, source hash semantics,
  exit codes `0`/`1`/`2`/`10`/`11`/`12`/`15`, no-partial-stdout,
  compaction policy. Not a contract drift; the implementation
  matches the proposal.

- **F02 (cosmetic; carries through from Phase 4; non-blocking)** —
  Problem-map §6 #7 ("storage-type support only indirectly
  observable") remains: there is no programmatic feature-flag
  surface for "which storage types export supports". Per
  `06-session-override-contract.md:44-50` that surface lives in
  06-schema-probe, not export. Recorded for completeness; not a
  finding against this PR.

- **F03 (cosmetic; carries through from Phase 4; non-blocking)** —
  A6's "real transcripts with benign clock skew would be rejected"
  trade is preserved by `validate_timestamp_order` returning
  `MalformedTranscript` on any timestamp regression
  (`session_export/mod.rs:343-351`). Worth surfacing in the README
  follow-up so harness consumers know that timestamp regression is
  a definitive failure rather than a normalization opportunity.

No new MEDIUM or HIGH supported-surface findings introduced by the
PR.

## Verdict rationale

**Termination signal #1 (`invalidated-assumption`)** does not fire.
A1 reads through Step 6a contract option (c); A2-A8 hold strictly
against diff evidence; the only A2 caveat is the same accepted
schema-probe residual that locate's PR review accepted.

**Termination signal #2 (`non-positive-value`)** does not fire. The
PR retires the same set of problem-map §6 entries Phase 4 Rev 2
counted, adds no new failure modes beyond the documented fail-closed
exits, and delivers the canonical-JSONL stdout surface that lets the
harness stop direct provider-JSONL parsing. Adjacent supported paths
are bit-stable; rollback is uninstall/avoid; observability is the
documented two-stream surface.

**Final verdict: LOW. Termination signal: none.**
