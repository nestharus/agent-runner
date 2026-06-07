# OEHX contract — external-path terminal-error honesty (33775d7..807f35c)

Functional commit: `807f35c` "fix external terminal failure finalization". Parity follow-up to the gated
in-tree fix (oeh-gate, f58c14f + a97e085): the EXTERNAL provider path now consumes the same shared
failure-exit/reason rules, so a provider-reported failure `terminal_signal` (e.g. opencode structured-error
`unknown` with `provider error: ...` evidence) with truthful `status: exited(0)` finalizes as failure
(`exit_code = -1`, terminal_reason carries the provider evidence) on both external launch and external resume.
Provider CleanExit + exit 0 unchanged; real nonzero codes preserved.

## Declared roles (touched files)

| File | Declared roles | Meaning |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `formatter` | Formats external terminal reasons; now a thin consumer of the shared terminal-reason rule. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `mapper`, `validator` | Maps provider launch results into host execution results via the shared failure-exit/reason helpers. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration`, `mapper`, `formatter`, `validator` | Facade; re-exports the shared terminal helpers crate-internally (3-line delta). |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper`, `validator` | Supervised output mapping; delegates reason/exit derivation to the shared helpers. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator` | Owner of the terminal-signal vocabulary and the shared failure-exit/reason rules (`terminal_exit_code_from_signal`, `terminal_reason_from_signal_status`). |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `mapper`, `validator` | Maps provider ProcessStatus/TerminalSignal into host terminal outcome via the shared helpers. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `orchestration`, `mapper`, `formatter`, `parser`, `accessor`, `predicate`, `validator` (TEST) | One-line assertion sync; suite-wide fixture helpers. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator` (TEST) | One-line assertion sync; suite-wide fixture helpers. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate` (TEST) | High-seam external launch/resume regression fixtures incl. the incident scenario. |

## Focused production inventory

| File | Function or symbol | A1 class | Meaning |
|---|---|---|---|
| `.../cli/terminal_signal.rs` | `terminal_exit_code_from_signal` | `mapper` | Single owner of the failure-exit rule: synthetic failure code when a failure-classified signal coincides with real exit 0; real code otherwise. |
| `.../cli/terminal_signal.rs` | `terminal_reason_from_signal_status` | `mapper` | Shared reason rule over `TerminalStatusEvidence` (kind-canonical reasons; provider-error evidence for Unknown; status reasons for exit kinds). |
| `.../cli/terminal_signal.rs` | `terminal_reason_from_signal` | `mapper` | Thin `ExitStatus` adapter over the shared status-based rule. |
| `.../cli/terminal_signal.rs` | `terminal_status_reason` | `mapper` | Maps `TerminalStatusEvidence` to the stable reason vocabulary. |
| `.../cli/supervision/terminal_outcome.rs` | `supervised_exit_code` | `mapper` | Delegates to `terminal_exit_code_from_signal` (rule no longer duplicated). |
| `.../external_provider/terminal_cancel_mapper.rs` | `map_terminal_cancel_outcome` | `mapper` | Builds host TerminalSignal then derives exit/reason via the shared rules. |
| `.../external_provider/terminal_cancel_mapper.rs` | `exit_code` | `mapper` | Status-only projection (input to the shared override). |
| `.../external_provider/terminal_cancel_mapper.rs` | `terminal_reason` | `mapper` | Status reason merged with signal-derived reason via the shared rule. |
| `.../diagnostics/external_provider/result_mapper.rs` | result construction helpers | `mapper` | Provider result → ExecutionResult with shared exit/reason derivation. |
| `.../diagnostics/external_provider/reason_format.rs` | reason formatting | `formatter` | Thin formatting over the shared rule (duplicate logic removed). |

## Proof plan

| Claim | Proof |
|---|---|
| External launch: provider failure signal + exited(0) finalizes failed with provider-error reason | `src-tauri/tests/s10_external_provider_resume.rs::external_provider_launch_terminal_error_exit_zero_finalizes_as_failed` (real binary against fake provider CLI; asserts envelope + invocation row) |
| External resume: same honesty | `...::external_provider_resume_terminal_error_exit_zero_finalizes_as_failed` |
| Clean external paths unchanged | `...::external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd` + existing S10 suites green |
| Unit-level mapper behavior (failure+0 → synthetic; clean+0 → 0; failure+nonzero → real code) | unit tests in `terminal_cancel_mapper.rs` / `result_mapper.rs` / `terminal_signal.rs` (see runtime-tests.log) |
| In-tree path unaffected (a97e085 semantics preserved) | `cargo test -p oulipoly-runtime` + `opencode_terminal_error_exit_zero` suite green |

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli.rs
    role: adapter
    Translates:
      - executor-public-entrypoint-contract
      - executor-cli-component-set-contract
      - executor-cli-test-fixture-contract
      - tempfile-unix-permissions-test-contract
  - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
    role: adapter
    Translates:
      - std-process-exit-status-contract
      - unix-signal-name-contract
      - signal-hook-forwarding-contract
      - executor-terminal-signal-dto-contract
      - terminal-signal-recognizer-contract
  - component: crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs
    role: adapter
    Translates:
      - terminal-signal-classification-contract
      - std-process-exit-status-contract
      - supervised-output-contract
  - component: crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs
    role: adapter
    Translates:
      - external-provider-process-status-contract
      - external-provider-terminal-signal-contract
      - executor-terminal-signal-dto-contract
      - terminal-failure-exit-reason-contract
  - component: crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs
    role: adapter
    Translates:
      - external-provider-process-status-contract
      - external-provider-terminal-signal-contract
      - execution-result-dto-contract
      - terminal-failure-exit-reason-contract
  - component: crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs
    role: adapter
    Translates:
      - external-provider-terminal-signal-contract
      - terminal-failure-exit-reason-contract
  - component: crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs
    role: adapter
    Translates:
      - executor-service-port test harness contract
      - provider-registry fixture contract
      - external-provider client cancellation contract
      - Unix fixture script and environment contract
      - serde_json fixture parsing contract
  - component: crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs
    role: adapter
    Translates:
      - executor-service-port test harness contract
      - provider-registry fixture contract
      - executor terminal-signal vocabulary contract
      - Unix fixture script and environment contract
      - serde_json fixture parsing contract
  - component: src-tauri/tests/s10_external_provider_resume.rs
    role: adapter
    Translates:
      - Unix CLI integration fixture contract
      - Oulipoly result-envelope JSON contract
      - StateDb invocation terminal fields
      - fake external provider CLI contract
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
    role: intrinsic-surface
    Domain: runtime terminal-signal vocabulary + reason mapping
    Owns:
      - full TerminalSignalKind vocabulary
      - terminal status and synthetic exit-code mapping
      - built-in terminal evidence construction
      - terminal reason canonicalization hook
      - provider-error terminal-reason evidence preservation for Unknown signals
      - shared failure-exit-code override rule (terminal_exit_code_from_signal)
  - component: crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs
    role: intrinsic-surface
    Domain: supervised_terminal_output_mapping
    Owns:
      - synthetic failure exit code when terminal failure signal coincides with real exit 0
      - real nonzero exit code preservation
      - supervised terminal reason propagation
```
