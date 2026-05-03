# WU-14-01 — Phase 5 Hookpoint Research

Status: no `NEEDS_INPUT`. The approved problem map and assumption register still hold.

## Reuse points

- `src-tauri/src/migration/mod.rs` is already the right owner for Claude target path construction. `migrate_chain_segment` currently:
  - locates the source transcript at `src-tauri/src/migration/mod.rs:120-125`;
  - validates source absolute/existence at `:129-139`;
  - extracts the current source-derived project dir name at `:155-161`;
  - builds `target_dir = projects_dir.join(cwd_hash)` at `:188`;
  - builds `<target_session_id>.jsonl` at `:195`;
  - preserves tmp-write-plus-rename at `:206-211`;
  - returns the actual written path in `MigratedSegment.target_jsonl_path` at `:251`.
- Recommendation: put `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path) -> Result<String, MigrationError>` in `src-tauri/src/migration/mod.rs`, near `migrate_chain_segment`, not in a sibling module. The helper is used by one production writer and shares `MigrationError`; splitting it out would add a module boundary without reducing duplication. `mod.rs` already owns source discovery plus target placement.
- Keep using the existing source discovery helpers:
  - `locate_transcript(...)` from `src-tauri/src/sessions/mod.rs`, already called by migration;
  - `find_claude_source_from_storage(...)` at `src-tauri/src/migration/mod.rs:256-269`.
  The proposal only changes target placement, not source discovery.
- Keep using existing session-chain DB APIs:
  - `latest_compaction_boundary(...)` at `src-tauri/src/migration/mod.rs:167-170`;
  - `find_conflicting_active_segment(...)` at `:196-199`;
  - `close_active_segment_returning(...)` at `:216-222`;
  - `open_chain_segment(...)` at `:223-231`.
- Keep using `build_command`'s current cwd handoff in `src-tauri/src/executor/cli.rs:332-350` as the reference for what the child process will receive. The new migration cwd should match this effective spawn cwd; do not move path hashing into executor.
- Existing test-only encoders:
  - `src-tauri/tests/session_migration_rca/mod.rs:129-130`;
  - `src-tauri/tests/fixtures/initiative_06.rs:886-888`;
  - `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995-997`;
  - `src-tauri/tests/fixtures/initiative_06_export.rs:605-607`.
  These should not be replaced by importing the production helper. The production helper is proposed as `pub(crate)`, so integration tests under `src-tauri/tests` cannot import it without widening API visibility. More importantly, the RCA fake-Claude helper is useful as an independent fixture for external CLI behavior; importing production encoding there would make the harness more tautological. The right reuse is: inline tests in `migration/mod.rs` validate `claude_project_dir_for`; integration fixtures may keep their local encoders for fixture construction.
- Existing executor resume argv composition remains reusable after deleting the dead path field:
  - `append_resume_args(...)` at `src-tauri/src/executor/cli.rs:300+`;
  - `compose_resume_provider_args(...)` at `:292-298`;
  - `execute_resume(...)` at `:453+`;
  - `execute_interactive(...)` at `:566+`.
  These already only need strategy + session id.

## Extension points

- New `migrate_chain_segment` parameter shape:
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
  The new parameter should be the child spawn cwd, not an encoded project-dir string.
- Inside `migrate_chain_segment`, replace the source-derived block at `src-tauri/src/migration/mod.rs:155-161` with `claude_project_dir_for(&target.name, resume_working_dir)?`, then keep `target_dir = projects_dir.join(cwd_project_dir)` at the existing target-dir hookpoint around `:188`.
- `run_repl` hookpoint: `src-tauri/src/main.rs:1606-1614`. Before this call, compute the effective spawn cwd from the same `working_dir: Option<&Path>` that later reaches `execute_interactive` at `src-tauri/src/main.rs:1705-1708`. Pass `&effective_spawn_cwd` into `migrate_chain_segment`.
- `run_resume` hookpoint: `src-tauri/src/main.rs:1830-1838`. Before this call, compute the effective spawn cwd from the same `working_dir: Option<&Path>` that later reaches `execute_resume` at `src-tauri/src/main.rs:1890-1895`. Pass `&effective_spawn_cwd` into `migrate_chain_segment`.
- Effective cwd behavior required by the approved proposal:
  - `Some(absolute)` => pass it as-is;
  - `Some(relative)` => absolutize relative to `std::env::current_dir()`;
  - `None` => use `std::env::current_dir()`;
  - do not canonicalize symlinks.
- After migration success, keep the current identity-only mutation:
  - `run_repl`: `resolved.active_provider` and `resolved.active_session_id` at `src-tauri/src/main.rs:1615-1617`;
  - `run_resume`: same fields at `src-tauri/src/main.rs:1839-1841`.
  Do not pass a migrated file path to executor.
- Add `MigrationError::SpawnCwdUnsupported { provider: String, cwd: String }` to the existing enum in `src-tauri/src/migration/mod.rs:8-65`. Existing production handling uses `Debug` output at both main call sites, so no formatter hook is required.

## Conflicting systems

- Inline migration test conflict: `migration_reuses_source_session_id_on_target_side` at `src-tauri/src/migration/mod.rs:309-378` asserts the old source-derived target path:
  - fixture source dir uses literal `cwd_hash`;
  - call site is `:355`;
  - exact old-path assertion is `:368-372`.
  Replace it with the proposal's two focused tests: same source/spawn cwd preserves the session id, and differing source/spawn cwd writes under spawn cwd.
- `src-tauri/tests/initiative_05_migration.rs` call sites that need the new `resume_working_dir` parameter:
  - `:644` in `migration_copies_claude_jsonl_to_target_projects_dir`;
  - `:723` in `migration_overwrites_target_when_same_chain_revisits_provider`;
  - `:802` in `migration_refuses_when_other_chain_owns_target_session`;
  - `:883` in `migration_overwrites_when_other_chain_segment_is_closed`;
  - `:910` in `migration_appends_chain_segment_with_correct_reason`;
  - `:958` in `migration_errors_on_source_jsonl_missing`;
  - `:988` in `migration_errors_on_source_path_malformed`;
  - `:1012` in `migration_truncates_target_jsonl_at_latest_compaction_boundary`;
  - `:1042` in `migration_copies_full_jsonl_when_no_compaction_boundary`;
  - `:1068` in `migration_picks_latest_of_multiple_compaction_boundaries`;
  - `:1094` in `migration_errors_when_compaction_boundary_not_in_jsonl`;
  - `:1120` in `pre_compaction_turns_remain_queryable_after_migration`;
  - `:1155` in `migration_mechanic_errors_codex_deferred_on_codex_active_provider`;
  - `:1185` in `migration_does_not_emit_migrate_stderr_on_codex_deferred`.
- Other concrete `migrate_chain_segment` callers found by `rg 'migrate_chain_segment\(' src-tauri/`:
  - production: `src-tauri/src/main.rs:1606`, `src-tauri/src/main.rs:1830`;
  - inline test: `src-tauri/src/migration/mod.rs:355`;
  - RCA harness: `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:20`;
  - integration migration tests listed above.
  No in-tree caller outside `main.rs`, `migration/mod.rs` tests, `session_migration_rca`, or `initiative_05_migration.rs` was found.
- Locator script verification: `scripts/claude-code-locate-transcript` still finds migrated files at the new path. It searches below the provided base with `base.rglob("*.jsonl")` and filters by exact filename `<session_id>.jsonl` before content-scan fallback. Since Option 1 still writes `<target_projects>/<cwd-derived-dir>/<session_id>.jsonl`, filename-based search remains valid. No script change is needed.
- `MigratedSegment.target_jsonl_path` consumers that assert exact pre-fix paths:
  - `src-tauri/src/migration/mod.rs:368-372` asserts target path under the source fixture `cwd_hash`;
  - `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:39` asserts `source_project_target` and `:40-43` asserts the resume-cwd path is absent;
  - `src-tauri/tests/initiative_05_migration.rs:734` asserts `stale_target`, built under `source_projects/cwd_hash` for rejoin-to-claude;
  - `src-tauri/tests/initiative_05_migration.rs:894` asserts `target_path`, also built under `source_projects/cwd_hash`.
- `MigratedSegment.target_jsonl_path` consumers that do not assert exact old path and can be mechanically updated around the new parameter:
  - broad starts-with/existence/content assertions at `src-tauri/tests/initiative_05_migration.rs:655-659`;
  - content reads at `:736`, `:896`, `:1022`, `:1054`, and `:1078`.
- Executor dead-path conflict: `ResumePayload.target_jsonl_path` exists at `src-tauri/src/executor/cli.rs:279`, but production main already passes `None` at `src-tauri/src/main.rs:1701` and `:1900`. The RCA harness passes `Some` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:57`, proving the field does not affect child lookup. Keeping this field would conflict with Option 1.

## Deletion candidates

- Delete `ResumePayload.target_jsonl_path` from `src-tauri/src/executor/cli.rs:279`.
- Delete `_target_jsonl_path: Option<&Path>` from `compose_resume_args` at `src-tauri/src/executor/cli.rs:282-286`.
- Remove `target_jsonl_path: None` initializers from executor tests at:
  - `src-tauri/src/executor/cli.rs:1096`;
  - `:1143`;
  - `:1190`;
  - `:1239`.
- Remove `target_jsonl_path: None` initializers from production main at:
  - `src-tauri/src/main.rs:1701`;
  - `src-tauri/src/main.rs:1900`.
- Remove `target_jsonl_path: Some(&migrated.target_jsonl_path)` from the RCA harness at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:57`.
- Delete, rather than rewrite, tests whose only assertion is that the target path parameter is ignored:
  - `compose_resume_args_ignores_target_jsonl_for_flag_strategy` at `src-tauri/src/executor/cli.rs:1786-1803`;
  - `compose_resume_args_ignores_target_jsonl_for_subcommand_strategy` at `src-tauri/src/executor/cli.rs:1807-1823`.
  The existing execute-resume/interactive argv tests remain useful; only the dead-parameter-specific tests should go.

## Open questions answered

- Helper location decision: use `src-tauri/src/migration/mod.rs`. It already builds migration target paths and owns `MigrationError`; a sibling module would be extra structure for one local helper.
- `working_dir = None` coverage:
  - Existing executor unit tests pass `None` into `execute_resume` at `src-tauri/src/executor/cli.rs:1138` and `:1185`, and into `execute_interactive` at `:1091` and `:1234`.
  - Existing top-level resume tests also omit `--project`, so `run_resume` receives `working_dir = None`, but the identified tests do not exercise migration target placement under that default cwd.
  - No existing test found exercises `migrate_chain_segment` through the `main.rs` migration path with `working_dir = None`.
  - Phase 6a should specify a small default-cwd case: migration should use `std::env::current_dir()` when `working_dir` is absent.
- Locator script verification: no script change is needed. Its exact filename search remains compatible with the new cwd-derived parent directory.
- Windows path-hash deferral: the future reproduction-harness path `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs` is non-conflicting. `test -e` returned absent for that path.

## Touched-surface delta vs problem map

- The Phase 2.5 problem map is materially correct. No return to Phase 2.5 is needed.
- Delta found: `src-tauri/src/balancer/mod.rs` imports/constructs `MigrationError::Db` in `decide_migration` at `:430`, but it does not match exhaustively on `MigrationError`; adding `SpawnCwdUnsupported` in migration does not require balancer changes.
- Delta found: the current `initiative_05_migration.rs` fixture helper returns `source_projects`/`target_projects` but no explicit spawn workspace. Phase 6 should add a fixture spawn cwd or pass a deterministic tempdir path at each migration call site. This is mechanical signature propagation, not a new design.
- The problem map did not overstate locator risk. Verification confirms the locator is broader than Claude Code lookup and remains compatible because filename search ignores the parent project-dir hash.
- The problem map's exact-path assertion list remains accurate; the only exact pre-fix `target_jsonl_path` assertions are the inline migration test, RCA harness, and the two stale/rejoin checks in `initiative_05_migration.rs`.
