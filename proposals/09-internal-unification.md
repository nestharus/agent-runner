# Proposal — `session_replace` consumes `session_lock` + `session_metadata`

Closes `S-PR-F02` (06-import-replace forward-compat carryover) and
`AIR-SUPPORTED-SURFACE-F04` (07-canonical-reader chain_id synthesis hazard).

## §1 What changes

`session_replace`'s private `internal::SessionLock` and
`internal::SessionMetadata` are deleted. Their callers consume the public
`session_lock::SessionLock` and `session_metadata::locate_session_metadata`
APIs. `export_metadata_for(...)` carries the real `chain_id` from
`session_metadata::SessionMetadata`.

Lock files on disk remain bit-compatible with the public schema; that is the
correctness invariant of the lift.

## §2 Decision points

### D1 — Reuse public types, no `From` adapters

The two public types (`session_lock::SessionLock`,
`session_metadata::SessionMetadata`) are the canonical home. We do **not**
keep the `internal::*` aliases as type re-exports — that just defers the
reconciliation. After this PR, `session_replace::internal::mod.rs` is gone.

### D2 — Map error variants explicitly at the boundary

`session_lock::LockError` → `ReplaceError::SessionBusy` /
`ReplaceError::OperationalError` etc. via `map_lock_error`.
`session_metadata::MetadataError` → `ReplaceError::*` via existing
`map_metadata_error` pattern.

### D3 — `SessionLock` ownership lives in `run_import_replace_bytes`

`session_lock::SessionLock::new(<lock_dir>)` constructs once per
operation; `acquire(...)` returns a `Lease`; on success the
operation runs to completion, then `release(session_id, lease.token)`
returns a `ReleaseReceipt`. On any error path the `Drop` impl on the
sentinel handle releases the flock (consistent with the existing
internal pattern).

### D4 — `recover_pending_replaces` lock-observation goes through public API

The recovery scanner currently calls
`internal::SessionLock::any_active_for_session(data_root, session_id)`. The
public API has the same `any_active_for_session` shape; the call site
moves to `session_lock::SessionLock::any_active_for_session(data_root, session_id)`.

### D5 — `chain_id` flows through

`export_metadata_for(metadata, ...)` takes `&session_metadata::SessionMetadata`
and copies `metadata.chain_id` into `ExportSessionMetadata.chain_id`. The
synthesized `String::new()` is gone.

### D6 — Resolver call now requires loaded `ProvidersConfig` / `SessionsConfig`

`session_metadata::locate_session_metadata` requires the caller to load
`ProvidersConfig`, `SessionsConfig`, and `ModelStore` and pass them in. The
session_replace caller already does this in `locate_session_metadata` (the
private one); we wire those existing loads into the public call.

## §3 Out of scope

- Lease renewal API surface (separate DECISIONS entry).
- Listing locks across sessions for diagnostics.
- Changes to receipt JSON, exit codes, or canonical record shape.

## §4 Test plan

Phase 6 tests:

- All existing `initiative_06_import_replace.rs` tests must remain green
  (29/29 + RC suite). T1, T2 specifically exercise the lock-acquire-release
  path through `agents session import-replace`.
- All existing `initiative_06_pause_handshake.rs` tests must remain green
  (the public `SessionLock` is exercised by `agents session pause-handshake` /
  `resume-handshake`).
- All existing `initiative_06_locate.rs` tests must remain green.
- New T-cross-module-lock-visibility: hold a lease via
  `agents session pause-handshake <id>`; verify
  `agents session import-replace <id>` exits 13 `session-busy`. (Pre-fix this
  fails because the two locks are stored in different files; post-fix one
  file backs both.) Today, both already write to
  `<state-data-dir>/locks/`. Verify naming is identical so the cross-module
  observer sees the same file.
- New T-chain-id-flows: assert that the receipt JSON's structured stderr
  format does not include `chain_id` (it never has) but that the
  `export_metadata_for` synthetic value is no longer empty in any code path
  that constructs `ExportSessionMetadata`. Component-level test.

## §5 Hookpoints

Files touched:

- `src-tauri/src/session_replace/internal/mod.rs` — **deleted entirely**.
- `src-tauri/src/session_replace/mod.rs`:
  - Replace `use internal::{SessionLock, SessionMetadata, StorageType}` with
    `use crate::session_lock::SessionLock as PublicLock` and
    `use crate::session_metadata::{SessionMetadata, SessionStorageType,
    MetadataError, locate_session_metadata as locate_metadata}`.
  - Delete the local `fn locate_session_metadata` (the raw-SQL one).
  - Delete `fn candidate_chain_ids`, `fn choose_chain`, and
    `fn locate_transcript_path` since they are now in `session_metadata`.
  - Replace `SessionLock::acquire(&data_root, &provider, &session)` with
    `PublicLock::new(&lock_dir)?` + `.acquire(&session, &provider, ttl)?`.
  - Replace `lock.release()` with `lock.release(&session, &lease.token)?`.
  - Replace `SessionLock::any_active_for_session` with the public one.
  - Add `fn map_lock_error(e: LockError) -> ReplaceError` and
    `fn map_metadata_error(e: MetadataError) -> ReplaceError`.
  - Update `export_metadata_for(metadata, ...)` to take
    `&SessionMetadata` (the public one) and use `metadata.chain_id`.
  - Update `StorageType` references to use `SessionStorageType` from the
    public module (or delete the local `StorageType` enum if it has no
    other callers).
- `src-tauri/src/session_replace/mod.rs` — `pub mod internal;` line is
  removed since the directory is deleted.

CLI dispatch in `src-tauri/src/main.rs` is untouched. No change to
`agents session pause-handshake` / `resume-handshake` paths (they already
use the public `session_lock::SessionLock`).

## §6 Risk gates (Phase 4 self-assessment)

| Gate | Verdict | Reasoning |
|---|---|---|
| Audit | LOW | Closes two named carryovers; lock file shape on disk is bit-compatible (verified by the test plan); resolver call delegates to a tested API. |
| Scope | LOW | Net delete; no new abstractions. ~350 lines deleted, ~80 lines added. |
| Shortcut | LOW | This is the option that closes the underlying purpose; type re-exports would defer it. |
| Supported surface | LOW | Receipt JSON, exit codes, on-disk transcript format unchanged. The forward-compat hazard becomes nil. |

## §7 Acceptance

- `cargo test --manifest-path src-tauri/Cargo.toml` passes (no new tests
  required to hit green; existing tests prove the lift preserves behavior).
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  clean.
- `src-tauri/src/session_replace/internal/mod.rs` does not exist.
- `git grep -n 'String::new()' src-tauri/src/session_replace/mod.rs`
  produces no occurrence inside `export_metadata_for`'s `chain_id` field.
- `agents session pause-handshake <id>` followed by
  `agents session import-replace <id>` returns exit `13` `session-busy`.
