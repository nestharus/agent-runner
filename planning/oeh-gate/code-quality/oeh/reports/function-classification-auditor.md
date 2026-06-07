# Function Classification Audit

## Inputs Read

| Input | Value |
|---|---|
| mode | phase-6 |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/function-classification-auditor.md` |
| base | `549daaa` |
| head | `HEAD` / `48bf5c1` |
| original_head | `3515d31` |
| wu_id | `oeh` |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | A1 category list, single-classification rule, `Function categories per function` row, `multi-classifier function` failure mode, Auditor Scope Boundary, and Touched-file ownership were present and non-contradictory. |
| `planning/oeh-gate/contracts/oeh.contract.md` | Phase 6 contract read before scoring; component and touched-file declared roles, focused inventory, adapter declarations, and intrinsic-surface declarations were available. |
| `planning/oeh-gate/proposal.md` | OEH context read before scoring; OpenCode terminal structured error honesty and supervised exit-zero failure finalization are the WU behavior surfaces. |
| `planning/oeh-gate/gates/diff.patch` | Touched-file discovery and changed-function evidence. |
| `planning/oeh-gate/gates/touched-files.txt` | Touched-file confirmation. |
| `planning/oeh-gate/evidence/runtime-tests.log` | Runtime evidence reference read; classification scoring did not depend on test success. |

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `.gitignore` | No admitted executable function-like symbols | diff hunk lines 49-52 | n/a | LOW | Ignore-pattern carrier only; no executable function body. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `terminal_outcome_from_status` | lines 28-34 | `mapper` | LOW | Maps an `ExitStatus` into the supervised terminal outcome tuple. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_output_from_terminal` | lines 36-65; diff hunk lines 14-29 | `mapper` | LOW | Maps provider output, terminal status, optional terminal signal, and optional real status into `SupervisedOutput`; helper roles are not attributed to the caller. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | closure in `terminal_signal.unwrap_or_else` | lines 45-53 | `orchestration` | LOW | Pure helper dispatch to `recognize_terminal_signal` with captured arguments. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_exit_code` | lines 67-77; diff hunk lines 31-41 | `mapper` | LOW | Maps terminal signal plus optional real status to the final supervised exit code, including synthetic failure substitution. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `tests::opencode_terminal_structured_error_exit_zero_carries_failure_reason_evidence` | lines 89-112; diff hunk lines 51-76 | `validator` | LOW | Asserts incident output has synthetic failure code, `Unknown` signal, and provider-error reason evidence. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `tests::opencode_error_event_followed_by_later_event_preserves_clean_exit` | lines 116-138; diff hunk lines 78-102 | `validator` | LOW | Asserts recovered stream output keeps clean exit and no terminal reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `classify_terminal_reason` | lines 65-80 | `mapper` | LOW | Maps `ExitStatus` outcomes to the stable terminal reason vocabulary. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | closure in `then` | line 67 | `formatter` | LOW | Formats the nonzero-exit reason literal as `String`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `signal_name` | lines 83-108 | `formatter` | LOW | Formats Unix signal numbers into signal-name strings. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `exit_code_from_status` | lines 120-134 | `mapper` | LOW | Maps child `ExitStatus` into runtime `i32` exit-code representation. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_status_from_exit_status` | lines 136-151 | `mapper` | LOW | Maps child `ExitStatus` into `TerminalStatusEvidence`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_reason_from_signal` | lines 153-170; diff hunk lines 117-131 | `mapper` | LOW | Maps terminal signal kind plus optional status into terminal reason, delegating `Unknown` handling to a named helper. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `unknown_terminal_reason` | lines 172-178; diff hunk lines 133-139 | `mapper` | LOW | Maps `Unknown` terminal signal evidence to provider-error reason only when evidence carries the provider-error prefix; otherwise maps to canonical reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `synthetic_exit_code` | lines 180-192 | `mapper` | LOW | Maps terminal signal kind into synthetic exit-code value. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `recognize_terminal_signal` | lines 194-203 | `orchestration` | LOW | Pure helper dispatch: build evidence then call provider recognizer. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_signal_evidence` | lines 205-218 | `mapper` | LOW | Constructs `TerminalSignalEvidence` from provider output and status inputs. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_signal_for_spawn_error` | lines 221-231 | `orchestration` | LOW | Test-only helper dispatch to `recognize_terminal_signal` with spawn-error status. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install` | lines 241-243 | `orchestration` | LOW | Delegates child-pid target installation to `install_for_target`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_process_group` | lines 245-247 | `orchestration` | LOW | Delegates process-group target installation to `install_for_target`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_for_target` | lines 249-258 | `orchestration` | LOW | Sequences signal installation, handle capture, forwarding-thread spawn, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `install_interactive_signals` | lines 262-264 | `orchestration` | LOW | Single dispatch to `Signals::new` with named error mapping. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `signal_install_error` | lines 267-269 | `formatter` | LOW | Formats signal-installation error text. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `child_signal_pid` | lines 272-274 | `accessor` | LOW | Retrieves child process id as signal pid. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `spawn_interactive_signal_thread` | lines 284-294 | `orchestration` | LOW | Sequences atomic flag setup and forwarding-thread spawn. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | closure in `thread::spawn` | lines 291-293 | `orchestration` | LOW | Pure dispatch to `forward_interactive_signals` inside spawned thread. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `forward_interactive_signals` | lines 297-307 | `orchestration` | LOW | Iterates received signals and dispatches predicate/send helpers; helper classifications are not attributed. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `should_forward_interactive_signal` | lines 310-324 | `predicate` | LOW | Answers whether a specific signal should be forwarded for the selected target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `should_forward_interactive_sigterm` | lines 327-329 | `predicate` | LOW | Answers whether SIGTERM should be forwarded while updating the atomic one-shot guard. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::drop` | lines 333-339 | `orchestration` | LOW | Sequences signal-handler close and optional thread join. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `send_signal` | lines 342-352 | `orchestration` | LOW | Performs one signal-forwarding operation: select target pid shape and invoke the OS signal dispatch. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `tests::exit_code_from_status_uses_unified_child_process_contract` | lines 360-374 | `validator` | LOW | Asserts exit-code mapping contract for success, nonzero, signal, and unknown statuses. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `Recognizer::recognize` | lines 20-38 | `orchestration` | LOW | Sequences pre-quota, structured-error, post-quota, and fallback recognizer helpers with structural `if let` dispatch. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in first `unwrap_or_else` | lines 22-23 | `formatter` | LOW | Supplies fallback evidence string literal. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in second `unwrap_or_else` | lines 32-33 | `formatter` | LOW | Supplies fallback evidence string literal. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `opencode_json_error_signal` | lines 41-46 | `orchestration` | LOW | Dispatches structured-error recognition across stdout then stderr helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `or_else` | line 45 | `orchestration` | LOW | Pure helper dispatch to inspect stderr when stdout has no JSON error signal. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_signal_from_stream` | lines 48-54; diff hunk lines 150-163 | `mapper` | LOW | Maps stream bytes to optional terminal signal kind and evidence using last non-empty line. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `stream_text` | lines 56-58 | `parser` | LOW | Decodes byte stream into UTF-8-lossy text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `non_empty_stream_lines` | lines 60-62; diff hunk lines 169-175 | `orchestration` | LOW | Pure helper dispatch from trimming helper into non-empty-line retention helper. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `trimmed_stream_lines` | lines 64-66; diff hunk lines 177-179 | `mapper` | LOW | Maps raw stream lines into trimmed line slices. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `retain_non_empty_lines` | lines 68-70; diff hunk lines 181-183 | `filter` | LOW | Retains only non-empty line slices from an existing collection. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `retain_non_empty_lines` | line 69 | `predicate` | LOW | Answers whether one line is non-empty for the filter helper. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_evidence` | lines 72-77; diff hunk lines 185-191 | `formatter` | LOW | Produces bounded provider-error evidence text, delegating JSON parsing and evidence construction to named helpers. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `json_error_line_evidence.and_then` | line 74 | `mapper` | LOW | Maps parsed JSON value into optional provider-error evidence through a named helper. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `json_error_line_evidence.unwrap_or_else` | line 75 | `formatter` | LOW | Formats fallback evidence by cloning the original line text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_evidence_from_value` | lines 79-82; diff hunk lines 193-196 | `mapper` | LOW | Maps parsed JSON event value into optional provider-error evidence. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_evidence` | lines 84-91; diff hunk lines 198-205 | `formatter` | LOW | Formats OpenCode provider-error evidence from error name and message. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_line_kind` | lines 93-98 | `mapper` | LOW | Maps a parsed structured error line to terminal signal kind. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `parse_json_error_line` | lines 100-102 | `parser` | LOW | Parses one JSON line into `serde_json::Value`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `json_error_event_error` | lines 104-109 | `validator` | LOW | Accepts only `type:error` event values and returns the valid error payload. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `normalized_error_message` | lines 111-113 | `mapper` | LOW | Maps error payload to lowercase internal comparison text. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `terminal_signal_kind_from_json_error` | lines 115-126; diff hunk lines 208-214 | `mapper` | LOW | Maps structured error predicates into terminal signal vocabulary, including fallback `Unknown`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_status_code` | lines 128-139 | `accessor` | LOW | Retrieves status code from supported JSON paths and delegates numeric-string conversion. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_message` | lines 141-146 | `accessor` | LOW | Retrieves structured error message from supported JSON paths. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_name` | lines 148-150; diff hunk lines 221-223 | `accessor` | LOW | Retrieves structured error name from supported JSON paths. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `value_at_paths` | lines 152-154 | `accessor` | LOW | Retrieves first JSON value found at supported paths. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `value_at_paths.find_map` | line 153 | `accessor` | LOW | Retrieves value at one JSON pointer path for `find_map`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `number_or_numeric_string` | lines 156-160 | `parser` | LOW | Parses numeric string values to `i64` while accepting already-numeric JSON values. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | closure in `number_or_numeric_string.and_then` | line 159 | `parser` | LOW | Parses one string as `i64`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_reports_rate_limit` | lines 162-164 | `predicate` | LOW | Answers whether an error status/message reports rate limiting. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `message_reports_rate_limit` | lines 166-168 | `predicate` | LOW | Answers whether normalized message contains rate-limit terms. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `error_reports_persistent_quota` | lines 170-177 | `predicate` | LOW | Answers whether normalized message reports persistent quota exhaustion. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::evidence` | lines 190-192 | `mapper` | LOW | Maps stdout/stderr fixture bytes into recognizer evidence with default status. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::evidence_with_status` | lines 194-206; diff hunk lines 238-251 | `mapper` | LOW | Constructs `TerminalSignalEvidence` from fixture fields and caller-selected terminal status. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::status_code_429_maps_to_rate_limited` | lines 209-216 | `validator` | LOW | Asserts status-code 429 maps to `RateLimited`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::persistent_quota_message_maps_to_quota_exhausted` | lines 219-226 | `validator` | LOW | Asserts persistent quota message maps to `QuotaExhaustedInband`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::terminal_unrelated_error_uses_structured_error_evidence_before_nonzero_exit` | lines 229-237; diff hunk lines 255-265 | `validator` | LOW | Asserts unrelated structured error maps to `Unknown` with structured evidence. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::terminal_structured_error_exit_zero_maps_to_failure_signal_with_incident_evidence` | lines 240-253; diff hunk lines 267-281 | `validator` | LOW | Asserts incident error event maps to `Unknown` and retains incident message evidence. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::recovered_session_error_followed_by_later_event_preserves_clean_exit` | lines 256-269; diff hunk lines 283-297 | `validator` | LOW | Asserts later stream event after an error preserves clean exit. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::ordinary_output_quota_and_rate_text_preserves_clean_exit` | lines 272-280; diff hunk lines 299-308 | `validator` | LOW | Asserts ordinary quota/rate words remain clean-exit classified on exit 0. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `tests::ordinary_output_quota_and_rate_text_preserves_nonzero_exit` | lines 283-291; diff hunk lines 310-319 | `validator` | LOW | Asserts ordinary quota/rate words remain nonzero-exit classified on exit 1. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_terminal_error_exit_zero_finalizes_one_shot_as_failed` | lines 17-39; diff hunk lines 342-365 | `validator` | LOW | Asserts one-shot process/result envelope and invocation row fail honestly for incident stream. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_error_event_followed_by_later_event_finalizes_one_shot_as_succeeded` | lines 42-58; diff hunk lines 367-384 | `validator` | LOW | Asserts recovered one-shot output and row remain succeeded. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_terminal_error_exit_zero_finalizes_resume_as_failed` | lines 61-82; diff hunk lines 386-408 | `validator` | LOW | Asserts resume invocation row fails honestly for incident stream. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_fixture_with_body` | lines 84-89; diff hunk lines 410-415 | `orchestration` | LOW | Sequences fixture creation plus model/provider setup helpers. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `opencode_body` | lines 91-99; diff hunk lines 417-425 | `formatter` | LOW | Formats shell fixture body lines and forced `exit 0`. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | closure in `opencode_body.map` | line 94 | `formatter` | LOW | Formats one shell `printf` line. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `fetch_invocation_row` | lines 101-112; diff hunk lines 427-438 | `accessor` | LOW | Retrieves persisted invocation fields from the fixture database. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | closure in `query_row` | line 109 | `accessor` | LOW | Retrieves selected row columns into a tuple. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `assert_invocation_row` | lines 114-127; diff hunk lines 440-453 | `validator` | LOW | Asserts persisted invocation status, success, exit code, and terminal reason match expectations. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | No admitted executable function-like symbol in a touched file inferred two or more A1 categories after re-evaluating helper-dispatch bodies under the pure-orchestrator rule. | n/a | n/a | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

The A1 metric source was readable and preserved: categories are `orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, and `parser`; the single-classification rule and `Function categories per function` LOW `1` / HIGH `>= 2` row are present; `multi-classifier function` is present in failure modes.

`planning/oeh-gate/gates/diff.patch` was usable as change evidence. Touched files resolved to `.gitignore`, `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs`, `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs`, `crates/oulipoly-runtime/src/executor/providers/opencode.rs`, and `src-tauri/tests/opencode_terminal_error_exit_zero.rs`.

Markdown sections, contract/proposal prose, diff carriers, runtime evidence logs, and `.gitignore` ignore patterns were excluded from A5 function inventory because they do not define executable function-like symbols with inspectable bodies.

VERDICT: LOW
