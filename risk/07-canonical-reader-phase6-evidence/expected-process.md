# Phase 6 expected process — 07-canonical-reader-unification

## Required child invocations

| Role | Model | Prompt | Output |
|---|---|---|---|
| Step 6b (test writer) | gpt-high | `.tmp/phase6/step6b-prompt.md` | `src-tauri/tests/initiative_07_canonical_reader_unification.rs`, modifications to `src-tauri/tests/initiative_06_import_replace.rs`, `.tmp/phase6/step6b-output-index.md` |
| Step 6c (code writer) | gpt-high | `.tmp/phase6/step6c-prompt.md` | `.tmp/phase6/step6c-reads.md` (firstness evidence), modifications to `src-tauri/src/session_export/mod.rs`, `src-tauri/src/session_replace/{mod,internal/mod}.rs` |

## Firstness rule

- Step 6c must write `.tmp/phase6/step6c-reads.md` BEFORE editing any product code.
- Step 6b's test files must have mtime BEFORE Step 6c's product files (verified at commit level: 31ec6f1 < a16b446).

## Commit ordering

Commits beyond main:
- 87aeddd `rca(...)` — Phase 0 RCA + Phase 3 proposal (docs only).
- e2e2e55 `risk(...)` — Phase 4 risk reports (docs only).
- 31ec6f1 `test(...)` — Phase 6 Step 6b (tests only).
- a16b446 `feat(...)` — Phase 6 Step 6c (product code, makes tests green).

Verifies: tests-first, docs-first, code-after.
