# Project Decisions

Out-of-scope choices recorded explicitly so they are not "deferrals" — these
are decisions that were considered, evaluated, and **declined** for the
indicated version. Each entry names the originating finding, the chosen
posture, the rationale, and the conditions under which the decision could be
revisited.

## D-001 — `SessionLock` lease renewal: out of scope for v1

- **Source**: Initiative 06 (`agents session pause-handshake` + import-replace
  consumer), CodeRabbit Phase 7 max-pass loop on PR #18 (`R6-F03`, `R7-F04`,
  `R8-F04`). CodeRabbit raised lease-renewal three passes in a row.
- **Decision**: v1 leases are fixed-TTL one-shots. There is no `lease.renew()`
  API and no on-the-fly TTL extension. The caller acquires with a TTL it
  expects to fit the operation; if the operation runs long, the caller
  releases and reacquires (which a competing acquirer can win).
- **Rationale**:
  - The single in-tree consumer of `SessionLock` is `agents session
    import-replace`, whose 17-step atomic flow (Initiative 06) finishes well
    inside the default 5-minute TTL. Long-running consumers do not exist
    today.
  - Renewal introduces ABA / token-rotation hazards (caller holds a stale
    lease while believing it is still valid) that the fixed-TTL model
    avoids by construction.
  - The `agents session pause-handshake` CLI lets external scripts wrap a
    long-running operation by passing a longer `--ttl-ms` up front, which
    covers the use cases Renew would address without API surface.
- **Revisit when**: a real consumer with a single critical section longer than
  the maximum acceptable TTL appears. At that point the design includes
  rotating the on-disk lease's `token_hash` to invalidate stale handles
  before the new lease takes ownership.

## D-002 — Multimodal canonical-record schema expansion: out of scope for v1

- **Source**: Initiative 06 import-replace (`R4-F01` carryover; CodeRabbit
  Phase 7 `R8-F05`). Initiative 07 canonical-reader `RC-2` discussion.
- **Decision**: the v1 canonical record carries text-only `user` and
  `assistant` turns. Tool-use, image, and other structured content are
  preserved in the source provider transcript and parsed as
  `ContentChunk { type: <kind>, text: None }` by the canonical reader, but
  the `CanonicalToProviderRenderer` rejects them with
  `exit 15 invalid-input-transcript` (a chunk with `text: None` cannot be
  losslessly emitted into Claude or Codex provider-native bytes today).
- **Rationale**:
  - The harness's documented v1 path is text-only; multimodal session round-
    trips are not on cohort-A's roadmap for the current quarter.
  - Extending the canonical schema to losslessly carry tool-use / image
    payloads requires deciding the on-wire shape for binary content,
    versioning the canonical-record schema (the JSONL format becomes a
    stable contract), and extending both readers and renderers in lockstep.
    The downstream blast radius is large; a v2 canonical schema is the
    appropriate vehicle.
- **Revisit when**: cohort A or another consumer needs round-trip preservation
  of tool-use blocks or image content. Treat the v2 canonical schema as a
  separate Initiative; mark v1 records explicitly as schema version `1` so
  the migration path is clean.

## D-003 — Race-barrier refactor in import-replace concurrency tests: not pursued

- **Source**: CodeRabbit Phase 7 max-pass loop on PR #18 (`R6-F04`, `R7-F05`).
- **Decision**: the existing concurrency test
  (`t9_concurrent_import_replace_allows_exactly_one_winner` in
  `src-tauri/tests/initiative_06_import_replace.rs`) keeps its current
  test-hook + subprocess-spawn shape rather than introducing a separate
  race-barrier helper.
- **Rationale**: the test asserts the one-winner contract and the loser
  cleanup contract end-to-end via two real subprocesses. A barrier helper
  would let the two threads synchronize on a shared signal before contending
  for the lock, which is more deterministic but tests less of the real flow
  (it would skip the OS-level filesystem race the lock primitive is meant to
  arbitrate). The current test passed at 489/489 across PRs #18, #19, and
  #21 without flake.
- **Revisit when**: the concurrency test flakes on CI. The refactor has a
  drop-in design (sentinel-flock based shared `Barrier` in `tests/fixtures/`)
  but is not warranted absent observed instability.

## D-004 — Strict empty-stderr success assertions in CLI tests: not pursued

- **Source**: CodeRabbit Phase 7 max-pass loop on PR #18 (`R7-F01`).
- **Decision**: integration tests assert exit code and stdout JSON shape on
  success. They do **not** assert that stderr is byte-empty unless the test
  is exercising a stderr-error path. A separate `assert_success_allowing_test_hook_stderr`
  helper exists for the test-hook paths that intentionally print
  `import-replace-test-hook:<phase>` lines to stderr.
- **Rationale**:
  - The CLI's stderr contract is "structured JSON error on failure;
    diagnostic noise is allowed on success." Tightening to byte-empty stderr
    would require auditing every code path that uses `eprintln!` for
    progress / diagnostic output.
  - The test-hook paths (env-only, opt-in) emit a marker line that the
    integration tests rely on for SIGKILL targeting. Compile-gating the
    hook would require a build-time feature flag and break the
    `tests/initiative_06_import_replace.rs` integration target's ability to
    exercise the path against the released binary.
- **Revisit when**: a real-world consumer surfaces stderr noise on success as
  a contract issue. At that point, audit all `eprintln!` paths and adopt a
  structured-stderr-only-on-error rule.

## D-005 — Auto-cleaning legacy `provider-*-session-*.lock` debris: out of scope for v1

- **Source**: Initiative 09 (`AIR-SUPPORTED-SURFACE-F03` migration record).
- **Decision**: the v1 lift from `session_replace::internal::SessionLock` to
  the public `session_lock::SessionLock` does **not** auto-clean the legacy
  `provider-*-session-*.lock` files that prior runs may have left under
  `<state-data-dir>/locks/`. Operators who want to scrub the dir can `rm`
  them manually.
- **Rationale**:
  - Cohort A is single-machine and a small per-session number of lock
    files is bounded debris (one per session ever import-replaced
    pre-PR-#21).
  - Auto-cleanup at startup would require a dedicated discovery pass over
    `<state-data-dir>/locks/` with explicit scope (only files matching the
    legacy pattern, only when they are stale, only when no live lease for
    the same session is held). The risk of mis-scoping outweighs the
    benefit of clearing harmless leftovers.
- **Revisit when**: an operator reports that the lock dir is materially
  cluttered. The implementation is a single startup-pass routine analogous
  to `recover_pending_replaces`; it is not technically blocked, just not
  prioritized.

## D-006 — Windows is a supported release target

- **Source**: WU-13-01 restored the Release workflow's Windows matrix row
  and replaced the POSIX-only `session_lock` primitive that had blocked
  Windows builds after Initiative 06.
- **Decision**: Windows is a supported release target for the `agents`
  binary alongside Linux and macOS. `session_lock` uses the cross-platform
  `fs4` sentinel-file locking abstraction, which maps to Unix `flock(2)`
  and Windows `LockFileEx`, while preserving the existing lease and release
  API.
- **Rationale**:
  - Unix keeps owner-only lock metadata permissions: `0o700` lock
    directories and `0o600` sentinel/temp metadata files.
  - Windows relies on default current-user profile/app-data ACL inheritance
    for lock metadata privacy in this single-user developer deployment.
    Explicit DACL hardening is intentionally outside WU-13-01.
  - `session_replace` publication continues to use same-root or sibling
    `std::fs::rename` paths. No hard-link publication is part of the mapped
    implementation.
  - Release assets use platform-suffixed bare binary names, while `.deb`,
    `.dmg`, `.msi`, and NSIS bundles keep conventional package names.
- **Revisit when**: Windows users require stronger multi-user metadata
  isolation than inherited app-data ACLs provide, or when release-run
  evidence shows a platform-specific packaging or filesystem behavior that
  needs a dedicated Windows hardening work unit.

---

## D-007 — Reproduction harness skipped for the Windows port and bare-binary collision regressions

- **Source**: Same release-restore work unit. The ticket explicitly
  authorized skipping the implementation pipeline's optional
  reproduction-harness step for these two regressions.
- **Decision**: No reproduction harness is produced for either regression.
- **Rationale**: Both root causes are documented inline in existing
  evidence and a harness would not clarify them:
  - The Windows removal is the unauthorized matrix change visible in
    `git show 9df5603 -- .github/workflows/release.yml`. That commit's
    own message records the POSIX-only `nix::fcntl` constraint that
    motivated it.
  - The bare-binary collision is visible in the pre-fix
    `.github/workflows/release.yml` upload pipeline: two build jobs
    uploaded an artifact named `oulipoly-agent-runner` and the
    release-publish step flattens them into a single `artifacts/`
    directory before invoking `softprops/action-gh-release@v2`, so
    the second-uploaded file overwrites the first by name.
  The new portable `SessionLock` integration test and the new
  structural `release.yml` parsing test cover both regressions
  directly, replacing the role a reproduction harness would have
  played.
- **Revisit when**: A future Windows or release regression has a root
  cause that is not directly observable from the workflow source or
  commit history. In that case author a reproduction harness before
  the fix.

---

## D-008 — Problem-map human approval gate pre-skipped for the release-restore work

- **Source**: Same release-restore work unit. The ticket pre-approved
  skipping the implementation pipeline's per-work-unit problem-map
  human checkpoint so the pipeline could advance from problem analysis
  to design without a manual approval round.
- **Decision**: The pipeline did not surface a manual problem-map
  approval prompt. `~/projects/agent-runner/planning/trunk/research/13-release-restore-problem-map.md` was
  carried into the design step on the strength of its own contents and
  the ticket's pre-approval.
- **Rationale**: Both regressions have well-understood scope (the
  `session_lock` POSIX surface and the `release.yml` upload step). The
  problem map's enumeration of touched files and assumptions did not
  surface a previously-unevaluated value, scope, or trade-off question
  for the user. A manual gate here would have been ceremonial.
- **Revisit when**: A future Windows-tier or release-pipeline work
  unit has a problem map that surfaces a previously-unevaluated value,
  scope, or trade-off question. In that case the pipeline must emit a
  problem-map question to the root and block on the answer rather than
  relying on this work unit's pre-approval.

---

## D-009 — Problem-map human approval gate pre-skipped for the session-migration-cwd work

- **Source**: Session migration cwd work unit (post-migration
  `claude --resume` failure RCA / fix). The root pre-approved
  skipping the implementation pipeline's per-work-unit
  problem-map human checkpoint, in parity with D-008.
- **Decision**: The pipeline did not surface a manual problem-map
  approval prompt. `~/projects/agent-runner/planning/trunk/research/14-problem-map.md` was carried into
  the design step on the strength of its own contents and the
  root's pre-approval; the orchestrator recorded the gate-skip in
  the run's audit-history.
- **Rationale**: The migration target-path mismatch has a single
  named root cause (RC-1) reproduced by an automated harness in
  `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs`.
  The problem map enumerated only the migration target-path
  computation, the executor's dead `target_jsonl_path` parameter,
  and the dead inline test that masked the bug — none of which
  surface a previously-unevaluated value, scope, or trade-off
  question. A manual gate here would have been ceremonial.
- **Revisit when**: A future migration work unit has a problem
  map that surfaces a previously-unevaluated value, scope, or
  trade-off question. In that case the pipeline must emit a
  problem-map question to the root and block on the answer
  rather than relying on this work unit's pre-approval.

---

## D-010 — Windows Claude project-directory hashing deferred from session-migration-cwd

- **Source**: Same session-migration-cwd work unit, Phase 4
  supported-surface gate and Phase 5 hookpoint research. The
  in-repo evidence for Claude Code's Windows cwd-hashing rule
  is absent: there is only a Unix-shaped decoder
  (`crates/oulipoly-runtime/src/session_metadata/mod.rs::decode_claude_project_dir_candidates`)
  and three test-only encoders that replace forward slashes with
  dashes. WU-13-01 restored Windows release builds but did not
  define Claude path hashing.
- **Decision**: The new helper
  `crates/oulipoly-runtime/src/migration/mod.rs::claude_project_dir_for`
  accepts an absolute Unix-style cwd and rejects any other shape
  (non-absolute, empty) via `MigrationError::SpawnCwdUnsupported`.
  Windows-style paths fall through to the same rejection in this
  work unit instead of guessing a hash.
- **Rationale**: Guessing a Windows hash would risk a silent
  wrong write that the resume child would still fail to find.
  Failing fast at the migration boundary preserves the runner's
  ability to surface the gap and gives a future work unit a clear
  reproduction target. Recorded as a residual in
  `~/projects/agent-runner/planning/trunk/risk/14-test-residuals.md`.
- **Revisit when**: A future work unit produces an authoritative
  Windows Claude Code path-hash contract or an in-repo Windows
  encoder. Reproduction harness path:
  `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`.
  The follow-up WU is named `WU-14-02-windows-claude-path-hash`.
- **Resolved by**: WU-14-02 / PR #42 — 2026-05-04.

---

## D-011 — Symlink/canonicalization behavior deferred from session-migration-cwd

- **Source**: Session-migration-cwd Phase 5 hookpoint research
  + Phase 4 assumption A3. The runner currently forwards
  `working_dir` directly to `cmd.current_dir(...)` without
  canonicalizing symlinks; Claude Code's own behavior with a
  symlinked cwd is unknown from in-repo evidence.
- **Decision**: The new effective-cwd derivation in
  `src-tauri/src/main.rs` for both `run_repl` and `run_resume`
  absolutizes relative paths but does not canonicalize symlinks.
  The migration helper does not canonicalize either.
- **Rationale**: Canonicalizing symlinks would change observable
  behavior compared to the existing executor handoff pattern,
  potentially producing a different cwd hash than Claude Code
  uses at spawn time. The conservative choice is to keep cwd
  string-equal between migration and executor and treat symlink
  semantics as a separate change. Recorded as a residual in
  `~/projects/agent-runner/planning/trunk/risk/14-test-residuals.md`.
- **Revisit when**: A real-Claude harness shows symlinked
  workspaces produce a different resume hash than the literal
  cwd, or a customer reports that symlinked workspaces fail to
  resume after migration.
- **Resolved by**: WU-14-02 / PR #42 — 2026-05-04.

---

## D-012 — WU-15-01 design intent override

- **Source**: WU-15-01 Phase 6 contract and Phase 0 RCA for
  empty-bodies-ref.
- **Decision**: Bodies-in-DB is the authoritative contract for
  session turn body storage. Proposals 01-trace-inspection,
  06-export, and 06-import-replace are superseded for
  body-storage purposes only. The canonical-record wire shape from
  `~/projects/agent-runner/planning/trunk/proposals/06-export.md` remains authoritative for
  `agents session export` output.
- **Rationale**: The work unit's explicit design intent is that
  `state.db` stores turn bodies directly, while those earlier
  proposals described provider JSONL as the body source of truth.
  This decision narrows the override to storage so export and
  import-replace keep their public canonical JSONL contract.
- **Revisit when**: A future work unit intentionally changes the
  canonical export record family or reopens the body-source policy.

---

## D-013 — WU-15-01 Phase 0 done

- **Source**: WU-15-01 Phase 0 RCA.
- **Decision**: The empty-bodies-ref RCA was performed pre-merge on
  `rca/empty-bodies-ref` at commit `242cb87`; reproduction
  harnesses shipped as RED on pre-fix HEAD `e9649a1`.
- **Rationale**: Recording the RCA and RED harness provenance makes
  the schema, ingest, export, and trace failures auditable after the
  fix lands.
- **Revisit when**: The Phase 0 provenance is found to point at the
  wrong branch or commit.

---

## D-014 — WU-15-01 Phase 2.5 human-gate skip

- **Source**: WU-15-01 process record and the standing
  pre-approval policy from WU-11-01 / WU-13-01 / WU-14-01.
- **Decision**: Phase 2.5 human gate was skipped under the standing
  pre-approval policy.
- **Rationale**: The problem map did not surface a new value,
  scope, or trade-off question beyond the already-approved
  bodies-in-DB contract.
- **Revisit when**: A future body-storage work unit surfaces a new
  product policy question or expands beyond the approved storage,
  export, import-replace, and trace surfaces.

---

## D-015 — WU-16-01 reproduction-harness skip

- **Source**: WU-16-01 ticket §"Source"; the cause is a
  well-understood release-process gap — `.github/workflows/release.yml`
  uploaded `artifacts/*` only, so binary-install users never received
  the body-aware adapter scripts shipped in #40. The `.deb`
  `data.tar.gz` audit confirmed no scripts in the package.
- **Decision**: Phase 0 (RCA reproduction harness) was skipped for
  WU-16-01.
- **Rationale**: The ticket evidence (`.deb` content audit, the
  WU-15-01 install-QA finding, and v0.1.26 binary expecting `body`)
  was fully diagnostic. The structural release-yml-contract test
  extension in `src-tauri/tests/release_yml_contract.rs` is the
  canonical regression guard — it RED-runs against pre-fix HEAD
  and GREEN-runs after the workflow change. A separate reproduction
  harness would not have added signal beyond the structural test.
- **Revisit when**: A future release-flow work unit produces a
  symptom whose cause is not visible from the workflow file or
  the contract test alone.

## D-016 — WU-16-01 Phase 2.5 human-gate skip

- **Source**: WU-16-01 process record and the standing pre-approval
  policy from WU-11-01 / WU-13-01 / WU-14-01 / WU-15-01.
- **Decision**: Phase 2.5 human gate was skipped under the standing
  pre-approval policy.
- **Rationale**: The problem map did not surface a new value,
  scope, or trade-off question beyond the ticket's stated install-QA
  fix. The touched surface (release.yml publish step, contract test,
  README install snippet, optional scripts/README.md cross-reference)
  matched the ticket Code Boundary exactly.
- **Revisit when**: A future release-asset / install-process work
  unit surfaces a new product policy question (e.g., versioned
  scripts, runtime version-skew detection, or bundling scripts into
  `.deb`/`.dmg`/`.msi`).

---

## D-017 — WU-14-02 Phase 2.5 human-gate skip

- **Source**: WU-14-02 process record and the orchestrator's
  standing pre-approval policy for problem-map human-gate skips.
- **Decision**: The Phase 2.5 problem-map human gate was skipped
  under the standing pre-approval policy.
- **Rationale**: The problem map did not surface a new value,
  scope, or trade-off question beyond the approved Claude
  project-dir encoder contract.
- **Revisit when**: A future migration work unit surfaces a new
  product policy question or expands beyond the approved migration
  encoder surface.

---

## D-018 — WU-14-02 Anti-scope amendment: encoder-mirror updates in five test loci

- **Source**: Two NEEDS_INPUT round-trips during WU-14-02 surfaced a
  ticket-language contradiction (Anti-scope vs AC-4) and then a
  follow-up misclassification of additional encoder mirrors:
  - `tmp/scratch/wu-14-02/questions/phase-3-r3-ticket-scope-contradiction.{md,answer.md}`
  - `tmp/scratch/wu-14-02/questions/phase-6c-third-encoder-mirror-conflict.{md,answer.md}`
- **Decision**: All encoder mirrors that depend on the slash-only
  rule are brought into agreement with the new full-rule production
  encoder. Five loci are updated; nothing else in the named test
  files is touched. The five loci are:
  1. `src-tauri/tests/session_migration_rca/mod.rs:129-130` — the
     `claude_project_dir_name` Rust helper (function body rewrite).
  2. `src-tauri/tests/session_migration_rca/mod.rs:109-115` — the
     fake-Claude Bash heredoc's `project="${PWD//\//-}"` lookup
     snippet (rewrite to apply the full rule via `sed`).
  3. `src-tauri/tests/initiative_05_migration.rs:636-638` — the
     `claude_project_dir_name` Rust helper (function body rewrite;
     same shape as locus 1, separate file).
  4. `src-tauri/tests/initiative_05_migration.rs` call sites at
     lines 680 and 846 — implicitly fixed by locus 3 (the helper
     update; the call sites themselves are untouched).
  5. `src-tauri/tests/pr_f_resume_integration.rs:951` — the inline
     `replace('/', "-")` expression (rewrite as a small character
     filter producing the same output as the production encoder).
- **Rationale**: Encoder mirrors that diverge from the production
  encoder produce false-negative test failures (the test fixture
  computes a different expected path than the production code
  writes). Each affected test still verifies the same observable
  invariant — migration writes under the resume workspace's encoded
  project directory, not the source workspace's. The test bodies,
  assertions, and contract semantics are preserved; only the
  encoder mirrors that previously aliased the old slash-only rule
  are updated. The WU-14-01 RC-1 cwd-mismatch contract remains
  intact.
- **Revisit when**: A future work unit needs to change any other
  fixture behavior in the named files, or a sixth encoder-mirror
  site is discovered. The orchestrator-recommended discovery method
  for the latter is `rg "replace\('/', \"-\"\)"` over
  `src-tauri/tests/` and `crates/oulipoly-runtime/src/` after a future
  production encoder change.
- **Process-improvement watch signal**: Phase 5 hookpoint research
  for this WU misclassified two of the three additional mirror
  sites (`tests/initiative_05_migration.rs` and
  `tests/pr_f_resume_integration.rs`) as "adjacent watchpoints"
  rather than "required conflicts" because static analysis cannot
  infer the `tempfile::tempdir()` `.` interaction. Future WUs that
  change encoder shape should explicitly enumerate slash-only
  encoder usages across the entire test suite, not just the
  worktree-immediately-touched files.

---

## Process

When a CodeRabbit pass / risk gate / synthesis review raises a finding that
the team chooses **not** to address in the current PR, log it here with the
five-field shape above (Source / Decision / Rationale / Revisit when). This
keeps deferrals from accumulating as ambiguous "we'll do it later" notes —
either the team commits to the work in a future Initiative, or the decision
is made explicit and dated.

## NES-251 — Phase 2.5.1 characterization-test waiver (2026-05-06)

**WU:** NES-251 — agents-binary `--resume <session_id>` mints fresh session_id per turn.
**Phase:** 2.5.1 (coverage inventory).
**Decision:** Skip the characterization-test dispatch. The "uncovered behaviors" enumerated by `nes-251-coverage-inventory.md` (headless / interactive resume where the provider turn script reports a different in-window session id; trace continuity across resumed turns; `find_session_for_invocation_window` ranking; `emit_known_session_id` overwrite path; chain row behavior under preserved invocation row id) are precisely the surfaces NES-251 redefines. Characterization tests of *current* behavior would pin the bug for one phase before Phase 6b deletes/inverts them.
**Justification:** Coverage inventory found no test that explicitly pins session-id-per-turn semantics on resumed turns; the only adjacent pin is `update_session_capture_safe_to_call_multiple_times` which asserts the lower-level last-write-wins primitive (and Phase 3 will decide if that primitive's caller surface or the primitive itself shifts). The bug-discovery rule (`risk-profile.md`) is self-referential here — the tracker ticket the rule would create *is* NES-251.
**Evidence:** `planning/nes-251-resume-session-id/research/nes-251-coverage-inventory.md` § "Tests that already pin the buggy behavior" / § "Uncovered behaviors".

## NES-251 — Phase 6c gate exceptions (2026-05-06)

**WU:** NES-251.
**Phase:** 6c (final gates).

**Decision 1 — `cargo test` baseline failure (orthogonal):** `src-tauri/tests/workflow_yml_contract.rs::assertion_a08_binary_clients_have_release_path` fails. The assertion requires `release.yml` to contain a `build-oulipoly-agent-cli` job because `crates/oulipoly-agent-cli` is registered as a binary client. The agent-cli crate was added in commit 9a51b2f without updating `release.yml`. The user has already staged deletion of this test file in trunk (`D src-tauri/tests/workflow_yml_contract.rs` per the orchestrator's initial gitStatus). The failure is pre-existing on this branch's base (`main` @ 9a51b2f) and is orthogonal to NES-251's session_id-preservation fix. Per ticket anti-scope ("Single agents-binary fix on the resume command's session_id handling"), NES-251 does not own release.yml or this test's lifecycle.
**Justification:** 294 of 295 cargo tests pass; the 1 failure is on the workflow-contract test and is bit-for-bit reproducible against `main` HEAD. CodeRabbit / Phase 8 multi-concern review may flag this for separate handling; NES-251 leaves it as-is.
**Evidence:** test failure stanza at `src-tauri/tests/workflow_yml_contract.rs:882` (the panic message names `oulipoly-agent-cli` as the binary lacking a release job, which is a release.yml configuration concern).

**Decision 2 — bun gates unavailable in this environment:** `bun install` fails to resolve `@fortawesome/sharp-solid-svg-icons` and `@fortawesome/sharp-regular-svg-icons` (FontAwesome Pro packages requiring an authenticated npm registry token not present in this dev environment). Without `node_modules`, `bun run check` (biome) and `bun run test` (vitest) cannot execute. The NES-251 fix is Rust-only — no `.ts` / `.tsx` / `.js` / `.jsx` / `.css` files were modified (verified via `git diff --name-only`). Cannot run JS-side gates here; on CI where the FontAwesome Pro token is configured, JS gates run normally and should pass trivially since no JS code changed.
**Justification:** Per orchestrator policy ("If you can't test the UI, say so explicitly rather than claiming success"), I am explicitly stating the JS gates are environmentally unavailable, NOT failing.
