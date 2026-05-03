# WU-14-01 — Phase 8 PR Supported-Surface Verification

Reviewer: `claude-opus`. Inputs evaluated against the actual diff
`git diff main..HEAD`, not the proposal.

## 1. Termination signal

`NONE`.

Per-assumption verification against the implemented diff
(`proposals/14-session-migration-cwd.md` §4 A1–A7):

- **A1** (both call sites know spawn cwd before migration) — **upheld
  in diff**. `src-tauri/src/main.rs:1047-1055` introduces
  `effective_spawn_cwd(Option<&Path>)` and `src-tauri/src/main.rs:1617`
  (`run_repl`) and `src-tauri/src/main.rs:1842` (`run_resume`) compute
  it from the same `working_dir` value that the executor will later
  forward into `cmd.current_dir(dir)`. Same `&Path` is then passed to
  `migrate_chain_segment` at both call sites
  (`src-tauri/src/main.rs:1620`, `src-tauri/src/main.rs:1849`). No
  third migration call site exists in the diff.
- **A2** (Unix project dir = absolute path with `/` → `-`) — **upheld
  in diff**. `src-tauri/src/migration/mod.rs:256-265`
  (`claude_project_dir_for`) does `cwd.to_string_lossy().replace('/', "-")`
  after asserting `cwd.is_absolute()` and non-empty. Inline tests
  `claude_project_dir_for_encodes_absolute_unix_path`,
  `claude_project_dir_for_rejects_relative_path`, and
  `claude_project_dir_for_rejects_empty_path`
  (`src-tauri/src/migration/mod.rs:464-495`) lock the encoder
  contract. The end-to-end RCA harness fake Claude reproduces the
  same `${PWD//\//-}` mapping
  (`src-tauri/tests/session_migration_rca/mod.rs:96-106`).
- **A3** (absolutize but do not canonicalize relative `working_dir`)
  — **upheld in diff**. `effective_spawn_cwd` joins a relative
  `working_dir` onto `std::env::current_dir()` but does not
  `canonicalize()` (`src-tauri/src/main.rs:1047-1055`); no
  `canonicalize` calls were added in the diff.
- **A4** (removing `ResumePayload.target_jsonl_path` is safe under
  Option 1) — **upheld in diff**. The field is deleted from
  `ResumePayload` and `compose_resume_args`
  (`src-tauri/src/executor/cli.rs:276-282`); both production call
  sites construct `ResumePayload` without it
  (`src-tauri/src/main.rs:1714-1716`,
  `src-tauri/src/main.rs:1913-1916`); the now-redundant
  `compose_resume_args_ignores_target_jsonl_*` tests are deleted
  rather than left as zombie coverage; the RCA harness reconstructs
  `ResumePayload` with only `session_id` + `strategy`
  (`src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:55-58`).
- **A5** (Windows hashing not answerable here, deferred) — **upheld
  in diff**. Helper rejects non-absolute / empty paths with
  `MigrationError::SpawnCwdUnsupported { provider, cwd }` rather than
  silently producing a wrong hash
  (`src-tauri/src/migration/mod.rs:256-265`); no Windows encoder is
  added; the README change scopes the behavior to "the Claude project
  directory derived from the child process working directory" without
  claiming Windows support (`README.md:654`).
- **A6** (source discovery may stay broader than target lookup) —
  **upheld in diff**. `find_claude_source_from_storage` and
  `locate_transcript` use sites are unchanged in
  `src-tauri/src/migration/mod.rs`; only the target directory
  computation switched from `source_path.parent().file_name()` to
  `claude_project_dir_for(&target.name, resume_working_dir)`
  (`src-tauri/src/migration/mod.rs:158-188`).
- **A7** (session graph independent of target dir name) — **upheld
  in diff**. The diff does not touch `src-tauri/src/state/db.rs`;
  the `chain_id_for_segment("claude2", session_id)` assertion in the
  new `migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`
  test (`src-tauri/src/migration/mod.rs:455-457`) confirms chain
  resolution still uses provider/session identity, not the JSONL
  directory name.

No assumption invalidated → no `RETURN_TO_RESEARCH`.
Net value remains positive (RC-1 RED→GREEN, see §3) → no `TERMINATE`.

## 2. Verdict

`LOW`.

## 3. Findings

### Diff matches the proposed surface — nothing extra, nothing missing

- Migration body change is exactly the target-dir swap proposed
  (`src-tauri/src/migration/mod.rs:158-188`, helper at
  `:256-265`). No widening into balancer, locator, or DB.
- The two main call sites (`run_repl`, `run_resume`) are the only
  migration call sites in the diff and both received the new arg.
- Executor cleanup matches: `ResumePayload.target_jsonl_path`
  deleted; `compose_resume_args` parameter dropped; redundant
  "ignores target_jsonl" tests deleted (per
  `~/ai/conventions/no-backwards-compatibility.md`); the four
  argv-shape tests retain the executor-resume-payload risk comment.
- README adds one paragraph at `README.md:654` describing the
  cwd-anchoring behavior, exactly as proposed in §1 / AC-7.
- Test surface widens only inside the named files: inline migration
  tests gain the same-cwd / different-cwd split, the
  `initiative_05_migration` cases all pass an explicit
  `resume_working_dir`, the new `session_migration_rca` harness ships
  with a single RC-1 reproduction, and `pr_f_resume_integration`
  gains an end-to-end positive+negative assertion that the migrated
  JSONL lands under the spawn-cwd-derived dir and not under the old
  fixture dir (`src-tauri/tests/pr_f_resume_integration.rs:948-963`).
- No frontend, no schema, no balancer, no locator change —
  consistent with the Phase 4 anti-scope and the supported-surface
  blast-radius claim.

### RC-1 RCA harness flips RED → GREEN on the diff

`tmp/scratch/wu-14-01/phase6/rc1-green-run.log` shows
`session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir`
passing on this branch. The harness asserts both sides of the fix:

1. `migrated.target_jsonl_path == resume_project_target` and the
   source-cwd-shaped path is **not** written
   (`src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:38-46`).
2. `execute_resume` against the fake Claude (which inverts cwd via
   `${PWD//\//-}`) returns `exit_code == 0`
   (`src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:60-66`).

This matches the RC-1 RED reproduction recorded in
`research/14-session-migration-rca.md:101-125`.

### Supported-surface guarantees from the proposal are preserved

- **Deployment mode**: still local Tauri/Rust desktop+CLI; no
  packaging or release shape change in the diff.
- **Customer cohort**: Linux/macOS users running multi-account Claude
  Code resume with quota or manual migration. Windows is left in the
  same place WU-13-01 left it — release builds restored, but
  migration on Windows now *fails fast* with
  `SpawnCwdUnsupported { provider, cwd }` instead of silently writing
  a Unix-shaped path. Strict improvement on the Windows surface.
- **Adjacent paths**: `agents ... --resume <id>` argv unchanged —
  argv tests in `src-tauri/src/executor/cli.rs:1062-1244` still
  assert exact resume argv. `[resume]`, `[migrate]`,
  `OULIPOLY_INVOCATION=...` lines unchanged. Locator script
  unchanged.
- **Blast radius**: bounded to migration target-dir computation, two
  main call sites, executor signature cleanup, named tests, and one
  README paragraph. Balancer, DB schema, locator semantics, and
  frontend are untouched in the diff (verified via
  `git diff --stat`).
- **Migration path**: code-only; no SQLite migration; old misplaced
  JSONLs simply get re-migrated on next resume. Diff confirms no
  in-place rewrite logic was added.
- **Rollback path**: `git revert` of this branch restores the old
  source-derived placement bug for future migrations only;
  cwd-derived JSONLs already on disk are inert. No DB cleanup
  required.
- **Observability**: existing `[migrate]` stderr line and
  `MigratedSegment.target_jsonl_path` (now meaning the cwd-derived
  path) preserved; no new metrics or IPC.

### Minor non-blocking observations

- `effective_spawn_cwd` lives as a private free function in
  `src-tauri/src/main.rs:1047-1055` without dedicated unit tests.
  Proposal §7 flagged that Phase 5/6a should "specify a small helper
  contract for effective cwd derivation" if `working_dir = None`
  coverage was missing. The diff covers the `working_dir = None`
  branch indirectly through `pr_f_resume_integration` (which sets
  `cmd.current_dir(self.dir.path())` for `repl --resume` without
  passing `--project`), so the integration path exercises both
  branches, but the helper itself has no inline unit test. Not
  blocking under LOW because the helper is two trivial branches and
  the integration harness exercises the resulting target path
  end-to-end.
- `MigrationError::SpawnCwdUnsupported` is plumbed but the only
  user-facing rendering remains the existing
  `eprintln!("migration failed: {err:?}")` at the call site. Phase 4
  already accepted this; the `Debug` representation now includes both
  `provider` and `cwd`, so a fail-stop is observable. Not blocking.
- `risk/14-test-residuals.md` exists in the diff (per the Phase 6b
  obligation in proposal §5), so the "real Claude binary" residual
  for A2 is named and accepted, not silently swallowed.

None of these widen blast radius or invalidate any A1–A7 assumption.

## 4. LOW + NONE justification

The diff exactly executes the Phase 4-cleared design: target JSONL
re-anchored under the spawn-cwd-derived Claude project directory,
dead executor parameter removed, all A1–A7 assumptions empirically
upheld in the implemented code, and the RC-1 RCA harness flipped
RED→GREEN with no scope creep into balancer, schema, locator, or
frontend.
