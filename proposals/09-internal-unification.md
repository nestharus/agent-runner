# Proposal — `session_replace` consumes `session_lock` + `session_metadata` (Rev 3)

Closes `S-PR-F02` (06-import-replace forward-compat carryover) and
`AIR-SUPPORTED-SURFACE-F04` (07-canonical-reader chain_id synthesis hazard).

**Rev 3 changes** (close Phase 4 R2 audit / scope / shortcut findings):
- D5 marks `active_segment_id` as `#[serde(skip)]` so the `agents session locate`
  JSON contract is unchanged (closes `AIR-SCOPE-R2-F02`).
- D6 rewritten to acknowledge `session_replace` does **not** currently load
  models, and specifies the four loads required (closes `AIR-AUDIT-R2-F02`,
  `AIR-SHORTCUT-R2-F01`).
- §4 `T-active-segment-id-flows` rewritten to assert the correct schema
  (`last_turn_id` on `session_chain_segments`, `last_used_at` on
  `session_chains`) (closes `AIR-AUDIT-R2-F01`).
- §5 adds `src-tauri/src/main.rs` as a hookpoint for the
  `LockError::Busy { expires_at, .. }` pattern update; the false claim that
  CLI dispatch is untouched is removed (closes `AIR-SCOPE-R2-F01`).

**Rev 2 changes** (closed Phase 4 R1 findings):
- D1 / D5 add `active_segment_id` to `session_metadata::SessionMetadata`
  (closes `AIR-AUDIT-F01`); D3 specifies an RAII release guard (closes
  `AIR-AUDIT-F02`); D4 adds public `session_lock::any_active_for_session`
  listing API (closes `AIR-SUPPORTED-SURFACE-F02`); D7 carries
  `LockError::Busy.token_hash` through to the import-replace error JSON
  (closes `AIR-SUPPORTED-SURFACE-F01`); D8 documents the on-disk layout
  migration explicitly (closes `AIR-SUPPORTED-SURFACE-F03`); §5 names the
  test fixture file as a hookpoint (closes `AIR-SUPPORTED-SURFACE-F05`).

## §1 What changes

`session_replace`'s private `internal::SessionLock` and
`internal::SessionMetadata` are deleted. Their callers consume the public
`session_lock::*` and `session_metadata::*` APIs. Three small additive
extensions to those public APIs are required to make the lift compile and
preserve the import-replace contract:

1. `session_metadata::SessionMetadata` gains `active_segment_id: i64`.
2. `session_lock::LockError::Busy` gains `token_hash: Option<String>`.
3. `session_lock` adds a free function
   `pub fn any_active_for_session(lock_dir: &Path, session_id: &str) -> Result<bool, LockError>`.

After the lift:
- `session_replace::internal::*` is gone.
- The `<state-data-dir>/locks/` directory transitions to `session_lock`'s
  layout (one canonical writer; legacy `provider-{p}-session-{s}.lock` files
  become harmless leftovers; `session-{s}.released` and `sentinel.lock`
  appear per the public schema).
- The `agents session import-replace` `13 session-busy` JSON keeps its
  `token` field with the existing `token_hash` content.

## §2 Decision points

### D1 — Reuse public types; **extend** them where required

Public `session_metadata::SessionMetadata` is the canonical home for session
metadata; `session_lock::SessionLock` and `LockError` are the canonical home
for lock primitives. Where their current shape misses fields that
`session_replace` requires, we **extend** the public type rather than alias
through `internal::*`. The `internal::*` aliases are deleted entirely.

### D2 — Map error variants explicitly at the boundary

`session_lock::LockError` → `ReplaceError::SessionBusy` /
`ReplaceError::OperationalError` etc. via `map_lock_error`.
`session_metadata::MetadataError` → `ReplaceError::*` via `map_metadata_error`.
Both maps live in `session_replace/mod.rs`.

### D3 — RAII release guard at the call site

`session_lock::SessionLock` requires explicit `release()` and has no Drop
release behavior (verified at `src-tauri/src/session_lock/mod.rs:81-245`).
`run_import_replace_bytes` has eight post-acquire return paths
(preimage compute, preimage compare, render, transcript rename, postimage
hash, fresh-export verify, DB open, DB write). Every one must release the
lease before returning, and every release error must not mask the original
operation error.

Add a private RAII guard in `session_replace/mod.rs`:

```rust
struct ImportReplaceLease<'a> {
    lock: &'a SessionLock,
    session_id: String,
    lease: Option<Lease>, // None after explicit commit/release.
}

impl<'a> ImportReplaceLease<'a> {
    fn commit(mut self) -> Result<(), ReplaceError> {
        if let Some(lease) = self.lease.take() {
            self.lock
                .release(&self.session_id, &lease.token)
                .map_err(map_lock_error)?;
        }
        Ok(())
    }
}

impl<'a> Drop for ImportReplaceLease<'a> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            // Best-effort release on error paths; logged at debug, never
            // promoted over the operation's primary error.
            let _ = self.lock.release(&self.session_id, &lease.token);
        }
    }
}
```

Success path: `lease.commit()?` after the SQLite COMMIT. Error paths: the
guard's Drop fires when the function returns, releasing the lease.

This is a code-level change in `session_replace`; no public API surface
changes for the guard.

### D4 — Add `any_active_for_session` to the public `session_lock` module

The orphan-canonical-records recovery scanner at
`session_replace/mod.rs:631-667` calls
`SessionLock::any_active_for_session(data_root, session_id)`. The internal
duplicate currently provides this. The public API does not.

Add to `src-tauri/src/session_lock/mod.rs`:

```rust
pub fn any_active_for_session(
    lock_dir: &Path,
    session_id: &str,
) -> Result<bool, LockError>;
```

Semantics:
- Return `true` if a non-expired lease for `session_id` exists in
  `lock_dir`.
- Return `false` for missing lock dir, missing lease file, or expired lease.
- Errors propagate as `LockError::Operational`.

This is a deliberate **supported-surface expansion** — documented in §6 row
5. Update the problem statement's "Out of scope" section to remove the
listing-API expansion (it is now in scope).

### D5 — `chain_id` and `active_segment_id` flow through

Extend `session_metadata::SessionMetadata`:

```rust
pub struct SessionMetadata {
    // ... existing fields ...
    #[serde(skip)]
    pub active_segment_id: i64,    // NEW; not in agents session locate JSON
}
```

`locate_session_metadata` populates `active_segment_id` from
`session_chain_segments.id` of the chosen active segment row.
`#[serde(skip)]` keeps it out of the `agents session locate` JSON contract
documented in `README.md` "Locating a Session"; only `session_replace`
consumes it.

`session_replace::run_import_replace_bytes` consumes `metadata.active_segment_id`
where it currently uses the private `SessionMetadata.active_segment_id`. The
journal write path and `replace_db_turns` call site become identity-preserving
via the public field.

`export_metadata_for(metadata, ...)` copies `metadata.chain_id` into
`ExportSessionMetadata.chain_id` — the synthesized `String::new()` is gone.

### D6 — `session_replace` loads models for the public resolver call

`session_metadata::locate_session_metadata` requires
`&StateDb, &ModelStore, &ProvidersConfig, &SessionsConfig, &str`. The current
private resolver in `session_replace/mod.rs:803-844` opens `StateDb` and
loads `ProvidersConfig` + `SessionsConfig` but **does not load models** —
model-aware resolution is currently unique to other call sites
(`main.rs::run_session_locate`, `main.rs::run_resume_list`, etc).

The lift adds a model-load step to `session_replace::run_import_replace_bytes`:

```rust
let state = StateDb::open_default().map_err(/* op error */)?;
let models = load_models(&default_models_dir())
    .map_err(/* op error */)?;
let providers = ProvidersConfig::load(&default_config_root().join("providers.toml"))
    .map_err(/* op error */)?;
let sessions = SessionsConfig::load(&default_config_root().join("sessions.toml"))
    .map_err(/* op error */)?;
let metadata = locate_session_metadata(&state, &models, &providers, &sessions, session_id)
    .map_err(map_metadata_error)?;
```

Side-effect contract for the load:
- Adds a `crate::config::load_models` call to `session_replace`. No new
  filesystem dependency — the models dir is the same one consumed by every
  other resolver call site.
- The public locator's model-validation surface (via
  `state.resolve_resume(models, ...)`) now applies to the import-replace
  resolution path. This tightens — not loosens — error reporting:
  `ResumeError::UnknownModel` and
  `ResumeError::ProviderModelMismatch` become reachable.
  `map_metadata_error` routes both to `ReplaceError::OperationalError` /
  `ReplaceError::UnsupportedStorage` per the existing `MetadataError`
  variants. Exit codes remain in {1, 12}; no new code introduced.

### D7 — Carry `token_hash` through `LockError::Busy`

Today's `agents session import-replace` busy JSON is:

```json
{"error": {"code": "session-busy", "token": "<token_hash>", "expires_at": "<rfc3339>"}}
```

The `token` value is the SHA-256 hash of the existing lease's token. Source:
`session_replace::internal::SessionLock::acquire` reads `token_hash` from
the on-disk JSON and stuffs it into `ReplaceError::SessionBusy.token`.

To preserve this contract, extend `session_lock::LockError`:

```rust
pub enum LockError {
    Busy {
        expires_at: String,
        token_hash: Option<String>,  // NEW
    },
    // ... unchanged variants ...
}
```

`SessionLock::acquire` reads `token_hash` from the existing on-disk lease
JSON when emitting `Busy` and populates the new field. `pause-handshake`'s
`exit 13` JSON is unaffected today (it does not currently emit token_hash);
it can opt in if it wants. `import-replace`'s `map_lock_error` reads
`token_hash` (or `""` if `None`) and populates `ReplaceError::SessionBusy.token`.

### D8 — On-disk lock-directory layout migration

The lift transitions `<state-data-dir>/locks/` from the
`session_replace::internal` schema to the `session_lock` schema:

| Aspect | Pre-lift | Post-lift |
|---|---|---|
| Lock filename | `provider-{p}-session-{s}.lock` | `session-{s}.lock` |
| Acquire mutex | transient `.acquire.guard/` mkdir | persistent `sentinel.lock` (flock) |
| Release artifact | none | `session-{s}.released` marker |
| Lock-dir mode | umask default | `0o700` |
| Lock-file mode | umask default | `0o600` |
| Token format on disk | `Uuid::new_v4()` | `pause_<32-hex>` |

Migration semantics:
- Legacy `provider-*-session-*.lock` files from prior runs are no longer
  read or written. They become harmless leftovers. We do **not** auto-clean
  them in this PR; an operator who wants to scrub the dir can `rm` them
  manually. (Alternative: opportunistic cleanup at startup; deferred to a
  separate decision since cohort A is single-machine and the noise is
  bounded.)
- New `session-*.released` markers and `sentinel.lock` appear per the public
  schema.
- `<state-data-dir>/locks/` is **internal filesystem state**; no documented
  consumer outside the binary parses its contents. The supported surface
  is the receipt JSON and exit codes, which are unchanged.

### D9 — Token-format change is accepted forward-compat

Internal generates `Uuid::new_v4()`; public generates `pause_<32-hex>`. The
user-visible field is `token_hash` (sha256 over the token), so the on-wire
hash shape is stable across both generators. The raw token is never exposed
in any current API. Accepted as forward-compat.

## §3 Out of scope

- Lease renewal API surface (separate `DECISIONS.md` entry).
- Auto-cleaning legacy `provider-*-session-*.lock` debris (separate decision).
- Changes to receipt JSON, exit codes, or canonical record shape.

(Deleted from Rev 1's out-of-scope: the `any_active_for_session` listing
API expansion is now in scope per D4.)

## §4 Test plan

Phase 6 tests:

- **Existing tests must remain green** after fixture updates listed in §5:
  - `tests/initiative_06_import_replace.rs` (29 tests + RC suite).
  - `tests/initiative_06_pause_handshake.rs` (12 tests).
  - `tests/initiative_06_locate.rs`.
  - `tests/initiative_07_canonical_reader_unification.rs` (4 tests).
  - The full library + integration suite (~489 tests baseline).

- **New T-cross-module-lock-visibility**: cross-module integration test
  asserting `agents session pause-handshake` followed by
  `agents session import-replace <id>` returns exit `13` `session-busy`
  with `expires_at` populated. Fails pre-lift because the two commands
  write to different lock-file shapes; passes post-lift.
  Lives in `tests/initiative_09_internal_unification.rs`.

- **New T-busy-token-hash-preserved**: assert that
  `agents session import-replace`'s `13 session-busy` JSON still includes
  the `token` field with the SHA-256 of the existing lease's token.
  (Closes `AIR-SUPPORTED-SURFACE-F01`'s no-existing-test gap.)

- **New T-error-path-release**: simulate a forced post-acquire failure
  (e.g., via `TEST_FAIL_POSTIMAGE_VERIFY` env hook); assert that a
  subsequent `pause-handshake` or `import-replace` on the same session
  acquires immediately. Closes `AIR-AUDIT-F02`'s release-on-error
  contract.

- **New T-active-segment-id-flows**: replace a session and assert:
  (a) `session_metadata::locate_session_metadata(...).active_segment_id`
      returns the chosen active segment row id pre-replace.
  (b) `session_chain_segments.last_turn_id` is updated on that exact
      `id` post-replace.
  (c) `session_chains.last_used_at` is updated for `metadata.chain_id`
      post-replace.
  Closes `AIR-AUDIT-F01` and `AIR-AUDIT-R2-F01` (correct schema:
  `last_turn_id` lives on `session_chain_segments`; `last_used_at` lives
  on `session_chains`).

- **New T-any-active-for-session-public**: component test on
  `session_lock::any_active_for_session` covering: missing lock dir,
  missing lease file, present-and-active lease, expired lease.

## §5 Hookpoints

Files touched (additive vs delete):

**Public API extension (additive):**
- `src-tauri/src/session_lock/mod.rs` — extend `LockError::Busy` with
  `token_hash`; populate it on acquire from existing on-disk JSON. Add
  `pub fn any_active_for_session`.
- `src-tauri/src/session_metadata/mod.rs` — add
  `#[serde(skip)] pub active_segment_id: i64` to `SessionMetadata`;
  populate it in `locate_session_metadata`. The `#[serde(skip)]` keeps the
  `agents session locate` JSON contract unchanged.
- `src-tauri/src/state/db.rs` — `ResolvedResume` may need
  `active_segment_id` (verify at Step 6c; if not, the resolver can
  perform the lookup as a single extra column read).

**CLI consumer update (additive pattern fix):**
- `src-tauri/src/main.rs` — `emit_lock_error` (or the equivalent caller of
  `LockError`) currently destructures `LockError::Busy { expires_at }`. After
  D7, that pattern needs `Busy { expires_at, .. }` (or
  `Busy { expires_at, token_hash, .. }` if `pause-handshake` opts to expose
  `token_hash` in its busy JSON). One-line pattern fix; no semantic change.
  The `pause-handshake` busy JSON does NOT change in this PR; the opt-in is
  a separate decision.

**Net delete + lift:**
- `src-tauri/src/session_replace/internal/mod.rs` — **deleted entirely**.
- `src-tauri/src/session_replace/mod.rs`:
  - Delete `pub mod internal;`.
  - Replace `use internal::{...}` with public-module imports.
  - Delete the local `fn locate_session_metadata`, `fn candidate_chain_ids`,
    `fn choose_chain`, `fn locate_transcript_path` (now in
    `session_metadata`).
  - Replace `SessionLock::acquire(...)` with `SessionLock::new(...)?` +
    `.acquire(...)?`.
  - Replace `lock.release()` with the RAII guard pattern (D3).
  - Replace `SessionLock::any_active_for_session` call with the public
    free function.
  - Add `fn map_lock_error(LockError) -> ReplaceError`.
  - Add `fn map_metadata_error(MetadataError) -> ReplaceError`.
  - Update `export_metadata_for(metadata: &session_metadata::SessionMetadata,
    ...)` to take the public type and use `metadata.chain_id`.
  - Delete the local `StorageType` enum (use
    `session_metadata::SessionStorageType` instead).

**Test fixture (additive):**
- `src-tauri/tests/fixtures/initiative_06_import_replace.rs`:
  - `lock_path(provider, session)` → `lock_path(session)` returning
    `session-{s}.lock`.
  - `write_active_lock(provider, session)` → either updated to write the
    new on-disk shape, OR (preferred) replaced with a helper that uses
    `session_lock::SessionLock` to acquire a real lease for the test.

**New test target:**
- `src-tauri/tests/initiative_09_internal_unification.rs` — homes for
  T-cross-module-lock-visibility, T-busy-token-hash-preserved,
  T-error-path-release, T-active-segment-id-flows,
  T-any-active-for-session-public.

CLI dispatch is touched only in the one spot listed above (the
`LockError::Busy` pattern); no functional behavior change to
`agents session pause-handshake` / `resume-handshake`.

## §6 Supported-surface migration record

| Aspect | Pre-lift | Post-lift | Migration |
|---|---|---|---|
| `agents session import-replace` exit codes | 0/1/2/10/11/12/13/14/15 | unchanged | none |
| `agents session import-replace` receipt JSON | as documented | unchanged | none |
| `13 session-busy` JSON `token` field | `token_hash` of existing lease | unchanged via D7 | none |
| `<state-data-dir>/locks/` filename shape | `provider-{p}-session-{s}.lock` | `session-{s}.lock` (D8) | legacy debris harmless |
| `<state-data-dir>/locks/` mutex artifact | `.acquire.guard/` | `sentinel.lock` (D8) | new permanent file |
| `<state-data-dir>/locks/` release artifact | none | `session-{s}.released` (D8) | new transient marker |
| `session_lock` public surface | no listing API | `any_active_for_session` (D4) | new additive function |
| `session_lock::LockError::Busy` | `{ expires_at }` | `{ expires_at, token_hash }` (D7) | additive field |
| `session_metadata::SessionMetadata` | no `active_segment_id` | `+ active_segment_id` (D5) | additive field |
| `agents session pause-handshake` / `resume-handshake` | as documented | unchanged | none |

## §7 Risk gates (Phase 4 self-assessment, Rev 2)

| Gate | Verdict | Reasoning |
|---|---|---|
| Audit | LOW | D1/D5 add `active_segment_id`; D3 specifies the RAII release guard with explicit error-path semantics; D4 adds the public listing API. All findings from Rev 1 audit have explicit closure. |
| Scope | LOW | The lift now openly includes three additive public-API extensions. Each is the minimum surface required. No abstractions beyond the closure of named findings. |
| Shortcut | LOW | The lift remains the right direction; D1/D4/D7 are explicit additive expansions, not shortcuts. |
| Supported surface | LOW | D7 preserves `13 session-busy` JSON content; D8 documents the on-disk migration; D4 calls the new listing API a deliberate addition with semantics; F05's fixture concern is in §5. |

## §8 Acceptance

- `cargo test --manifest-path src-tauri/Cargo.toml` passes (existing tests
  green after fixture updates; new tests in
  `initiative_09_internal_unification.rs` green).
- `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
  clean.
- `src-tauri/src/session_replace/internal/mod.rs` does not exist.
- `git grep -n 'String::new()' src-tauri/src/session_replace/mod.rs` produces
  no occurrence inside `export_metadata_for`'s `chain_id` field.
- `agents session pause-handshake <id>` followed by
  `agents session import-replace <id>` returns exit `13` `session-busy`.
- `13 session-busy` error JSON includes a populated `token` field.
- A forced post-acquire failure releases the lease before returning (no
  stuck `session-busy` after error).
