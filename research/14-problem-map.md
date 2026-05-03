# WU-14-01 Existing-State Problem Map

## 1. Touched surface

- `src-tauri/src/migration/mod.rs:68` `MigratedSegment` is the migration return record; today it includes `target_jsonl_path` as the file path migration wrote, but downstream resume paths do not preserve it.
- `src-tauri/src/migration/mod.rs:79` `migrate_chain_segment` resolves source and target providers, rejects missing resume blocks and Codex storage, locates the source transcript, copies JSONL bytes, updates the session chain, and logs `[migrate]`.
- `src-tauri/src/migration/mod.rs:120` source transcript lookup first uses `sessions.toml` locator output and then falls back to storage scanning via `find_claude_source_from_storage`.
- `src-tauri/src/migration/mod.rs:155` derives `cwd_hash` from `source_path.parent().file_name()`, so the target project directory is source-transcript-derived today.
- `src-tauri/src/migration/mod.rs:188` writes into `projects_dir.join(cwd_hash)` under the target provider's Claude Code storage root.
- `src-tauri/src/migration/mod.rs:206` writes a `.jsonl.tmp` and renames it into place, preserving an atomic write shape for the target JSONL.
- `src-tauri/src/migration/mod.rs:216` closes the active source chain segment and opens a target segment with the same session id.
- `src-tauri/src/migration/mod.rs:256` `find_claude_source_from_storage` scans each direct child under a Claude `projects_dir` and returns the first `<session_id>.jsonl` found.
- `src-tauri/src/migration/mod.rs:309` inline test `migration_reuses_source_session_id_on_target_side` asserts the target path reuses the source `cwd_hash`; that test currently exercises a same-hash fixture rather than a spawn-cwd mismatch.
- `src-tauri/src/main.rs:1533` `run_repl` resolves `--resume` into a `ResolvedResume` before interactive launch.
- `src-tauri/src/main.rs:1599` `run_repl` builds an effective migration pool and asks `decide_migration` whether interactive resume should migrate.
- `src-tauri/src/main.rs:1606` interactive resume call site invokes `migrate_chain_segment`; on success it mutates `resolved.active_provider` and `resolved.active_session_id` only.
- `src-tauri/src/main.rs:1693` interactive executor handoff builds `ResumePayload` with `target_jsonl_path: None`.
- `src-tauri/src/main.rs:1705` interactive resume launches `execute_interactive` with the caller-supplied `working_dir`.
- `src-tauri/src/main.rs:1760` `run_resume` is the session-bound resume path used when top-level `--resume` has a prompt, file, or stdin content.
- `src-tauri/src/main.rs:1823` `run_resume` builds a migration pool and calls `decide_migration`.
- `src-tauri/src/main.rs:1830` session-bound resume call site invokes `migrate_chain_segment`; on success it keeps only migrated provider/session identity and recomputes the execution target.
- `src-tauri/src/main.rs:1890` session-bound executor handoff calls `execute_resume` with the same `working_dir` the child will use.
- `src-tauri/src/main.rs:1900` session-bound `ResumePayload` also passes `target_jsonl_path: None`.
- `src-tauri/src/executor/cli.rs:276` `ResumePayload` carries `session_id`, `strategy`, and optional `target_jsonl_path`.
- `src-tauri/src/executor/cli.rs:282` `compose_resume_args` accepts `_target_jsonl_path` but ignores it.
- `src-tauri/src/executor/cli.rs:292` `compose_resume_provider_args` ignores `ResumePayload.target_jsonl_path` and appends only the resume strategy arguments.
- `src-tauri/src/executor/cli.rs:300` `append_resume_args` emits either `<flag> <session_id>` or `<subcommand...> <session_id>`.
- `src-tauri/src/executor/cli.rs:348` `build_command` sets `cmd.current_dir(dir)` when a working directory is supplied; this is the cwd Claude Code uses for its own resume lookup.
- `src-tauri/src/executor/cli.rs:453` `execute_resume` composes provider args, disables session capture, runs the child from `working_dir`, and classifies resume acceptance.
- `src-tauri/src/executor/cli.rs:566` `execute_interactive` composes interactive resume args, builds the command from `working_dir`, and inherits terminal streams.
- `scripts/claude-code-locate-transcript:19` locator expands the supplied base dir and searches by exact `<session_id>.jsonl` filename before a content-scan fallback.
- `README.md:252` `providers.toml` documentation describes provider resume and `session_storage.projects_dir`; current text does not document migrated resume re-anchoring.

The ticket Code Boundary appears complete for product code. The test boundary necessarily includes `src-tauri/tests/session_migration_rca/*` and migration tests, but that is separate from the product Code Boundary.

## 2. Already-risky/brittle behavior

- The core brittleness is source-derived target path computation: `migrate_chain_segment` reads the target directory name from the source transcript parent at `src-tauri/src/migration/mod.rs:155`, then writes under that same name in the target store at `src-tauri/src/migration/mod.rs:188`.
- `MigratedSegment.target_jsonl_path` looks like the authoritative resume artifact path, but both supported executor handoffs drop it by setting `None` at `src-tauri/src/main.rs:1701` and `src-tauri/src/main.rs:1900`.
- The executor API contains a dead parameter: `ResumePayload.target_jsonl_path` is defined at `src-tauri/src/executor/cli.rs:279`, `_target_jsonl_path` is intentionally ignored by `compose_resume_args` at `src-tauri/src/executor/cli.rs:285`, and `compose_resume_provider_args` never reads the field at `src-tauri/src/executor/cli.rs:292`.
- `find_claude_source_from_storage` returns the first matching session filename under any direct child of `projects_dir` at `src-tauri/src/migration/mod.rs:263`; if duplicates exist, discovery order controls the source path.
- The source locator and fallback have broader discovery semantics than Claude Code resume. The locator finds any filename match under a projects tree at `scripts/claude-code-locate-transcript:34`, while Claude Code resume is observed to look under a cwd-derived project directory.
- Existing migration tests mostly assert "inside target projects and bytes copied" (`src-tauri/tests/initiative_05_migration.rs:655`) or reuse a fixture `cwd_hash` (`src-tauri/src/migration/mod.rs:314`), so they do not force source cwd and spawn cwd to differ.
- The inline migration test at `src-tauri/src/migration/mod.rs:367` asserts the current source-derived path shape; it can pass even when the child process would not honor resume from a different cwd.
- Atomic write behavior exists (`src-tauri/src/migration/mod.rs:206` and `src-tauri/src/migration/mod.rs:211`) but there is no cleanup path for a failed rename after the tmp write.

## 3. Adjacent surfaces inside the blast radius

- `src-tauri/src/balancer/mod.rs:396` `decide_migration` decides when a resume should migrate. It is out of scope by ticket policy, but both touched resume paths depend on its `MigrationDecision::Migrate` output at `src-tauri/src/balancer/mod.rs:49`.
- `src-tauri/src/main.rs:1224` `resume_execution_target` rehydrates the effective provider after migration; it shares the assumption that provider/session identity is enough for resume handoff.
- `src-tauri/src/main.rs:1478` `resume_migration_pool` builds the effective provider pool used by both migration call sites; it filters provider config through `providers_cfg.effective_provider`.
- `src-tauri/src/sessions/mod.rs:171` `locate_transcript` runs user-configured transcript locators and returns a single path; migration uses this path as source truth.
- `src-tauri/src/session_metadata/mod.rs:257` `derive_claude_workspace_root` consumes Claude project directory names by inverting the path hash for metadata, not by generating a cwd-derived target directory.
- `src-tauri/src/session_metadata/mod.rs:338` `decode_claude_project_dir_candidates` is the only production Claude path-hash helper found; it decodes an encoded project dir into possible Unix absolute paths.
- `src-tauri/tests/initiative_05_migration.rs:637` migration integration tests consume `MigratedSegment.target_jsonl_path` and assert copied bytes. Several later tests assert exact target paths for stale-overwrite behavior at `src-tauri/tests/initiative_05_migration.rs:734` and `src-tauri/tests/initiative_05_migration.rs:894`.
- `src-tauri/tests/session_migration_rca/mod.rs:129` RCA fixture helper encodes Claude project dir names with `path.to_string_lossy().replace('/', "-")`.
- `src-tauri/tests/fixtures/initiative_06.rs:886`, `src-tauri/tests/fixtures/initiative_06_import_replace.rs:995`, and `src-tauri/tests/fixtures/initiative_06_export.rs:605` contain test-only Claude project-dir encoders with the `-` prefix and slash replacement.
- `README.md:285` documents `session_storage.kind = "claude_code"` and `projects_dir`, so any changed runtime meaning around migrated resume placement needs a matching doc note per ticket AC-7.

## 4. Supported / user-reachable paths through the touched surface

- Top-level `--resume <id>` dispatches at `src-tauri/src/main.rs:431`. If the user supplies a prompt, `--file`, or stdin prompt, it calls `run_resume` at `src-tauri/src/main.rs:456`; otherwise it calls `run_repl` at `src-tauri/src/main.rs:468`.
- Interactive resume path: `run_repl` resolves the chain at `src-tauri/src/main.rs:1533`, optionally migrates at `src-tauri/src/main.rs:1606`, mutates provider/session identity at `src-tauri/src/main.rs:1615`, then launches `execute_interactive` at `src-tauri/src/main.rs:1705`. The user observes `[resume] -> <provider>` at `src-tauri/src/main.rs:1596`, `[migrate] <source> -> <target>` from `src-tauri/src/migration/mod.rs:232` when migration succeeds, and then the child CLI's own terminal output because `execute_interactive` inherits stderr/stdout at `src-tauri/src/executor/cli.rs:584`.
- Session-bound resume path: `run_resume` resolves the chain at `src-tauri/src/main.rs:1790`, optionally migrates at `src-tauri/src/main.rs:1830`, mutates provider/session identity at `src-tauri/src/main.rs:1839`, starts an invocation row at `src-tauri/src/main.rs:1876`, and invokes `execute_resume` at `src-tauri/src/main.rs:1890`. The user observes `[resume] -> <provider>` at `src-tauri/src/main.rs:1820`, `[migrate]` from migration, `OULIPOLY_INVOCATION=...` at `src-tauri/src/main.rs:1888`, and on failure the child stderr plus diagnostics at `src-tauri/src/main.rs:1959`.
- In both paths, migration happens before the child is spawned, and the child cwd is whatever `working_dir` reaches `cmd.current_dir` at `src-tauri/src/executor/cli.rs:348`. Today that cwd does not participate in migration target path computation.

## 5. Cross-platform considerations

- WU-13-01 cross-platform locking added process-lock coverage in `src-tauri/tests/session_lock_cross_platform.rs:80`, but that surface is `SessionLock` only; it does not define Claude path hashing.
- `rg` found no production helper that encodes a workspace cwd into Claude Code's project directory name. Production code only has an inversion helper: `decode_claude_project_dir_candidates` at `src-tauri/src/session_metadata/mod.rs:338`.
- The production inversion helper assumes encoded names start with `-` and reconstructs Unix-style absolute candidates beginning at `/` at `src-tauri/src/session_metadata/mod.rs:339` and `src-tauri/src/session_metadata/mod.rs:364`; it does not model Windows drive roots or backslash input.
- Test-only encoders replace forward slashes with dashes (`src-tauri/tests/fixtures/initiative_06.rs:888`) and do not normalize backslashes to slashes. The RCA harness helper also only replaces `/` at `src-tauri/tests/session_migration_rca/mod.rs:130`.
- Unknown from code alone: whether Claude Code on Windows hashes the raw backslash form, a forward-slash-normalized form, a canonicalized path, or a drive-letter-normalized variant. The ticket specifically calls out backslash to slash normalization, but the existing production code does not answer it.

## 6. Pre-fix evidence

- The RCA names RC-1 as the source-derived target path mismatch and cites the executor dropping the migrated path at `research/14-session-migration-rca.md:65`.
- The red-run log is recorded in `research/14-session-migration-rca.md:94`, with failure output beginning at `research/14-session-migration-rca.md:101` and the child error at `research/14-session-migration-rca.md:111`.
- The reproduction harness is `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`. It sets up different source and resume workspaces, calls `migrate_chain_segment` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:20`, then launches `execute_resume` from the resume workspace at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:47`.
- The pre-fix assertion that will need to flip post-fix is `assert_eq!(migrated.target_jsonl_path, source_project_target);` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:39`. The companion negative existence assertion at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:40` also encodes the pre-fix setup expectation that the resume-cwd target path is absent.
- The harness passes `target_jsonl_path: Some(&migrated.target_jsonl_path)` into `ResumePayload` at `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:54`, but the executor ignores it, so the child still searches by cwd.

## 7. Open questions / unknowns

- Whether Claude Code on Windows uses backslash or forward-slash hashing is not answerable from this repository. Existing helpers do not encode Windows paths.
- Whether the spawn cwd passed into `execute_resume` is canonical/realpath form or symlink form depends on the caller's `--project` input and OS behavior; `build_command` forwards the supplied `Path` to `cmd.current_dir` at `src-tauri/src/executor/cli.rs:348` without canonicalizing it.
- Whether Claude Code itself canonicalizes symlinks before hashing cwd is not answerable from this code. Existing metadata code canonicalizes when inverting stored paths (`src-tauri/src/session_metadata/mod.rs:307`), but that is not spawn-time behavior.
- Whether inline test `migration_reuses_source_session_id_on_target_side` currently passes because source and target cwd hashes coincide: yes, the fixture uses one literal `cwd_hash` for the source path and expected target path at `src-tauri/src/migration/mod.rs:314` and `src-tauri/src/migration/mod.rs:367`; it does not represent separate source and spawn cwd hashes.
- Whether duplicate `<session_id>.jsonl` files across multiple Claude project directories are supported or accidental is not clear. `find_claude_source_from_storage` returns the first direct-child hit at `src-tauri/src/migration/mod.rs:263`.
- Whether `target_jsonl_path` is intended to remain an executor contract or only a migration audit field is ambiguous in current code: it is exposed in `ResumePayload` at `src-tauri/src/executor/cli.rs:279`, returned by `MigratedSegment` at `src-tauri/src/migration/mod.rs:75`, but not consumed on either supported resume path.
