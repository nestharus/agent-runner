# PR-E Test Quality Audit

I reviewed `tmp/02-pr-e-contract.md`, `VALUES.md`, `git diff main..HEAD`, the new integration suite in `src-tauri/tests/pr_e_repl_integration.rs`, and the targeted unit-test additions in `src-tauri/src/main.rs`, `src-tauri/src/config/model.rs`, and `src-tauri/src/executor/cli.rs`. I also ran the new integration test binary and focused unit-test patterns; the PR-E tests exercised here passed. Overall this is a solid, mostly contract-shaped suite. The important watch-points are present: `should_emit_invocation_line` has both branches asserted (`src-tauri/src/main.rs:905`, `src-tauri/src/main.rs:910`), the panic-path `FinalizerGuard` test uses `catch_unwind` and then verifies the row ended up failed (`src-tauri/src/main.rs:943`), and the Unix `SIGTERM` integration test uses a `term.txt` trap marker plus an infinite loop, so it is not fooled by a natural child exit (`src-tauri/tests/pr_e_repl_integration.rs:243`, `src-tauri/tests/pr_e_repl_integration.rs:247`, `src-tauri/tests/pr_e_repl_integration.rs:249`).

## Letter Grades

| Dimension | Grade | Notes |
| --- | --- | --- |
| Independence | A | Tempdirs are per-test, env mutation is serialized with a lock, and no ordering assumptions are visible. |
| Determinism | A | Synchronization uses sentinel files and bounded polling; no time-of-day coupling and no `sleep`-for-correctness pattern beyond acceptable polling. |
| Specificity | B | Most unit tests are tight, but several integration tests intentionally bundle multiple contract checks, which weakens fault localization. |
| Avoidance of impl-detail coupling | A | Assertions mostly stay on CLI parse results, exit codes, stderr payloads, child env/cwd, and DB rows rather than private internals. |
| Coverage of failure modes | B | Key failures are covered, but there is no direct `repl` test for malformed model config / config-load failure. |
| Cross-platform discipline | A | Unix-only signal tests are correctly gated and shell-script fixtures that require Unix are also gated. |
| Fixture quality | A | Fixtures are minimal, quoted correctly, and temp-scoped; the CodeRabbit quoting fix is reflected in the script bodies. |

## Contract Walk

| Contract Area | Status | Evidence |
| --- | --- | --- |
| 1. CLI parsing, all 7 cases | COVERED | `src-tauri/src/main.rs:674`, `src-tauri/src/main.rs:721`, `src-tauri/src/main.rs:752`, `src-tauri/src/main.rs:774`, `src-tauri/src/main.rs:792`, `src-tauri/src/main.rs:805`, `src-tauri/src/main.rs:824`, `src-tauri/src/main.rs:835` |
| 2. ProviderConfig round-trip + validation, 5 cases | COVERED | `src-tauri/src/config/model.rs:878`, `src-tauri/src/config/model.rs:906`, `src-tauri/src/config/model.rs:923`, `src-tauri/src/config/model.rs:938`, `src-tauri/src/config/model.rs:981` |
| 3. `execute_interactive` argv + cwd + env, 4 cases | COVERED | `src-tauri/src/executor/cli.rs:724`, `src-tauri/src/executor/cli.rs:742`, `src-tauri/src/executor/cli.rs:768`, `src-tauri/src/executor/cli.rs:791` |
| 4. `FinalizerGuard` happy / panic / early-error | COVERED | `src-tauri/src/main.rs:915`, `src-tauri/src/main.rs:943`, `src-tauri/src/main.rs:971` |
| 5. Stderr emission gating | COVERED | Unit-helper branches at `src-tauri/src/main.rs:905` and `src-tauri/src/main.rs:910`; non-TTY exact-once check at `src-tauri/tests/pr_e_repl_integration.rs:139` and `src-tauri/tests/pr_e_repl_integration.rs:165` |
| 6. Parent env var read, 3 cases | COVERED | `src-tauri/src/main.rs:847`, `src-tauri/src/main.rs:856`, `src-tauri/src/main.rs:879`, `src-tauri/src/main.rs:887` |
| 7. End-to-end repl lifecycle | COVERED | `src-tauri/tests/pr_e_repl_integration.rs:165`, `src-tauri/tests/pr_e_repl_integration.rs:200` |
| 8. Signal handling, Unix only | COVERED | `src-tauri/tests/pr_e_repl_integration.rs:240`, `src-tauri/tests/pr_e_repl_integration.rs:285` |

## Below-A Notes

**Specificity: B.** The unit tests are generally disciplined, but some integration cases collapse several contract clauses into one body. `repl_happy_path_emits_single_invocation_line_and_finalizes_succeeded_row` checks exit code, exactly-one stderr emission, DB success/finalization, absent parent linkage, and child env propagation in one test (`src-tauri/tests/pr_e_repl_integration.rs:165`). `repl_resolves_parent_env_and_overwrites_child_parent_env_payload` similarly covers both parent-row resolution and child env replacement (`src-tauri/tests/pr_e_repl_integration.rs:200`). Those are legitimate end-to-end assertions, but when one fails the reader gets a broader failure surface than necessary. The suite is still readable; it is just not maximally surgical.

**Coverage of failure modes: B.** The required failure paths are mostly present: empty `interactive_args` rejection is covered at load time (`src-tauri/src/config/model.rs:923`) and at `repl` runtime (`src-tauri/tests/pr_e_repl_integration.rs:344`), malformed and unknown parent env values fall back silently (`src-tauri/src/main.rs:879`, `src-tauri/src/main.rs:887`, `src-tauri/src/main.rs:896`), and panic recovery through the drop guard is verified against persisted DB state (`src-tauri/src/main.rs:943`). The missing piece is a direct config-load failure test through the `repl` path: there is no integration or runner-level test that a malformed model TOML or structurally invalid model file is surfaced as a runner-side exit-code-1 failure from `load_models` / `ModelConfig::from_toml`. Given the contract’s failure list, that is the only meaningful hole I found.

## Verdict

**PASS.** The suite hits the PR-E contract comprehensively, the signal tests are meaningfully constructed rather than decorative, and the helper/guard watch-points are asserted correctly. Existing PR-A/B/C/D coverage does not appear weakened; the test-side diff is additive, and the regression checks for the flat CLI path and `Trace` parsing remain in place (`src-tauri/src/main.rs:674`, `src-tauri/src/main.rs:721`, `src-tauri/src/main.rs:752`). The remaining issues are quality nits, not release blockers: splitting a couple of broader integration tests and adding one malformed-config `repl` test would move this from strong to clean.
