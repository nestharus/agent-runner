# Phase 6 Step 6a — Contract for `agents session import-replace`

This contract bridges `proposals/06-import-replace.md` (Rev 4) and Phase 6
implementation.

## 1. CLI surface

```text
agents session import-replace <session-id> [--from-file <path>] [--preimage-sha256 <hex>]
```

Input: canonical JSONL on stdin (default) or `--from-file <path>`.

Clap shape — extend `SessionSubcommands`:

```rust
enum SessionSubcommands {
    Locate { ... },
    SchemaProbe,
    Export { ... },
    PauseHandshake { ... },
    ResumeHandshake { ... },
    ImportReplace {
        session_id: String,
        #[arg(long)] from_file: Option<PathBuf>,
        #[arg(long)] preimage_sha256: Option<String>,
    },
}
```

## 2. Public types (new module `src-tauri/src/session_replace/`)

```rust
pub struct ReplaceReceipt {
    pub session_id: String,
    pub provider_name: String,
    pub storage_type: String,           // "claude_code" | "codex_session"
    pub operation: String,              // "import-replace"
    pub preimage_sha256: String,
    pub postimage_sha256: String,
    pub jsonl_path: PathBuf,
    pub state_updated: bool,
    pub committed_at: String,
}

pub enum ReplaceError {
    InvalidSessionId { input: String },
    SessionNotFound { input: String },
    AmbiguousSession { input: String },
    UnsupportedStorage { provider_name: String, reason: String },
    SessionBusy { token: String, expires_at: String },
    SchemaIncompatible { reason: String },
    InvalidInputTranscript { reason: String, line: Option<u64> },
    PreimageMismatch { expected: String, actual: String },
    OperationalError { message: String },
}

pub trait CanonicalToProviderRenderer {
    fn render(&self, records: &[CanonicalRecord]) -> Result<Vec<u8>, ReplaceError>;
}

pub struct ClaudeCodeRenderer;  // implements provider-native Claude JSONL
pub struct CodexSessionRenderer;  // implements provider-native Codex rollout JSONL

pub fn run_import_replace(
    session_id: &str,
    input_path_or_stdin: Option<&Path>,
    preimage_sha256: Option<&str>,
) -> Result<ReplaceReceipt, ReplaceError>;
```

## 3. Resolution flow (per Rev 4 §4)

```text
1. Read canonical JSONL input (stdin or --from-file). Validate each line per
   CanonicalRecord schema. Compute canonical_records_hash for journal use.
2. Write canonical records to per-process staging path:
   <state-data-dir>/replace_journal/staging/<operation_uuid>.canonical.jsonl
3. Resolve session ownership via SessionMetadata::locate_session_metadata.
   Map errors: InvalidUuid→2, NoChainFound→10, Ambiguous→11, UnsupportedStorage→12.
4. SessionLock::acquire(session_id, provider_name, default-ttl).
   On Busy → unlink staging file → exit 13.
5. Under lock: rename staging → per-session canonical records path:
   <state-data-dir>/replace_journal/session-<session_id>.canonical.jsonl
6. Write journal entry under lock:
   <state-data-dir>/replace_journal/session-<session_id>.pending
   Schema: {schema_version, operation, operation_uuid, started_at, session_id,
            chain_id, active_segment_id, provider_name, storage_type,
            jsonl_path, preimage_sha256, postimage_sha256_expected,
            canonical_records_path, db_state_pending: true,
            expected_turn_count}
7. Read existing transcript at jsonl_path; compute preimage_sha256.
   If --preimage-sha256 given and mismatch → exit 15 PreimageMismatch.
8. Render canonical → provider-native via CanonicalToProviderRenderer.
   Lossy records → exit 15 InvalidInputTranscript with reason.
9. Write rendered bytes to <jsonl_path>.replace-<operation_uuid>.tmp; fsync.
10. rename(<jsonl_path>.tmp, <jsonl_path>) — atomic.
11. SQLite BEGIN; replace session_turns rows for (provider_name, session_id);
    refresh segment last_used_at. Do NOT commit yet.
12. Compute postimage_sha256 by reading <jsonl_path>; verify matches expected.
    Mismatch → SQLite ROLLBACK; LEAVE journal in place; exit 1 operational.
13. Run fresh export verification: parse <jsonl_path> through canonical reader;
    compare to canonical_records_path. Mismatch → ROLLBACK; LEAVE journal;
    exit 1 operational.
14. SQLite COMMIT.
15. Delete journal entry + canonical records file (idempotent unlink).
16. Release SessionLock.
17. Emit ReplaceReceipt JSON to stdout; exit 0.
```

## 4. Crash recovery on agent-runner startup

Scan `<state-data-dir>/replace_journal/session-*.pending`:
For each entry (parsed JSON):
1. Read transcript at `jsonl_path`; compute hash.
2. If hash == `postimage_sha256_expected`:
   - Re-apply DB updates from `canonical_records_path`: replace session_turns
     for (provider_name, session_id); refresh segment last_used_at.
   - Delete journal + canonical records file.
   - Log recovery success.
3. If hash == `preimage_sha256`:
   - Rename never landed (or rolled back). Delete journal + canonical records.
4. Else (ambiguous):
   - Move journal + canonical records to `<state-data-dir>/replace_journal/quarantine/`.
   - Log warning.

## 5. Exit codes

| Exit | Trigger |
|---|---|
| 0 | Success |
| 1 | Operational error |
| 2 | Clap usage / invalid session-id |
| 10 | session-not-found |
| 11 | ambiguous-session |
| 12 | unsupported-storage |
| 13 | session-busy (SessionLock::acquire returned Busy) |
| 14 | schema-incompatible |
| 15 | invalid-input-transcript or preimage-mismatch |

## 6. Side-effect contract

Permitted:
- Read state DB.
- Read input JSONL (stdin or `--from-file`).
- Create `<state-data-dir>/replace_journal/staging/`, `<state-data-dir>/replace_journal/`, `<state-data-dir>/replace_journal/quarantine/`.
- Write/rename/unlink files in `replace_journal/` (per-process staging + per-session canonical records + journal).
- Write `<jsonl_path>.replace-<uuid>.tmp` and rename onto `<jsonl_path>`.
- SQLite UPDATE/DELETE/INSERT on session_turns + UPDATE on session_chain_segments under transaction.
- SessionLock acquire/release.

Forbidden:
- Provider commands, quota refresh, auth.
- Migration, scan.
- Telemetry, invocation rows beyond what session_turns updates require.

## 7. Test-intent track

T1-T16 per proposal §9.1. Critical:
- T-valid-replace: Claude session; canonical input; assert receipt fields, postimage matches, transcript bytes are provider-native.
- T-codex-replace: Codex session; canonical input; assert provider-native bytes.
- T-from-file: --from-file equivalent to stdin.
- T-preimage-match: --preimage-sha256 matches; succeeds.
- T-preimage-mismatch: --preimage-sha256 mismatch; exit 15.
- T-busy: existing pause-handshake lock held; exit 13.
- T-recovery-rename-only: kill between rename and commit; restart; verify session_turns matches canonical input.
- T-recovery-ambiguous-hash: kill; corrupt transcript; restart; verify quarantine.
- T-concurrent-import-replace: spawn 2 subprocesses on same session; exactly one succeeds, other exits 13; no journal pollution.
- T-readonly-on-error: preimage mismatch leaves transcript and DB unchanged.
- T-no-deletion-before-verify: postimage mismatch leaves journal in place.
- T-unsupported-storage-other: provider with `Other` storage; exit 12.
- T-malformed-input-record: invalid JSON line on stdin; exit 15 with line number.
- T-field-loss-explicit: imported turns have NULL parent_turn_id, is_sidechain, is_compaction_boundary.
- T-resolver-error-mapping: 10/11/12.
- T-session-not-found: well-formed UUID with no chain; exit 10.

## 8. Process-tree audit

Step 6b and Step 6c separate agent invocations. Step 6c writes `.tmp/phase6/step6c-reads.md` BEFORE product-code change.

## 9. References

- Proposal: `proposals/06-import-replace.md` (Rev 4).
- Hookpoints: `research/06-import-replace-hookpoints.md`.
- 06-locate's `SessionMetadata` API.
- 06-export's `CanonicalRecord` schema and storage parsers.
- 06-pause-handshake's `SessionLock` API.
