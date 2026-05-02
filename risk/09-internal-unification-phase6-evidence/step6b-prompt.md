# Phase 6 Step 6b — Test Writer for 09-internal-unification

You are Step 6b test writer. **Separate from Step 6c.** No product code.

You are `gpt-high`. Operate in worktree `worktrees/09-internal-unification`.

## Authoritative inputs
- `research/09-internal-unification-problem.md`
- `proposals/09-internal-unification.md` (Rev 3) — D1..D9 + §4 test plan + §5 hookpoints
- `risk/09-internal-unification-{audit,scope,shortcut,supported-surface}.md` (all LOW)

## Test placement
- New: `src-tauri/tests/initiative_09_internal_unification.rs` — homes for the 5 new tests in §4.
- Modify: `src-tauri/tests/fixtures/initiative_06_import_replace.rs`:
  - `lock_path(provider_name, session_id)` → `lock_path(session_id)` returning `<data_root>/locks/session-{s}.lock` (matching `session_lock` schema).
  - `write_active_lock(provider_name, session_id)` → either updated to write the new on-disk shape (`session_lock::SessionLock`'s on-disk format), OR (preferred) replaced with a helper that constructs `session_lock::SessionLock::new(...)?` and calls `acquire(...)?` to produce a real lease. The real-lease form is preferred because it's resilient to internal schema drift.

## Tests required (§4 of proposal)

For each test, include the standard test annotation block (Risk / Level / Source / Observable / Residual).

1. `T-cross-module-lock-visibility` — CLI integration. `agents session pause-handshake <id>` to acquire a lease; immediately `agents session import-replace <id>` and assert exit 13 `session-busy`. Cleanup: `agents session resume-handshake <id> --token <token>`.

2. `T-busy-token-hash-preserved` — CLI integration. Hold a lease via `agents session pause-handshake`; run `agents session import-replace`; parse stderr JSON; assert `error.token` is non-empty hex of length 64 (matches sha256). Tests the token-hash-content carryover via D7.

3. `T-error-path-release` — CLI integration. Force a post-acquire failure using `OULIPOLY_IMPORT_REPLACE_TEST_HOOK=fail-postimage-verification`; run import-replace; assert exit 1; then run a fresh import-replace immediately and assert it does NOT exit 13 (lease was released on the error path).

4. `T-active-segment-id-flows` — component-level. Build an `ImportReplaceFixture`, run import-replace, assert:
   - (a) pre-replace: query `session_chain_segments` SELECT id WHERE active for this session; `session_metadata::locate_session_metadata(...).active_segment_id` returns the same id. (Use the public API directly via library imports.)
   - (b) post-replace: `session_chain_segments.last_turn_id` updated on that exact id.
   - (c) post-replace: `session_chains.last_used_at` updated for `metadata.chain_id`.

5. `T-any-active-for-session-public` — component-level on `session_lock::any_active_for_session`. Cases: missing lock dir → false; missing lease file → false; present-and-active lease → true; expired lease → false (use a sleep or short-TTL lease).

## Compile-fail OK

These tests reference public API extensions that Step 6c will add:
- `session_lock::any_active_for_session` (D4)
- `session_lock::LockError::Busy { token_hash }` (D7)
- `session_metadata::SessionMetadata { active_segment_id }` (D5)

If any such reference fails to compile until Step 6c lands, that's expected. Run `cargo build --tests` to verify your test files compile after Step 6c-style stubs land. Do not stub anything yourself.

## Output
1. Test files committed.
2. `.tmp/phase6/step6b-output-index.md` listing test files and per-test annotations.

## Boundaries
- No `src-tauri/src/` modifications.
- No `Co-Authored-By:` trailers.
- Commit as `test(09-internal-unification): Phase 6 Step 6b — RC regression tests + fixture migration`.
