# Contract — WU-14-01 session-migration-cwd

Owner: implementation-pipeline-orchestrator (Phase 6a; orchestrator-authored)
Source:
- `proposals/14-session-migration-cwd.md` (revised, Phase 4 LOW)
- `research/14-problem-map.md`
- `research/14-hookpoints.md`
- `research/14-session-migration-rca.md` (Phase 0 RCA)
- `tmp/scratch/wu-14-01/ticket.md` / `tickets/phase-14:plans/tickets/phase-14/WU-14-01.md`
Inputs to Step 6b (test writer) and Step 6c (code writer).

This contract is the orchestrator's interface between the test
agent (Step 6b) and the code agent (Step 6c). The test agent
does NOT see the code agent's output. The code agent reads this
contract, the proposal, the hookpoints, the problem map, the
RCA, and the Step 6b output index — and only then writes
product code.

---

## 1. Acceptance criteria (from ticket)

- **AC-1** —
  `session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir`
  passes on the post-fix branch. After a same-CLI cross-account
  Claude migration, the target child process can resume the
  session from its working directory.
- **AC-2** — When the target provider's `session_storage` is
  `ClaudeCode`, `migrate_chain_segment` writes the JSONL under
  the directory hash that matches the **child process cwd** at
  spawn time, not the source transcript's parent directory.
- **AC-3** — Existing migration tests in
  `src-tauri/tests/initiative_05_migration.rs` and the inline
  `migration::tests` module stay green on the post-fix branch.
  The migration's atomicity contract (tmp + rename) is preserved.
- **AC-4** — Cross-CLI migration (`SessionStorage::ClaudeCode`
  ↔ `SessionStorage::Codex`) continues to be rejected with
  `MigrationError::CodexMigrationDeferred`. This WU does NOT
  introduce a canonical-record cross-CLI migration path.
- **AC-5** — The session graph (`session_chains` +
  `session_chain_segments`) remains correctly updated by
  migration: source segment closed, target segment opened,
  chain pointer updated. DB-side semantics are not touched.
- **AC-6** — `cd src-tauri && cargo fmt --check && cargo clippy
  -- -D warnings && cargo test --no-fail-fast` all green on
  Linux + macOS in CI. Frontend regression gates stay green
  (no `src/` changes expected): `bun run check && bunx tsc
  --noEmit && bun run test`.
- **AC-7** — `README.md` §Load Balancing or §Resuming a session
  is updated to document that migration during a session-bound
  resume re-anchors the transcript at the child's cwd-derived
  path. One short paragraph; no API contract changes.

## 2. Code surfaces (in-scope)

- `src-tauri/src/migration/mod.rs`:
  - Add `pub(crate) fn claude_project_dir_for(provider: &str,
    cwd: &Path) -> Result<String, MigrationError>` (signature
    in § 4 below).
  - Add `MigrationError::SpawnCwdUnsupported { provider: String,
    cwd: String }` variant.
  - Change `migrate_chain_segment` signature to take
    `resume_working_dir: &Path` immediately before
    `target_provider_index`.
  - Replace the source-derived block at
    `src-tauri/src/migration/mod.rs:155-161` with a call to
    `claude_project_dir_for(&target.name, resume_working_dir)?`.
  - Replace the inline test
    `migration_reuses_source_session_id_on_target_side` with
    two focused tests (§ 5 below).
- `src-tauri/src/main.rs`:
  - At `run_repl` migration call site (`:1606-1614`): compute
    effective spawn cwd (§ 4 below) and pass it to
    `migrate_chain_segment`.
  - At `run_resume` migration call site (`:1830-1838`): same.
  - Remove `target_jsonl_path: None` initializer from
    `ResumePayload` construction at `:1701` and `:1900`.
- `src-tauri/src/executor/cli.rs`:
  - Delete the `target_jsonl_path: Option<&'a Path>` field from
    `ResumePayload` (`:279`).
  - Delete the `_target_jsonl_path: Option<&Path>` parameter
    from `compose_resume_args` (`:282-286`).
  - Delete tests
    `compose_resume_args_ignores_target_jsonl_for_flag_strategy`
    (`:1786-1803`) and
    `compose_resume_args_ignores_target_jsonl_for_subcommand_strategy`
    (`:1807-1823`). They assert dead behavior.
  - Remove `target_jsonl_path: None` initializer from executor
    tests at `:1096`, `:1143`, `:1190`, `:1239`.
- `src-tauri/tests/initiative_05_migration.rs`:
  - Update every `migrate_chain_segment(...)` call site to
    pass an explicit fixture spawn cwd. Listed call sites: `:644`,
    `:723`, `:802`, `:883`, `:910`, `:958`, `:988`, `:1012`,
    `:1042`, `:1068`, `:1094`, `:1120`, `:1155`, `:1185`.
  - Update exact-target-path assertions at `:734` and `:894` to
    expect the spawn-cwd-derived path; do NOT relax to a
    `starts_with` shape.
- `src-tauri/tests/pr_f_resume_integration.rs`:
  - The fixture's `stage_claude_jsonl` helper at `:222-234`
    constructs the source/target Claude project dir using a
    fixed string `"cwd-hash-fixture"`. The test
    `repl_resume_migrates_to_least_loaded_provider` at `:902-951`
    spawns the runner via `fixture.run_repl(...)`, which exercises
    `run_repl` end-to-end including migration. Post-fix, the
    migration writes under the SPAWN-cwd-derived directory rather
    than `cwd-hash-fixture`. The fixture invokes the runner with
    `current_dir(&self.cwd)` (see `Fixture::run_repl`); the
    `effective_spawn_cwd` for migration will therefore be the
    fixture cwd. The test assertion at `:945-950` must derive the
    expected target path from the fixture cwd via the same
    encoding the production helper uses (i.e., test-only
    `claude_project_dir_name(&fixture.cwd)` analogous to
    `src-tauri/tests/session_migration_rca/mod.rs:129`).
  - Update the `repl_resume_stays_when_active_is_least_loaded`
    test only if its source-jsonl staging breaks; the stay-path
    does not trigger migration, so the test should remain green
    without target-path edits.
  - This is mechanical signature/expectation propagation, like
    the `initiative_05_migration.rs` updates. Phase 5 hookpoint
    research missed this file because it does NOT call
    `migrate_chain_segment(...)` directly; it goes through
    `run_repl` end-to-end. Step 6b SHALL update it.
- `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`:
  - Update expected post-fix path: `migrated.target_jsonl_path
    == resume_project_target` (the cwd-derived target).
  - Update the negative-existence assertion's polarity (the
    source-cwd target should NOT exist post-fix because
    migration writes only one place; assert
    `!source_project_target.exists()`).
  - Pass `&fixture.resume_workspace` as the new
    `resume_working_dir` argument to `migrate_chain_segment`.
  - Drop `target_jsonl_path` from the `ResumePayload`
    construction (`:57`) — the field no longer exists.
- `src-tauri/tests/session_migration_rca/mod.rs`:
  - Add no new helper unless Step 6b needs one for the new
    same-cwd / different-cwd inline tests.
- `README.md`:
  - Append/insert one short paragraph in the resume / session
    storage section describing that Claude Code migration
    during resume re-anchors the target JSONL under the child
    process cwd-derived project directory.

## 3. Code surfaces (anti-scope; do NOT touch)

- `src-tauri/src/balancer/` — migration POLICY (when migration
  triggers) is correct per ticket; only content-transfer
  mechanics change here.
- `src-tauri/src/state/db.rs` — no schema changes.
- `src-tauri/src/sessions/mod.rs` — `locate_transcript`
  semantics unchanged.
- `src-tauri/src/session_metadata/mod.rs` — `decode_claude_project_dir_candidates`
  inversion helper untouched.
- `src-tauri/src/session_metadata/` body-storage — separate
  deferred WU (`research/12-empty-bodies-ref-rca.md`).
- `src/` (frontend), `e2e/`, `playwright.config.ts`.
- `src-tauri/tests/routing_fanout_rca/` — WU-11-01 territory.
- `src-tauri/tests/release_yml_contract.rs`,
  `src-tauri/tests/session_lock_cross_platform.rs` — WU-13-01
  territory.
- `scripts/claude-code-locate-transcript` — no script change
  per Phase 5 verification (§ 5 of `research/14-hookpoints.md`).
- Codex-side migration — remains rejected via
  `CodexMigrationDeferred`; do NOT extend.
- Test-only Claude project-dir encoders in
  `src-tauri/tests/session_migration_rca/mod.rs:129`,
  `src-tauri/tests/fixtures/initiative_06.rs:886`,
  `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995`,
  `src-tauri/tests/fixtures/initiative_06_export.rs:605` —
  leave them as fixture helpers (Phase 5 § Reuse points
  recommends NOT importing the production helper into
  integration tests; production helper is `pub(crate)`).
- No backwards-compatibility shim for the old source-derived
  target path.
- Symlink canonicalization — out of scope; do NOT canonicalize
  paths inside migration or main.rs cwd derivation.
- Windows path-hash — out of scope (deferred to
  `WU-14-02-windows-claude-path-hash` with future harness
  `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`).
  Rejection happens via `MigrationError::SpawnCwdUnsupported`
  for non-absolute or empty cwd.

## 4. Schemas, signatures, and constants

### New helper

```rust
// src-tauri/src/migration/mod.rs
pub(crate) fn claude_project_dir_for(
    provider: &str,
    cwd: &Path,
) -> Result<String, MigrationError> {
    // 1. Reject empty paths.
    // 2. Reject non-absolute paths via MigrationError::SpawnCwdUnsupported.
    // 3. Convert via path.to_string_lossy(), replace every '/' with '-',
    //    and return the resulting String.
}
```

Notes for Step 6c:

- Empty path is `cwd.as_os_str().is_empty()`.
- Non-absolute is `!cwd.is_absolute()`.
- Both error cases produce `MigrationError::SpawnCwdUnsupported
  { provider: provider.to_string(), cwd: cwd.display().to_string() }`.
- For an absolute Unix path like `/home/nes/x`, the returned
  string is `-home-nes-x` (i.e., `path.to_string_lossy().replace('/', "-")`).
- Visibility is `pub(crate)`. Inline tests live in the same
  module under `#[cfg(test)] mod tests`.
- The helper does NOT canonicalize symlinks.
- The helper does NOT special-case Windows. On Windows,
  `cwd.is_absolute()` for `C:\x` is true, but the encoding would
  produce `C:\x` → `-C:\x` after the slash-replace, which is
  wrong for Claude Code's Windows hashing. The proposal scopes
  Windows out; for this WU on Linux/macOS only, no special
  handling is needed beyond rejection of non-absolute paths.
  If Step 6c needs to be doubly-defensive, return
  `SpawnCwdUnsupported` when the path string contains `\`
  (Windows backslash), but this is OPTIONAL — no Windows test
  exercises it in this WU.

### `MigrationError::SpawnCwdUnsupported`

```rust
// src-tauri/src/migration/mod.rs
#[derive(Debug, Clone)]
pub enum MigrationError {
    // ... existing variants ...
    SpawnCwdUnsupported {
        provider: String,
        cwd: String,
    },
    // ... existing variants ...
}
```

- Existing main.rs handling at `src-tauri/src/main.rs:1624` and
  `:1851` uses `eprintln!("migration failed: {err:?}")`. The
  `Debug` derive prints the variant name + provider + cwd.
  Acceptable for this WU; no `Display` impl required.

### `migrate_chain_segment` — new signature

```rust
pub fn migrate_chain_segment(
    state: &StateDb,
    sessions_cfg: &SessionsConfig,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    resume_working_dir: &Path,
    target_provider_index: usize,
    reason: TransitionReason,
    stderr: &mut dyn Write,
) -> Result<MigratedSegment, MigrationError>
```

- New parameter `resume_working_dir: &Path` is inserted between
  `resolved` and `target_provider_index`. This matches the
  Phase 5 hookpoint recommendation and minimizes downstream
  test churn (only the order matters).
- Inside the function, replace lines 155-161 with:

```rust
let cwd_project_dir = claude_project_dir_for(&target.name, resume_working_dir)?;
```

  Then update `target_dir = projects_dir.join(&cwd_project_dir)`
  at the existing target-dir hookpoint (current `:188`).

- All other behavior (source discovery, byte read, compaction
  slicing, atomic write, conflict check, chain segment
  close/open, `[migrate]` emission, `MigratedSegment` return
  shape) is unchanged.

### `ResumePayload` — field removal

```rust
// src-tauri/src/executor/cli.rs
pub struct ResumePayload<'a> {
    pub session_id: &'a str,
    pub strategy: &'a ResumeStrategy,
    // target_jsonl_path field is DELETED.
}
```

- Update `compose_resume_args` to drop the `_target_jsonl_path`
  parameter:

```rust
pub fn compose_resume_args(
    strategy: &ResumeStrategy,
    session_id: &str,
) -> Result<Vec<String>, String> { ... }
```

- `compose_resume_provider_args` is unchanged in body but
  consumes `ResumePayload` without the deleted field.

### Effective-cwd derivation in main.rs

Both `run_repl` (around `:1606`) and `run_resume` (around
`:1830`) do:

```rust
let effective_spawn_cwd = match working_dir {
    Some(p) => {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|base| base.join(p))
                .map_err(|e| format!("failed to resolve cwd: {e}"))?
        }
    }
    None => std::env::current_dir()
        .map_err(|e| format!("failed to resolve cwd: {e}"))?,
};
```

- The error mapping shape (`format!` into a String) matches
  existing `format_resume_error` patterns in `main.rs`. If
  `format_resume_error` is the canonical wrapper, prefer it.
  Step 6c picks the closest existing error-conversion shape;
  the bare `String` form is acceptable.
- The cwd is computed BEFORE the migration call; it is passed
  by reference (`&effective_spawn_cwd`) into
  `migrate_chain_segment`.
- Do NOT canonicalize symlinks. Do NOT change `working_dir`
  itself; the executor receives the same `working_dir` it does
  today. Migration computes its OWN local copy of the
  effective-cwd for its target-path computation.
- This computation runs unconditionally in the migration
  branch only (i.e., gated by
  `Ok(balancer::MigrationDecision::Migrate { .. })`). Don't
  run it in the non-migrating happy path — that would change
  observable error behavior.

## 5. Test boundary (Step 6b)

### Test file — RCA harness update (`src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`)

The RCA harness is the **central regression test**. Step 6b
does NOT delete or weaken it. Step 6b modifies it as follows:

- Pass `&fixture.resume_workspace` as the new
  `resume_working_dir` argument to `migrate_chain_segment`.
- Update the expected target path to be the resume-cwd-derived
  path:
  ```rust
  assert_eq!(migrated.target_jsonl_path, resume_project_target);
  ```
- Update the negative-existence assertion: post-fix migration
  writes ONLY the resume-cwd target path, so the source-cwd
  target should NOT exist:
  ```rust
  assert!(!source_project_target.exists(),
      "post-fix migration must not write under the source workspace project dir");
  ```
- Drop `target_jsonl_path: Some(&migrated.target_jsonl_path)`
  from the `ResumePayload` construction (the field no longer
  exists).
- The exit-code assertion at the end of the test stays:
  `assert_eq!(result.exit_code, 0, ...);` This is the
  **AC-1** signal that flips RED→GREEN.

risk annotation:
```rust
// risk: RC-1 cwd/source project dir mismatch; level: particular-integration; source: research/14-session-migration-rca.md (RC-1).
```

### Test file — same-cwd happy path (inline `migration::tests`)

`migration_reuses_source_session_id_when_source_and_spawn_cwd_match`
in `src-tauri/src/migration/mod.rs` `#[cfg(test)] mod tests`:

- Source workspace and resume workspace share the same path.
- Migration succeeds; `target_jsonl_path` ends with
  `<target_projects>/<-encoded-cwd>/<session_id>.jsonl`.
- Target session id == source session id.
- DB chain id maps to target provider/session.

risk annotation:
```rust
// risk: Migration mechanic source UUID reuse; level: particular-integration; source: proposal §11.1 Migration mechanic / A1.
```
(This carries forward the original test's annotation; it is the
same-cwd half of the previous test split.)

### Test file — different-cwd corrected path (inline `migration::tests`)

`migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`:

- Source workspace path differs from resume workspace path.
- Migration succeeds; `target_jsonl_path` ends with
  `<target_projects>/<-encoded-RESUME-cwd>/<session_id>.jsonl`,
  NOT `<target_projects>/<-encoded-SOURCE-cwd>/<session_id>.jsonl`.
- Target session id == source session id.
- DB chain id maps to target provider/session.
- The source-cwd-derived target directory is empty / does not
  contain a JSONL with this session id.

risk annotation:
```rust
// risk: RC-1 cwd/source project dir mismatch; level: particular-integration; source: research/14-session-migration-rca.md (RC-1) + research/14-problem-map.md §2.
```

### Test file — helper unit tests (inline `migration::tests`)

Three small unit tests for `claude_project_dir_for`:

- `claude_project_dir_for_encodes_absolute_unix_path`:
  `/home/nes/x` → `-home-nes-x`.
- `claude_project_dir_for_rejects_relative_path`:
  `Path::new("relative/x")` → `Err(MigrationError::SpawnCwdUnsupported { .. })`.
- `claude_project_dir_for_rejects_empty_path`:
  `Path::new("")` → `Err(MigrationError::SpawnCwdUnsupported { .. })`.

risk annotation (same on all three):
```rust
// risk: Cwd-to-project-dir encoding correctness; level: unit; source: proposal §1 helper signature / A2.
```

### Test file — `src-tauri/tests/initiative_05_migration.rs` updates

Mechanical signature propagation. For every
`migrate_chain_segment(...)` call site listed in § 2 above,
add the new `&effective_spawn_cwd` argument. The simplest pattern:

- Define a single `let resume_working_dir = source_workspace.clone();`
  (or whatever the test fixture's spawn-cwd happens to be) at
  the top of each test. For tests that don't construct an
  explicit "source workspace" path, use the `tempdir().path()`
  itself (it's absolute) as the spawn-cwd.
- Pass `&resume_working_dir` into every migration call.
- Tests at `:734` and `:894` that assert the EXACT old target
  path: update the expected path to be derived from the new
  `resume_working_dir`. Do NOT relax to `starts_with`.

The Codex deferred tests at `:1155` and `:1185` need the
parameter too, but their assertions remain unchanged (early
return rejects the migration before path computation).

### Test file — `src-tauri/tests/pr_f_resume_integration.rs` updates

End-to-end test that drives `run_repl` and observes the
side-effects (target JSONL placement, stderr migration line,
DB chain pointer). After the WU-14-01 fix:

- The test fixture's source JSONL placement under
  `<source_projects>/cwd-hash-fixture/<session>.jsonl` (via
  `stage_claude_jsonl` at `:222-234`) stays as-is. This is the
  SOURCE side, found by `find_claude_source_from_storage`'s
  read-dir scan, and is independent of the post-fix target
  placement. Do NOT rename the helper or remove
  `cwd-hash-fixture` from the source-side fixture.
- `base_repl_command` at `:326-339` does NOT call
  `cmd.current_dir(...)`, so the spawned child inherits the
  test process's cwd (typically `<repo>/src-tauri` when
  `cargo test` runs). Add a deterministic spawn cwd by
  setting:

  ```rust
  cmd.current_dir(self.dir.path());
  ```

  in `base_repl_command` (and matching it in the resume
  base-commands if those paths trigger migration in tests
  that already exercise migration; see existing usages).
  Doing so makes the post-fix target directory deterministic
  for every existing test.
- The post-migration assertion at
  `repl_resume_migrates_to_least_loaded_provider:945-950`:

  ```rust
  // before:
  assert!(
      target_projects
          .join("cwd-hash-fixture")
          .join(format!("{session_id}.jsonl"))
          .exists()
  );

  // after:
  let expected_target_dir =
      fixture.dir.path().to_string_lossy().replace('/', "-");
  assert!(
      target_projects
          .join(&expected_target_dir)
          .join(format!("{session_id}.jsonl"))
          .exists(),
      "expected target JSONL under spawn-cwd-derived dir {}",
      expected_target_dir
  );
  ```

  Use the test-only encoder pattern (slash-replaced) directly
  inline; do NOT import the production `pub(crate)` helper.
- Add a negative-existence assertion that
  `target_projects.join("cwd-hash-fixture").join(format!("{session_id}.jsonl"))`
  does NOT exist post-fix. This locks in the AC-2 contract
  end to end.
- If any other test in this file exercises migration (search
  for `[migrate]` stderr assertions or `target_projects` reads
  after the runner runs), apply the same encoding shape.

risk annotation:
```rust
// risk: RC-1 cwd/source project dir mismatch end-to-end via run_repl;
//       level: end-to-end; source: research/14-session-migration-rca.md (RC-1) + contract §5.
```

### Test file — `src-tauri/src/executor/cli.rs` test updates

- Delete tests
  `compose_resume_args_ignores_target_jsonl_for_flag_strategy`
  and
  `compose_resume_args_ignores_target_jsonl_for_subcommand_strategy`.
  They assert that a now-deleted parameter is ignored; the
  parameter is gone.
- Update the four tests at `:1089`, `:1133`, `:1180`, `:1232`
  to drop `target_jsonl_path: None` from their `ResumePayload`
  construction.

### Step 6b output-index requirements

Step 6b MUST produce
`tmp/scratch/wu-14-01/phase6/step6b-output-index.md` with
exactly the schema from `~/ai/workflows/implementation-pipeline.md`
Phase 6b output-index spec. Required fields:

- approved proposal path:
  `proposals/14-session-migration-cwd.md`
- contract path:
  `product-strategy/contracts/wu-14-01-session-migration-cwd.md`
- approved problem map path:
  `research/14-problem-map.md`
- supported-surface risk path: `risk/14-supported-surface.md`
- hookpoint research path: `research/14-hookpoints.md`
- Step 6b prompt path: (Step 6b's own prompt path)
- Step 6b log path: (Step 6b's own log path)
- For each test-intent item from the proposal § 5:
  - the named risk (RC-1, A1, A2, A3, A4, A5, A6, A7)
  - selected level (unit / particular-integration / component)
  - proposal or assumption-register source
  - emitted test file path
  - test or test-group identifier (function name)
  - residual entry path when applicable
  - declared fixture source / fixture application point
  - documented non-applicability reason when no test is
    emitted
- A "Step 6c MUST consume" checklist that lists every file
  Step 6c will need to read or modify, in the same order
  Step 6c reads them.
- A list of risks the tests could not encode, mapped to a
  residual class per Phase 6b spec
  (`combinatorial/path-state`, `bounded-model`,
  `integration-hidden`, `emergent-interaction`,
  `temporal/concurrency`, `generator/search-budget`).

If a named risk cannot be verified by tests, Step 6b writes
`risk/14-test-residuals.md` listing each unverified risk with
the residual class, technique attempted, scope, budget, result,
remaining residual, and whether it changes the net-value case.

### Step 6b MUST NOT

- Modify `src-tauri/src/migration/mod.rs` PRODUCT code (the
  function body of `migrate_chain_segment`, the helper, or the
  error variant). Inline tests under `#[cfg(test)] mod tests`
  in the same file ARE in Step 6b's scope.
- Modify `src-tauri/src/main.rs` (production migration
  call sites).
- Modify `src-tauri/src/executor/cli.rs` PRODUCT code (the
  `ResumePayload` struct, `compose_resume_args` signature,
  etc.). Test-only updates within the `#[cfg(test)] mod tests`
  block ARE in Step 6b's scope.
- Modify `Cargo.toml`, `README.md`, the locator script, or the
  release workflow.
- Modify any file under `src-tauri/tests/routing_fanout_rca/`,
  `src-tauri/tests/release_yml_contract.rs`, or
  `src-tauri/tests/session_lock_cross_platform.rs`.

## 6. Code boundary (Step 6c)

Step 6c reads, in order:

1. `tmp/scratch/wu-14-01/phase6/step6b-output-index.md`
2. The Step 6b test files referenced in the index (each one)
3. This contract
4. `proposals/14-session-migration-cwd.md`
5. `research/14-hookpoints.md`
6. `research/14-problem-map.md`
7. `research/14-session-migration-rca.md`

Step 6c then:

- Adds `claude_project_dir_for` per § 4.
- Adds `MigrationError::SpawnCwdUnsupported` per § 4.
- Changes `migrate_chain_segment` signature per § 4.
- Replaces the source-derived target-dir block at
  `src-tauri/src/migration/mod.rs:155-161` with the helper
  call.
- Updates `src-tauri/src/main.rs` `run_repl` and `run_resume`
  migration call sites with the effective-cwd derivation +
  new argument.
- Removes `target_jsonl_path: None` from those call sites'
  `ResumePayload` construction.
- Deletes the `target_jsonl_path` field from `ResumePayload`
  in `src-tauri/src/executor/cli.rs`.
- Deletes the `_target_jsonl_path` parameter from
  `compose_resume_args`.
- Updates `README.md` per § 7 (one paragraph; AC-7).
- Re-runs the gate suite below until green:

  - `cd src-tauri && cargo fmt --check`
  - `cd src-tauri && cargo clippy -- -D warnings`
  - `cd src-tauri && cargo test --no-fail-fast`
  - (regression-only frontend, run once at the end)
    `bun run check && bunx tsc --noEmit && bun run test`

If any gate fails, Step 6c is re-dispatched with the failure
output. The test agent is NOT re-dispatched on a code-side
failure.

If a test in Step 6b is wrong (e.g., asserts an incorrect
expected path), the contract is wrong; revise § 4 / § 5,
regenerate the test, and re-dispatch Step 6c.

### README update text guidance (AC-7)

Step 6c writes one short paragraph in
`README.md` near the existing `session_storage` /
`providers.toml` documentation (§ Resuming a session, §
Migration, or § Load balancing — pick the closest match).
Suggested wording (Step 6c may rephrase to fit the existing
voice; this is a guidance, not a literal):

> Migration during session-bound resume re-anchors the migrated
> Claude transcript under the **child process working
> directory** project hash, so `--resume <session-id>`
> consistently finds the JSONL after a cross-account migration.
> The migration writes only one location (the cwd-derived
> path); it does not duplicate under the source provider's
> project directory.

## 7. Observable signals (success criteria for joins)

- `tests/session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir`
  passes.
- Inline `migration::tests::migration_reuses_source_session_id_when_source_and_spawn_cwd_match`
  passes.
- Inline `migration::tests::migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`
  passes.
- Inline `migration::tests::claude_project_dir_for_*` (3 tests)
  pass.
- All updated tests in
  `src-tauri/tests/initiative_05_migration.rs` pass.
- All updated executor argv-composition tests in
  `src-tauri/src/executor/cli.rs` pass.
- `cargo fmt --check`, `cargo clippy -- -D warnings`,
  `cargo test --no-fail-fast` green from `src-tauri/`.
- `bun run check && bunx tsc --noEmit && bun run test` green
  (regression).
- README contains the AC-7 paragraph.

## 8. Risk annotations

- **R1** — RC-1 reproduction harness flip. The Phase 0 test
  must turn from RED to GREEN. Captured in § 5 above and
  Phase 9 evidence record.
- **R2** — Cross-CLI migration regression. Codex deferred
  tests must remain green. Captured in § 5.
- **R3** — Atomic-write contract. The tmp + rename sequence
  must be preserved; no test asserts the tmp filename
  directly, but `cargo test --no-fail-fast` covers the
  aggregate behavior.
- **R4** — Windows/symlink residuals. Out of scope; deferred
  to `WU-14-02-windows-claude-path-hash`. The new
  `MigrationError::SpawnCwdUnsupported` rejects malformed cwd
  input rather than silently producing wrong hashes.
- **R5** — `working_dir = None` coverage. No existing test
  drives `migrate_chain_segment` through `main.rs` with
  `working_dir = None`. Step 6b SHOULD add either an inline
  test that exercises `claude_project_dir_for` against
  `std::env::current_dir()` output, OR document the residual
  in `risk/14-test-residuals.md`. The latter is acceptable
  given the helper is purely deterministic and the
  effective-cwd derivation in main.rs is straightforward.

## 9. Notes for both writers

- The reproduction harness (`rc1_*`) is the load-bearing
  contract surface. If a test expectation needs to change,
  update this contract first, then regenerate.
- The `MigratedSegment.target_jsonl_path` field stays. It
  remains the authoritative record of where migration wrote.
- The `[migrate]` stderr line stays unchanged.
- The `MigrationError` enum's variant order does not affect
  consumers (no exhaustive match in `balancer/mod.rs` per
  Phase 5 § 6 delta); insert `SpawnCwdUnsupported` adjacent
  to `SourcePathMalformed` for readability.
- `pub(crate)` visibility on the helper means integration
  tests under `src-tauri/tests/` cannot import it. This is
  intentional per Phase 5 § 1 reuse-points discussion. Tests
  that need to encode a path use the existing test-only
  `claude_project_dir_name` helper at
  `src-tauri/tests/session_migration_rca/mod.rs:129`.
