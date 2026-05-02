# Problem statement — Lift `session_replace::internal::*` duplicates

## Context

Initiative 06 shipped `agents session import-replace` with a private
`session_replace::internal::SessionLock` and `session_replace::internal::SessionMetadata`.
Those types existed because the parallel sibling features (06-pause-handshake,
06-locate) had not yet merged at branch-creation time.

After merge, both sibling modules now exist as the canonical public API:

- `crate::session_lock::{SessionLock, Lease, ReleaseReceipt, LockError}`
  (Initiative 06-pause-handshake / PR #17, on `main`).
- `crate::session_metadata::{SessionMetadata, SessionStorageType, MetadataError, locate_session_metadata}`
  (Initiative 06-locate / PR #14, on `main`).

`session_replace::internal::*` therefore duplicates module code that already
lives at the canonical site.

## Concrete carryovers from prior synthesis

- **S-PR-F02** (Initiative 06-import-replace, `risk/06-import-replace-supported-surface-pr.md`,
  recorded in `risk/06-import-replace-synthesis.md` as a "non-blocking carryover"):
  > forward-compat note: `internal::SessionLock`, `internal::SessionMetadata`
  > are private. When sibling 06-locate / 06-pause-handshake PRs land, they
  > will reconcile.
- **AIR-SUPPORTED-SURFACE-F04** (Initiative 07-canonical-reader,
  `risk/07-canonical-reader-supported-surface.md`):
  > `chain_id` synthesis in `export_metadata_for` is a forward-compat hazard.
  > `session_replace::mod::export_metadata_for` synthesizes
  > `chain_id: String::new()` because `session_replace`'s private
  > `internal::SessionMetadata` lacks a chain_id field that callers pass
  > through. Today's parsers do not read `chain_id`. The hazard fires the
  > moment a future canonical-reader caller does.

Both findings have the same root cause: `session_replace` does not consume the
public sibling APIs.

## Why this is not yet a defect

- `session_lock::SessionLock` and `internal::SessionLock` are filesystem-
  compatible: both write `.lock` files to `<state-data-dir>/locks/` with the
  same naming scheme. The wire format on disk differs in field ordering but
  not in fields, and neither is read by an external consumer today.
- `session_metadata::SessionMetadata` carries a real `chain_id`;
  `internal::SessionMetadata` synthesizes empty `chain_id` for the
  `export_metadata_for` -> `parse_claude_code_jsonl` path. Today's parsers do
  not read `chain_id`, so the synthesized empty value is observably benign.

The hazards are forward-compat: any future cross-module observer (e.g., a
`session_locks` listing tool) would find the import-replace locks invisible
to `session_lock::*` listing APIs, or vice versa.

## Reproduction

This is not a regression with a red repro test. It is a structural duplication
audit. Direct evidence:

```bash
$ wc -l src-tauri/src/session_replace/internal/mod.rs \
        src-tauri/src/session_lock/mod.rs \
        src-tauri/src/session_metadata/mod.rs
   357 src-tauri/src/session_replace/internal/mod.rs
   620 src-tauri/src/session_lock/mod.rs
   456 src-tauri/src/session_metadata/mod.rs
```

`session_replace/internal/mod.rs` reimplements lock acquire / release
semantics, lock-file naming, sentinel-flock orchestration, lease metadata
shape, and a DB-walking session resolver that all already live in the public
modules.

## Required invariant for the fix

After the fix:

1. `session_replace::internal::SessionLock` and
   `session_replace::internal::SessionMetadata` no longer exist.
2. `session_replace`'s lock acquire / release calls go through
   `session_lock::SessionLock::{new, acquire, release}`.
3. `session_replace`'s session resolution goes through
   `session_metadata::locate_session_metadata`.
4. `export_metadata_for(...)` populates `chain_id` from
   `session_metadata::SessionMetadata::chain_id` rather than `String::new()`.
5. The on-disk filesystem state under `<state-data-dir>/locks/` is
   bit-compatible with PR #17's lock-file shape (one canonical writer; one
   canonical reader).

## Out of scope

- Lease renewal (Phase 7 max-pass design-scope expansion; documented as a
  separate decision, not in this PR).
- `session_metadata::TranscriptState` / `mutable` field changes.

(Rev 3 note: the proposal moves
`session_lock::any_active_for_session` listing API expansion **into scope**
because the orphan-canonical-records recovery scanner requires it. The
problem-statement out-of-scope line that previously listed it is therefore
deleted; see `proposals/09-internal-unification.md` D4.)

## Observable contract preserved

- `agents session import-replace` exit codes 0/1/2/10/11/12/13/14/15
  unchanged.
- `agents session import-replace` receipt JSON shape unchanged.
- `agents session pause-handshake` and `resume-handshake` unchanged.
