# Phase 4 Scope Risk Gate — 07-canonical-reader-unification

**Verdict: LOW**

Implementation on `07-canonical-reader-rca` matches the bounded scope declared
in `proposals/07-canonical-reader-unification.md`. Expansions beyond the
proposal's enumerated hookpoints are forced consequences of the chosen
approach (D1, D3) and pull in the same direction as the stated invariant
("exactly one canonical reader on `main`, and `session_replace` calls it").
Per Phase 4 zero-risk semantics, this gate closes.

## Scope envelope vs. proposal

Proposal §1 / §2 / §5 declared:

- Delete `session_replace::internal::CanonicalRecord` and `ContentChunk`.
- Replace `session_replace`'s private parsers with calls to
  `session_export::parse_*`.
- Add a metadata bridge (`export_metadata_for(...)`) and an
  `ExportError → ReplaceError` mapping.
- Un-`#[ignore]` T1, T2, T4.
- Estimated delta: ~200 lines deleted, ~50 lines added in
  `session_replace/mod.rs`.

Diff (`git diff main..07-canonical-reader-rca --stat`):

```text
src-tauri/src/session_export/mod.rs             |  61 ++++-
src-tauri/src/session_replace/internal/mod.rs   |  44 ++--
src-tauri/src/session_replace/mod.rs            | 320 +++++++-----------------
src-tauri/tests/initiative_06_import_replace.rs |   5 -
```

Net effect in `session_replace/mod.rs`: ~225 lines removed, ~80 added —
within the proposal's order-of-magnitude estimate. All six listed deletions
(`parse_claude_native`, `parse_codex_native`, `extract_claude_content`,
`extract_codex_content`, `extract_text_items`, `source_value`,
`jsonl_data_lines`, `string_field`, the local `ContentChunk` and
`CanonicalRecord`) are gone. T1, T2, T4 have their `#[ignore]` attributes
removed (`tests/initiative_06_import_replace.rs:18, 63, 215`).

## Findings

### AIR-SCOPE-F01 — Public surface added in `session_export` not enumerated in proposal §5

**Severity:** LOW

**Evidence:** `session_export/mod.rs:101-124` adds two new `pub` functions:
`read_canonical_transcript_from_bytes(metadata, bytes)` and
`canonical_jsonl_bytes(records)`. Proposal §5 ("Files touched") lists only
`session_replace/internal/mod.rs`, `session_replace/mod.rs`, and the test
file; `session_export/mod.rs` is absent. Proposal D4 ("`canonical_jsonl_bytes`
keeps its current shape (`writeln_json`)") implied no change to serialization,
but the implementation moves the canonical serializer from `session_replace`
into `session_export`.

**Why this is LOW, not MEDIUM:** the additions are forced and aligned. The
postimage hash path needs to canonicalize *in-memory rendered bytes* (not a
file on disk), which the existing `parse_*` entry points cannot do. Pulling
`canonical_jsonl_bytes` into `session_export` is the natural place for it
once that module owns the canonical record type — keeping it in
`session_replace` would re-introduce a fork of canonicalization logic, which
is exactly what the RCA is fixing. The new functions extend, not alter,
existing behavior.

**What would close it:** none required for LOW verdict. (Optional
documentation update to proposal §5 to acknowledge the
`session_export` additions and to D4 to note the move.)

### AIR-SCOPE-F02 — `session_id` parameter threaded through four internal `session_replace` functions

**Severity:** LOW

**Evidence:** `session_replace/mod.rs:1115, 1133, 1149, 1166` —
`canonical_records_from_provider_file`, `canonical_hash_from_provider_file`,
`canonical_records_from_provider_bytes`, and
`canonical_hash_from_provider_bytes` all gain a new `session_id: &str`
parameter. Five call sites updated (`mod.rs:396, 444, 483, 494, 567, 663`).

**Why this is LOW, not MEDIUM:** forced by D2/D6. `ExportSessionMetadata`
requires `session_id`, and `export_metadata_for` constructs that metadata.
The plumbing is mechanical and stays inside the module's private surface
(no public API changes). Total ~6 call sites, each with the
`metadata.session_id` (or `journal.session_id`) already in scope.

**What would close it:** none required for LOW verdict.

### AIR-SCOPE-F03 — Metadata bridge helper duplication resolved

**Severity:** LOW

**Evidence:**
- `session_replace/internal/mod.rs` now defines only `SessionMetadata` and
  `StorageType`; there is no `SessionMetadata::to_export_metadata` helper.
- `session_replace/mod.rs` keeps the single private `export_metadata_for(...)`
  bridge used by the canonical reader calls.

**Why this is LOW, not MEDIUM:** this is documentation cleanup for a stale
audit note. The code surface already has one helper, so there is no production
path divergence.

**What would close it:** closed as of the CodeRabbit loop; no code change is
needed for this finding.

### AIR-SCOPE-F04 — `ExportError → ReplaceError` mapping is more precise than proposal D6 anticipated

**Severity:** LOW

**Evidence:** Proposal §2 D6 stated "Other variants are unreachable on the
postimage path because the metadata is already validated upstream; they map
to `OperationalError` defensively." The implementation
(`session_replace/mod.rs:1095-1115`, `map_export_error`) maps
`InvalidSessionId`, `SessionNotFound`, `AmbiguousSession`, and
`UnsupportedStorage` to *named* `ReplaceError` variants of the same shape,
not to `OperationalError`.

**Why this is LOW, not MEDIUM:** strictly an improvement on the proposal —
preserves error semantics that callers (CLI exit-code mapping) may rely on.
Does not introduce new public surface; `ReplaceError` already had these
variants.

**What would close it:** none required for LOW verdict.

## Out-of-scope cleanups deliberately not done (consistent with proposal §3)

- `session_replace::internal::SessionLock` — kept (proposal §3).
- `session_replace::internal::SessionMetadata` — kept (proposal §3).
- Schema-probe / pause-handshake reconciliation — untouched (proposal §3).
- `ClaudeCodeRenderer::render` / `CodexSessionRenderer::render` — only the
  `ContentChunk` shape adaptation (`mod.rs:194-198`) was applied; renderer
  semantics are unchanged (proposal D5).

## Files changed (verification)

Only the four files enumerated in proposal §5 were modified, plus the two
new docs (`research/07-…`, `proposals/07-…`). No incidental edits to
unrelated modules, no formatting churn beyond the dedicated `rustfmt`
commit (`b0d68fc`).

## Summary

Scope is bounded, deletions match the proposal, additions are forced by
the chosen approach (D1, D3) and consistent with the stated invariant.
F01–F04 are visibility/ergonomic notes, not scope expansions that warrant
gating. Gate closes at **LOW**.
