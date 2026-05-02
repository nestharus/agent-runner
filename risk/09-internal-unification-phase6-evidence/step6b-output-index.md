# Phase 6 Step 6b Output Index

## Test Files

- `src-tauri/tests/initiative_09_internal_unification.rs`
  - New home for the five Initiative 09 regression tests from proposal §4.
- `src-tauri/tests/fixtures/initiative_06_import_replace.rs`
  - Migrated lock fixture helpers to the canonical `session_lock` lease shape.
  - Added shared pause/resume CLI helpers for cross-module lock tests.
- `src-tauri/tests/initiative_06_import_replace.rs`
  - Updated the remaining lock-path call site to the canonical `session-{id}.lock` helper.

## Per-Test Annotations

### T-cross-module-lock-visibility

- Risk: import-replace may not see leases acquired through the public pause-handshake path.
- Level: CLI integration.
- Source: proposal §4 T-cross-module-lock-visibility; D1-D4, D8.
- Observable: pause-handshake acquires a lease; immediate import-replace exits 13 session-busy with expires_at populated; resume-handshake releases the token.
- Residual: does not prove visibility for non-CLI in-process lock holders beyond the shared on-disk lease shape.

### T-busy-token-hash-preserved

- Risk: lifting import-replace to public session_lock may drop or reformat the busy token hash.
- Level: CLI integration.
- Source: proposal §4 T-busy-token-hash-preserved; D7.
- Observable: import-replace exits 13 while a pause-handshake lease is live, and stderr JSON error.token is a non-empty 64-character SHA-256 hex string.
- Residual: validates the user-visible hash shape, not the raw pause token value hidden on disk.

### T-error-path-release

- Risk: a post-acquire import-replace failure may leave a stuck session lease.
- Level: CLI integration.
- Source: proposal §4 T-error-path-release; D3.
- Observable: forced fail-postimage-verification exits 1; a fresh immediate import-replace on the same session does not exit 13.
- Residual: covers the forced postimage-verification path, not every possible OS-level write/fsync failure.

### T-active-segment-id-flows

- Risk: public session_metadata resolution may lose the active segment identity needed by import-replace DB writes.
- Level: component.
- Source: proposal §4 T-active-segment-id-flows; D5, D6.
- Observable: pre-replace locate_session_metadata.active_segment_id equals the active row id; post-replace last_turn_id changes on that same id; session_chains.last_used_at changes for metadata.chain_id.
- Residual: focuses on the selected active segment and does not cover ambiguous multi-chain resolution.

### T-any-active-for-session-public

- Risk: recovery may lack a public way to detect active session leases after internal lock deletion.
- Level: component.
- Source: proposal §4 T-any-active-for-session-public; D4.
- Observable: any_active_for_session returns false for missing lock dir and missing lease file, true for an active lease, and false for an expired lease.
- Residual: does not cover malformed lock JSON, which remains an operational-error path.
