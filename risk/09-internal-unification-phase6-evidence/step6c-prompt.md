# Phase 6 Step 6c — Code Writer for 09-internal-unification

You are Step 6c code writer. **Separate from Step 6b.**

You are `gpt-high`. Operate in worktree `worktrees/09-internal-unification`.

## STEP ZERO (firstness evidence — MANDATORY)

Write `.tmp/phase6/step6c-reads.md` BEFORE editing any product code. Include:
- ISO timestamp.
- Paths and mtimes of inputs you read (RCA, proposal Rev 3, Step 6b output index, the test files Step 6b produced).

## Authoritative inputs
- `research/09-internal-unification-problem.md`
- `proposals/09-internal-unification.md` (Rev 3) — D1..D9 + §5 hookpoints
- `.tmp/phase6/step6b-output-index.md`
- Test files: `src-tauri/tests/initiative_09_internal_unification.rs` (currently RED at Step 6b state).

## Implementation per proposal §5

### 1. Public API extensions (additive)

`src-tauri/src/session_lock/mod.rs`:
- Extend `LockError::Busy` to `Busy { expires_at: String, token_hash: Option<String> }`.
- Populate `token_hash` from existing on-disk lease JSON in `SessionLock::acquire`'s busy path.
- Add `pub fn any_active_for_session(lock_dir: &Path, session_id: &str) -> Result<bool, LockError>` per D4 semantics.

`src-tauri/src/session_metadata/mod.rs`:
- Add `#[serde(skip)] pub active_segment_id: i64` to `SessionMetadata`.
- Populate it in `locate_session_metadata` from the chosen `session_chain_segments.id`.

`src-tauri/src/state/db.rs`:
- If `ResolvedResume` doesn't expose `active_segment_id`, add a `StateDb` helper to look up the active segment id for a chain. Or extend `ResolvedResume` itself. Choose the minimal path.

### 2. CLI consumer pattern fix

`src-tauri/src/main.rs`:
- Update `emit_lock_error` (or equivalent) to destructure `LockError::Busy { expires_at, .. }` (or `Busy { expires_at, token_hash, .. }` if you want to opt pause-handshake into surfacing token_hash; the proposal says don't change pause-handshake's busy JSON in this PR, so use `..`).

### 3. Net delete + lift in `session_replace`

`src-tauri/src/session_replace/internal/mod.rs` — **delete entirely**.

`src-tauri/src/session_replace/mod.rs`:
- Remove `pub mod internal;`.
- Replace `use internal::{...}` with `use crate::session_lock::SessionLock` and `use crate::session_metadata::{SessionMetadata, MetadataError, locate_session_metadata, SessionStorageType}`.
- Delete the local `fn locate_session_metadata`, `fn candidate_chain_ids`, `fn choose_chain`, `fn locate_transcript_path`.
- Replace `SessionLock::acquire(&data_root, &provider, &session)` with the public API: construct via `SessionLock::new(&lock_dir)?`, then `lock.acquire(&session, &provider, ttl)?` returning a `Lease`.
- Implement the `ImportReplaceLease` RAII guard per D3:
  ```rust
  struct ImportReplaceLease<'a> { lock: &'a SessionLock, session_id: String, lease: Option<Lease> }
  impl<'a> ImportReplaceLease<'a> {
      fn commit(mut self) -> Result<(), ReplaceError> { ... }
  }
  impl<'a> Drop for ImportReplaceLease<'a> {
      fn drop(&mut self) { if let Some(lease) = self.lease.take() { let _ = self.lock.release(&self.session_id, &lease.token); } }
  }
  ```
  Construct it in `run_import_replace_bytes` after `acquire`. Call `lease.commit()?` after the SQLite COMMIT (D3).
- Replace the `cleanup_orphan_canonical_records` call to `internal::SessionLock::any_active_for_session(...)` with `session_lock::any_active_for_session(&lock_dir, session_id)?`.
- Add `fn map_lock_error(LockError) -> ReplaceError`. The `Busy` arm:
  ```rust
  LockError::Busy { expires_at, token_hash } =>
      ReplaceError::SessionBusy { token: token_hash.unwrap_or_default(), expires_at }
  ```
- Add `fn map_metadata_error(MetadataError) -> ReplaceError`.
- Replace the resolver block with:
  ```rust
  let state = StateDb::open_default().map_err(|e| ReplaceError::OperationalError { message: e })?;
  let models = load_models(&default_models_dir()).map_err(|e| ReplaceError::OperationalError { message: e })?;
  let providers = ProvidersConfig::load(&default_config_root().join("providers.toml"))
      .map_err(|e| ReplaceError::OperationalError { message: e })?;
  let sessions = SessionsConfig::load(&default_config_root().join("sessions.toml"))
      .map_err(|e| ReplaceError::OperationalError { message: e })?;
  let metadata = locate_session_metadata(&state, &models, &providers, &sessions, session_id)
      .map_err(map_metadata_error)?;
  ```
- Update `export_metadata_for(metadata: &SessionMetadata, ...)` to take the public type and use `metadata.chain_id` (drop `String::new()`).
- Delete the local `StorageType` enum if it has no other callers; otherwise convert via `SessionStorageType::*`.
- The `replace_db_turns` call uses `metadata.active_segment_id` from the public type now.

## Iteration

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

All must pass. Step 6b's red tests (T-cross-module-lock-visibility, T-busy-token-hash-preserved, T-error-path-release, T-active-segment-id-flows, T-any-active-for-session-public) must turn green.

## Boundaries

- Do not modify any test file.
- No `Co-Authored-By:` trailers.
- Step Zero file before any `src-tauri/src/` edit.
- Commit as `feat(09-internal-unification): Phase 6 Step 6c — lift session_replace::internal duplicates`.
