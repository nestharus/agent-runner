# OEH Gate Proposal

OEH fixes the OpenCode incident where terminal structured error events could be followed by a real process exit code of `0` and still be recorded as successful. The functional delta is `f58c14f` and `a97e085`: OpenCode now recognizes only the last non-empty stream line as a terminal structured `type:error` event, carries provider-error evidence into the terminal reason, and supervised finalization substitutes the synthetic failure code when a terminal failure signal coincides with a real exit `0`.

The incident context is three production sessions on one OpenCode account crashing simultaneously with `Failed to execute statement`, emitting terminal NDJSON `{"type":"error"}`, exiting `0`, and being recorded `success:true`. The fix keeps recovered streams honest: an earlier error event followed by later stream output and a clean exit remains succeeded. It also preserves F4 parity: ordinary output containing quota or rate-limit words is not classified as quota or rate limit unless it is inside OpenCode's structured terminal error event.

The audited base range is `549daaa..HEAD`, including pre-gate validation remediation commit `bdbb9e3` for OpenCode F4 unit coverage without changing runtime behavior. Artifact-only commits in the original `549daaa..3515d31` range are excluded from the functional behavior surface: `8db1a02`, `37b6223`, `be9761b`, and `3515d31` only synchronize gate evidence or artifact hygiene. The `.gitignore` line in `gates/touched-files.txt` comes from `3515d31` and is treated as planning scratch hygiene, not product behavior.

No `state.db` schema change, private OpenCode database fallback, frozen contract schema edit, installer change, or new feature is introduced.

## Proof plan

Evidence log: `planning/oeh-gate/evidence/runtime-tests.log`.

Runtime claim: The incident stream, a terminal OpenCode structured error event with real process exit `0`, finalizes one-shot and resume invocations as `success=false`, `exit_code=-1`, and `terminal_reason` carrying the provider error message.

Proof method: `src-tauri/tests/opencode_terminal_error_exit_zero.rs::opencode_terminal_error_exit_zero_finalizes_one_shot_as_failed`, `src-tauri/tests/opencode_terminal_error_exit_zero.rs::opencode_terminal_error_exit_zero_finalizes_resume_as_failed`, `crates/oulipoly-runtime/src/executor/providers/opencode.rs::tests::terminal_structured_error_exit_zero_maps_to_failure_signal_with_incident_evidence`, and `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs::tests::opencode_terminal_structured_error_exit_zero_carries_failure_reason_evidence`.

Evidence-class match: runtime unit plus CLI integration. The recognizer test proves the incident NDJSON is a failure signal with provider-error evidence; the supervised-output test proves real exit `0` is replaced by the synthetic failure code and preserves the incident message; the one-shot CLI test asserts the emitted result envelope has `status=failed`, `success=false`, `exit_code=-1`, and the exact provider-error terminal reason; the resume CLI test asserts the durable invocation row is `Failed`, `success=0`, `exit_code=-1`, and carries the same terminal reason. The evidence log records `cargo test -p oulipoly-runtime` and `cargo test -p oulipoly-agent-runner --test opencode_terminal_error_exit_zero --test structural_segmentation` under isolated XDG roots with `OULIPOLY_DATA_DIR` scrubbed.

Runtime claim: An OpenCode error event followed by later stream events and real process exit `0` stays succeeded.

Proof method: `src-tauri/tests/opencode_terminal_error_exit_zero.rs::opencode_error_event_followed_by_later_event_finalizes_one_shot_as_succeeded`, `crates/oulipoly-runtime/src/executor/providers/opencode.rs::tests::recovered_session_error_followed_by_later_event_preserves_clean_exit`, and `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs::tests::opencode_error_event_followed_by_later_event_preserves_clean_exit`.

Evidence-class match: runtime unit plus CLI integration. The recognizer and supervised-output tests prove last-line-only terminal classification preserves a clean terminal signal when a later stream event follows the error. The one-shot CLI test asserts process exit `0`, result `status=succeeded`, `success=true`, `exit_code=0`, null `terminal_reason`, and a succeeded invocation row.

Runtime claim: F4 parity holds for OpenCode: quota and rate-limit substrings in ordinary output never classify as quota or rate-limit signals.

Proof method: `crates/oulipoly-runtime/src/executor/providers/opencode.rs::tests::ordinary_output_quota_and_rate_text_preserves_clean_exit` and `crates/oulipoly-runtime/src/executor/providers/opencode.rs::tests::ordinary_output_quota_and_rate_text_preserves_nonzero_exit`.

Evidence-class match: runtime unit. The tests feed ordinary non-error OpenCode output containing both `quota exhausted` and `rate limit exceeded`; clean process status remains `CleanExit`, and nonzero process status remains `NonzeroExit` rather than `QuotaExhaustedInband` or `RateLimited`. These tests are covered by `cargo test -p oulipoly-runtime` in the evidence log.
