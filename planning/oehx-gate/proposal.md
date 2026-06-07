# OEHX proposal — external-path terminal-error honesty parity

## Problem

The in-tree honesty fix (oeh-gate, closed 6/6 LOW at 46181c6) made a terminal failure signal coinciding with
real exit 0 finalize as failure. The EXTERNAL provider path had the identical hole, unfixed: its mappers
(`terminal_cancel_mapper.rs`, `result_mapper.rs`) projected `exit_code`/`terminal_reason` from `ProcessStatus`
alone, so the provider's failure `terminal_signal` (now emitted by agent-runner-opencode 9a75498 for
stream-terminal opencode structured errors) reached only the signal field while the invocation finalized
`success: true`.

Incident grounding: 2026-06-06 22:04:14 PDT — three production sessions on one account crashed simultaneously
(opencode-internal SQLite `Failed to execute statement`), emitted a terminal `{"type":"error"}` NDJSON event,
exited 0, and were recorded succeeded. The in-tree path is fixed; this delta closes the external path before
any model is re-flipped to external providers.

## Design (one owner for the rule)

`terminal_signal.rs` (the gated intrinsic owner of "terminal status and synthetic exit-code mapping") gains the
shared rule `terminal_exit_code_from_signal` and the status-evidence-based
`terminal_reason_from_signal_status`; `supervised_exit_code` (in-tree) and both external mappers consume them.
`reason_format.rs` duplicate logic deleted (-67 lines). No frozen oulipoly-provider contract changes; no
state.db schema changes; provider `status` stays truthful — only host-side finalization semantics change.

## Audited range

Functional: `33775d7..807f35c` (single commit 807f35c). Base 33775d7 is the pushed oeh-gate closure.

## Proof plan

Evidence log: `planning/oehx-gate/evidence/runtime-tests.log` (XDG-isolated, OULIPOLY_DATA_DIR scrubbed).

Runtime claim: An external provider launch whose LaunchExitEvent carries a failure-classified terminal_signal (kind `unknown` with `provider error: ...` evidence) and truthful `status: exited(0)` finalizes as `success=false`, `exit_code=-1`, and `terminal_reason` carrying the provider-error evidence — for the result envelope and the durable invocation row.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_terminal_error_exit_zero_finalizes_as_failed`, plus unit tests `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs::tests` and `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs::tests` asserting the mapper-level synthetic override and reason propagation.

Evidence-class match: runtime CLI integration plus runtime unit. The high-seam test runs the real `agents` binary against a fake external provider CLI that emits the failure terminal_signal with exited(0) status, then asserts the emitted result envelope (`status=failed`, `success=false`, `exit_code=-1`, provider-error `terminal_reason`) and the persisted StateDb invocation row — the production launch path, not a proxy. The unit tests pin the shared-rule consumption (`terminal_exit_code_from_signal`, `terminal_reason_from_signal_status`) at the mapper seams. Commands recorded in the evidence log: `cargo test -p oulipoly-runtime`, `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Runtime claim: An external provider RESUME with the same failure-signal-plus-exited(0) evidence finalizes the invocation row as Failed with the same exit/reason honesty.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_terminal_error_exit_zero_finalizes_as_failed`.

Evidence-class match: runtime CLI integration — real binary, fake external provider CLI, resume entrypoint; asserts the durable invocation row outcome. Recorded in the evidence log under `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Runtime claim: Clean external paths are unchanged — provider clean_exit with exited(0) stays succeeded, and real nonzero exit codes are preserved.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`, `src-tauri/tests/s10_external_provider_resume.rs::external_launch_session_id_alias_persists_external_capture_method_without_session_capability`, and the mapper unit tests covering clean+0 -> 0/None and failure+nonzero -> real-code preservation.

Evidence-class match: runtime CLI integration plus runtime unit — the integration fixtures exercise the unchanged success path end-to-end; the unit tests pin the non-override branches of the shared rule. Recorded in the evidence log.

Runtime claim: The in-tree path semantics shipped by the oeh-gate (f58c14f + a97e085) are unchanged by this delta.

Proof method: `cargo test -p oulipoly-runtime` (374 tests incl. the opencode recognizer/supervision suites) and `cargo test -p oulipoly-agent-runner --test opencode_terminal_error_exit_zero` (3 tests: incident one-shot failed, recovered succeeded, incident resume failed).

Evidence-class match: runtime unit plus CLI integration, identical suites to the oeh-gate proof rows, re-run green at this delta's HEAD and recorded in the evidence log.
