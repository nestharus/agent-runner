# LSV contract — launch stream volume fix (2fe6745 delta over 80d6904)

Functional commit: `7d76426` "Fix bounded launch stream parsing"; declaration prep: `8a11fba` (doc-comments
only); split-only/declaration-sync remediation: `2fe6745`. Incident: a healthy 15m13s external turn failed `stdout_limit_exceeded` -> spawn_error 4s before
completion. Fix: launch JSONL stdout parses INCREMENTALLY with bounded retention (events, decoded output,
diagnostics evidence tail, marker evidence); valid streams over the old 1MiB capture limit finalize from the
exit event; non-launch one-shot responses keep capped semantics; heartbeat-gap liveness unchanged.

## Declared roles (touched files)

| File | Declared roles |
|---|---|
| `crates/oulipoly-provider/src/client.rs` | orchestration, validator, parser, mapper, accessor, predicate |
| `crates/oulipoly-provider/src/process.rs` | orchestration, mapper, predicate, accessor, filter, validator |
| `crates/oulipoly-provider/src/stream.rs` | parser, validator, mapper, accessor, filter, orchestration, formatter |
| `crates/oulipoly-provider/src/testkit.rs` | orchestration, formatter, mapper, accessor, predicate, parser, filter, validator |
| `crates/oulipoly-provider/tests/launch_stream.rs` | validator, orchestration, mapper, accessor, predicate |
| `crates/oulipoly-provider/tests/launch_stream_protocol.rs` | validator, orchestration, formatter, mapper, accessor |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | orchestration, accessor, mapper, formatter, parser, predicate, validator |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | mapper, accessor, predicate |
| `src-tauri/tests/s10_external_provider_resume.rs` | orchestration, formatter, mapper, accessor, parser, validator, predicate, filter |

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
as failed with provider-error terminal reason in both the result envelope and invocation row.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_terminal_error_exit_zero_finalizes_as_failed`,
recorded by `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Evidence-class match: runtime CLI integration — production-shaped runner plus fake provider CLI asserts returned
provider-error envelope and persisted failed invocation state.

Runtime claim: External resume with provider failure `terminal_signal` plus provider process `exited(0)` has the
same provider-error envelope and failed invocation-row honesty behavior.

Proof method: `src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_terminal_error_exit_zero_finalizes_as_failed`,
recorded by `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`.

Evidence-class match: runtime CLI integration — resume-specific runner path plus fake provider CLI asserts both
the returned envelope and persisted invocation row.

Runtime claim: Clean external paths remain unchanged: `clean_exit` with `exited(0)` succeeds, and real nonzero
model exits are preserved as model outcomes rather than provider transport failures.

Proof method: `crates/oulipoly-provider/tests/launch_stream.rs::launch_model_nonzero_final_exit_is_outcome_not_provider_transport_failure`,
`src-tauri/tests/s10_external_provider_resume.rs::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd`,
and `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_stream_over_capture_limit_finalizes_succeeded`.

Evidence-class match: runtime subprocess integration plus runtime CLI integration — provider-client tests pin
stream final-exit mapping, and the CLI regression pins production-shaped clean external launch success.

Runtime claim: In-tree oeh-gate semantics are unchanged for terminal-error honesty and launch-result mapping.

Proof method: `cargo test -p oulipoly-runtime`, including launch-result mapper and terminal-error honesty rows in
`planning/lsv-gate/evidence/runtime-tests.log`.

Evidence-class match: runtime unit — the runtime suite directly exercises the in-tree mapper/terminal decision
surface without relying on the external CLI wrapper.

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-provider/src/client.rs
    role: adapter
    Translates:
      - provider-client-options-contract
      - provider-cli-subprocess-contract
      - oulipoly-provider-generated-dto-contract
      - launch-jsonl-stream-contract
      - byte-limit-capture-contract
  - component: crates/oulipoly-provider/src/process.rs
    role: adapter
    Translates:
      - provider-cli-subprocess-contract
      - std-process-command-contract
      - std-process-exit-status-contract
      - process-supervision-liveness-contract
      - byte-limit-capture-contract
  - component: crates/oulipoly-provider/src/stream.rs
    role: adapter
    Translates:
      - launch-jsonl-stream-contract
      - oulipoly-provider-generated-dto-contract
      - byte-limit-capture-contract
      - provider-client-error-contract
      - launch-event-retention-contract
  - component: crates/oulipoly-provider/src/testkit.rs
    role: adapter
    Translates:
      - fake-provider-fixture-contract
      - provider-cli-subprocess-contract
      - process-supervision-liveness-contract
      - rustc-fixture-compilation-contract
      - test-process-environment-contract
  - component: crates/oulipoly-provider/tests/launch_stream.rs
    role: adapter
    Translates:
      - fake-provider-fixture-contract
      - provider-client-options-contract
      - provider-cli-subprocess-contract
      - launch-jsonl-stream-contract
  - component: crates/oulipoly-provider/tests/launch_stream_protocol.rs
    role: adapter
    Translates:
      - launch-jsonl-stream-contract
      - oulipoly-provider-generated-dto-contract
      - byte-limit-capture-contract
      - provider-client-error-contract
  - component: crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs
    role: adapter
    Translates:
      - fake-provider-fixture-contract
      - provider-cli-subprocess-contract
      - launch-jsonl-stream-contract
      - oulipoly-provider-generated-dto-contract
      - process-supervision-liveness-contract
  - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
    role: adapter
    Translates:
      - launch-jsonl-stream-contract
      - runtime-execution-result-contract
      - terminal-cancel-outcome-contract
      - session-capture-contract
      - submitted-user-turn-marker-contract
  - component: src-tauri/tests/s10_external_provider_resume.rs
    role: adapter
    Translates:
      - external-provider-runtime-cli-contract
      - provider-launch-jsonl-contract
      - invocation-state-db-contract
      - session-resume-contract
      - test-fixture-process-contract
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-provider/src/client.rs
    role: intrinsic-surface
    Domain: provider client transport orchestration
    Owns:
      - provider timeout and output-limit defaults
      - typed JSON invocation and launch entrypoints
      - request validation and response-envelope protocol mapping
      - stdout envelope parsing and launch stdout stream handoff
      - process diagnostics and last-invocation argv capture
  - component: crates/oulipoly-provider/src/process.rs
    role: intrinsic-surface
    Domain: provider subprocess supervision and bounded byte capture
    Owns:
      - ByteLimit, CapturedBytes, and accumulator truncation semantics
      - CancellationToken and ProcessLimits lifecycle inputs
      - ProcessCommand, ProcessOutcome, and ProcessRunner public surfaces
      - total-runtime and stdout-line-gap timeout behavior
      - cross-platform process group termination and executable checks
  - component: crates/oulipoly-provider/src/stream.rs
    role: intrinsic-surface
    Domain: launch JSONL decoding and bounded retention
    Owns:
      - DecodedLaunchEvent, LaunchExit, and LaunchResult shapes
      - LaunchStreamLimits defaults and bounded-by projection
      - event order, finality, contract, request-id, and schema checks
      - retained event, marker, stdout, stderr, and raw stdout budgets
      - launch stdout drain behavior for live subprocess integration
  - component: crates/oulipoly-provider/src/testkit.rs
    role: intrinsic-surface
    Domain: provider client fixture harness
    Owns:
      - FakeProvider compile/run/spawn helper surface
      - FakeProviderMode vocabulary and env projection
      - LeakProbe descendant observation and cleanup assertions
      - cross-platform fixture executable and process cleanup helpers
      - temporary fixture root and wrapper script allocation
  - component: crates/oulipoly-provider/tests/launch_stream.rs
    role: intrinsic-surface
    Domain: launch stream provider-client integration test suite
    Owns:
      - valid launch JSONL decoding and binary payload ordering coverage
      - request-id echo and exact argv/stdin invocation coverage
      - model exit versus provider process exit behavior coverage
      - launch stdout truncation precedence coverage
      - malformed protocol diagnostic and process-status preservation coverage
  - component: crates/oulipoly-provider/tests/launch_stream_protocol.rs
    role: intrinsic-surface
    Domain: launch JSONL protocol parser test suite
    Owns:
      - valid stdout, stderr, marker, heartbeat, and final-exit coverage
      - bounded retention and output-byte budget coverage
      - malformed line, unknown kind, and schema-invalid event coverage
      - contract, request-id, and base64 rejection coverage
      - sequence and finality error matrix coverage
  - component: crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs
    role: intrinsic-surface
    Domain: fake provider executable fixture modes and protocol payloads
    Owns:
      - fake-provider mode vocabulary and environment dispatch
      - describe, settings, rotation, migration, and launch fixture payloads
      - invocation record, count, artifact, and probe sidecar file behavior
      - process-tree, hang, pipe-pressure, and signal-resistance scenarios
      - request-id correlation fallback and JSON escaping helpers
  - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
    role: intrinsic-surface
    Domain: external-provider launch result mapping
    Owns:
      - LaunchResult stdout/stderr projection into ExecutionResult
      - terminal classification override and fallback mapping
      - launch session object to runtime session-capture mapping
      - submitted-user-turn marker extraction semantics
      - returned-artifact and child-invocation empty defaults for launch results
  - component: src-tauri/tests/s10_external_provider_resume.rs
    role: intrinsic-surface
    Domain: external-provider launch/resume CLI regression suite
    Owns:
      - isolated config/data fixture materialization
      - external provider Python script generation and executable setup
      - launch/resume command invocation and environment isolation
      - provider record parsing and subcommand filtering assertions
      - invocation session/outcome database assertions
```
