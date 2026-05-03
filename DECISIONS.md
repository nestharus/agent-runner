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

## D-006 — Windows is not a supported target

- **Source**: Initiative 06 PR #17 (`session_lock`) and PR #18
  (`session_replace`) introduced POSIX-only primitives:
  `nix::fcntl::flock`, `std::os::fd::AsRawFd`, hard-link publication
  (`fs::hard_link`), atomic rename semantics (POSIX rename atomicity is
  stronger than Windows MoveFileEx), 0o600 file modes, and Claude path-hash
  decomposition that assumes `/`-separated paths. Discovered when the manual
  Release workflow's `windows-latest` build started failing on `cargo build`
  with `error[E0432]: unresolved import nix::fcntl` after PR #17 merged.
  CI (`ci.yml`) runs only `ubuntu-latest`, so the regression went unflagged
  until the first post-Initiative-06 release attempt.
- **Decision**: Linux and macOS are the supported targets. Windows is
  removed from the Release workflow matrix. The CLI is documented as a
  Unix-only tool. No Windows shim, no `#[cfg(unix)]` gates, no NTFS-based
  alternative locking implementation.
- **Rationale**:
  - The features that depend on POSIX primitives (`agents session
    pause-handshake` / `import-replace` / `locate` / `export`) are core to
    the project's value, not optional surfaces. A Windows port would have
    to provide functionally-equivalent semantics for: advisory file locks
    that release on process exit (Windows lacks POSIX `flock` semantics
    natively — `LockFileEx` is the closest, with different exclusivity and
    inheritance rules); atomic rename across same-volume directory entries
    (NTFS-via-`MoveFileEx` is close enough); hard-link publication
    (NTFS supports it, but the same-inode invariant differs); and Claude /
    Codex transcript path conventions that the providers themselves only
    document on Linux/macOS.
  - The harness consumer (`agent-harness`) and the user's actual day-to-day
    usage are on Linux. macOS coverage already exercises the same POSIX
    code paths.
  - Maintaining a Windows port would double the QA surface for a feature
    set that is primarily about coordinating local-machine provider CLIs
    (`claude`, `codex`) which themselves are not first-class on Windows.
- **Revisit when**: a real user reports needing Windows. At that point,
  evaluate whether to (a) implement a separate `session_lock_windows`
  module with `LockFileEx` semantics and equivalent rename/publish
  primitives, or (b) provide a "feature-gated stub" that compiles on
  Windows but returns "unsupported on this platform" errors for every
  affected CLI subcommand. Either route is several days of work plus a
  Windows-shaped test environment; not warranted absent demand.

---

## D-007 — WU-11-01 Phase 2.5 problem-map human gate: skipped per user pre-approval

- **Source**: WU-11-01 (routing-fanout) implementation pipeline run on
  branch `impl/wu-11-01` (PR #36). The implementation-pipeline-orchestrator's
  Phase 2.5 ("Existing-State Risk Profile") emits a `NEEDS_INPUT`
  human gate to the root for problem-map approval before Phase 3
  proposal authoring (`~/ai/workflows/implementation-pipeline.md`
  Phase 2.5; orchestrator spec "human-gate-restricted" rule). The
  question artifact for WU-11-01 is at
  `tmp/scratch/wu-11-01/questions/q-9cfb2e90-9935-4cdd-bcff-7e993a189b46.question.json`;
  the answer artifact (Option A — approve as-is) is at the matching
  `.answer.json`. The root provided the answer with `decision_summary`:
  "Phase 2.5 problem-map approved as-is via user's pre-approval to
  skip this human gate."
- **Decision**: For WU-11-01 specifically, the orchestrator advanced
  to Phase 3 without surfacing the problem-map to the user for an
  interactive approval response. The Phase 2.5 problem map at
  `worktrees/impl-wu-11-01/research/11-routing-fanout-problem-map.md`
  (109 lines, six required sections present, file:line references
  validated against worktree HEAD) is treated as approved.
- **Rationale**:
  - The user pre-authorized skipping this specific human gate before
    the orchestrator was dispatched.
  - The Phase 2.5 artifact independently satisfies its quality bar
    (size, section coverage, verified file:line references); the
    orchestrator's Phase 2.5 verification step explicitly checks
    these criteria before emitting the human gate, and they all
    passed.
  - The downstream Phase 4 risk gates (audit, scope, shortcut,
    supported-surface) would catch any framing problem the human
    gate would have caught — and did, in fact, surface the round-1
    MEDIUM verdicts for observability (supported-surface) and AC-3/
    AC-4 test-intent track gaps (audit). Round 2 closed both with
    LOW after a `gpt-high` proposal-revision pass.
  - The orchestrator session-files-fallback mechanism handled a
    mid-resume provider auto-migration (`claude3 → claude` on
    `quota_threshold`) that broke the original `resume-by-session-id`
    path; continuation evidence is at
    `tmp/scratch/wu-11-01/session-graph/b526007b-c996-4b07-96ae-87cde636f0c0/continuations/q-9cfb2e90-9935-4cdd-bcff-7e993a189b46.fallback.json`.
    The skipped human-gate decision is preserved end-to-end across
    that fallback.
- **Revisit when**: A future WU surfaces a Phase 2.5 problem map
  that the orchestrator's verification step does not catch and that
  Phase 4 risk gates also miss. At that point, evaluate whether to
  reinstate the Phase 2.5 human gate per-WU rather than relying on
  the user's blanket pre-approval, or to add a stronger orchestrator-
  side verification check that closes the specific gap.

---

## Process

When a CodeRabbit pass / risk gate / synthesis review raises a finding that
the team chooses **not** to address in the current PR, log it here with the
five-field shape above (Source / Decision / Rationale / Revisit when). This
keeps deferrals from accumulating as ambiguous "we'll do it later" notes —
either the team commits to the work in a future Initiative, or the decision
is made explicit and dated.
