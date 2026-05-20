# spec-session-lifecycle — Resume, chain segments, migration, session lock, metadata

## Source files

- `crates/oulipoly-runtime/src/sessions/mod.rs`
- `crates/oulipoly-runtime/src/session_export/mod.rs`
- `crates/oulipoly-runtime/src/session_export/metadata.rs`
- `crates/oulipoly-runtime/src/session_lock/mod.rs`
- `crates/oulipoly-runtime/src/session_metadata/mod.rs`
- `crates/oulipoly-runtime/src/session_metadata/ambiguity.rs`
- `crates/oulipoly-runtime/src/session_metadata/cwd.rs`
- `crates/oulipoly-runtime/src/session_metadata/errors.rs`
- `crates/oulipoly-runtime/src/session_metadata/ids.rs`
- `crates/oulipoly-runtime/src/session_metadata/locator.rs`
- `crates/oulipoly-runtime/src/session_metadata/locator/claude.rs`
- `crates/oulipoly-runtime/src/session_metadata/locator/codex.rs`
- `crates/oulipoly-runtime/src/session_metadata/metadata_shape.rs`
- `crates/oulipoly-runtime/src/session_metadata/mutability.rs`
- `crates/oulipoly-runtime/src/session_metadata/registry.rs`
- `crates/oulipoly-runtime/src/session_metadata/resume.rs`
- `crates/oulipoly-runtime/src/session_metadata/tests.rs`
- `crates/oulipoly-runtime/src/session_metadata/transcript.rs`
- `crates/oulipoly-runtime/src/session_metadata/workspace.rs`
- `crates/oulipoly-runtime/src/session_replace/mod.rs`
- `crates/oulipoly-runtime/src/migration/mod.rs`

## Preconditions

- A `RuntimeConfig` identifying provider-specific session storage roots
  (the on-disk locations where Claude / Codex persist conversation state).
- A `StateDb` connection for the canonical row-version-tracked session
  metadata.
- For resume operations: a session locator (provider + account + session
  id, possibly a dual-id pair pre/post-migration).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| New invocation, no prior session. | Synthesize a fresh session id; record locator metadata; acquire session lock. |
| Resume a known session id. | Locate session artifacts on disk; verify cwd/workspace correspondence; replay state into the runtime. |
| Resume a session whose provider has migrated (claude pre-flag-day → post-flag-day). | Resolve through `migration/mod.rs`: locate the new id from the dual-id mapping, return success. |
| Resume a session that no longer exists on disk. | Return a structured "not found" error carrying the provider/account; do NOT auto-create a new session. |
| Two callers race to resume the same session id. | `session_lock` serializes: first wins, second blocks or errors per the lock policy. |
| Session export request. | Emit a canonical transcript record covering the session's full chain; `session_export/metadata.rs` produces the metadata sidecar. |
| Session replace request (overwrite ingest). | Resolve target session, validate the replacement payload, atomically swap on-disk artifacts. |
| Locator queries with ambiguous (cwd, workspace) — multiple matches. | `ambiguity.rs` returns an explicit `Ambiguous` outcome carrying every candidate; caller must disambiguate, runtime does not guess. |

## Edge cases

- Pre-migration and post-migration session ids both present on disk —
  prefer the post-migration id; emit a diagnostic noting the duplicate.
- Session transcript file is truncated mid-record — `transcript.rs`
  reports a parse error with the byte offset; the caller decides whether
  to surface or skip.
- Workspace path is a symlink — `cwd.rs` canonicalizes before comparison.
- Provider session storage root does not exist yet — return a structured
  "not initialized" error; do NOT create the directory implicitly.
- Session metadata's `mutability` flag says read-only and a write is
  attempted — `mutability.rs` rejects with a typed error.

## Error conditions

- `SessionNotFound` — locator query returned zero candidates.
- `SessionAmbiguous` — locator query returned multiple candidates.
- `SessionLockContended` — another caller holds the lock and the request
  cannot wait.
- `SessionTranscriptParse` — transcript file is malformed.
- `SessionMigrationFailed` — dual-id resolution failed mid-migration.
- `SessionExportFailed` / `SessionReplaceFailed` — typed errors carrying
  the operation phase.

## Boundaries

- Session lifecycle does NOT decide which provider to use — that is the
  balancer. The lifecycle takes a locator as input.
- Session lifecycle does NOT execute the provider process — that is the
  executor. It hands the executor a resume id, then ingests outputs back.
- Session lifecycle does NOT classify terminal signals — that is the
  recognizer.
- Session metadata writes go through the row-version-tracked `oulipoly-state`
  surface; the lifecycle module never bypasses that.

## Declared test patterns

Per `~/ai/conventions/testing.md`: locator-resolution table tests,
migration dual-id tests, lock contention tests, transcript round-trip
tests.

- `crates/oulipoly-runtime/tests/age35_lifecycle_service_parity.rs`
- `crates/oulipoly-runtime/tests/age37_export_service_parity.rs`
- `crates/oulipoly-runtime/tests/age37_lock_service_parity.rs`
- `crates/oulipoly-runtime/tests/age37_replace_service_parity.rs`
- `crates/oulipoly-runtime/tests/migration_service_parity.rs`
- `crates/oulipoly-runtime/tests/resume_service_parity.rs`
- `crates/oulipoly-runtime/tests/session_lifecycle_service.rs`
- `src-tauri/tests/age100_one_shot_quota_migration.rs`
- `src-tauri/tests/age100_resume_quota_migration.rs`
- `src-tauri/tests/age123_resume_provider_identity.rs`
- `src-tauri/tests/age134_main_prompt_resolution.rs`
- `src-tauri/tests/age134_main_session_and_migrate.rs`
- `src-tauri/tests/age37_session_export_format_validation.rs`
- `src-tauri/tests/age37_session_import_replace_missing_file.rs`
- `src-tauri/tests/age37_session_lock_malformed_metadata.rs`
- `src-tauri/tests/age37_session_pause_handshake_lock_operational.rs`
- `src-tauri/tests/age53_session_id_dual_id_integration.rs`
- `src-tauri/tests/initiative_05_migration.rs`
- `src-tauri/tests/initiative_06_export.rs`
- `src-tauri/tests/initiative_06_import_replace.rs`
- `src-tauri/tests/initiative_06_locate.rs`
- `src-tauri/tests/initiative_06_pause_handshake.rs`
- `src-tauri/tests/initiative_06_schema_probe.rs`
- `src-tauri/tests/session_lock_cross_platform.rs`
- `src-tauri/tests/session_metadata_component.rs`
- `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`
- `src-tauri/tests/structural_segmentation.rs`

## Cross-references

- `planning/coverage/spec-state-db.md` — the row-version-tracked metadata
  store this surface writes through.
- `planning/coverage/spec-executor.md` — passes resume ids to the
  provider executable.
- `planning/coverage/spec-discovery.md` — discovers installed providers
  whose sessions this surface manages.
- `AGENTS.md` § "State DB Schema Migrations".
