# Internal Unification Shortcut - Risk Assessment

**Verdict:** LOW

## Context

The shortcut risk for this initiative is whether the branch merely hides the
`session_replace::internal::*` duplication instead of deleting it and routing
behavior through the canonical modules.

## Observations

- The private `session_replace/internal/mod.rs` module is deleted rather than
  aliased or left as a compatibility wrapper.
- `session_replace` imports and maps public `session_lock` and
  `session_metadata` errors at the boundary, keeping translation local instead
  of leaking duplicate types.
- The additive public APIs are named in the proposal and tested directly:
  `any_active_for_session`, `LockError::Busy.token_hash`, and
  `SessionMetadata.active_segment_id`.
- The on-disk lock layout migration is accepted and documented. The branch
  does not add dual readers for both old and new lock-file shapes.
- Legacy lock debris is documented as inert internal state, not papered over
  with best-effort cleanup in this PR.

## Findings

No medium or high shortcut finding remains. The implementation takes the
direct path: one canonical lock implementation, one canonical metadata
resolver, and boundary-specific error mapping.

## Verdict

Proceed. The remaining residuals are explicit follow-up choices, not hidden
compatibility shims or temporary bypasses.
