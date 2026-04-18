**PR-F Test Quality Audit**

Overall verdict: `PASS`

PR-F clears the test-quality gate. The suite is independent, deterministic, and contract-shaped on the important edges: CLI parsing, config validation, DB lookup/indexing, executor argv composition, provider-resolution happy/failure paths, pre-spawn provenance writes, trace rendering, and the PR-E no-resume regression. I re-read the required sources, walked `git diff main..HEAD`, and ran targeted Rust tests with `cargo test --manifest-path src-tauri/Cargo.toml resume_`, `cargo test --manifest-path src-tauri/Cargo.toml find_provider_for_session`, and `cargo test --manifest-path src-tauri/Cargo.toml resumed_session_`; those runs passed.

**Dimension Grades**

| Dimension | Grade | Notes |
| --- | --- | --- |
| Independence | A | Fresh tempdirs/DBs are used consistently; I did not find shared mutable state or ordering dependencies. |
| Determinism | A | No `sleep` synchronization, no time-of-day dependence, and the ordering test uses fixed RFC3339 timestamps via `ts(...)`. |
| Specificity | B | Most tests are focused, but a few cases bundle multiple behaviors, which weakens fault localization. |
| Avoidance of impl-detail coupling | B | Most assertions hit DB rows, stderr, exit codes, ASCII, or JSON, but a small cluster in `main.rs` pins private helper policy directly. |
| Coverage of failure modes | A | Malformed UUID, unknown session, mismatch with and without suggestions, and missing `[providers.resume]` are all present. |
| Cross-platform discipline | A | Unix-only process-fixture tests are gated correctly; the integration file starts with `#![cfg(unix)]`, and executor argv tests are `#[cfg(unix)]`. |
| Fixture quality | A | Tempdirs are scoped, shell fixture paths are quoted, and I did not see leaks or cross-test reuse. |

**Contract Walk**

| Contract Item | Status | Evidence |
| --- | --- | --- |
| 1. CLI parsing (4-5 cases) | `COVERED` | `src-tauri/src/main.rs:967-1044` covers no-resume, resume after/before model, missing model, and missing resume value. |
| 2. `ResumeStrategy` round-trip + 5 validation rejections + canonical shapes | `COVERED` | `src-tauri/src/config/model.rs:1101-1292` covers both round-trips, five rejection cases, and Claude/Codex canonical shapes. |
| 3. DB index presence + `find_provider_for_session` empty/single/ordered multi | `COVERED` | `src-tauri/src/state/db.rs:2077-2117` and `src-tauri/src/state/db.rs:2733-2821`. |
| 4. Executor argv composition (Flag / Subcommand / None regression) | `COVERED` | `src-tauri/src/executor/cli.rs:852-962`. |
| 5. Stderr emission (always-on short line, multi-match detail gate) | `COVERED` | Integration coverage in `src-tauri/tests/pr_f_resume_integration.rs:174-245`; helper gate coverage in `src-tauri/src/main.rs:1128-1157`. |
| 6. Provider lookup happy path (integration) | `COVERED` | Flag path at `src-tauri/tests/pr_f_resume_integration.rs:143-170`; subcommand path at `src-tauri/tests/pr_f_resume_integration.rs:414-460`. |
| 7. Provider lookup failure paths (4 cases) | `COVERED` | `src-tauri/tests/pr_f_resume_integration.rs:249-378` covers unknown session, mismatch with suggestion, mismatch without suggestion, malformed UUID, and missing resume block. |
| 8. `update_session_capture("resumed")` write timing pre-spawn | `COVERED` | `src-tauri/tests/pr_f_resume_integration.rs:384-410` uses `exit 7` and still finds `session_id` plus `"resumed"` in the row. |
| 9. Trace renderer extension | `OVER-TESTED` | Focused tests at `src-tauri/src/trace/mod.rs:1256-1299`, nonzero-exit warning at `1302-1327`, full bundle at `1330-1373`, JSON at `1377-1386`. |
| 10. No-resume regression (`"none"`) | `COVERED` | Integration at `src-tauri/tests/pr_f_resume_integration.rs:464-488`, plus DB writer coverage at `src-tauri/src/state/db.rs:2513-2545`. |

The explicit watch-points are satisfied. The suggestion test seeds two models and names the other model in stderr (`src-tauri/tests/pr_f_resume_integration.rs:271-308`). The single-match stderr test uses a negative assertion (`197-216`). The multi-match detail-line test seeds two providers with distinct fixed timestamps and asserts the ordered provider list in the emitted line (`220-245`). The pre-spawn timing test uses a non-zero child (`384-410`). The resumed transcript test seeds a real transcript path and asserts `TranscriptState::Available` (`src-tauri/src/trace/mod.rs:1271-1279`). The ASCII test asserts the explicit `Resume target:` label substring (`1291-1298`). The DB ordering test uses fixed timestamps, not `Utc::now()` (`src-tauri/src/state/db.rs:2776-2821`).

`Specificity` is a `B` because a few tests collapse several contract assertions into one scenario. The clearest example is `src-tauri/src/trace/mod.rs:1330-1373`, where `resumed_session_with_nonzero_exit_carries_full_trace_bundle` checks warning text, transcript resolution, turn counts, ASCII labeling, and JSON capture method together. That test is useful as a backstop, but it is also redundant with the focused tests at `1256-1299` and `1377-1386`, so a failure there would not localize quickly. A smaller version of the same issue appears in `src-tauri/tests/pr_f_resume_integration.rs:414-460`, which simultaneously proves subcommand argv composition, successful spawn, stderr emission, and DB provenance. I would keep those tests, but they are why this dimension is not an `A`.

`Avoidance of impl-detail coupling` is a `B` because `src-tauri/src/main.rs:1128-1157` tests `should_emit_resume_detail_line()` and `should_emit_resume_short_line()` directly. The contract is about observable stderr behavior, and that is already covered by the integration file; pinning the helper functions makes the suite more sensitive to harmless refactors inside `main.rs`. This is moderate rather than severe because the rest of the PR is much more contract-surface oriented: config tests validate load-time acceptance/rejection, DB tests assert index presence and ordered row results, executor tests assert child argv, integration tests assert exit codes and stderr, and trace tests assert ASCII/JSON outputs rather than internal trace structs.

No coverage gaps remain in the required walk. The one place I would trim, not expand, is item 9: the trace section already has both focused checks and a kitchen-sink bundle, so additional tests there would likely add noise before they add confidence.
