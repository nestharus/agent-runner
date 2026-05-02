# Phase 4 — Shortcut risk gate · 07-canonical-reader-unification

**Verdict: LOW**

The shortcut gate per `~/ai/workflows/implementation-pipeline.md` Phase 4 asks
whether the proposed shortcut "defeats the underlying purpose." The underlying
purpose, as the RCA states, is the invariant:

> there is exactly one canonical reader on `main`, and `session_replace` calls
> it. Equivalently, `sha256(agents session export <S>.stdout) ==
> receipt.postimage_sha256`.

The proposal selects option (a) — `session_replace` consumes
`session_export::parse_*` directly — and the implementation does exactly that,
deleting the duplicate parsers rather than patching them. RC-1..RC-6 are closed
by deletion of the duplicated code path; they are not reintroducible without
reverting the unification. Tests T1, T2, T4 (red on `941e6e8`) un-`#[ignore]`
and pass; the full `initiative_06_import_replace` suite is green at 29/29; and
`cargo clippy --all-targets -- -D warnings` is clean.

Below are the trade-offs the shortcut explicitly took, the evidence that they
do not defeat the purpose, and two LOW-severity findings to hand to the audit
and supported-surface gates.

## Why the chosen shortcut is the right one

| Alternative considered (per proposal §6) | Verdict | Reason |
|---|---|---|
| (a) consume `session_export::parse_*` directly | **chosen** | Smallest delta. Closes all six RCs by deletion. One reader on `main`. |
| (b) extract a shared crate-internal `canonical_reader` module | rejected | Creates a third copy at the moment of refactor; doubles the surface for future drift; gains nothing because session_export already owns the canonical reader semantics. |
| (c) keep both readers + property-test pinning | rejected | Leaves the duplicate code path. Property tests cannot enumerate every input class (RC-4/RC-5/RC-6 are validation-side asymmetries that pinning by sample does not catch). The `#[ignore]` deferral that produced this RCA is a precedent for how this approach decays in practice. |

The diff confirms (a) was implemented:

- `src-tauri/src/session_replace/mod.rs` — `parse_claude_native`,
  `parse_codex_native`, `extract_claude_content`, `extract_codex_content`,
  `extract_text_items`, `source_value`, `string_field`, `jsonl_data_lines`
  all deleted (~210 lines).
- `src-tauri/src/session_replace/internal/mod.rs` — `CanonicalRecord` and
  `ContentChunk` deleted; `pub use crate::session_export::CanonicalRecord` is
  the now the only definition.
- `canonical_records_from_provider_{file,bytes}` and
  `canonical_hash_from_provider_{file,bytes}` are reduced to
  `export::read_canonical_transcript[_from_bytes]` + `map_export_error`.
- `canonical_jsonl_bytes` delegates to `session_export::canonical_jsonl_bytes`.

There is no remaining surface where `session_replace` could compute canonical
bytes differently from `session_export`.

## Decision-point fidelity

| Proposal decision | Implementation | Defeats purpose? |
|---|---|---|
| D1 — reuse `session_export::CanonicalRecord` / `ContentChunk` | Done. `pub use crate::session_export::CanonicalRecord` at `session_replace/mod.rs:19`; internal types removed. | No. |
| D2 — bridge metadata via thin converter | Done via `export_metadata_for` (free fn at `session_replace/mod.rs:1074`). `chain_id: String::new()` placeholder is safe — verified `chain_id` is never read by `parse_claude_code_jsonl_bytes` / `parse_codex_rollout_jsonl_bytes` (it appears only as a struct field at `session_export/mod.rs:57`). | No. |
| D3 — drop the private parsers | Done (see deletions above). | No. |
| D4 — `canonical_jsonl_bytes` keeps shape | Done; identical `serde_json::to_string` + `\n`. | No. |
| D5 — renderers stay in `session_replace` | Renderers retained, but their inner content-chunk handling adapted to the new struct shape (see F02 below). | No (purpose preserved); see F02. |
| D6 — `ExportError` → `ReplaceError` mapping | Done via `map_export_error` (`mod.rs:1094`). `MalformedTranscript.line == 0` correctly demoted to `None`. `Operational` correctly mapped to `OperationalError`. Resolver variants pass through. | No. |

## Findings

### AIR-SHORTCUT-F01 — Dead helper from D2 not wired in (LOW)

**Evidence:** `internal::SessionMetadata::to_export_metadata`
(`session_replace/internal/mod.rs:46-57`) was added per proposal D2 with
`#[allow(dead_code)]` and is never called. The mod.rs callers instead use a
parallel free function `export_metadata_for(storage_type, session_id,
provider_name, jsonl_path)` (`session_replace/mod.rs:1074`).

**Why it does not defeat the purpose:** the parallel helper produces the same
`ExportSessionMetadata` shape, and unification is achieved either way. The
duplicate is purely cosmetic and bounded to two adjacent files.

**What would close it:** delete `internal::SessionMetadata::to_export_metadata`
(and its `#[allow(dead_code)]`) since the mod.rs path does not need a
`&SessionMetadata`-bound conversion (the call sites have raw fields), or
alternatively, refactor the mod.rs callers to take `&SessionMetadata` and call
the helper. Either is a < 20-line follow-up.

### AIR-SHORTCUT-F02 — Renderer adapted to new chunk shape without analysis (LOW for shortcut gate; flag for audit/supported-surface)

**Evidence:** the old private `ContentChunk` was an enum with only
`Text { text }`, so `ClaudeCodeRenderer` and `CodexSessionRenderer` could only
emit text content. The new `session_export::ContentChunk` is
`{ r#type: String, text: Option<String> }`, with no
`#[serde(skip_serializing_if)]`. The renderers were updated mechanically:

```rust
// session_replace/mod.rs:759-764  (Claude)
let kind = chunk.r#type.as_str();
let text = chunk.text.as_deref().unwrap_or("");
json!({"type": kind, "text": text})
```

Combined with `extract_content_chunks` at `session_export/mod.rs:418-449`,
which preserves non-text item types and emits `text: None` (not `Some("")`),
this changes import-replace's input contract: a Claude `assistant`/`user` line
with `content: [{"type": "tool_use", "name": ..., "input": ...}]` previously
flagged `unsupported_record: true` in the old reader and was rejected by
`validate_record_for_render`. Under the new shape, the same line yields
`ContentChunk { type: "tool_use", text: None }` with
`unsupported_record: false`, passes `validate_record_for_render`, and is
re-rendered as `{"type": "tool_use", "text": ""}`, dropping the original
`name`/`input` payload.

**Why it does not defeat the SHORTCUT gate's purpose:** the gate's purpose is
"unify the canonical reader so round-trip equality holds." Round-trip
equality is preserved — both halves of the round-trip use
`session_export::parse_*`, so `actual_postimage == postimage_expected` and
`receipt.postimage_sha256 == sha256(agents session export <S>)`. The lossy
behavior is in the *renderer*, not the reader, and the renderer is explicitly
out of the RCA's scope (RC-1..RC-6 are all reader-side).

**Why I flag it anyway:** the proposal's D5 ("renderers do not change") was
narrowly true at the type-signature level but not at the behavioral level.
The audit and supported-surface gates should evaluate whether silently
accepting + lossily rendering non-text Claude content (vs. the prior
early-rejection behavior) is the desired contract for `agents session
import-replace`. Existing test fixtures do not exercise this case — the green
T1..T4 results do not refute the concern.

**What would close it (if the audit gate decides closure is required here):**
either (i) extend `validate_record_for_render` to reject records whose
`ContentChunk` lacks `text` (preserves prior early-rejection contract), or
(ii) extend the renderers to reconstruct the original chunk payload from
`record.source` for non-text chunks (preserves data fidelity). Either is a
focused follow-up after audit/supported-surface verdicts.

## Build / test evidence

```text
$ cargo build --manifest-path src-tauri/Cargo.toml --tests
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 32s

$ cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_import_replace
... 29 passed; 0 failed; 0 ignored; 0 measured ...
   (T1, T2, T4 all pass; #[ignore] removed)

$ cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --tests -- -D warnings
    Finished `dev` profile in 48.85s   (no warnings)
```

## Boundaries observed

- I did not propose alternative fixes; the proposal already enumerated
  (a)/(b)/(c) and chose (a), and the implementation matches.
- Findings are named (`AIR-SHORTCUT-F01`, `AIR-SHORTCUT-F02`) per workflow
  convention; both are LOW.
- F02's downstream behavioral concern is handed to the audit and
  supported-surface gates rather than re-litigated here.

## Verdict

**LOW.** The chosen shortcut achieves the RCA's stated invariant by
construction (single reader on `main`), the previously-deferred tests now
pass, and no implementation shortcut was taken that re-introduces or hides
RC-1..RC-6. The two findings above are LOW: F01 is cosmetic dead code; F02
flags a downstream contract shift for the audit/supported-surface gates to
evaluate but does not defeat this gate's purpose.
