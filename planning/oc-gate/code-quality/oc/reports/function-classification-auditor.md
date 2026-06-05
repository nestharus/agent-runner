# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/function-classification-auditor.md`
- `mode=phase-6`

## References Read

- `/home/nes/ai/conventions/code-quality.md`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md`

A1 preservation check: `/home/nes/ai/conventions/code-quality.md` contains the single-classification rule at lines 54-58, the category list at lines 60-69, the `Function categories per function` threshold row at line 298, and the `multi-classifier function` failure mode at line 306.

## Functions In Touched Files

Inventory boundary applied: production Rust functions added or changed by the OpenCode P0+P1 diff and enumerated by the Phase 6 contract. Markdown/procedure prose, Rust test functions, re-export/module-only files, `.gitignore`, and `scripts/opencode-turns` are excluded from this A5 function inventory per the caller's production-function scope and explicit script exclusion.

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-config/src/model.rs` | `SessionCapture::validate` | lines 243-282 | `validator` | LOW | Accepts/rejects `SessionCapture` by kind and required fields; returns `Ok(())` or validation error strings. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs` | `stdout_json_event_capture_args` | lines 30-41 | `formatter` | LOW | Formats capture config into argv tokens, optionally appending last-message flag/path. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `build_stdout_json_event_capture_plan` | lines 72-97 | `mapper` | LOW | Maps validated capture config into `CapturePlan`, argv fragments, and temp-file ownership; validation is delegated to helpers. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `stdout_json_event_json_args` | lines 99-110 | `validator` | LOW | Accepts non-empty `json_args` or legacy `json_flag`, rejects missing/empty config, and returns the accepted argv fragment. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `finalize_capture` | lines 34-55 | `mapper` | LOW | Matches capture plan variants and delegates construction of the corresponding `SessionCaptureResult`. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `finalize_stdout_json_event_capture` | lines 108-122 | `mapper` | LOW | Maps streamed session ID or parsed stdout JSON event result into stdout-json-event capture result. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `maybe_restore_plain_stdout` | lines 141-153 | `orchestration` | LOW | Pure dispatch between sidecar restore helper and stdout fallback based on capture plan shape. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `execute_provider_with_arg_parts_and_supervisor_config` | lines 62-107 | `orchestration` | LOW | Sequences launch assembly, supervisor execution, return-channel cleanup, and raw result construction through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `ProviderRecognizer::for_provider` | lines 86-99 | `mapper` | LOW | Maps provider name/executable prefix into the bounded `ProviderRecognizer` enum, now including `OpenCode`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `ProviderRecognizer::recognize` | lines 101-111 | `orchestration` | LOW | Pure delegating dispatch to provider-specific recognizers. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `raw_result_from_supervised_output` | lines 74-99 | `mapper` | LOW | Maps supervised output, capture outcome, terminal metadata, and returned artifacts into `RawResult`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `run_provider_supervisor` | lines 102-117 | `orchestration` | LOW | Delegates supervisor execution and maps supervisor errors for the executor. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `execute_with_supervisor` | lines 119-188 | `orchestration` | LOW | Sequences child setup, drain loop, live signal checks, streamed-session observation, terminal output mapping, and stdin-error handling through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `observe_streamed_session_id` | lines 190-209 | `orchestration` | LOW | Guarded helper dispatch: exits when already observed or wrong capture plan, delegates JSON session parsing, records first helper result. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_output_from_terminal` | lines 36-68 | `mapper` | LOW | Maps terminal status/signal/stdout/stderr into `SupervisedOutput`, initializing `streamed_session_id` to `None`. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs` | `output_reports_missing_session` | lines 33-41 | `predicate` | LOW | Answers whether lowercased provider output contains any known missing-session phrase. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `Recognizer::recognize` | lines 19-37 | `orchestration` | LOW | Sequences pre-quota terminal status handling, OpenCode JSON error recognition, post-quota status handling, and fallback through named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `opencode_json_error_signal` | lines 40-45 | `orchestration` | LOW | Pure helper dispatch over stdout first, then stderr. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_signal_from_stream` | lines 47-59 | `parser`, `filter`, `formatter` | HIGH | Decodes bytes to text at line 48, filters non-empty lines at line 49, and formats bounded evidence at lines 53-56 in the same body. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_kind` | lines 61-77 | `parser`, `validator`, `predicate`, `mapper` | HIGH | Parses JSON at line 62, validates event shape at lines 63-65, runs rate/quota predicates at lines 70 and 73, and maps to terminal signal kinds at lines 71 and 74. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_status_code` | lines 79-90 | `accessor` | LOW | Retrieves the first supported status-code field path and delegates numeric conversion. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_message` | lines 92-97 | `accessor` | LOW | Retrieves supported message paths and returns owned text without deciding terminal meaning. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `value_at_paths` | lines 99-101 | `accessor` | LOW | Retrieves the first JSON pointer value from a path list. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `number_or_numeric_string` | lines 103-107 | `parser` | LOW | Parses numeric JSON or numeric string into `i64`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `message_reports_rate_limit` | lines 109-111 | `predicate` | LOW | Answers whether message text reports rate limiting. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `message_reports_persistent_quota` | lines 113-120 | `predicate` | LOW | Answers whether message text reports persistent quota exhaustion. |
| `crates/oulipoly-state/src/db.rs` | `StateDb::resolve_resume` | lines 6290-6309 | `orchestration` | LOW | Sequences wrong-ID-kind rejection, chain lookup, active segment lookup, model resolution, and result assembly through named helpers. |
| `src-tauri/src/run/resume/orchestration.rs` | `reject_invalid_resume_input` | lines 88-96 | `orchestration` | LOW | Delegates validation and routes failure to stderr plus exit code. |
| `src-tauri/src/run/resume/validator.rs` | `validate_resume_input` | lines 3-8 | `validator` | LOW | Accepts non-empty resume input and rejects blank input with a validation message. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| FC-001 | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_signal_from_stream` | `parser`, `filter`, `formatter` | Line 48 decodes bytes to text; line 49 filters lines; lines 53-56 format bounded terminal evidence while returning the parsed kind. | Split stream decoding/candidate-line selection, terminal-kind lookup, and evidence-excerpt formatting into separate responsibility boundaries; leave this function as pure orchestration over those helpers. | blocking | changed_function | same_domain |
| FC-002 | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_kind` | `parser`, `validator`, `predicate`, `mapper` | Line 62 parses JSON; lines 63-65 validate `type = error`; lines 70 and 73 run rate/quota predicates; lines 71 and 74 map to `TerminalSignalKind`. | Split JSON parse/event-shape validation from terminal-kind classification; keep message/status predicates and terminal-kind mapping as separately named single-class helpers. | blocking | changed_function | same_domain |

```yaml
- id: FC-001
  path: crates/oulipoly-runtime/src/executor/providers/opencode.rs
  function: json_error_signal_from_stream
  line_span_or_diff_hunk: lines 47-59
  categories_mixed: [parser, filter, formatter]
  evidence: "Line 48 decodes bytes into text, line 49 filters non-empty candidate lines, and lines 53-56 format a bounded evidence excerpt while returning the parsed terminal kind."
  failure_mode: multi-classifier function
  blocking_or_residual: blocking
  finding_origin: changed_function
  domain_relation: same_domain
  suggested_split:
    direction: "Separate stream decoding/candidate-line filtering, terminal-kind lookup, and evidence-excerpt formatting; the current function can then become pure orchestration that calls those single-role helpers."
    convergence_proof:
      current_blocking_finding: "FC-001 on json_error_signal_from_stream"
      why_split_reduces_blocking_set: "The mixed parser/filter/formatter operations would no longer coexist in one body; the remaining wrapper would dispatch already-named helpers and satisfy the pure-orchestrator rule."
      helper_overlay_handling: "Any introduced helpers stay in the touched file/component and must be audited under A5 with one category each, such as parser for decoding, filter for candidate selection, and formatter for excerpt shaping."
- id: FC-002
  path: crates/oulipoly-runtime/src/executor/providers/opencode.rs
  function: json_error_line_kind
  line_span_or_diff_hunk: lines 61-77
  categories_mixed: [parser, validator, predicate, mapper]
  evidence: "Line 62 parses JSON, lines 63-65 validate the event shape, lines 70 and 73 predicate over status/message conditions, and lines 71 and 74 map recognized conditions to terminal signal kinds."
  failure_mode: multi-classifier function
  blocking_or_residual: blocking
  finding_origin: changed_function
  domain_relation: same_domain
  suggested_split:
    direction: "Separate JSON parsing and error-event validation from terminal-kind classification, with existing or extracted predicate helpers answering rate-limit/quota questions and a mapper converting classified conditions to `TerminalSignalKind`."
    convergence_proof:
      current_blocking_finding: "FC-002 on json_error_line_kind"
      why_split_reduces_blocking_set: "The current parser/validator/predicate/mapper body would be replaced by single-role helpers plus a thin dispatcher, eliminating the multi-classifier function."
      helper_overlay_handling: "Introduced helpers remain subject to the same A5 overlay; parser, validator, predicate, and mapper responsibilities must stay in distinct helper bodies rather than being re-consolidated."
```

## Residual Ambiguity / Stop-Condition Notes

- No `NEEDS_INPUT` condition: the two HIGH findings are in changed production Rust and have line-cited body evidence.
- Touched files with no admitted production function inventory for this pass: `.gitignore`, `crates/oulipoly-config/src/providers.rs`, `crates/oulipoly-runtime/src/executor/cli.rs`, `crates/oulipoly-runtime/src/executor/mod.rs`, `crates/oulipoly-runtime/src/executor/providers/mod.rs`, `crates/oulipoly-runtime/src/executor/terminal_signal.rs`, and Rust integration/characterization test files touched only for coverage.
- `scripts/opencode-turns` contains executable Python functions, but the caller explicitly declared it out of the A5 Rust production function inventory for this audit. It is therefore excluded rather than scored.

Verdict: HIGH

VERDICT: HIGH
