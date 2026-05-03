# WU-14-01 — Session Migration CWD Proposal

Phase: 3 proposal
Work unit: `session-migration-cwd`
Intent: make Claude Code session migration write the target JSONL under the
same cwd-derived project directory that the child process will use for
`--resume`.

## 1. Design + Scope + Architecture + Tradeoffs

Chosen option: **Option 1, migration takes child cwd as input**.

`src-tauri/src/migration/mod.rs` changes:

- Change `migrate_chain_segment` to accept the child spawn cwd as an explicit
  argument, e.g. `resume_working_dir: &Path`, before it computes the target
  Claude project directory.
- Add one production cwd-to-Claude-project-dir helper in
  `src-tauri/src/migration/mod.rs`:
  `pub(crate) fn claude_project_dir_for(provider: &str, cwd: &Path) -> Result<String, MigrationError>`.
  The helper accepts only non-empty absolute Unix cwd paths, meaning
  `cwd.as_os_str()` is not empty, `cwd.is_absolute()` is true, and the lossy
  path string starts with `/`. A non-absolute path, an empty path, or an
  absolute non-Unix path returns
  `MigrationError::SpawnCwdUnsupported { provider: provider.to_string(), cwd: cwd.to_string_lossy().into_owned() }`.
  For an absolute Unix path it maps every `/` to `-` and returns the resulting
  string. For `/home/nes/x`, that yields `-home-nes-x`; for `/`, it yields
  `-`. This is equivalent to prefixing `-` to the path without its leading
  slash after replacing separators; it must not double-prefix an already
  leading-slash replacement. Inline tests
  `claude_project_dir_for_maps_absolute_unix_cwd` and
  `claude_project_dir_for_rejects_relative_or_empty_cwd` validate this helper
  contract.
- Replace the current source-derived target directory computation
  (`source_path.parent().file_name()`) with the cwd-derived project directory
  for `target_dir = projects_dir.join(cwd_project_dir)`.
- Preserve existing source transcript discovery, byte copy, compaction slicing,
  tmp-write-plus-rename atomicity, conflict detection, chain segment close/open,
  and `[migrate]` emission.
- Keep `MigratedSegment.target_jsonl_path` as the path migration actually
  wrote. Its contract remains "target provider storage path of the migrated
  JSONL"; it no longer means a source-derived project directory.
- Add one new migration error variant for unsupported or malformed spawn cwd
  paths:
  `MigrationError::SpawnCwdUnsupported { provider: String, cwd: String }`.
  The expected supported input is an absolute Unix path. Windows path hashing
  is out of scope for this WU because the problem map found no production
  encoder and no evidence for Claude Code's Windows hashing rules.

`src-tauri/src/main.rs` changes:

- At both migration call sites, compute the effective spawn cwd before calling
  `migrate_chain_segment`:
  `working_dir` when supplied, otherwise `std::env::current_dir()`.
- If `working_dir` is relative, absolutize it relative to
  `std::env::current_dir()` before passing it into migration. Do not
  canonicalize symlinks in this WU because `build_command` forwards the caller
  cwd via `cmd.current_dir(dir)` and the problem map says Claude's symlink
  canonicalization behavior is unknown.
- Pass that same effective cwd into `migrate_chain_segment` in both
  `run_repl` and `run_resume`.
- The migration body is the canonical producer of
  `MigrationError::SpawnCwdUnsupported`: `main.rs` computes and passes the
  effective cwd, but if a relative, empty, or non-Unix cwd reaches
  `migrate_chain_segment`, the helper returns the new variant. The existing
  `eprintln!("migration failed: {err:?}")` handling at the two main call sites
  is acceptable for this WU because `Debug` output includes both `provider` and
  `cwd`; no additional user-facing formatting is required in this proposal.
- Continue mutating only provider/session identity after migration. The child
  process still receives normal resume args; it does not need a migrated file
  path because the file is now placed where Claude Code's cwd-scoped lookup
  expects it.

`src-tauri/src/executor/cli.rs` changes:

- Remove `ResumePayload.target_jsonl_path`.
- Remove the dead `_target_jsonl_path` parameter from `compose_resume_args`.
- Update `compose_resume_provider_args`, `execute_resume`,
  `execute_interactive`, and all tests/callers to use only the resume strategy
  and session id.
- Delete or replace tests whose only assertion is that compose helpers ignore
  `target_jsonl_path`; after Option 1 that parameter no longer exists. This
  follows `~/ai/conventions/no-backwards-compatibility.md`.

`src-tauri/tests/session_migration_rca/*` changes:

- Keep the Phase 0 RCA harness
  `rc1_migrated_transcript_must_be_honorable_from_resume_working_dir` and make
  it turn GREEN post-fix. Its expected target path should become the
  resume-cwd-derived target, not the source-cwd-derived target.
- Remove `target_jsonl_path` from `ResumePayload` construction in the harness.
  The harness should prove the executor does not need a file path once migration
  writes the cwd-derived location.
- Keep the existing test-only encoder as fixture support, but validate the new
  production helper separately so tests are not the only source of encoding
  behavior.

`src-tauri/src/migration/mod.rs` inline test correction:

- Replace `migration_reuses_source_session_id_on_target_side` with two focused
  tests rather than only changing its fixture:
  `migration_reuses_source_session_id_when_source_and_spawn_cwd_match` and
  `migration_writes_target_under_spawn_cwd_when_source_and_spawn_cwd_differ`.
- This split keeps the original invariant that the target side reuses the same
  session id, while adding the missing spawn-cwd mismatch contract flagged by
  the problem map.

`src-tauri/tests/initiative_05_migration.rs` changes:

- Update all `migrate_chain_segment` call sites to supply an explicit fixture
  spawn cwd.
- Tests that only assert the migrated file starts under target projects can
  remain broad.
- Tests that assert exact target paths for stale overwrite/rejoin behavior must
  assert the path under the supplied spawn cwd. Their existing overwrite and
  chain semantics remain unchanged.

`scripts/claude-code-locate-transcript`:

- No planned code change. Phase 6 should verify it still finds the migrated
  file by exact `<session_id>.jsonl` filename under the provider projects tree.

`README.md`:

- Add one short paragraph near resume/session storage documentation explaining
  that Claude Code migration during resume re-anchors the target JSONL under
  the child process cwd-derived project directory. No config shape changes.

Tradeoffs:

- Option 1 is preferred because both supported migration call sites already
  know the spawn cwd before launching the child, and it keeps cwd-hash logic in
  one place: migration. It also deletes the executor's dead path parameter.
- Option 2 is rejected because it would split cwd-hash/write logic between
  migration and executor and would make the executor perform file placement
  immediately before `exec`, increasing blast radius in process-launch code.
- Option 3 is rejected because writing both source-derived and cwd-derived
  paths duplicates transcript files and preserves a legacy path shape solely as
  a defensive compatibility path, contrary to the no-compatibility convention.
- A hybrid is rejected. Migration owns the placement; executor owns argv and
  process cwd only.

## 2. Anti-Scope

Ticket anti-scope, restated:

- Do not introduce body-storage-in-DB as a shortcut. That is a separate WU
  tied to `research/12-empty-bodies-ref-rca.md`.
- Do not change migration policy or `src-tauri/src/balancer/mod.rs`; the user
  confirmed migration during session-bound resume is correct, and only
  content-transfer mechanics are wrong.
- Do not introduce backwards-compatibility shims for the old source-derived
  target path computation.
- Do not extend `MigrationError` into a new cross-CLI migration system.
  Codex-side migration remains rejected with `CodexMigrationDeferred`.
- Do not change `MigratedSegment.target_jsonl_path` so that it points outside
  the target provider storage root.
- Do not change `src-tauri/src/state/db.rs` or session graph schema.
- Do not change frontend files under `src/`.

Additional anti-scope from the problem map:

- Do not change `resume_execution_target` or `resume_migration_pool` semantics
  beyond passing the effective spawn cwd into migration.
- Do not change `locate_transcript` discovery semantics or the fallback
  `find_claude_source_from_storage` duplicate-session behavior.
- Do not add a canonical-record cross-CLI migration path.
- Do not canonicalize symlinks or claim symlink-cwd equivalence; the problem
  map says Claude Code's own symlink hashing behavior is unknown.
- Do not implement Windows Claude Code path hashing in this WU. Future WU:
  `WU-14-02-windows-claude-path-hash`, with reproduction harness
  `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`.
- Do not touch `src-tauri/tests/routing_fanout_rca/`,
  `src-tauri/tests/release_yml_contract.rs`, or
  `src-tauri/tests/session_lock_cross_platform.rs`.
- Do not add e2e/Playwright coverage for this backend-only migration fix.

## 3. Supported-Surface Track

Deployment mode:

- Local desktop/control-plane app and CLI runner built with Tauri/Rust.
- Release builds currently include Linux, macOS, and Windows after WU-13-01,
  but the specific Claude Code cwd-hash migration behavior proposed here is
  scoped to Unix-style absolute cwd paths because all in-repo evidence and the
  production decoder are Unix-shaped.

Customer cohort:

- Local developer users running multiple Claude Code provider accounts through
  Oulipoly, especially session-bound `--resume` paths that migrate across
  accounts because quota or manual migration policy selects a sibling provider.

Adjacent public or user-reachable paths:

- `agents ... --resume <session-id>` with prompt/file/stdin enters
  `run_resume`.
- `agents ... --resume <session-id>` with no prompt enters interactive
  `run_repl`.
- Users observe `[resume] -> <provider>`, `[migrate] <source> -> <target>`,
  `OULIPOLY_INVOCATION=...`, and the child CLI's own acceptance or rejection
  output.
- README provider/session-storage documentation remains user-reachable and
  must describe the re-anchor behavior.

Blast-radius notes for unchanged adjacent paths:

- Migration policy remains unchanged; this proposal only changes where the
  target Claude JSONL is written after policy decides to migrate.
- Session chain DB semantics remain unchanged: source segment closes, target
  segment opens, and the chain pointer remains provider/session based.
- Resume argv remains unchanged: provider resume flag/subcommand plus session
  id. The executor no longer accepts or ignores an unused file path.
- Locator behavior remains broader than Claude Code's cwd-scoped resume lookup,
  but migration now writes a path that satisfies Claude's narrower lookup.
- Existing same-cwd migration remains supported because the spawn-cwd hash and
  source transcript parent hash match in that case.

Migration path:

- Code migration only; no SQLite schema migration.
- Update all `migrate_chain_segment` call sites and test call sites in one
  change. Delete the executor path parameter rather than preserving a bridge.
- Existing already-migrated JSONLs are not rewritten in bulk. Future resume
  attempts that trigger migration will write the corrected cwd-derived target
  path. The RCA defect was in migration-at-resume content transfer, not a
  stored schema shape.

Rollback path:

- Rollback is a normal git revert of the WU.
- No DB rollback is required.
- JSONLs written under cwd-derived target directories are harmless local
  transcript files. Reverting returns the old source-derived placement bug for
  future migrations; it does not require cleanup to keep the app starting.

Observability:

- Existing `[migrate]` stderr confirms that migration ran.
- `MigratedSegment.target_jsonl_path` in tests and internal results confirms
  the exact file path written.
- The RCA harness observes the child process exit code and stderr; post-fix
  the fake Claude process finds the target file from the resume working dir.
- README documents the runtime behavior.
- No new metric or IPC shape is proposed.

## 4. Assumption Register

A1. Both production migration call sites know the child spawn cwd before
migration runs.

- Evidence: `run_repl` and `run_resume` already receive `working_dir` and pass
  it to `execute_interactive`/`execute_resume`; `build_command` sets
  `cmd.current_dir(dir)` when supplied.
- Invalidated by: Phase 5 finding a supported migration path that calls
  `migrate_chain_segment` without a knowable spawn cwd or launches the child
  from a different cwd than the one passed to migration.

A2. Claude Code's Unix project directory hash for an absolute cwd is the path
string with `/` replaced by `-`.

- Evidence: RCA observed `/home/nes/...` mapping to `-home-nes-...`; the RCA
  harness fake Claude uses `${PWD//\//-}`; test fixtures use the same encoding;
  production decoder only reconstructs Unix absolute paths from encoded names.
- Invalidated by: a real Claude Code Unix run that hashes canonicalized,
  normalized, escaped, or otherwise transformed cwd strings differently.

A3. Relative `working_dir` should be absolutized but not canonicalized.

- Evidence: `build_command` forwards `working_dir` directly to
  `cmd.current_dir`; the problem map says symlink/canonicalization behavior is
  unknown; no production spawn-time canonicalization exists today.
- Invalidated by: Phase 5 proving Claude Code canonicalizes symlinks before
  hashing cwd, or that Rust `current_dir` plus relative `current_dir` produces
  a different path string than Claude hashes in supported use.

A4. Removing `ResumePayload.target_jsonl_path` is safe under Option 1.

- Evidence: both production call sites pass `None`; `compose_resume_args`
  ignores the parameter; `compose_resume_provider_args` appends only strategy
  args and session id; the RCA harness proves passing `Some` does not help
  pre-fix because the executor ignores it.
- Invalidated by: Phase 5 finding an external public caller or in-tree
  supported path that needs the executor to consume a migrated path directly.

A5. Windows Claude Code cwd hashing is not answerable from this repository and
is out of scope for this WU.

- Evidence: problem map found no production encoder, only a Unix-shaped
  decoder and test encoders that replace `/`; WU-13-01 restored Windows release
  builds but did not define Claude path hashing; the ticket itself flags
  Windows backslash normalization as an open question.
- Invalidated by: Phase 5 finding an official or in-repo Windows Claude Code
  hash contract that can be implemented and tested without expanding this WU
  beyond its approved boundary.

A6. Source transcript discovery can remain broader than target resume lookup.

- Evidence: migration uses `locate_transcript` and fallback storage scanning
  to find source bytes; the failure is not source discovery but writing the
  copied bytes under a directory the child cwd lookup does not inspect.
- Invalidated by: Phase 5 finding that changed target placement requires
  narrowing source discovery or resolving duplicate source session files.

A7. Session graph semantics do not depend on the target project directory name.

- Evidence: `migrate_chain_segment` records provider/session/reason in the
  state DB and stores `target_jsonl_path` only in the returned record; chain
  resolution uses provider/session identity, not JSONL directory name.
- Invalidated by: Phase 5 finding a supported state query or resume resolver
  that derives active provider/session from the returned target path.

## 5. Test-Intent Track

| Test or group | Risk | Acceptance condition | Level | Fixture source/application point | Assumption link | Expected observable signal | Residual risk |
|---|---|---|---|---|---|---|---|
| Existing `session_migration_rca::rc1_cwd_project_dir_mismatch::rc1_migrated_transcript_must_be_honorable_from_resume_working_dir` | **RC-1**, from `research/14-session-migration-rca.md`: migrated JSONL lands under source cwd, child resumes from spawn cwd | Post-fix migration writes under the resume working dir's project directory; fake Claude exits 0 from that cwd without receiving any target path | particular-integration | Existing RCA fixture in `src-tauri/tests/session_migration_rca/mod.rs` and harness in `rc1_cwd_project_dir_mismatch.rs`; update expected path and `ResumePayload` shape | A1, A2, A4 | Test turns RED-to-GREEN; `result.exit_code == 0`; `migrated.target_jsonl_path == resume_project_target` | Does not verify real Claude binary behavior, only the cwd lookup contract reproduced by the fixture |
| New production helper tests `claude_project_dir_for_maps_absolute_unix_cwd` and `claude_project_dir_for_rejects_relative_or_empty_cwd` | Helper could encode Unix cwd differently than observed Claude Code | Absolute `/home/nes/project` maps to `-home-nes-project`; root `/` maps to `-`; relative, empty, or non-Unix paths return `MigrationError::SpawnCwdUnsupported` | unit | Inline tests near helper in `src-tauri/src/migration/mod.rs` | A2, A3, A5 | Exact string assertions and error assertions pass | Does not answer Windows or symlink canonicalization |
| Replacement inline tests: same-cwd and different-cwd migration target placement | Existing inline test used a same-hash fixture and missed source/spawn mismatch | Same-cwd case preserves session id and writes existing shape; different-cwd case reuses session id but writes under spawn-cwd hash, not source parent hash | particular-integration | Replace `migration_reuses_source_session_id_on_target_side` in `src-tauri/src/migration/mod.rs` with two tests | A1, A2, A7 | Both target paths exist; target session id equals source session id; DB chain id maps to target provider/session | Does not exercise child process spawn; RCA harness covers that |
| Updated `initiative_05_migration::migration_copies_claude_jsonl_to_target_projects_dir` and related copy/compaction tests | Existing migration mechanics could regress while target path changes | Bytes copied exactly, compaction slicing preserved, target path is inside target projects and under supplied spawn cwd | particular-integration | Existing `src-tauri/tests/initiative_05_migration.rs` fixtures with explicit spawn cwd added to all migration calls | A1, A2, A7 | Existing byte/line assertions stay green; target path assertions updated to cwd-derived directory | Does not test interactive resume path directly |
| Updated stale overwrite / provider rejoin exact-path tests in `initiative_05_migration.rs` | Changing target path could break same-chain revisit overwrite behavior | Same-chain revisit overwrites the target file at the spawn-cwd-derived path and keeps chain ownership semantics | particular-integration | Existing tests around `migration_overwrites_target_when_same_chain_revisits_provider` and later exact target path assertions | A1, A2, A7 | Target file contains current source bytes; active provider query remains expected | Does not cover duplicate source session discovery order |
| Existing Codex deferred tests | Option 1 might accidentally broaden cross-CLI migration while changing signature | Codex source or target storage still returns `MigrationError::CodexMigrationDeferred` and emits no `[migrate]` line | particular-integration | Existing `migration_mechanic_errors_codex_deferred_on_codex_active_provider` and `migration_does_not_emit_migrate_stderr_on_codex_deferred` with explicit spawn cwd added | A7 | Existing error and stderr assertions stay green | Does not implement future Codex migration |
| Executor resume payload/argv tests after deleting path parameter | Removing dead executor field could accidentally change resume argv | Flag strategy emits `<flag> <session_id>`; subcommand strategy emits `<subcommand...> <session_id>`; no JSONL path argument exists | unit | Existing `src-tauri/src/executor/cli.rs` compose tests renamed/replaced after signature cleanup | A4 | Exact argv vectors unchanged except no target path input parameter | Does not prove child finds files; migration/RCA tests cover placement |
| Production `run_repl` and `run_resume` migration call-site compile/integration coverage | Main call sites might pass a different cwd to migration than to executor | Both call sites compile with new signature and pass the same effective cwd used for child spawn | particular-integration | Existing top-level resume tests in `src-tauri/tests/initiative_05_migration.rs` plus Rust type checking | A1, A3 | `cargo test` compiles; resume tests remain green | Existing tests may not cover `working_dir = None`; add unit/helper coverage if Phase 5 finds no existing path |
| README documentation update review/build | Users may still infer migration writes under the source transcript directory | README states migrated Claude JSONL is re-anchored under child cwd-derived project dir during resume migration | component | `README.md` section around provider session storage/resume | None | Documentation diff contains the short paragraph required by AC-7 | No automated semantic doc checker unless Phase 6 adds one |
| Full Rust verification | Signature migration or path helper could regress adjacent Rust code | `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test --no-fail-fast` pass from `src-tauri` on Linux/macOS CI | particular-integration | Whole Rust workspace/test suite | A1-A7 | Exit 0 for all commands | Does not prove Windows path hashing, real Claude behavior, or symlink behavior |

Phase 6b residual-risk artifact obligation: if any row above cannot be encoded
or mapped to an existing test group, create `risk/14-test-residuals.md` with
the unverified risk, attempted technique, remaining residual, and whether it
changes the net-value case.

## 6. Qualitative Net-Value Statement

Yes. This proposal reduces a concrete current-state risk on the supported
resume-migration surface.

`known risky or brittle behavior already present`: migration currently reports
success and writes a target JSONL under the source-derived project directory,
which the target Claude Code child may not inspect from its spawn cwd.

`current supported and user-reachable paths through the surface`:
`agents ... --resume <session-id>` through both `run_resume` and interactive
`run_repl` can trigger account-to-account Claude migration and then hand the
session id to the child CLI.

`adjacent surfaces within the blast radius`: the change is bounded to migration
target path computation, two main call sites, executor resume payload cleanup,
Rust tests, and one README note, with balancer policy, state schema, locator
semantics, and frontend files unchanged.

`adjacent public or user-reachable paths`: users still observe the same
`[resume]`, `[migrate]`, `OULIPOLY_INVOCATION=...`, and child CLI resume
surfaces; the only intended behavior change is that the migrated file is placed
where Claude Code's cwd-scoped lookup can find it.

`migration path`: this is code migration only, with no SQLite schema migration
and no bulk rewrite of already-migrated JSONLs.

`rollback path`: rollback is a normal git revert; cwd-derived JSONLs already
written are harmless local transcript files, and revert only restores the old
source-derived placement bug for future migrations.

Conclusion: the reduction clearly outweighs the added blast radius and
migration/rollback burden. The accepted residuals are explicitly named:
Windows Claude path hashing and symlink canonicalization remain unproven and
outside this WU.

## 7. Open Questions Left for Phase 5

- Confirm all in-tree `migrate_chain_segment` call sites and remove the
  executor `target_jsonl_path` field everywhere, including inline tests around
  `compose_resume_args`.
- Confirm whether `working_dir = None` has existing coverage; if not, Phase 6a
  should specify a small helper contract for effective cwd derivation.
- Confirm `scripts/claude-code-locate-transcript` still finds the migrated file
  after cwd-derived placement; expected answer is no script change.
- Windows path-hash question: WU-13-01 restored Windows release artifacts, but
  the problem map does not prove Claude Code's Windows cwd encoding. Leave this
  for `WU-14-02-windows-claude-path-hash` with future harness
  `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`.
- Symlink path-hash question: determine whether a future WU needs a real-Claude
  harness for symlinked working directories before any canonicalization change
  is proposed.
