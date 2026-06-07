# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate`
- `wu_id=oc`
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
- Production Rust source files named in `touched_surfaces_path`

A1 preservation check: `/home/nes/ai/conventions/code-quality.md` contains the single-classification rule at lines 54-58, the category list at lines 60-69, the `Function categories per function` threshold row at line 298, and the `multi-classifier function` failure mode at line 306. The bound row is LOW = 1 and HIGH = >= 2; MEDIUM is n/a.

## Functions In Touched Files

Inventory boundary applied: production Rust functions added or meaningfully changed by the OpenCode P0+P1 diff and enumerated by the Phase 6 contract. Markdown/procedure prose, Rust test functions, re-export/module-only files, `.gitignore`, and `scripts/opencode-turns` are excluded from this A5 function inventory per the caller's production-function scope and explicit non-Rust script exclusion.

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-config/src/model.rs` | `SessionCapture::validate` | lines 243-269 | `validator` | LOW | Accepts or rejects `SessionCapture` by capture kind and required event fields, delegating stdout JSON shape validation before returning `Ok(())` or an error. |
| `crates/oulipoly-config/src/model.rs` | `SessionCapture::validate_stdout_json_event_shape` | lines 271-305 | `validator` | LOW | Enforces exclusive valid shapes for `json_args` or legacy `json_flag`/`last_message_flag` and reports invalid combinations. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs` | `stdout_json_event_capture_args` | lines 30-41 | `formatter` | LOW | Formats capture configuration into provider argv tokens, optionally appending last-message flag/path. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `build_stdout_json_event_capture_plan` | lines 77-102 | `mapper` | LOW | Maps capture config into `CapturePlan`, argv fragments, and temp-file ownership; shape validation and required-field checks are delegated. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `stdout_json_event_shape` | lines 104-141 | `validator` | LOW | Accepts a valid stdout JSON event shape and returns a transformed-valid shape object, or rejects empty/mixed/missing config. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `finalize_capture` | lines 34-55 | `mapper` | LOW | Matches capture plan variants and maps them to the corresponding `SessionCaptureResult` construction path. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `finalize_stdout_json_event_capture` | lines 108-122 | `mapper` | LOW | Maps a streamed session ID or parsed stdout JSON event result into a stdout-json-event capture result. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `maybe_restore_plain_stdout` | lines 141-153 | `orchestration` | LOW | Pure dispatch between sidecar restore helper and stdout fallback based on capture plan shape. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `execute_provider_with_arg_parts_and_supervisor_config` | lines 62-107 | `orchestration` | LOW | Sequences launch assembly, supervisor execution, return-channel cleanup, and raw result construction through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `ProviderRecognizer::for_provider` | lines 86-99 | `mapper` | LOW | Maps provider name/executable prefix into the bounded `ProviderRecognizer` enum, now including `OpenCode`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `ProviderRecognizer::recognize` | lines 101-111 | `orchestration` | LOW | Pure delegating dispatch to provider-specific recognizers. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `raw_result_from_supervised_output` | lines 74-99 | `mapper` | LOW | Maps supervised output, capture outcome, terminal metadata, and returned artifacts into `RawResult`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `run_provider_supervisor` | lines 102-117 | `orchestration` | LOW | Delegates supervisor execution and maps supervisor errors for the executor. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `execute_with_supervisor` | lines 119-188 | `orchestration` | LOW | Sequences child setup, drain loop, live signal checks, streamed-session observation, terminal output mapping, and stdin-error handling through named helpers. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `observe_streamed_session_id` | lines 190-209 | `orchestration` | LOW | Guarded helper dispatch: exits when already observed or wrong capture plan, delegates JSON session parsing, and records the first helper result. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_output_from_terminal` | lines 36-68 | `mapper` | LOW | Maps terminal status/signal/stdout/stderr into `SupervisedOutput`, initializing `streamed_session_id` to `None`. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs` | `output_reports_missing_session` | lines 30-33 | `predicate` | LOW | Answers whether lowercased provider output contains a verified missing-session phrase. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `Recognizer::recognize` | lines 20-38 | `orchestration` | LOW | Sequences pre-quota terminal status handling, OpenCode JSON error recognition, post-quota status handling, and fallback through named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `opencode_json_error_signal` | lines 41-46 | `orchestration` | LOW | Pure helper dispatch over stdout first, then stderr. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_signal_from_stream` | lines 48-57 | `orchestration` | LOW | Pure helper dispatch plus structural loop/control flow: delegates stream decoding, candidate-line filtering, line classification, and evidence formatting to named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `stream_text` | lines 59-61 | `parser` | LOW | Decodes provider output bytes into lossy UTF-8 text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `non_empty_stream_lines` | lines 63-68 | `filter` | LOW | Selects trimmed non-empty candidate lines from decoded stream text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_evidence` | lines 70-72 | `formatter` | LOW | Formats a bounded terminal-signal evidence excerpt from a candidate JSON line. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_kind` | lines 74-79 | `orchestration` | LOW | Pure helper dispatch: delegates JSON parsing, error-event validation, message normalization, and terminal-kind mapping to named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `parse_json_error_line` | lines 81-83 | `parser` | LOW | Parses one provider output line as JSON. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_event_error` | lines 85-90 | `validator` | LOW | Accepts only OpenCode `type = "error"` events and returns their `error` object. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `normalized_error_message` | lines 92-94 | `formatter` | LOW | Formats the provider error message into lowercase text for predicate matching. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `terminal_signal_kind_from_json_error` | lines 96-107 | `mapper` | LOW | Maps classified OpenCode error facts from predicate helpers into runner terminal-signal kinds. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_status_code` | lines 109-120 | `accessor` | LOW | Retrieves the first supported status-code field path and delegates numeric conversion. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_message` | lines 122-127 | `accessor` | LOW | Retrieves the supported OpenCode error message field path and returns owned text without deciding terminal meaning. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `value_at_paths` | lines 129-131 | `accessor` | LOW | Retrieves the first JSON pointer value from a path list without changing meaning. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `number_or_numeric_string` | lines 133-137 | `parser` | LOW | Parses a numeric JSON value or numeric string into an integer status code. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_reports_rate_limit` | lines 139-141 | `predicate` | LOW | Answers whether status code or message text reports a rate limit. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `message_reports_rate_limit` | lines 143-145 | `predicate` | LOW | Answers whether lowercased provider message text reports rate limiting. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_reports_persistent_quota` | lines 147-154 | `predicate` | LOW | Answers whether lowercased provider message text reports persistent quota exhaustion. |
| `crates/oulipoly-state/src/db.rs` | `StateDb::resolve_resume` | lines 6293-6313 | `orchestration` | LOW | Sequences strict input validation, wrong-ID-kind rejection, chain lookup, active segment lookup, model resolution, and result assembly through named helpers. |
| `crates/oulipoly-state/src/db.rs` | `StateDb::validate_resume_input_id` | lines 6315-6323 | `validator` | LOW | Accepts UUID or strict OpenCode provider-session grammar and returns `Ok(())`, otherwise reports `ResumeError::InvalidUuid`. |
| `crates/oulipoly-state/src/db.rs` | `StateDb::is_opencode_provider_session_id` | lines 6325-6332 | `predicate` | LOW | Answers whether an input has the `ses_` prefix plus minimum-length alphanumeric suffix. |
| `src-tauri/src/error_emit.rs` | `invalid_session_id_message` | lines 260-262 | `formatter` | LOW | Formats provider-session-neutral invalid resume id text. |
| `src-tauri/src/resume_cli.rs` | `format_resume_error` | lines 161-236 | `formatter` | LOW | Formats `ResumeError` variants into CLI stderr text, including invalid id wording and resume hints. |
| `src-tauri/src/run/repl/orchestration.rs` | `run_repl` | lines 39-118 | `orchestration` | LOW | Delegates early resume validation and sequences REPL preparation, provider selection, invocation start, resume binding, and final execution through named helpers. |
| `src-tauri/src/run/resume/orchestration.rs` | `reject_invalid_resume_input` | lines 88-96 | `orchestration` | LOW | Delegates validation and routes failure to stderr plus exit code. |
| `src-tauri/src/run/resume/validator.rs` | `validate_resume_input` | lines 8-16 | `validator` | LOW | Accepts nonblank UUID or strict OpenCode provider-session id input and reports validation messages for blank or malformed input. |
| `src-tauri/src/run/resume/validator.rs` | `is_opencode_provider_session_id` | lines 18-25 | `predicate` | LOW | Answers whether an input has the `ses_` prefix plus minimum-length alphanumeric suffix. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | No admitted production Rust function in the touched inventory classified as two or more A1 categories after applying the pure-orchestrator body-shape rule. | n/a | n/a | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

- No `NEEDS_INPUT` or `BLOCKED` condition fired: `code_quality_ref`, `contract_path`, `proposal_path`, `diff_path`, `touched_surfaces_path`, and the inspected production Rust source files were readable.
- Touched Rust files with no admitted production function inventory for this pass: `crates/oulipoly-config/src/providers.rs`, `crates/oulipoly-runtime/src/executor/cli.rs`, `crates/oulipoly-runtime/src/executor/mod.rs`, `crates/oulipoly-runtime/src/executor/providers/mod.rs`, and `crates/oulipoly-runtime/src/executor/terminal_signal.rs` because the diff only changed tests, comments, exports, module declarations, or data fields there.
- `scripts/opencode-turns` is a non-Rust shell/Python adapter and was explicitly declared out of the A5 Rust production function inventory for this audit.
- Markdown/document files in the diff, including `.gitignore`, `DECISIONS.md`, and prior planning reports, do not define executable product functions admitted to this A5 inventory.

Verdict: LOW

VERDICT: LOW
