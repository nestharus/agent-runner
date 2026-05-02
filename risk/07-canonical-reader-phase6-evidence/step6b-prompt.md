# Phase 6 Step 6b — Test Writer for 07-canonical-reader-unification

You are Step 6b test writer. **Separate from Step 6c.** No product code.

You are `gpt-high`. Operate in worktree `worktrees/07-canonical-reader-rca`.

## Authoritative inputs
- `research/07-canonical-reader-divergence-rca.md` — RCA with RC-1..RC-6.
- `proposals/07-canonical-reader-unification.md` — proposal D1..D6 + §4 test plan.
- `risk/07-canonical-reader-supported-surface.md`, `-scope.md`, `-shortcut.md`, `-audit.md` — Phase 4 LOW.

## Test placement
- `src-tauri/tests/initiative_07_canonical_reader_unification.rs` — new integration test target.
- Modify `src-tauri/tests/initiative_06_import_replace.rs` to remove `#[ignore]` from T1, T2, T4 (these covered RC-1 and RC-3 but were deferred at merge).

## Test obligations

### Existing tests (un-ignore)
Remove `#[ignore]` markers and TODO comments from:
- `t1_valid_replace_claude_stdin_emits_receipt_and_provider_native_transcript`
- `t2_codex_replace_writes_codex_rollout_jsonl`
- `t4_preimage_match_succeeds_with_current_canonical_export_hash`

### New RC-2/RC-4/RC-5/RC-6 regression tests in initiative_07_canonical_reader_unification.rs

Use `mod fixtures; use fixtures::initiative_06_import_replace::*;` to access the existing `ImportReplaceFixture` API. Each test should be a CLI-integration test where:
- The fixture's existing transcript exhibits the RC-X edge case.
- `agents session export` is called to derive the canonical hash oracle.
- `agents session import-replace --preimage-sha256 <oracle>` is run.

For each test, include the standard test annotation block (Risk / Level / Source / Observable / Residual).

T-rc2 (Claude content array with non-text chunk): fixture has `{"type": "tool_use", ...}` chunk in `message.content`. Pass export hash via `--preimage-sha256`. Expect exit 0; assert receipt's `preimage_sha256 == oracle`.

T-rc4 (Claude isCompactSummary): fixture has 3 lines (pre, summary, post) where the middle has `isCompactSummary: true`. Same expectation as T-rc2.

T-rc5 (Claude out-of-order timestamps): fixture has 2 lines where the second timestamp precedes the first.
- Assert `agents session export` exits non-zero.
- Assert `agents session import-replace` exits non-zero.
- **Strengthen per TA-07-F03**: also assert (a) the original transcript bytes are unchanged (no mutation), (b) the database `session_turns` rows for this session are unchanged (no mutation), (c) no per-session pending journal exists. Use `fixture.mutation_snapshot(...)` (it returns transcript_bytes, turn_rows, journal_files).

T-rc6 (Codex without session_meta): fixture has Codex transcript missing the `session_meta` line.
- Same exit-code assertions as T-rc5.
- Same no-mutation assertion (TA-07-F03).

## Crucial constraint

These tests must compile against pre-fix product code (where `session_replace` has its own parsers). They will FAIL behaviorally pre-fix and PASS post-fix — that is the RCA-track regression contract.

## Output
1. Test files committed to the branch (compile must succeed).
2. `.tmp/phase6/step6b-output-index.md` listing:
   - Test file paths and the specific tests added or modified.
   - Mtime of each test file (for process-tree firstness).
   - Description of each test's Risk/Level/Source/Observable/Residual.

## Boundaries
- No `src-tauri/src/` modifications (product code is Step 6c's job).
- No `Co-Authored-By:` trailers.
- Run `cargo build --tests` to verify tests compile. Tests will fail; that's expected pre-fix.
- Commit as `test(07-canonical-reader): Phase 6 Step 6b — RC-1..RC-6 regression tests`.
