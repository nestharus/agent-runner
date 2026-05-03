Verdict: KEEP_AS_ONE

# WU-14-01 multi-concern review

## What the PR contains

The branch `impl/wu-14-01` is four commits ahead of `main`
(`754ebb8`):

1. `796fe4e` — Phase 0 RCA + reproduction harness
   (`research/14-session-migration-rca.md`,
   `src-tauri/tests/session_migration_rca.rs` shim, and
   `src-tauri/tests/session_migration_rca/{mod,rc1_cwd_project_dir_mismatch}.rs`).
   Lands RED.
2. `4b72162` — Planning artifacts (Phase 2.5–6a):
   `research/14-{problem-map,hookpoints}.md`,
   `proposals/14-session-migration-cwd.md`,
   `risk/14-{audit,scope,shortcut,supported-surface,test-residuals,process-tree-audit-phase4}.md`,
   `product-strategy/contracts/wu-14-01-session-migration-cwd.md`.
3. `bf52308` — The fix:
   - `src-tauri/src/migration/mod.rs` — adds
     `pub(crate) fn claude_project_dir_for(provider, cwd) -> Result<String, MigrationError>`,
     adds `MigrationError::SpawnCwdUnsupported`, threads
     `resume_working_dir: &Path` through `migrate_chain_segment`,
     and replaces the source-derived target dir with a
     cwd-derived one. Inline test
     `migration_reuses_source_session_id_on_target_side` is
     split into the same-cwd and different-cwd cases plus three
     helper unit tests.
   - `src-tauri/src/main.rs` — new helper `effective_spawn_cwd`
     and updates both `run_repl` (`:1606-1614`) and `run_resume`
     (`:1830-1838`) call sites; removes
     `target_jsonl_path: None` from both `ResumePayload`
     constructions.
   - `src-tauri/src/executor/cli.rs` — deletes the
     `ResumePayload.target_jsonl_path` field and the
     `_target_jsonl_path` parameter on `compose_resume_args`,
     deletes the two
     `compose_resume_args_ignores_target_jsonl_*` tests that
     asserted the dead behavior, and updates four executor
     argv-shape tests to drop the field.
   - `src-tauri/tests/initiative_05_migration.rs` — propagates
     the new `&resume_working_dir` argument across 14 call
     sites and updates the two exact-target-path assertions
     (`:734`, `:894`) to derive from the supplied spawn cwd.
   - `src-tauri/tests/pr_f_resume_integration.rs` — sets a
     deterministic spawn cwd in `base_repl_command` and updates
     the post-migration assertion in
     `repl_resume_migrates_to_least_loaded_provider` to derive
     the expected target from the fixture cwd encoding.
   - `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs` —
     flips RED→GREEN with the new `resume_working_dir` argument
     and target-path expectation.
   - `README.md` — adds the AC-7 paragraph describing the
     re-anchor (one sentence inside the existing resume docs).
4. `8a89207` — `risk/14-process-tree-audit-phase6.md`
   (Phase 6 process-tree audit, PASS).

(The user's prompt named three commits; the fourth is
`796fe4e`, the Phase 0 RCA reproduction. It is part of this
diff and reviewed below as part of the same concern.)

## Single-concern argument

Every change in the diff is required for one user-facing fix:
**after a same-CLI cross-account Claude migration, the target
child must be able to resume the session from its working
directory.** The product-code surface is exactly what the
ticket Code Boundary names — `migration/mod.rs`,
`main.rs`, `executor/cli.rs` — and the boundary explicitly
authorizes the dead-parameter cleanup
("Either consume it or remove it; do not leave it as dead
code"). Every test change is signature propagation or the
RCA flip. The README paragraph is the AC-7 deliverable.

The planning artifacts (`research/14-*`, `proposals/14-*`,
`risk/14-*`, `product-strategy/contracts/wu-14-01-*`) are all
prefixed with the WU id and only describe this WU; none of
them touch other workflow numbers or unrelated WUs. They are
workflow trail for this fix.

The diff respects every anti-scope item the proposal names:
balancer policy unchanged (`src-tauri/src/balancer/` not in
the diff), state DB schema unchanged (`src-tauri/src/state/db.rs`
not in the diff), session-graph and chain semantics unchanged,
locator semantics unchanged, frontend (`src/`) unchanged,
Codex migration still rejected via `CodexMigrationDeferred`,
no symlink canonicalization, no Windows hashing.

## Considered splits and why each is rejected

- **Planning artifacts → separate doc PR.** The artifacts only
  make sense alongside the fix they describe; landing them
  alone leaves a stub with no implementation, and landing the
  fix without them severs the workflow trail
  (proposal Round 2 LOW verdict, problem map, contract,
  Phase 6 audit) future reviewers depend on. This matches the
  bundling convention used for WU-11-01 (`risk/11-pr-multi-concern.md`)
  and prior WUs.

- **Phase 0 RCA harness → separate "reproduction" PR.** The
  harness lands RED at `796fe4e`. Splitting it off would put
  CI in a knowingly-RED state until the fix lands, violating
  AC-6 (`cargo test --no-fail-fast` green). It is also
  trivially small (one test file plus shared fixture mod) and
  is the AC-1 signal source — separating it from the GREEN
  flip would split a regression test from the code that
  justifies it.

- **`executor/cli.rs` `target_jsonl_path` cleanup → separate
  refactor PR.** The dead field is the abandoned former
  approach to this same bug — it pretended to carry the
  migrated path through to the child but the executor ignored
  it. Removing it is what makes Option 1 a complete fix
  rather than a half-step. The ticket's Code Boundary
  authorizes the deletion, and `~/ai/conventions/no-backwards-compatibility.md`
  forbids leaving a shim. A separate refactor PR would either
  land before the fix (breaking the executor signature with
  no caller change to motivate it) or after (leaving a dead
  field across the WU's GREEN window).

- **Test-call-site signature propagation → separate
  mechanical PR.** The `migrate_chain_segment` signature now
  takes `resume_working_dir: &Path`; every caller has to
  change in lockstep with the production signature change.
  Rust compilation enforces the lockstep — splitting would
  produce a non-compiling intermediate state.

## Findings

- 21 files changed, +2933/−98 lines. The bulk is documentation
  (planning artifacts in `research/`, `risk/`,
  `product-strategy/contracts/`, `proposals/`); product code is
  three files (`migration/mod.rs`, `main.rs`, `executor/cli.rs`)
  totaling ~260 net lines, with the rest being tests and the
  one-paragraph README addition.

- Product-code surface matches the ticket Code Boundary 1:1
  (proposal §1, contract §2). No file outside the boundary is
  touched.

- Anti-scope is observed: `src-tauri/src/balancer/`,
  `src-tauri/src/state/db.rs`, `src-tauri/src/sessions/mod.rs`,
  `src-tauri/src/session_metadata/mod.rs`,
  `scripts/claude-code-locate-transcript`, and `src/` are all
  absent from the diff (`git diff main..HEAD --name-only`).

- The dead-parameter cleanup in `executor/cli.rs:276-296` and
  the deleted `compose_resume_args_ignores_target_jsonl_*`
  tests are the same conceptual concern as the migration fix
  per ticket Code Boundary and proposal §1.

- The README change is exactly one paragraph at `README.md:654`
  inside the existing resume documentation; it does not touch
  unrelated sections.

- No bundled bug fixes for unrelated issues, no opportunistic
  refactors, no test-infrastructure changes outside the WU's
  test surface.

## Conclusion

KEEP_AS_ONE. Every product-code change, every test change,
the README paragraph, the planning artifacts, and the Phase 6
audit document address the single user-facing concern named
in the ticket. The dead-parameter cleanup in `executor/cli.rs`
is part of the same concern by ticket authorization, not a
piggy-backed refactor. Each candidate split would either break
the AC-6 green-suite invariant, produce a non-compiling
intermediate state, separate a fixture from the code that
justifies it, or fragment workflow trail. Advance as one PR.
