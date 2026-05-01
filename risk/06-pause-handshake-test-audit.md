# Test-Audit Gate: 06-pause-handshake

## Verdict: PARTIAL

The 06-pause-handshake test set is strong enough to establish the main CLI
contract shape and the named multi-process subprocess surface: the landed Rust
integration target passes, tests carry risk/level/source annotations, fixtures
are mostly externalized into a dedicated fixture module, and the Phase 6
process-tree report supports intent-first separation.

The gate is **PARTIAL**, not PASS, because the load-bearing stale-concurrency
test is probabilistic and several proposal §9.1 acceptance rows are not pinned
by landed tests or an explicit residual/non-applicability record. These are
ordinary fix-pass findings. I found no supported-surface termination signal.

## Verification Run

Command run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_pause_handshake
```

Result: PASS, 12 passed, 0 failed.

## Firstness / Provenance

Verdict: PASS with evidence limitation.

The committed process-tree audit reports Step 6b and Step 6c as separate
`gpt-high` Codex invocations, with Step 6b finishing before Step 6c started,
Step 6b adding only test/fixture files, Step 6c consuming the Step 6b index and
test paths before product-code edits, and the later fixture repair changing
only subprocess stdio capture (`risk/06-pause-handshake-process-tree-audit.md`).
That is sufficient to accept firstness for this gate.

Evidence limitation: the raw `.tmp/phase6/step6b-output-index.md`,
`.tmp/phase6/step6c-reads.md`, Step 6b prompt/log, and Step 6c prompt/log are
not present in this checkout. The process-tree audit says it verified those
artifacts while they were available, so this is not a contradiction, but
synthesis should not claim the raw companion files are currently present.

## Coverage Map

The critical contract list in `research/06-pause-handshake-contract.md:160`
through `:175` is covered by landed subprocess or CLI integration tests:

| Contract / proposal risk | Landed evidence | Assessment |
| --- | --- | --- |
| Resolver pass-through and receipt fields | `pause_success_receipt_uses_resolved_active_session_and_provider`, `src-tauri/tests/initiative_06_pause_handshake.rs:8` | Covered. Also checks token hash is persisted instead of raw token. |
| Invalid UUID before state/lock access | `pause_invalid_uuid_exits_two_before_state_or_lock_access`, `src-tauri/tests/initiative_06_pause_handshake.rs:41` | Covered. |
| Resolver error mapping 10/11/12 | `pause_resolver_error_mapping_covers_not_found_ambiguous_and_twelve`, `src-tauri/tests/initiative_06_pause_handshake.rs:60` | Covered. |
| Concurrent pause, same session | `concurrent_pause_only_one_subprocess_acquires_same_session`, `src-tauri/tests/initiative_06_pause_handshake.rs:92` | Covered as a subprocess smoke test; see F1 for strength issue. |
| Per-session scope | `pause_locks_are_scoped_per_session`, `src-tauri/tests/initiative_06_pause_handshake.rs:128` | Covered. |
| Token format and token rotation | `pause_tokens_have_required_format_and_change_between_acquisitions`, `src-tauri/tests/initiative_06_pause_handshake.rs:145` | Covered at CLI level. |
| TTL default/max/expiry | `ttl_default_max_bound_and_expiry_are_respected`, `src-tauri/tests/initiative_06_pause_handshake.rs:172` | Covered using the Step 6a contract's 60s/600s policy. |
| Concurrent stale acquire | `concurrent_stale_pause_only_one_subprocess_replaces_expired_lock`, `src-tauri/tests/initiative_06_pause_handshake.rs:205` | Covered as a subprocess smoke test; see F1. |
| Busy lock | `active_pause_blocks_second_pause_until_release_or_expiry`, `src-tauri/tests/initiative_06_pause_handshake.rs:236` | Covered, including no token leakage in busy stderr. |
| Release cycle / idempotent replay / expired matching release | `pause_release_cycle_is_idempotent_and_allows_future_pause`, `src-tauri/tests/initiative_06_pause_handshake.rs:263` | Covered. |
| Wrong / malformed / marker-mismatch token | `release_wrong_or_malformed_token_exits_16_and_preserves_lock`, `src-tauri/tests/initiative_06_pause_handshake.rs:303` | Covered. The malformed release after the marker exists exercises marker mismatch. |
| No lock and no marker | `release_without_lock_or_marker_exits_lock_expired`, `src-tauri/tests/initiative_06_pause_handshake.rs:340` | Covered. |

## Findings

### F1 — MEDIUM — Stale-concurrency coverage is not deterministic enough for the Rev 4 race risk

The stale-race test prewrites one expired lock and then starts eight CLI
children in a loop (`src-tauri/tests/initiative_06_pause_handshake.rs:211`
through `:222`). It then asserts one success and the rest busy. This is useful
smoke coverage, but it does not force multiple contenders to observe the stale
state before any contender replaces it. A flawed stale-eviction implementation
with a small TOCTOU window could pass whenever the OS schedules the children
serially enough that only the first process sees the expired lease.

The contract calls this load-bearing multi-process mutual exclusion
(`research/06-pause-handshake-contract.md:164` through `:175`), and the proposal
Rev 4 specifically exists to close stale contender interleavings. Add a
deterministic stress hook or separate test-only binary mode that can coordinate
contenders around the stale-read / acquire point, or run a bounded repeated
stress test with enough iterations to make the old Rev 3 failure observable.

### F2 — MEDIUM — Side-effect contract is not pinned by tests

The proposal requires a side-effect test row: only lock state should mutate
beyond accepted `StateDb::open` effects, with DB counts/transcript mtimes
unchanged (`proposals/06-pause-handshake.md:596`). The Step 6a contract also
forbids session table writes, provider commands, quota refresh/auth, migration,
scan, telemetry, and invocation rows (`research/06-pause-handshake-contract.md:144`
through `:158`).

The landed suite checks invalid UUID avoids state setup
(`src-tauri/tests/initiative_06_pause_handshake.rs:41`) and uses a provider
command string named `provider-command-that-must-not-run`
(`src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:99` through `:113`),
but it does not snapshot session/invocation row counts, transcript mtimes, or
provider-command non-execution for a successful pause/resume cycle. Add one
integration test around successful pause and resume that records DB/session
counts and any fixture transcript/provider-command sentinels before and after.

### F3 — LOW — Permissions and README truth proposal rows are uncovered

Proposal §9.1 includes explicit rows for owner-private lock state permissions
and README truth (`proposals/06-pause-handshake.md:595` and `:598`). The fixture
module imports `PermissionsExt` only to set permissions on manually written
expired lock fixtures (`src-tauri/tests/fixtures/initiative_06_pause_handshake.rs:358`
through `:365`); no test asserts the product-created lock directory,
`sentinel.lock`, live lockfile, or release marker modes. The branch also has no
README diff for pause/resume-handshake.

Add a Unix integration assertion for `0700` lock dir and `0600` sentinel,
lock, and marker files after pause/resume. Either add the README update and a
light doc-truth check, or record a deliberate non-applicability/residual if the
documentation row has been moved out of this PR.

## Fixture Externality / Assertion Strength

Verdict: PASS with F1 caveat.

The tests route DB/config/subprocess setup through
`src-tauri/tests/fixtures/initiative_06_pause_handshake.rs`, not inline fixture
bodies. The external fixture shape is appropriate for this integration target.
The main assertion-strength issue is not fixture placement; it is the absence of
a deterministic synchronization point in the stale-concurrency test.

I found no assertion weakening, baseline regeneration, coverage deletion, or
test edit that appears to make a red test green without a product-code fix. The
post-implementation fixture-only change is documented in the process-tree audit
as stdio capture repair and the diff supports that characterization.

## Residuals / Handoff

No `risk/06-pause-handshake-test-residuals.md` exists. The uncovered
side-effect, permissions, and documentation rows therefore should not be treated
as accepted residuals yet.

Recommended next action: ordinary test fix pass for F1-F3, then rerun this
test-audit gate and the named Rust integration target.
