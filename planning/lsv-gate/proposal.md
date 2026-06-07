# LSV proposal — launch stream volume fix

## Problem

The long-turn E2E (re-enable bar) failed at 15m13s: a healthy external gpt-none turn died with
`external provider transport failed: stdout_limit_exceeded` → spawn_error, 4 seconds before completion. The
launch path buffered the entire NDJSON stream then parsed post-exit (`parse_launch_output` over the captured
buffer); `ProviderOutputLimits.stdout_bytes = 1MiB` bounded that buffer; `truncated` was a hard transport
error. Healthy long turns are production-normal; streams grow — this is the volume sibling of the fixed
DEFAULT_LAUNCH_TIMEOUT bug.

## Design

`stream.rs` parses launch JSONL incrementally; retention is bounded (event sample, decoded output window,
diagnostics evidence tail, marker evidence) independent of stream length; a valid final LaunchExitEvent
finalizes the launch even when raw capture was truncated; truncation WITHOUT a valid exit stays a transport
error; non-launch one-shot invocations keep capped response semantics; heartbeat-gap liveness supervision
unchanged. Runtime submitted-turn mapping reads retained marker evidence (no reliance on the bounded sampled
event list). No frozen contract/v1 changes; no state.db changes.

## Audited range

`80d6904..2fe6745`: functional `7d76426`, declaration prep `8a11fba` (doc-comments only), and
split-only/declaration-sync remediation `2fe6745`.

## Proof plan

Evidence log: `planning/lsv-gate/evidence/runtime-tests.log` (XDG-isolated, OULIPOLY_DATA_DIR scrubbed).

Runtime claim: A valid launch stream larger than the transport capture limit completes successfully from its
exit event instead of failing stdout_limit_exceeded; diagnostics honestly record bounded retention.

Proof method: `crates/oulipoly-provider/tests/launch_stream.rs::launch_accepts_valid_stream_larger_than_transport_capture_limit`
(real fake-provider subprocess streaming past the limit) plus the bounded-retention unit tests in
`crates/oulipoly-provider/src/stream.rs` and `process.rs`.

Evidence-class match: runtime subprocess integration plus runtime unit — the integration test exercises the
real client/process/stream pipeline against a compiled fake provider binary; unit tests pin retention bounds
independent of stream length. Recorded under `cargo test -p oulipoly-provider`.

Runtime claim: Truncation without a valid final exit event remains a transport error, and non-launch one-shot
invocations keep capped stdout semantics.

Proof method: `launch_stream.rs::launch_stdout_truncation_takes_precedence_over_parseable_exit_prefix`,
`launch_stream.rs::launch_provider_nonzero_without_final_exit_is_transport_error`, and the existing
invocation-limit tests in `client.rs`.

Evidence-class match: runtime subprocess integration plus runtime unit, same suites, recorded in the evidence log.

Runtime claim: External launch/resume finalization (incl. the oehx terminal-error honesty semantics and
submitted-turn marker mapping) is unchanged for ordinary streams.

Proof method: `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume` (5 tests) and
`cargo test -p oulipoly-runtime` (916 tests incl. launch_result_mapper retained-marker mapping).

Evidence-class match: runtime CLI integration (real binary + fake provider CLI) plus runtime unit; recorded in
the evidence log.

Runtime claim: External launch with provider failure `terminal_signal` plus provider process `exited(0)` finalizes
as failed with provider-error terminal reason, and both the result envelope and invocation state row record the
failed outcome honestly.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_terminal_error_exit_zero_finalizes_as_failed`,
recorded by `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Evidence-class match: runtime CLI integration — the test runs the production-shaped `oulipoly-agent-runner`
external launch path against the fake provider CLI, asserts the response envelope's provider-error status/reason,
and reads the state DB invocation row to verify the failed status and terminal reason.

Runtime claim: External resume with provider failure `terminal_signal` plus provider process `exited(0)` has the
same honesty behavior as launch: failed provider-error result envelope plus failed invocation state row.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_terminal_error_exit_zero_finalizes_as_failed`,
recorded by `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Evidence-class match: runtime CLI integration — the test exercises the resume-specific runner path with the fake
provider CLI terminal-error stream, then checks both the returned envelope and persisted invocation row.

Runtime claim: Clean external paths remain unchanged: `clean_exit` with provider process `exited(0)` succeeds, and
real nonzero model exits are preserved as model outcomes rather than provider transport failures.

Proof method: `crates/oulipoly-provider/tests/launch_stream.rs::launch_model_nonzero_final_exit_is_outcome_not_provider_transport_failure`,
`src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`,
and `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_stream_over_capture_limit_finalizes_succeeded`,
recorded by `cargo test -p oulipoly-provider` and
`cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Evidence-class match: runtime subprocess integration plus runtime CLI integration — the provider-client tests pin
launch stream final-exit mapping for clean and model-nonzero exits, while the CLI regression test verifies the
production-shaped external launch path still returns success for a clean exit.

Runtime claim: In-tree oeh-gate semantics are unchanged for terminal-error honesty and mapper behavior.

Proof method: `cargo test -p oulipoly-runtime`, including the launch result mapper and terminal-error honesty rows
recorded in `planning/lsv-gate/evidence/runtime-tests.log`.

Evidence-class match: runtime unit — the runtime suite directly exercises the in-tree mapper/terminal decision
surface without the external CLI wrapper, so it catches regressions in the oeh-gate semantics independent of the
provider subprocess fixture.
