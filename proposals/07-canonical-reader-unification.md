# Proposal — Unify canonical reader: `session_replace` consumes `session_export`

Drives the fix for `research/07-canonical-reader-divergence-rca.md` RC-1..RC-6.

## §1 What changes

`session_replace` deletes its private Claude/Codex parsers and consumes
`session_export::parse_claude_code_jsonl` / `parse_codex_rollout_jsonl`
directly. Receipt `preimage_sha256` and `postimage_sha256` are computed by
hashing the canonical-JSONL bytes that `session_export` would emit for the
same transcript file, so round-trip equality with `agents session export`
becomes the definition of correctness.

## §2 Decision points

### D1 — Reuse `session_export::CanonicalRecord`, not the private one

Drop `session_replace::internal::CanonicalRecord` and
`session_replace::internal::ContentChunk`. Re-export
`session_export::{CanonicalRecord, ContentChunk, RecordSource}` as the public
canonical-record API for both modules.

**Why:** RC-1, RC-2 are differences in *how* records are constructed; the
struct shapes are nearly identical (`source: Value` vs `source: RecordSource`
is the only meaningful difference and is purely shape, not content). One
type, one parser eliminates both classes of drift.

**Invalidator:** if any consumer of session_replace's `CanonicalRecord` relies
on `ContentChunk` being a Rust `enum` rather than a `struct`, this breaks
that consumer. The audit step must grep for ContentChunk pattern matches.

### D2 — Bridge ExportSessionMetadata via a thin converter, do not duplicate

`session_replace::run_import_replace` currently locates session metadata via
its own `internal::SessionMetadata` (UUID-only, no `chain_id`).
`session_export::parse_*` requires `ExportSessionMetadata` (with `chain_id`).
Add `From<&internal::SessionMetadata> for ExportSessionMetadata` (or a free
function `to_export_metadata(...)`) that copies the shared fields. `chain_id`
on the export side is informational only when the parsers themselves don't
read it; verify by code-reading.

**Why:** rewriting `session_replace`'s metadata path to use
`ExportSessionMetadata` end-to-end is larger than the fix needs.

### D3 — Drop session_replace's `parse_claude_native` / `parse_codex_native` / `extract_claude_content` / `extract_codex_content`

Replace `canonical_records_from_provider_file` and
`canonical_records_from_provider_bytes` with calls to
`session_export::parse_claude_code_jsonl(&export_metadata)` /
`parse_codex_rollout_jsonl(&export_metadata)`.

**Why:** RC-1..RC-6 are all in the body of these private parsers. Deleting
them is the smallest fix that closes all six.

### D4 — `canonical_jsonl_bytes` keeps its current shape (`writeln_json`)

Both modules already serialize `CanonicalRecord` via `serde_json::to_value`
followed by a newline. After D1, both produce identical bytes. Verify by
test.

**Why:** no churn unless evidence shows otherwise.

### D5 — Provider-native bytes ≠ canonical bytes; renderers stay in `session_replace`

`session_replace::ClaudeCodeRenderer::render` and `CodexSessionRenderer::render`
write *provider-native* JSONL (this is the on-disk format the resumer reads).
Those renderers do not change. Only the *canonical* path (preimage/postimage
hash + fresh-export verify) is unified.

**Why:** sanity check — render path is the round-trip target; canonical
read is the round-trip oracle. Different concerns.

### D6 — Error mapping: `ExportError` → `ReplaceError`

`session_export` returns `ExportError`. `session_replace`'s callers expect
`ReplaceError`. Add a `From<ExportError> for ReplaceError` or a per-callsite
match. Specifically:

- `MalformedTranscript { line, .. }` → `InvalidInputTranscript { reason, line }`
  with reason copied through.
- `Operational { message }` → `OperationalError { message }`.
- Other variants are unreachable on the postimage path because the metadata
  is already validated upstream; they map to `OperationalError` defensively.

## §3 Out of scope

- Removing `session_replace::internal::SessionLock` (S-PR-F02 carryover, not
  in this RCA's blast radius).
- Removing `session_replace::internal::SessionMetadata` (used by the
  pre-resolution path that session_export also depends on; reconciling with
  06-locate is a different initiative).
- Schema-probe / pause-handshake reconciliation — different issue.

## §4 Test plan

Phase 6 tests:

- T1, T2, T4 (existing, currently red per RCA reproduction) → green.
- T-canonical-reader-shared: a unit test that parses a representative
  Claude transcript and a representative Codex transcript, asserts the
  canonical-JSONL bytes from `session_replace`'s public path equal the bytes
  from `session_export::parse_*` + serialization.
- T-compaction-summary: Claude transcript with `isCompactSummary: true`
  partway through; assert both readers drop pre-summary records (RC-4).
- T-codex-without-session-meta: Codex transcript missing `session_meta`; both
  paths now return `MalformedTranscript` (RC-6).

All other initiative_06 tests must continue to pass.

## §5 Hookpoints

Files touched:

- `src-tauri/src/session_replace/internal/mod.rs` — delete
  `CanonicalRecord`, `ContentChunk`. Keep `StorageType`, `SessionMetadata`,
  `SessionLock`, `LockAcquireGuard`.
- `src-tauri/src/session_replace/mod.rs` — rewrite
  `canonical_records_from_provider_file`, `canonical_records_from_provider_bytes`,
  `canonical_hash_from_provider_file`, `canonical_hash_from_provider_bytes`,
  `export_session_canonical`, and `parse_canonical_jsonl` to use
  `session_export::CanonicalRecord`. Delete `parse_claude_native`,
  `parse_codex_native`, `extract_claude_content`, `extract_codex_content`,
  `extract_text_items`, `source_value`, and the local `ContentChunk` re-export.
- `src-tauri/src/session_replace/internal/mod.rs` — add helper
  `pub fn export_metadata_for(meta: &SessionMetadata) -> session_export::ExportSessionMetadata`.
- `src-tauri/tests/initiative_06_import_replace.rs` — un-`#[ignore]` T1, T2,
  T4 (already done in Phase 0 commit).

Renderers (`ClaudeCodeRenderer::render`, `CodexSessionRenderer::render`),
`SessionLock`, the journal lifecycle, and the recovery scan are unchanged.

## §6 Risk gates (Phase 4 self-assessment)

| Gate | Verdict |
|---|---|
| Audit (alignment with RCA) | LOW — directly closes RC-1..RC-6. |
| Scope | LOW — bounded to ~200 lines deleted, ~50 lines added in `session_replace/mod.rs`. |
| Shortcut | LOW — option (a) "consume session_export directly" is the smallest delta; (b) shared crate-internal module would create a third copy; (c) property-test pinning leaves duplicated code. |
| Supported surface | LOW — `agents session import-replace` receipt fields and exit codes unchanged. The receipt's `preimage_sha256`/`postimage_sha256` semantics narrow to "identical to `agents session export <id>`'s SHA-256," which is what the contract already promised (proposal §6 of `proposals/06-import-replace.md`). |

## §7 Acceptance

After this proposal lands:

- `cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_import_replace`
  passes with all 29 tests (no `#[ignore]` for T1/T2/T4).
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  passes.
- `agents session export <id>` and `agents session import-replace <id>`
  share exactly one canonical reader.
- `session_replace::internal::CanonicalRecord` and `ContentChunk` no longer
  exist.
