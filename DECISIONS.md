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
  approval prompt. `research/13-release-restore-problem-map.md` was
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
  approval prompt. `research/14-problem-map.md` was carried into
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
  (`src-tauri/src/session_metadata/mod.rs::decode_claude_project_dir_candidates`)
  and three test-only encoders that replace forward slashes with
  dashes. WU-13-01 restored Windows release builds but did not
  define Claude path hashing.
- **Decision**: The new helper
  `src-tauri/src/migration/mod.rs::claude_project_dir_for`
  accepts an absolute Unix-style cwd and rejects any other shape
  (non-absolute, empty) via `MigrationError::SpawnCwdUnsupported`.
  Windows-style paths fall through to the same rejection in this
  work unit instead of guessing a hash.
- **Rationale**: Guessing a Windows hash would risk a silent
  wrong write that the resume child would still fail to find.
  Failing fast at the migration boundary preserves the runner's
  ability to surface the gap and gives a future work unit a clear
  reproduction target. Recorded as a residual in
  `risk/14-test-residuals.md`.
- **Revisit when**: A future work unit produces an authoritative
  Windows Claude Code path-hash contract or an in-repo Windows
  encoder. Reproduction harness path:
  `src-tauri/tests/session_migration_rca/rc2_windows_cwd_project_dir_hash.rs`.
  The follow-up WU is named `WU-14-02-windows-claude-path-hash`.

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
  `risk/14-test-residuals.md`.
- **Revisit when**: A real-Claude harness shows symlinked
  workspaces produce a different resume hash than the literal
  cwd, or a customer reports that symlinked workspaces fail to
  resume after migration.

---

## Process

When a CodeRabbit pass / risk gate / synthesis review raises a finding that
the team chooses **not** to address in the current PR, log it here with the
five-field shape above (Source / Decision / Rationale / Revisit when). This
keeps deferrals from accumulating as ambiguous "we'll do it later" notes —
either the team commits to the work in a future Initiative, or the decision
is made explicit and dated.
