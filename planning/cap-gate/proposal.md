# Capture-Time Session Persistence Proposal

The live race was that fresh spawns produced a PID sidecar row with `session_id = NULL`, while `state.db` only persisted the provider session id at finalize. A mid-turn `notify agent-bash-complete` therefore had process identity evidence but no owner session and returned `no_owner`; on the second live E2E, re-running the identical notify after finalize delivered and the agent replied `WOKE rc: 0`.

The fix is a single-fire `None` to `Some` capture-transition hook in the supervised stdout path. When stdout-json capture first yields a session id, the hook backfills the existing sidecar row via `set_session_id` and marks the captured session running through the same `session_runtime` upsert used for sessions known at spawn time. Backfill failures are non-fatal: the runner logs warnings and preserves provider execution.

The completeness invariant is provider-contract dependent: for this capture path, the provider's first stdout event is the session-capture event and it precedes any model tool call. Because spooler-originated notifies come from model tool calls, those notifies run post-capture and can resolve the owner from the backfilled sidecar row. The shipped opencode fixture exercises this ordering; it does not prove providers that violate the capture-before-tool-call contract.

Commit `9ba1275` is test infrastructure, not runtime behavior. The sweep scrubs the higher-precedence `OULIPOLY_DATA_DIR` pin from XDG-isolated harnesses after a live poison pin leaked production state into supposedly isolated tests; the evidence includes the age100 poison-pin spot proof and full workspace clean/poisoned summaries.

## Proof plan

Runtime claim: Stdout-json capture on a fresh spawn backfills the PID sidecar row with the captured session id and writes a `session_runtime` running row through the running path.
Proof method: `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::stdout_json_event_capture_backfills_sidecar_and_marks_runtime_running`.
Evidence-class match: State-backed runtime integration; it asserts one sidecar row, `row.session_id == CAPTURED_SESSION_ID`, `session_runtime` fields matching invocation/provider/model/PID identity, and the later idle transition.

Runtime claim: Captured stdout session id without spawn identity does not create or backfill a sidecar row.
Proof method: `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs::stdout_json_event_capture_without_spawn_identity_does_not_backfill_sidecar`.
Evidence-class match: Negative runtime integration; it executes the same capture provider with `parent_invocation_env: None` and asserts the sidecar DB path does not exist.

Runtime claim: A mid-turn notify on a fresh opencode-style stdout-capture spawn resolves owner via the capture-time sidecar row and the mailbox row is delivered.
Proof method: `src-tauri/tests/wu_d_proactive_wake_integration.rs::opencode_mid_turn_notify_resolves_capture_time_sidecar_owner`.
Evidence-class match: End-to-end CLI integration; it asserts notify `status = enqueued`, `owner_session_id = ses_capturemidturn`, `session_source = sidecar_session_id`, `wake.status = busy`, the sidecar provider row contains the captured session id, the mailbox row is delivered, and runtime ends idle.

Runtime claim: XDG-isolated runtime and src-tauri test harnesses are immune to a poison `OULIPOLY_DATA_DIR` pin, including the age100 live-failure class.
Proof method: `planning/cap-gate/evidence/runtime-tests.log`, plus shipped spot-proof `src-tauri/tests/age100_one_shot_quota_migration.rs::one_shot_all_pool_members_quota_exhausted_returns_blocked_all_providers_exhausted` under a poison pin.
Evidence-class match: Suite-run evidence plus one exact shipped spot test; the log records poisoned `300 suites, 2550 passed, 0 failed, 9 ignored`, clean `300 suites, 2550 passed, 0 failed, 9 ignored`, and the age100 poisoned spot-proof passing. No single shipped test alone proves all 51 scrubbed harnesses; the full-run log is the evidence for the sweep.
