# Phase 6 Step 6c — Code Writer for 07-canonical-reader-unification

You are Step 6c code writer. **Separate from Step 6b.**

You are `gpt-high`. Operate in worktree `worktrees/07-canonical-reader-rca`.

## STEP ZERO (firstness evidence — MANDATORY)

Write `.tmp/phase6/step6c-reads.md` BEFORE editing any product code. Include:
- ISO timestamp (e.g., `date -u +%Y-%m-%dT%H:%M:%SZ`).
- Paths of all inputs you read (contract/proposal/RCA, Step 6b prompt, Step 6b output index, the test files Step 6b produced).
- Mtime of each.

This file's mtime must be earlier than any product-code edit; the process-tree-auditor checks for it.

## Authoritative inputs
- `research/07-canonical-reader-divergence-rca.md` (RCA RC-1..RC-6)
- `proposals/07-canonical-reader-unification.md` (D1..D6 + §5 hookpoints)
- `.tmp/phase6/step6b-output-index.md` (Step 6b's output index)
- Test files at `src-tauri/tests/initiative_06_import_replace.rs` and `src-tauri/tests/initiative_07_canonical_reader_unification.rs` (Step 6b's regression harnesses; currently red pre-fix).

## Implementation per proposal §5

### 1. session_export — add public bytes-level API
- `pub fn read_canonical_transcript_from_bytes(metadata: &ExportSessionMetadata, bytes: &[u8]) -> Result<Vec<CanonicalRecord>, ExportError>`.
- `pub fn canonical_jsonl_bytes(records: &[CanonicalRecord]) -> Result<Vec<u8>, ExportError>` — emits `to_string(record) + "\n"` per record.
- Refactor internal `scan_jsonl(path)` into `scan_jsonl_bytes(bytes, path)` so both file and bytes paths share one scanner. `parse_claude_code_jsonl` and `parse_codex_rollout_jsonl` call the bytes form.

### 2. session_replace::internal — drop duplicates
- Delete `internal::CanonicalRecord`, `internal::ContentChunk`.
- Add `pub fn StorageType::to_export() -> session_export::SessionStorageType`.
- (Optional) Add `pub fn SessionMetadata::to_export_metadata() -> ExportSessionMetadata`. If unused, omit.

### 3. session_replace::mod — consume session_export
- Replace `pub use internal::CanonicalRecord` with `pub use crate::session_export::CanonicalRecord`.
- Import `crate::session_export::{self as export, ContentChunk, ExportError, ExportSessionMetadata}`.
- Delete `parse_claude_native`, `parse_codex_native`, `extract_claude_content`, `extract_codex_content`, `extract_text_items`, `source_value`, `jsonl_data_lines`, the local `JsonlDataLine` struct, and `string_field` if it becomes unused.
- Replace local `canonical_jsonl_bytes(records)` to delegate to `export::canonical_jsonl_bytes(records)` mapped through `map_export_error`.
- Add `fn export_metadata_for(storage_type, session_id, provider_name, jsonl_path) -> Result<ExportSessionMetadata, ReplaceError>` and `fn map_export_error(err: ExportError) -> ReplaceError`.
- Plumb `session_id` through `canonical_records_from_provider_*` and `canonical_hash_from_provider_*` (they now take session_id).
- Update all callers in `session_replace/mod.rs` to pass `metadata.session_id` (or `journal.session_id` on the recovery path).
- Update `content_json` and Codex renderer's content extraction to consume `ContentChunk` as a struct (`chunk.r#type`, `chunk.text.as_deref().unwrap_or("")`) rather than enum variant matching.
- Update `canonical_semantics_equal` to compare via a small `content_chunks_equal` helper since `ContentChunk` is no longer Eq.

## Iteration

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

All must pass. Step 6b's regression tests must turn green.

## Boundaries

- Do not modify any test file. Step 6b owns tests.
- No `Co-Authored-By:` trailers.
- Step Zero file must exist before any `src-tauri/src/` edit.
- Commit as `feat(07-canonical-reader): Phase 6 Step 6c — unify canonical reader through session_export`.

## Final summary

```
=== Step 6c product code complete ===
Files touched: ...
cargo test result: PASS / FAIL with count
Step Zero file: <path>
=== End summary ===
```
