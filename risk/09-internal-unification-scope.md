# Internal Unification - Scope Risk Assessment

**Verdict:** LOW

## Purpose

The initiative lifts `session_replace` off its private
`session_replace::internal::{SessionLock, SessionMetadata}` duplicates and
onto the canonical public modules that already exist on `main`:
`session_lock` and `session_metadata`.

## Boundaries

In scope:

- Delete `src-tauri/src/session_replace/internal/mod.rs`.
- Replace import-replace lock operations with `session_lock::SessionLock`.
- Replace import-replace metadata resolution with
  `session_metadata::locate_session_metadata`.
- Preserve import-replace receipt JSON, error JSON, and exit codes.
- Add the minimal public extensions required by the lift:
  `LockError::Busy.token_hash`, `SessionMetadata.active_segment_id`, and
  `session_lock::any_active_for_session`.
- Update fixtures and regression tests that directly depended on the old
  private lock-file shape.

Out of scope:

- Lease renewal.
- Auto-cleaning legacy `provider-*-session-*.lock` files.
- Changing canonical record shape, receipt JSON, or pause/resume-handshake
  JSON.
- Redesigning `session_metadata::TranscriptState` or the `mutable` field.

## Stakeholders

- CLI users of `agents session import-replace`.
- CLI users of `agents session pause-handshake` and `resume-handshake`.
- Future session tooling that needs one canonical lock and metadata surface.
- Maintainers of the session export/import/locate code paths.

## Assumptions

- `<state-data-dir>/locks/` is internal filesystem state; external consumers
  rely on documented CLI JSON and exit codes, not lock-file names.
- Public `session_lock` and `session_metadata` are the canonical homes for
  lease and session metadata behavior.
- Legacy lock files can be ignored safely because only the current binary
  writes the active canonical lock shape after this lift.

## Deliverables

- Public lock and metadata extensions are implemented.
- Private duplicate module is removed.
- Import-replace uses the public APIs.
- Phase 6 regression tests cover cross-module lock visibility, busy token
  preservation, error-path release, active segment id flow, and the public
  active-session listing helper.

## Scope Determination

The scope is narrow and coherent. The public API additions are explicit and
directly required by the duplicate-removal goal; no unrelated session behavior
is pulled into this branch.
