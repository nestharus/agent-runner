# Internal Unification - Audit Risk Report

**Verdict:** LOW

## Context

This audit covers `proposals/09-internal-unification.md` Rev 3 and the
implemented Phase 6 branch. The change deletes the private
`session_replace::internal::*` duplicates and routes import-replace through the
public `session_lock` and `session_metadata` modules. The primary risks were
lease visibility, error-path lease release, active segment identity, and
preserving the import-replace busy JSON contract.

## Findings

### A1 - Lease visibility across modules - CLOSED

`session_replace` now acquires locks through `session_lock::SessionLock`, and
the Phase 6 test `t_cross_module_lock_visibility` proves a pause-handshake
lease blocks import-replace with exit 13. This closes the prior risk that two
lock implementations could write mutually invisible files.

### A2 - Error-path release - CLOSED

The import-replace call path now uses a release guard so post-acquire failures
do not leave a live lease behind. `t_error_path_release` forces a postimage
verification failure and then verifies a fresh import-replace is not stuck on
`session-busy`.

### A3 - Active segment identity - CLOSED

`SessionMetadata` carries `active_segment_id` as a skipped serde field for
internal callers. `t_active_segment_id_flows` verifies the located active
segment id is the row updated during replacement, while `agents session locate`
JSON remains unchanged.

### A4 - Busy token contract - CLOSED

`LockError::Busy` carries the stored token hash and import-replace maps it
back to the existing `session-busy` JSON `token` field. The test
`t_busy_token_hash_preserved` pins the non-empty SHA-256 hex shape.

### A5 - Public listing API - CLOSED

`session_lock::any_active_for_session` is an explicit additive API required by
orphan recovery after deleting the private implementation. Component coverage
checks missing directories, absent leases, active leases, and expired leases.

## Residuals

- Legacy `provider-*-session-*.lock` files become inert debris under
  `<state-data-dir>/locks/`; auto-cleanup remains out of scope.
- Lease renewal remains out of scope and is not needed for this lift.
- The public resolver's model-validation surface now applies to
  import-replace. This is a tightening of resolution, not a new exit-code
  surface.

## Verification

- `cargo test --manifest-path src-tauri/Cargo.toml` passed before CodeRabbit
  pass 1.
- Phase 6 process-tree audit passed with advisory-only provenance notes in
  `risk/09-internal-unification-process-tree-audit.md`.

## Recommendation

Proceed. The implementation retires the duplicate internal modules and adds
the minimum public API surface needed to preserve existing import-replace
behavior.
