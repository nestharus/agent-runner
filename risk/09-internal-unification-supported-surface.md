# Internal Unification - Supported-Surface Risk Report

**Termination signal:** none
**LOW / MEDIUM / HIGH:** LOW

## Supported Surface

Preserved user-visible surface:

- `agents session import-replace` exit codes remain
  `0/1/2/10/11/12/13/14/15`.
- Import-replace success receipt JSON is unchanged.
- Import-replace `13 session-busy` JSON keeps a populated `token` field and
  `expires_at`.
- `agents session pause-handshake` and `resume-handshake` JSON remain
  unchanged.
- `agents session locate` JSON remains unchanged because
  `SessionMetadata.active_segment_id` is `#[serde(skip)]`.

Additive internal/public Rust surface:

- `session_lock::any_active_for_session(lock_dir, session_id)`.
- `session_lock::LockError::Busy { token_hash, .. }`.
- `session_metadata::SessionMetadata.active_segment_id`.

Internal filesystem migration:

- Active lock files move from the private import-replace shape to
  `session_lock`'s `session-{session_id}.lock` schema.
- `sentinel.lock` and `session-{session_id}.released` may appear under the
  internal lock directory.
- Old `provider-*-session-*.lock` files are not consumed by the new code and
  are harmless leftovers.

## Findings

### SS1 - Import-replace busy JSON - PRESERVED

The public lock error now carries the stored token hash, and import-replace
maps it to the existing error JSON. `t_busy_token_hash_preserved` pins the
user-visible hash shape.

### SS2 - Pause/resume-handshake - PRESERVED

The only CLI dispatch change is the `LockError::Busy` pattern update needed
for the additive enum field. Pause/resume-handshake JSON does not opt into
exposing `token_hash`.

### SS3 - Locate JSON - PRESERVED

`active_segment_id` is available to Rust callers but skipped during serde
serialization, so the documented locate JSON does not grow a field.

### SS4 - Public active-session listing helper - ADDITIVE

`any_active_for_session` is a new Rust API for recovery code. Its semantics
are bounded: missing directory or missing/expired lease returns `false`;
operational failures return `LockError::Operational`.

## Recommendation

Proceed. The branch keeps user-visible CLI contracts stable while making the
needed public Rust surface explicit and tested.
