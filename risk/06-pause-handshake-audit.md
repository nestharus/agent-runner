# 06-pause-handshake - Phase 4 Audit Risk Report (Rev 1)

**Verdict: HIGH**

Rev 1 has the major proposal sections expected by Phase 3: scope, assumptions, command shape, JSON receipts, exit codes, side effects, tests, README work, supported-surface notes, residuals, and cross-feature compliance. The audit gate does not clear because two required contract surfaces remain unresolved before implementation: idempotent release marker storage is explicitly left for Phase 5, and lock observation by current/future writer paths is deferred despite being part of the harness and Initiative 06 acceptance surface. There is also a side-effect/test-intent mismatch around `StateDb::open` write behavior.

## Findings

### F1 - HIGH - Idempotent release marker storage is still a design fork

Rev 1 requires same-token replay to succeed (`already_released: true`) and says a missing lockfile may be accepted only when a release marker proves the same token (`proposals/06-pause-handshake.md:158`, `proposals/06-pause-handshake.md:210`). But the storage contract for that proof is not selected: §6 says "Phase 5 chooses one shape" between replacing the lockfile and writing a sibling marker (`proposals/06-pause-handshake.md:294`). §12 repeats that "release idempotency needs a marker policy selected in Phase 5" (`proposals/06-pause-handshake.md:429`).

This is not hookpoint research; it is part of the public behavior contract for `resume-handshake` exit `0` vs `16`, lock cleanup, permissions, stale-state handling, rollback, and tests. Phase 4 audit is specifically responsible for contracts, migrations, test intent, fixture source, and residual artifacts (`~/ai/workflows/implementation-pipeline.md:105`). Phase 5 may map the chosen design to files, but it should not choose between two externally visible storage semantics.

Impact: Phase 6 cannot write a stable contract or first tests for idempotent replay, missing-lock wrong-token behavior, marker permissions, cleanup, or rollback without making a design decision the proposal left open.

### F2 - HIGH - Writer-path lock observation is deferred outside the feature despite being required acceptance surface

The harness behavior spec says `pause-handshake` acquires a lock that blocks new agent-runner writes/imports/migrations, waits for active agent-runner-owned writes to drain, and returns `session-busy` if an active provider process cannot be paused safely (`04-session-pause-handshake.md:16`). Its acceptance criteria explicitly require preventing concurrent `import-replace` or migration and requiring `agents resume` / `repl --resume` to check the lock before write paths (`04-session-pause-handshake.md:56`, `04-session-pause-handshake.md:61`).

Initiative 06 carries the same constraint into every proposal: `import-replace`, `migrate_chain_segment`, `run_repl`, `run_resume`, and balanced one-shot must observe pause-handshake locks once 06-pause-handshake lands (`06-session-override-contract.md:114`). The problem map identifies these as current supported/user-reachable write paths and adjacent blast radius (`research/06-pause-handshake-problem-map.md:34`, `research/06-pause-handshake-problem-map.md:92`).

Rev 1 instead makes "no sibling writer-path observation in this PR" explicit (`proposals/06-pause-handshake.md:41`), records A6 as an assumption that sibling paths observe in later PRs (`proposals/06-pause-handshake.md:53`), and marks the cross-feature constraint only "Partial by design" (`proposals/06-pause-handshake.md:442`). The test-intent track correspondingly covers only lock-vs-lock concurrency and never covers pause/import, pause/migration, pause/resume, pause/repl, or observer fail-closed behavior (`proposals/06-pause-handshake.md:352`).

Impact: the proposed feature can pass its tests while failing the harness-level purpose of being the common guard before transcript override. If Rev 2 keeps observer wiring out of scope, it needs an explicit Phase-level decision that narrows the harness acceptance surface and records the accepted residual; otherwise the audit contract is incomplete.

### F3 - MEDIUM - The side-effect contract permits unbounded existing `StateDb::open` mutations while claiming lock-state-only behavior

The harness says side effects are lock state only (`04-session-pause-handshake.md:52`). The problem map records that `StateDb::open_default` calls mutating `open`, and `StateDb::open` creates directories, ensures schemas, and runs chain backfill before returning (`research/06-pause-handshake-problem-map.md:20`, `research/06-pause-handshake-problem-map.md:21`). It also records open-path backfill as a session-state write not tied to a named user operation (`research/06-pause-handshake-problem-map.md:70`).

Rev 1 says pause may open default state/config (`proposals/06-pause-handshake.md:330`) and that neither command may modify `session_turns` or chain/segment ownership (`proposals/06-pause-handshake.md:339`), but then carves out "unavoidable existing `StateDb::open` behavior" (`proposals/06-pause-handshake.md:345`). The side-effect test also allows "unchanged except existing open effects" (`proposals/06-pause-handshake.md:372`). That exception makes the side-effect contract passable even if the command mutates session tables before touching lock state.

Impact: the proposal does not pin whether pause-handshake must consume the preceding schema-probe/read-only open surface or tolerate DB backfill side effects. This weakens the lock-state-only contract and makes the side-effect test insufficiently strict.

### F4 - MEDIUM - Test-intent track is missing required assumption links and residuals per test group

Phase 3 requires each expected test or test group to name the change or verification risk, intended behavior, selected level, fixture source/application point, assumption-register link when applicable, expected observable signal, and residual risk the test will not verify (`~/ai/workflows/implementation-pipeline.md:96`). Rev 1's test-intent table has risk, behavior, level, fixture/application point, and signal columns only (`proposals/06-pause-handshake.md:354`).

Several rows depend directly on assumptions but do not identify them: resolver pass-through depends on A1/A2, TTL and stale acquisition depend on A5, permissions and lockfile behavior depend on A7, and the side-effect row depends on the unresolved read-only/open behavior above. The table also does not state residuals per group, even though §12 names residuals such as no active provider drain, sibling observer deferral, Windows semantics, and balanced one-shot post-hoc session discovery (`proposals/06-pause-handshake.md:421`).

Impact: Phase 6b would have to infer which assumptions each test validates and which named risks remain unverified. That undermines the Step 6b output index and any later `risk/NN-test-residuals.md` decision.

## Checklist Notes

- Present: proposal artifact, approved problem-map input, assumption register, supported-surface track, net-value statement, command schemas, exit namespace, JSON receipts, side-effect section, README work, residual section.
- Not audit-clear: unresolved release-marker design; deferred writer-path observation; side-effect exception around mutable DB open; test-intent table missing assumption links/residual-risk mapping.

## Required Rev 2 Closure

Rev 2 should close F1-F4 in the proposal itself, not by relying on Phase 5 or implementation discretion. Because the verdict is HIGH, Phase 4 must be rerun across all four risk reports after substantive revision.
