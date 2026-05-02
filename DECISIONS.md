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

---

## Process

When a CodeRabbit pass / risk gate / synthesis review raises a finding that
the team chooses **not** to address in the current PR, log it here with the
five-field shape above (Source / Decision / Rationale / Revisit when). This
keeps deferrals from accumulating as ambiguous "we'll do it later" notes —
either the team commits to the work in a future Initiative, or the decision
is made explicit and dated.
