# tsb Step-6a Proposal

The live bug was an unbounded pre-dispatch turn scan for OpenCode: implicit session enumeration could walk historical sessions and per-session exports could hang before provider selection completed. The fix shape is bounded and conservative: enumerate only the recent quota-balancing window when session timestamps are available, fall back to a small max-session cap when timestamps are absent, apply per-OpenCode-call timeout and a whole-adapter deadline, emit a degraded marker after partial progress, and let the Rust runtime enforce a hard script deadline around user-configured scripts.

The adapter remains a public-CLI adapter. It calls `opencode session list --json` and `opencode export <sessionID>` rather than OpenCode private storage, normalizes exported messages to runtime JSONL, and exits 0 with `{"degraded":true,"count":...}` if discovery/export timing degrades. The runtime remains the owner of process-level script deadlines: session and quota script wrappers apply a bounded timeout, classify timeout errors with `script_timeout`, kill the spawned process group on Unix, wait the child, and proceed conservatively through scan errors instead of blocking provider selection indefinitely.

## Proof plan

Evidence log: `planning/tsb-gate/evidence/runtime-tests.log`.

Runtime claim: OpenCode adapter integration uses the public `opencode session list --json` surface and ingests normalized JSONL without private-layout discovery.

Proof method: `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs::opencode_read_turns_ingests_normalized_jsonl`.

Evidence-class match: particular-integration; fake OpenCode accepts `session list --json` and `export ses_fixture`, then assertions cover `report.errors == []`, `report.new_turns == 2`, repeated scan `new_turns == 0`, `counts.total == 2`, and `counts.assistant == 1`. Evidence log records this test as `ok`.

Runtime claim: Implicit OpenCode discovery is recent-window bounded when session-list timestamps are present.

Proof method: `scripts/tests/opencode-turns.test.sh::test_exports_only_recent_window_sessions`.

Evidence-class match: script integration; mock sessions at 1h and 5h are exported, the 9h session and the timestampless session are excluded, stdout contains only recent session IDs, and no degraded marker is emitted. Evidence log records the shell suite command as passed.

Runtime claim: OpenCode export timeout degrades best-effort instead of hanging provider selection.

Proof method: `scripts/tests/opencode-turns.test.sh::test_timeout_emits_degraded_best_effort_and_exits_zero`.

Evidence-class match: script integration; with `OPENCODE_TURNS_CALL_TIMEOUT=1` and `OPENCODE_TURNS_DEADLINE=3`, the adapter exits 0, emits the fast session record, emits `"degraded":true`, emits `"count":1`, and finishes within the asserted elapsed bound. Evidence log records the shell suite command as passed.

Runtime claim: Runtime session ingest recognizes the adapter degraded marker as degradation evidence rather than malformed turn JSON.

Proof method: `crates/oulipoly-runtime/src/sessions/mod.rs::tests::degraded_marker_is_reported_without_malformed_turn_error`.

Evidence-class match: unit; scan over `{"degraded":true,"count":1}` yields `new_turns == 0`, exactly one error containing `degraded`, and no `malformed` error. Evidence log records this related regression test by name.

Runtime claim: Runtime session script timeout is classified and proceeds conservatively without persisting turns.

Proof method: `crates/oulipoly-runtime/src/sessions/mod.rs::tests::turn_script_timeout_is_classified_and_does_not_persist_turns`.

Evidence-class match: unit; `scan_provider_with_timeout(..., 1)` against `sleep 60` yields `new_turns == 0`, DB assistant count `0`, one error containing `script_timeout`, and one error containing `turn script`. Evidence log records this related regression test by name.

Runtime claim: Runtime quota script timeout is classified with a stable `script_timeout` token.

Proof method: `crates/oulipoly-runtime/src/quota/process.rs::tests::quota_script_timeout_is_classified`.

Evidence-class match: unit; `run_script_with_timeout("sleep 60", 1)` returns an error containing `script_timeout` and `quota script`. This is a shipped test, but it is not shown in `planning/tsb-gate/evidence/runtime-tests.log`.

Runtime claim: Runtime quota script timeout kills Unix process-group children.

Proof method: `crates/oulipoly-runtime/src/quota/process.rs::tests::quota_script_timeout_kills_process_group_children`.

Evidence-class match: unit, Unix-only; a timed-out quota script starts a background child that would write a marker after 2s, the timeout fires at 1s, and after 3s the marker does not exist. This is a shipped test, but it is not shown in `planning/tsb-gate/evidence/runtime-tests.log`.

Runtime claim: Timestampless implicit OpenCode discovery falls back to the configured max-session cap.

Proof method: `scripts/tests/opencode-turns.test.sh::test_timestampless_session_list_applies_max_sessions_cap`.

Evidence-class match: script integration; the mock `opencode session list --json` returns five timestampless sessions, the test runs with `OPENCODE_TURNS_MAX_SESSIONS=3`, and assertions verify exactly the first three sessions are exported while the fourth and fifth are absent from stdout. Evidence log records this proof-risk test by name.

Runtime claim: Python adapter timeout cleanup kills all OpenCode process-group descendants, not just the direct process.

Proof method: `scripts/tests/opencode-turns.test.sh::test_timeout_kills_opencode_process_group_descendant`.

Evidence-class match: script integration; the mock OpenCode export spawns a same-process-group descendant that writes a marker periodically, then wedges past `OPENCODE_TURNS_CALL_TIMEOUT=1`. The test asserts the adapter exits 0 with a degraded marker, the descendant marker stops growing after timeout cleanup, and the descendant process is no longer running. Evidence log records this proof-risk test by name.

Runtime claim: Runtime session-script process-group timeout kills shell grandchildren.

Proof method: `crates/oulipoly-runtime/src/sessions/mod.rs::tests::turn_script_timeout_kills_process_group_children`.

Evidence-class match: unit, Unix-only; a timed-out session turn script starts a background child that would write a marker after 2s, the timeout fires at 1s, and after 3s the marker does not exist. Evidence log records this proof-risk test by name.
