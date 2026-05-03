# WU-14-01 — Phase 4 Scope Risk

## 1. Verdict

**LOW**

## 2. Findings

Boundary check against `tickets/phase-14:plans/tickets/phase-14/WU-14-01.md`:

### Code Boundary

The proposal's product-code edits all fall inside the ticket's listed
in-scope surfaces:

- `src-tauri/src/migration/mod.rs` — signature change to
  `migrate_chain_segment`, new `claude_project_dir_for` helper, new
  `MigrationError::SpawnCwdUnsupported` variant
  (proposal lines 13–48). Ticket lists this file explicitly:
  "`src-tauri/src/migration/mod.rs` — `migrate_chain_segment` signature
  and target-path computation."
- `src-tauri/src/main.rs` — both call sites in `run_repl` and
  `run_resume` (proposal lines 50–72). Ticket lists:
  "`src-tauri/src/main.rs:1600-1610, 1824-1900` — call sites of
  `migrate_chain_segment` and the executor handoff."
- `src-tauri/src/executor/cli.rs` — removal of `ResumePayload.target_jsonl_path`
  and the dead `_target_jsonl_path` parameter from `compose_resume_args`
  (proposal lines 74–83). Ticket explicitly authorizes this:
  "Either consume it or remove it; do not leave it as dead code."
- `scripts/claude-code-locate-transcript` — proposal plans no code
  change, only a Phase 6 verification (proposal lines 119–121); ticket
  matches: "Likely no changes needed; confirm."
- `README.md` — short paragraph documenting cwd-derived re-anchoring
  (proposal lines 123–127); ticket matches AC-7.

### Anti-scope

The proposal does not leak into any forbidden surface:

- `src-tauri/src/balancer/mod.rs` — proposal section 2 explicitly:
  "Do not change migration policy or `src-tauri/src/balancer/mod.rs`"
  (line 150). No edits proposed.
- `src-tauri/src/state/db.rs` — proposal section 2: "Do not change
  `src-tauri/src/state/db.rs` or session graph schema" (line 158). No
  edits proposed; chain segment open/close uses existing API.
- Body-storage-in-DB — proposal section 2: "Do not introduce
  body-storage-in-DB as a shortcut" (line 147).
- Frontend (`src/`) — proposal section 2: "Do not change frontend
  files under `src/`" (line 159). Supported-surface track section 6
  also confirms "frontend files unchanged" (line 350).
- Codex migration — proposal preserves `CodexMigrationDeferred`
  rejection. Section 2: "Codex-side migration remains rejected with
  `CodexMigrationDeferred`" (line 155). Test-intent row at line 322
  asserts the existing deferred error stays green.
- `MigratedSegment.target_jsonl_path` contract — proposal keeps it as
  "the path migration actually wrote" inside the target provider
  storage root (lines 38–42), satisfying ticket anti-scope item.
- No new cross-CLI migration path; no new `MigrationError` family
  beyond the single `SpawnCwdUnsupported` variant scoped to spawn-cwd
  validation (proposal section 2 line 154).

### Test Boundary

The proposal's test edits fall inside the ticket's authorized test
surfaces:

- `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`
  flips RED→GREEN (proposal lines 87–91); ticket explicitly lists this
  as in-scope and required to turn green.
- `src-tauri/src/migration/mod.rs` inline tests — split of
  `migration_reuses_source_session_id_on_target_side` into two tests
  plus two new helper tests (proposal lines 98–106, 318); ticket lists
  inline migration tests as in-scope.
- `src-tauri/tests/initiative_05_migration.rs` — fixture updates to
  supply explicit spawn cwd at every `migrate_chain_segment` call site
  (proposal lines 108–116). Ticket AC-3 explicitly names
  `initiative_05_*` tests as required-green; mechanical signature-
  propagation updates are necessary to keep them compiling and are
  not new test creation. Ticket Test Boundary out-of-scope list does
  not include this file.
- `src-tauri/src/executor/cli.rs` compose tests — replaced/renamed
  after dead-parameter removal (proposal line 323); covered by the
  ticket's in-scope executor surface and its "Either consume it or
  remove it" clause.

The forbidden test trees are all left untouched:

- `src-tauri/tests/routing_fanout_rca/` — proposal line 174:
  "Do not touch [it]". No edits proposed.
- `src-tauri/tests/release_yml_contract.rs` — proposal line 174:
  "Do not touch". No edits proposed.
- `src-tauri/tests/session_lock_cross_platform.rs` — proposal line 175:
  "Do not touch". No edits proposed.
- `e2e/` — proposal line 176: "Do not add e2e/Playwright coverage".

### Deferrals (no-deferred-stubs.md)

The proposal carries two deferrals; both are correctly named, not
stubbed silently:

- **Windows path hashing.** Proposal line 47: "Windows path hashing is
  out of scope for this WU". Lines 171–173 name the future work unit
  `WU-14-02-windows-claude-path-hash` and the future reproduction
  harness path `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`.
  At the call boundary, non-Unix input does not silently no-op; it
  raises `MigrationError::SpawnCwdUnsupported { provider, cwd }`
  (proposal lines 23–28, 43–48), which matches no-deferred-stubs.md's
  required pattern: "Raise an explicit error on use, not a silent
  stub." Helper-test row at line 318 covers the rejection contract.
- **Symlink canonicalization.** Proposal A3 (lines 268–276) explicitly
  documents non-canonicalization as a preserved current-state behavior
  — symlinks are forwarded as-is to `cmd.current_dir`, matching the
  existing executor path. This is not a deferred stub: there is no new
  surface that promises symlink handling and silently returns nothing.
  The Phase 5 open question (lines 382–384) is a research item
  conditional on future evidence, which the convention permits.

The new `MigrationError::SpawnCwdUnsupported` variant is also tested as
deferred behavior (proposal line 318:
`claude_project_dir_for_rejects_relative_or_empty_cwd`), satisfying
no-deferred-stubs.md's "Test the deferred stub as deferred."

## 3. Justification

The proposal stays inside every boundary the ticket draws — code,
anti-scope, and tests — and its only deferral (Windows hashing) is
named to a future WU with a future reproduction harness path and an
explicit error at the boundary, satisfying no-deferred-stubs.md.
