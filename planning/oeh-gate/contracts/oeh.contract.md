# OEH Gate Contract

## Component declared roles

Component: OpenCode terminal structured error honesty and supervised exit-zero failure finalization.

Declared roles: `mapper`, `parser`, `predicate`, `formatter`, `filter`, `orchestration`, `accessor`, `validator`.

## Source scope

The functional audit subject starts from `549daaa` and includes the OpenCode-error-honesty product commits `f58c14f` and `a97e085`. Pre-gate validation remediation commit `bdbb9e3` adds OpenCode F4 unit tests only; it does not alter runtime behavior.

Artifact-only commits in the original `549daaa..3515d31` range are excluded from the functional behavior surface: `8db1a02`, `37b6223`, `be9761b`, and `3515d31`. The `.gitignore` path in `planning/oeh-gate/gates/touched-files.txt` is artifact hygiene for scratch planning directories and not product behavior.

Touched files in scope are exactly `planning/oeh-gate/gates/touched-files.txt`.

## Touched-file roles

| File | Declared roles | Role notes |
|---|---|---|
| `.gitignore` | `formatter` | Artifact-hygiene ignore pattern for scratch directories; no product runtime behavior. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper`, `validator` | Maps terminal signal plus optional real child status into supervised output; tests verify incident and recovered stream finalization. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator` | Existing terminal-signal adapter; OEH changes map provider-error evidence on `Unknown` signals into terminal reasons. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `parser`, `filter`, `mapper`, `predicate`, `formatter`, `validator` | Parses OpenCode NDJSON terminal error lines, selects the terminal last line, maps structured errors to terminal signals/evidence, and proves F4 parity. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `orchestration`, `accessor`, `mapper`, `formatter`, `validator` | CLI integration fixture for one-shot/resume finalization and StateDb result assertions. |

## Focused production inventory

Only added or meaningfully changed production responsibilities are listed; auditors still own whole touched-file inspection.

| File | Function or symbol | A1 class | Meaning |
|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_output_from_terminal` | `mapper` | Builds `SupervisedOutput` from stdout/stderr, terminal status, optional terminal signal, and optional real child status. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_exit_code` | `mapper` | Maps terminal signal and real status to the final supervised exit code, using synthetic failure only when real exit is `0` and the terminal signal is failing. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `PROVIDER_ERROR_EVIDENCE_PREFIX` | `formatter` | Stable evidence prefix used to carry provider-error text as a terminal reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_reason_from_signal` | `mapper` | Maps terminal signal kind and optional real status into the runtime terminal reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `unknown_terminal_reason` | `mapper` | Maps `Unknown` terminal signals to provider-error evidence only when the evidence carries the provider-error prefix. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_signal_from_stream` | `mapper` | Maps the last non-empty stream line to an OpenCode structured terminal signal when that line is a `type:error` event. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `non_empty_stream_lines` | `orchestration` | Delegates trimming and emptiness filtering to single-class helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `trimmed_stream_lines` | `mapper` | Maps raw stream text into trimmed lines. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `retain_non_empty_lines` | `filter` | Selects non-empty lines. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_evidence` | `formatter` | Formats bounded terminal evidence for a structured OpenCode error line. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_evidence_from_value` | `mapper` | Maps parsed JSON values into provider-error evidence when they are OpenCode error events. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_evidence` | `formatter` | Formats provider name, error name, and message into terminal evidence. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_kind` | `mapper` | Maps one parsed structured error line into a terminal-signal kind. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `terminal_signal_kind_from_json_error` | `mapper` | Maps a structured OpenCode error object and normalized message into the terminal signal vocabulary. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_name` | `accessor` | Retrieves the structured error name from supported OpenCode error paths. |

## Test function inventory

| File | Function | A1 class | Meaning |
|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `opencode_terminal_structured_error_exit_zero_carries_failure_reason_evidence` | `validator` | Verifies the incident stream plus real exit `0` yields `exit_code=-1` and provider-error terminal reason evidence. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `opencode_error_event_followed_by_later_event_preserves_clean_exit` | `validator` | Verifies a later stream event after an error preserves clean terminal output. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `evidence_with_status` | `mapper` | Builds recognizer evidence fixtures with caller-selected terminal status. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `terminal_unrelated_error_uses_structured_error_evidence_before_nonzero_exit` | `validator` | Verifies non-quota structured OpenCode errors become `Unknown` with provider evidence instead of plain nonzero exit. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `terminal_structured_error_exit_zero_maps_to_failure_signal_with_incident_evidence` | `validator` | Verifies the incident event maps to `Unknown` with the incident message. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `recovered_session_error_followed_by_later_event_preserves_clean_exit` | `validator` | Verifies last-line-only classification preserves recovered clean output. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `ordinary_output_quota_and_rate_text_preserves_clean_exit` | `validator` | Verifies ordinary text with quota/rate words remains clean on exit `0`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `ordinary_output_quota_and_rate_text_preserves_nonzero_exit` | `validator` | Verifies ordinary text with quota/rate words remains nonzero-exit classified on exit `1`. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_terminal_error_exit_zero_finalizes_one_shot_as_failed` | `validator` | Verifies one-shot result envelope and StateDb row fail honestly for the incident stream. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_error_event_followed_by_later_event_finalizes_one_shot_as_succeeded` | `validator` | Verifies recovered one-shot output remains succeeded. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_terminal_error_exit_zero_finalizes_resume_as_failed` | `validator` | Verifies resume StateDb finalization fails honestly for the incident stream. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_fixture_with_body` | `orchestration` | Builds an isolated CLI fixture with OpenCode model/provider bodies. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_body` | `formatter` | Formats shell fixture body lines and forced `exit 0`. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `assert_invocation_row` | `validator` | Reads and asserts the persisted invocation terminal fields. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs
    role: adapter
    Translates:
      - terminal-signal-classification-contract
      - std-process-exit-status-contract
      - supervised-output-contract
  - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
    role: adapter
    Translates:
      - executor-terminal-signal-dto-contract
      - canonical-terminal-reason-vocabulary
      - std-process-exit-status-contract
      - unix-signal-name-contract
      - signal-hook-forwarding-contract
  - component: crates/oulipoly-runtime/src/executor/providers/opencode.rs
    role: adapter
    Translates:
      - OpenCode json stream event contract
      - Oulipoly terminal-signal-recognizer contract
      - Oulipoly terminal-signal evidence contract
  - component: src-tauri/tests/opencode_terminal_error_exit_zero.rs
    role: adapter
    Translates:
      - Unix CLI integration fixture contract
      - Oulipoly result-envelope JSON contract
      - StateDb invocation terminal fields
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-runtime/src/executor/providers/opencode.rs
    role: intrinsic-surface
    Domain: opencode_terminal_signal_recognition
    Owns:
      - OpenCode structured `type:error` terminal event recognition
      - last non-empty stream line terminality rule
      - provider-error evidence formatting for OpenCode structured errors
      - rate-limit and persistent-quota classification only inside structured error events
      - ordinary output with quota/rate words preserving process-status classification
  - component: crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs
    role: intrinsic-surface
    Domain: supervised_terminal_output_mapping
    Owns:
      - synthetic failure exit code when terminal failure signal coincides with real exit 0
      - real nonzero exit code preservation
      - supervised terminal reason propagation
  - component: crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs
    role: intrinsic-surface
    Domain: terminal_reason_mapping
    Owns:
      - canonical terminal reason mapping by TerminalSignalKind
      - provider-error evidence preservation for Unknown terminal signals
      - std::process::ExitStatus reason mapping for clean/nonzero/signal exits
```
