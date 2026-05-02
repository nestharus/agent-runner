# RCA — Canonical reader divergence between `session_replace` and `session_export`

## Symptom

After Initiative 06's five PRs (#14–#18) all merged to `main`, three integration
tests in `src-tauri/tests/initiative_06_import_replace.rs` are gated with
`#[ignore]`:

- `t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript`
- `t2_codex_replace_writes_codex_rollout_jsonl`
- `t4_preimage_match_succeeds_with_current_canonical_export_hash`

These tests assert byte-equality between the receipt's `postimage_sha256` /
`preimage_sha256` and the SHA-256 of `agents session export <id>` stdout for
the same session.

## Reproduction (red, against pre-fix HEAD)

Reproduction harness lives in the existing test target. To run:

```bash
cd src-tauri
cargo test --test initiative_06_import_replace -- --include-ignored \
  t1_valid t2_codex t4_preimage
```

Captured output at HEAD `941e6e8` (post-merge, pre-fix):

```text
test t4_preimage_match_succeeds_with_current_canonical_export_hash ... FAILED
test t2_codex_replace_writes_codex_rollout_jsonl ... FAILED
test t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript ... FAILED

---- t1 ---- assertion `left == right` failed
  left:  "d8ae8b5b3c786d734ccf0bedccd4760c216295c0a4114013c91977457c724b6c"  (export)
  right: "77d1ea9e80ed581a8821e52d49e90958c2715ab39d06767b3c3ec92ac6af1294"  (receipt.postimage_sha256)

---- t2 ---- assertion `left == right` failed
  left:  "2fbaca460626f80bc8995316d68509dc113235f2f75948218da64c28e77fb465"  (export)
  right: "84f8f74311fb74033c736f672285343dd1b8472666111cb6a21f788ce8cfcec3"  (receipt.postimage_sha256)

---- t4 ---- exit 15 preimage-mismatch:
  expected (export-derived): "e5699cc8ea9070db748214aeb0f88b798856541664862006e12989d1718f9446"
  actual   (replace-internal): "823a9d2ce6d6ca23be6411384f63358429e4828629576e0cd2baf520a88b576d"

test result: FAILED. 0 passed; 3 failed; 0 ignored
```

Tests fail because two distinct canonical readers compute different bytes for
the same provider transcript:

1. `session_export::{parse_claude_code_jsonl, parse_codex_rollout_jsonl}` — the
   public `agents session export` CLI (Initiative 06-export, PR #16).
2. `session_replace::{parse_claude_native, parse_codex_native}` — used inside
   `run_import_replace` to compute `preimage_sha256`, `postimage_sha256`, and
   the round-trip verify.

Both readers were developed in parallel branches off `main` and were
intentionally duplicated under the assumption that one would be merged first
and the other would converge on it. Neither did before merge.

## Root causes

### RC-1 — Claude `role` field source diverges

Same Claude transcript line, different `role` extraction:

- `session_export` (`src-tauri/src/session_export/mod.rs:131`): `role =
  native_type` (i.e., `value["type"]`). Records with `type` outside
  `{user, assistant}` get `unsupported_record: true` and `content: []`.
- `session_replace` (`src-tauri/src/session_replace/mod.rs:1125-1130`): `role
  = value["type"] OR value["message"]["role"] OR "assistant"`. Records with
  `type=summary` or similar get a non-empty role from `message.role`.

Effect: same line yields different `role` strings → different canonical bytes.

### RC-2 — Claude content extraction diverges

- `session_export` (`src-tauri/src/session_export/mod.rs:362-410`): walks
  `message.content` as an array of structured items, emitting
  `ContentChunk { type: "text" | …, text: Option<String> }` for each item;
  preserves non-text item types as their canonical kind.
- `session_replace` (`src-tauri/src/session_replace/mod.rs:1213-1259`): emits
  `ContentChunk::Text { text }` only; non-text items mark
  `unsupported_record = true`.

Effect: tool-use / image content is dropped entirely by `session_replace`,
whereas `session_export` preserves a chunk for it. Receipt postimage hash
differs even when no content is "lost" by either reader.

### RC-3 — Codex `turn_id` fallback diverges

- `session_export` (`src-tauri/src/session_export/mod.rs:202-207`): `turn_id =
  payload.id  ||  format!("{jsonl_path}:{line}")`.
- `session_replace` (`src-tauri/src/session_replace/mod.rs:1185-1190`):
  `turn_id = value.id  ||  payload.id  ||  format!("codex-line-{n}")`.

Effect: when `payload.id` is empty (common for Codex rollouts), both fall
back, but to different strings. Different `turn_id` → different bytes.

### RC-4 — Claude compaction-summary filter only in `session_export`

- `session_export` (`src-tauri/src/session_export/mod.rs:151-161`): if any
  record has `isCompactSummary: true`, all records before its index are
  dropped from the canonical output.
- `session_replace`: no such filter.

Effect: post-compaction sessions produce a strictly shorter canonical record
list under `session_export` than under `session_replace`. SHA-256 diverges.

### RC-5 — Timestamp-order validation only in `session_export`

- `session_export` (`src-tauri/src/session_export/mod.rs:162`): calls
  `validate_timestamp_order(&records, ...)` and rejects out-of-order rows
  with `MalformedTranscript`.
- `session_replace`: no such validation.

Effect: a session with one out-of-order timestamp succeeds in `session_replace`
but fails in `session_export`; their hashes can never match for that input.

### RC-6 — Codex `session_meta` enforcement only in `session_export`

- `session_export` (`src-tauri/src/session_export/mod.rs:223-232`): rejects
  any Codex transcript that does not include a `session_meta` line whose
  `payload.id` matches the requested session.
- `session_replace`: parses `session_meta` opportunistically; never errors if
  it is absent.

Effect: corrupt or stripped Codex transcripts pass `session_replace` but fail
`session_export`. Different exit codes, different bytes.

## Hypothesis confirmation

All six root causes reproduce against pre-fix main (`941e6e8`):

- RC-1 and RC-3 — reproduced by T1, T2, T4 in
  `src-tauri/tests/initiative_06_import_replace.rs` (output captured above).
- RC-2, RC-4, RC-5, RC-6 — reproduced by
  `src-tauri/tests/initiative_07_canonical_reader_unification.rs`. Run command
  (against pre-fix `941e6e8` with the test file copied in):

```bash
cd src-tauri
cargo test --test initiative_07_canonical_reader_unification
```

Captured red output:

```text
test rc2_preimage_with_claude_tool_use_chunk_matches_export_hash ... FAILED
test rc4_preimage_with_claude_compaction_summary_matches_export_hash ... FAILED
test rc5_out_of_order_timestamps_surface_consistently ... FAILED
test rc6_codex_missing_session_meta_surfaces_consistently ... FAILED

test result: FAILED. 0 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out
```

RC-2 and RC-4 fail at the receipt's `preimage_sha256` not matching the
SHA-256 of `agents session export <id>` stdout. RC-5 and RC-6 fail at the
`assert_ne!(import_replace.exit_code, 0)` assertion: pre-fix import-replace
silently succeeds on a corrupt transcript that `session_export` rejects.

## Why this was not caught pre-merge

The RCA reveals a process gap, not a code gap:

- 06-import-replace's branch shipped local `parse_claude_native` /
  `parse_codex_native` functions because A1 ("06-export lands first") was
  invalidated. The synthesis (`risk/06-import-replace-supported-surface-pr.md`
  S-PR-F02) accepted the duplication as a "forward-compat hazard, not a
  current-supported-surface break."
- The forward-compat hazard materialized at merge: `agents session export`
  on `main` is now `session_export::*`, but `session_replace` still calls
  its own copy.
- I deferred the failing tests with `#[ignore]` instead of fixing the
  divergence at merge time. That deferral is the proximate process error;
  the divergence itself is the latent technical error.

## Required invariant for the fix

After the fix, for every supported provider transcript T and resolved session
S:

```text
sha256(agents session export <S>.stdout)
  == receipt.postimage_sha256 produced by import-replace whose postimage
     transcript bytes are the result of importing canonical(T)
```

Equivalently: there is exactly one canonical reader on `main`, and
`session_replace` calls it.

## Out of scope for this RCA

This RCA does not propose a fix. The proposal phase (Phase 3) selects between:
(a) `session_replace` consumes `session_export::parse_*` directly,
(b) extract a shared `canonical_reader` crate-internal module both consume,
(c) keep both and add a property test that pins their outputs equal.

Pick in Phase 3.
