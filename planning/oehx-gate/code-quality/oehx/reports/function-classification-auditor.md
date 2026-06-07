# Function Classification Audit

## Inputs Read

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=33775d7`
- `head_ref=HEAD`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- `risk_profile_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/multi-classifier-risk.md` attempted; file was not present.
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/function-classification-auditor.md`

## References Read

- `/home/nes/ai/agents/function-classification-auditor.md`
- `/home/nes/ai/conventions/code-quality.md`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- Touched source files under `worktree_path`: `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs`, `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs`, `crates/oulipoly-runtime/src/executor/cli.rs`, `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs`, `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs`, `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs`, `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs`, `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs`, `src-tauri/tests/s10_external_provider_resume.rs`.

Verified A1 preservation: `code-quality.md` contains the A1 category list (`orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`), the single-classification rule, the `Function categories per function` threshold row (`LOW = 1`, `MEDIUM = n/a`, `HIGH = >= 2`), and the `multi-classifier function` failure mode.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `fixed_reason_for_kind` | line 5 | formatter | LOW | Matches `TerminalSignalKind` to stable terminal-reason text only. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `map_terminal_classify_result` | line 12 | mapper | LOW | Converts provider classify result plus request status into `TerminalClassification`. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `terminal_reason` | line 34 | mapper | LOW | Maps provider process status and runtime signal evidence to optional terminal reason. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `runtime_kind` | line 51 | mapper | LOW | Maps provider terminal-signal enum variants to runtime enum variants. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `signal_evidence` | line 69 | mapper | LOW | Maps optional provider evidence to concrete runtime evidence string. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `tests::request` | line 87 | mapper | LOW | Builds a test service request fixture from process status. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `tests::result` | line 99 | mapper | LOW | Builds a test classify result fixture from signal kind/evidence. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `tests::unknown_provider_error_signal_with_exit_zero_maps_synthetic_failure_and_reason` | line 110 | validator | LOW | Asserts synthetic failure exit and preserved provider-error reason. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `tests::clean_exit_signal_with_exit_zero_stays_success_without_reason` | line 129 | validator | LOW | Asserts clean exit remains successful with no reason. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `tests::unknown_provider_error_signal_with_real_nonzero_preserves_real_code` | line 141 | validator | LOW | Asserts real nonzero code is preserved while reason remains present. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::fixture_script` | line 138 | mapper | LOW | Materializes a shell-script fixture from body text into `FixtureScript`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::age141_supervisor_config` | line 152 | mapper | LOW | Builds a fixed `SupervisorConfig` fixture. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::age141_model_for_provider` | line 160 | mapper | LOW | Builds `ModelConfig` fixture from provider and prompt mode. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::age141_provider` | line 170 | mapper | LOW | Builds `ProviderConfig` fixture from script path. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::age141_execute_script_with_config` | line 187 | orchestration | LOW | Sequences fixture model/provider construction and executor helper dispatch. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::age141_signal` | line 212 | validator | LOW | Asserts terminal-signal presence/kind and returns accepted evidence. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t05_interactive_silent_child_does_not_use_headless_helper` | line 225 | validator | LOW | Asserts interactive clean-exit behavior. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t06_repl_interactive_posture_has_no_idle_timeout` | line 237 | validator | LOW | Asserts exit code and elapsed wait posture. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t07_terminal_signal_clean_exit` | line 253 | validator | LOW | Asserts clean terminal signal and no reason. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t08_terminal_signal_nonzero_exit` | line 265 | validator | LOW | Asserts nonzero terminal signal, code, and reason. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t09_terminal_signal_unix_signal_exit` | line 277 | validator | LOW | Asserts Unix signal exit code, reason, and signal kind. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t10_terminal_signal_spawn_error_preserves_public_error` | line 289 | validator | LOW | Asserts spawn-error message and signal fields. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t12_legacy_quota_text_preserves_clean_and_nonzero_exit` | line 330 | validator | LOW | Asserts legacy quota text does not override terminal classification. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t14_binary_stdout_preserved_under_supervisor` | line 364 | validator | LOW | Asserts binary stdout preservation and clean signal. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `tests::t17_session_capture_normal_drain_carries_clean_signal` | line 375 | validator | LOW | Asserts session capture and clean terminal signal behavior. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `terminal_outcome_from_status` | line 28 | mapper | LOW | Maps `ExitStatus` into supervised terminal outcome tuple. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_output_from_terminal` | line 36 | mapper | LOW | Maps terminal status, optional signal, stdout/stderr, and exit status into `SupervisedOutput`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_exit_code` | line 67 | mapper | LOW | Maps terminal signal plus optional real status to final exit code. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `tests::opencode_terminal_structured_error_exit_zero_carries_failure_reason_evidence` | line 85 | validator | LOW | Asserts provider error event yields failure code and reason evidence. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `tests::opencode_error_event_followed_by_later_event_preserves_clean_exit` | line 112 | validator | LOW | Asserts later clean event preserves clean exit and no reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `classify_terminal_reason` | line 66 | mapper | LOW | Maps child `ExitStatus` to optional stable terminal reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `signal_name` | line 84 | formatter | LOW | Formats Unix signal number as signal name text. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `signal_name` | line 112 | formatter | LOW | Formats non-Unix signal number as text. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `exit_code_from_status` | line 126 | mapper | LOW | Maps `ExitStatus` to runtime integer exit code. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_status_from_exit_status` | line 142 | mapper | LOW | Maps `ExitStatus` to `TerminalStatusEvidence`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_reason_from_signal` | line 159 | mapper | LOW | Adapts `ExitStatus` into status evidence and delegates reason mapping. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_reason_from_signal_status` | line 167 | mapper | LOW | Maps terminal signal kind and status evidence to canonical reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_status_reason` | line 186 | mapper | LOW | Maps terminal status evidence to status-derived reason. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `unknown_terminal_reason` | line 198 | mapper | LOW | Maps unknown signal evidence to provider-error reason or canonical fallback. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `synthetic_exit_code` | line 206 | mapper | LOW | Maps terminal signal kind to synthetic exit code. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_exit_code_from_signal` | line 220 | mapper | LOW | Maps terminal signal plus real code to final code under failure override rule. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `recognize_terminal_signal` | line 228 | orchestration | LOW | Builds named evidence and dispatches recognizer without inline classification. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_signal_evidence` | line 239 | mapper | LOW | Builds `TerminalSignalEvidence` DTO from provider streams and status. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `terminal_signal_for_spawn_error` | line 255 | mapper | LOW | Builds spawn-error terminal signal fixture through recognizer helper. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install` | line 275 | orchestration | LOW | Dispatches guard installation for child PID target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_process_group` | line 279 | orchestration | LOW | Dispatches guard installation for process-group target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::install_for_target` | line 283 | orchestration | LOW | Sequences signal iterator, handle, forwarding thread, and guard construction. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `install_interactive_signals` | line 296 | orchestration | LOW | Installs named interactive signals and maps only installation error helper. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `signal_install_error` | line 301 | formatter | LOW | Formats signal-handler installation error message. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `child_signal_pid` | line 306 | accessor | LOW | Retrieves child process id as signal pid. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `spawn_interactive_signal_thread` | line 318 | orchestration | LOW | Creates shared flag and launches forwarding thread. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `forward_interactive_signals` | line 331 | orchestration | LOW | Iterates signal stream and dispatches predicate/send helpers. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `should_forward_interactive_signal` | line 344 | predicate | LOW | Answers whether a signal should be forwarded for a target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `should_forward_interactive_sigterm` | line 361 | predicate | LOW | Answers whether SIGTERM should be forwarded once. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `InteractiveSignalGuard::drop` | line 367 | orchestration | LOW | Closes signal handle and joins forwarding thread. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `send_signal` | line 376 | orchestration | LOW | Sends the given signal to the selected child/process-group target. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `tests::exit_code_from_status_uses_unified_child_process_contract` | line 394 | validator | LOW | Asserts exit-code mapping contract for success, nonzero, signal, and fallback statuses. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `map_terminal_cancel_outcome` | line 19 | mapper | LOW | Maps provider cancel status/signal into `TerminalCancelOutcome`. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `exit_code` | line 40 | mapper | LOW | Maps provider `ProcessStatus` to status-derived exit code. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `terminal_reason` | line 51 | mapper | LOW | Maps provider status and runtime signal to terminal reason. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `terminal_status_evidence` | line 65 | mapper | LOW | Maps provider status into runtime terminal status evidence. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `terminal_signal_kind` | line 78 | mapper | LOW | Maps provider terminal-signal enum to runtime enum. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `tests::provider_signal` | line 101 | mapper | LOW | Builds provider terminal-signal fixture. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `tests::unknown_provider_error_signal_with_exit_zero_maps_synthetic_failure_and_reason` | line 113 | validator | LOW | Asserts synthetic failure and preserved reason for exit-zero provider error. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `tests::clean_exit_signal_with_exit_zero_stays_success_without_reason` | line 135 | validator | LOW | Asserts clean exit stays successful without reason. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `tests::unknown_provider_error_signal_with_real_nonzero_preserves_real_code` | line 147 | validator | LOW | Asserts real nonzero code preservation with provider-error reason. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `EnvScope::set` | line 77 | mapper | LOW | Maps required env pairs into optional env pairs and delegates scope setup. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `EnvScope::set_optional` | line 86 | mapper | LOW | Maps env overrides to restoration scope with previous values. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `EnvScope::drop` | line 99 | orchestration | LOW | Restores saved environment entries on drop. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `env_lock` | line 106 | accessor | LOW | Retrieves process-wide environment mutex guard. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `set_env` | line 113 | orchestration | LOW | Applies one environment set/remove operation under caller-held lock. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `fixture_script` | line 124 | mapper | LOW | Materializes script body into an executable `ScriptFixture`. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `write_executable` | line 134 | orchestration | LOW | Writes file and applies executable permissions. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `provider_ref_path` | line 141 | mapper | LOW | Builds path-flavored provider implementation ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `provider_ref_binary` | line 151 | mapper | LOW | Builds binary-flavored provider implementation ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `provider_ref_script` | line 161 | mapper | LOW | Builds script-flavored provider implementation ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `provider_ref_crate` | line 171 | mapper | LOW | Builds crate-flavored provider implementation ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `model_without_external_ref` | line 181 | mapper | LOW | Builds model fixture without provider ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_model` | line 185 | mapper | LOW | Builds external model fixture from fixture provider path. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_model_with_ref` | line 189 | mapper | LOW | Builds external model fixture with supplied provider ref. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `crate_external_model` | line 199 | mapper | LOW | Builds crate-ref external model fixture. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `model_with_provider_ref` | line 206 | mapper | LOW | Builds `ModelConfig` from command and provider-ref inputs. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `model_with_provider_ref_and_inputs` | line 231 | mapper | LOW | Adds input definitions to model fixture. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_execution_equivalent` | line 241 | validator | LOW | Asserts service and direct execution results are equivalent. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_terminal_signal_equivalent_except_observed_at` | line 276 | validator | LOW | Asserts terminal signals match except independent observed timestamp. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `dispatch_registry_for_models` | line 310 | mapper | LOW | Builds provider registry with default options. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `dispatch_registry_for_models_with_options` | line 314 | mapper | LOW | Builds provider registry from model configs and options. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `execute_dispatch_aware_service` | line 322 | orchestration | LOW | Runs dispatch-aware service seam and returns result/observation. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `execute_dispatch_aware_result` | line 340 | orchestration | LOW | Delegates service execution and extracts result. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `execute_external_fixture` | line 347 | orchestration | LOW | Builds model/registry and dispatches facade request. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `execute_external_fixture_effective` | line 365 | orchestration | LOW | Builds model/registry and dispatches effective request. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `make_external_fixture` | line 389 | mapper | LOW | Materializes external-provider fixture from capability/policy/launch modes. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `shell_quote` | line 433 | formatter | LOW | Formats path as shell-quoted text. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `fake_provider_body` | line 437 | formatter | LOW | Formats provider script body from typed fixture modes. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `policy_mode_wire` | line 455 | mapper | LOW | Maps policy mode enum to wire token. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `launch_mode_wire` | line 464 | mapper | LOW | Maps launch mode enum to wire token. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `fake_provider_script_body` | line 480 | formatter | LOW | Formats the fake provider Python script text. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `py_bool` | line 693 | formatter | LOW | Formats boolean as Python boolean token. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `read_json` | line 697 | parser | LOW | Reads JSON text and delegates JSON parsing. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `read_json_text` | line 701 | accessor | LOW | Retrieves JSON record text from path. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `parse_json_value` | line 705 | parser | LOW | Parses JSON text into `serde_json::Value`. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `json_string_array` | line 709 | mapper | LOW | Maps JSON array values to string vector. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `json_array_items` | line 716 | accessor | LOW | Retrieves array items from JSON value. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `json_value_to_string` | line 720 | mapper | LOW | Maps JSON string value to owned string. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `order_lines` | line 726 | parser | LOW | Reads order text and maps it into line strings. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `read_order_text` | line 730 | accessor | LOW | Retrieves order-record text from path. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `lines_to_strings` | line 734 | parser | LOW | Parses text lines into owned strings. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_model_request_carries_effective_inputs` | line 738 | validator | LOW | Asserts provider model request carries expected inputs. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_external_dispatch_failure` | line 756 | validator | LOW | Asserts dispatch error category/kind and absence of legacy exit mapping. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `runtime_executor_dispatch_no_ref_preserves_legacy_bytes_with_unrelated_registry` | line 777 | validator | LOW | Asserts no-ref dispatch preserves legacy bytes and seam observation. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `runtime_executor_dispatch_no_ref_does_not_construct_or_invoke_provider_client` | line 825 | validator | LOW | Asserts unrelated provider client is not invoked. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_runtime_disabled_crate_fails_before_provider_call` | line 869 | validator | LOW | Asserts crate provider ref fails runtime-disabled before fallback. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_missing_policy_or_launch_capability_fails_without_builtin_fallback` | line 891 | validator | LOW | Asserts missing required external capabilities fail without fallback. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_policy_evaluate_runs_before_launch_and_uses_selected_provider_settings` | line 914 | validator | LOW | Asserts policy-before-launch order and selected settings propagation. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_policy_request_passes_hybrid_launch_shape` | line 943 | validator | LOW | Asserts hybrid policy request launch shape. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_request_carries_selected_settings_id_and_effective_inputs` | line 984 | validator | LOW | Asserts launch request carries selected settings and effective inputs. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `explicit_working_dir` | line 1019 | mapper | LOW | Materializes explicit working-directory fixture path. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `effective_extra_inputs` | line 1025 | mapper | LOW | Builds effective extra-input map fixture. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `parent_invocation_env` | line 1035 | accessor | LOW | Supplies fixed parent invocation env string. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `tempdir_path` | line 1044 | mapper | LOW | Materializes tempdir path fixture. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `runner_data_dir_from_xdg` | line 1052 | formatter | LOW | Formats runner data dir path from XDG data root. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `selected_opencode_xdg` | line 1059 | formatter | LOW | Formats selected opencode account XDG path. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_model_for_provider` | line 1066 | mapper | LOW | Maps fixture/provider identity into modified external model fixture. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `dispatch_registry_for_model` | line 1077 | mapper | LOW | Builds dispatch registry for a single model. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_selected_settings_ids` | line 1081 | validator | LOW | Asserts policy and launch settings ids. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_launch_working_dir` | line 1092 | validator | LOW | Asserts launch working directory value. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_launch_parent_invocation` | line 1099 | validator | LOW | Asserts parent invocation env propagation. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_launch_argv_prefix` | line 1106 | validator | LOW | Asserts launch argv prefix. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_launch_argv_tail` | line 1118 | validator | LOW | Asserts prompt argv tail. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_effective_input_flags` | line 1126 | validator | LOW | Asserts expected input flag pairs via assertion helper. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `effective_input_flag_pairs` | line 1132 | accessor | LOW | Supplies fixed expected flag-pair list. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_effective_input_flag` | line 1140 | validator | LOW | Asserts one effective input flag pair is present. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_no_arg_mode_stdin` | line 1149 | validator | LOW | Asserts arg-mode launch omits stdin. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_env_carries_host_linkage_without_openai_keys` | line 1157 | validator | LOW | Asserts host linkage env and OpenAI secret exclusion. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_host_linkage_envs` | line 1204 | validator | LOW | Asserts host linkage env values and exclusions. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_env_uses_selected_opencode_auth_context_without_openai_keys` | line 1236 | validator | LOW | Asserts selected opencode account auth env. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_selected_opencode_auth_envs` | line 1282 | validator | LOW | Asserts selected opencode auth env details. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_env_omits_ambient_xdg_for_default_opencode_auth_context` | line 1304 | validator | LOW | Asserts default opencode auth does not leak ambient XDG. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_default_opencode_auth_envs` | line 1340 | validator | LOW | Asserts default auth env omissions and secret exclusion. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `provider_launch_envs` | line 1352 | accessor | LOW | Retrieves env objects from policy and launch JSON. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `json_object` | line 1362 | accessor | LOW | Retrieves JSON object reference. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_env_value` | line 1366 | validator | LOW | Asserts env key has expected value. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_env_value_not` | line 1375 | validator | LOW | Asserts env key does not have unexpected value. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `env_string` | line 1383 | accessor | LOW | Retrieves env value as optional string. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_env_key_absent` | line 1387 | validator | LOW | Asserts env key absence. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_no_openai_env_keys` | line 1391 | validator | LOW | Asserts no OpenAI env keys cross boundary. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_no_openai_api_key` | line 1398 | validator | LOW | Asserts no OpenAI API key crosses boundary. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `openai_env_key_is_absent` | line 1405 | predicate | LOW | Answers whether env key is not OpenAI key/base-url. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `openai_api_key_is_absent` | line 1409 | predicate | LOW | Answers whether env key is not OpenAI API key. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `assert_no_ambient_openai_secret` | line 1413 | validator | LOW | Asserts serialized env excludes ambient secret. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `env_json` | line 1420 | formatter | LOW | Formats env map as JSON string. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_validates_schema_inputs_before_policy_or_launch` | line 1425 | validator | LOW | Asserts invalid schema inputs fail before external provider calls. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_policy_rejection_skips_launch` | line 1479 | validator | LOW | Asserts policy rejection diagnostics and launch skip. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_policy_request_canonicalizes_opencode_account_one_settings_id` | line 1512 | validator | LOW | Asserts account-one settings id canonicalization. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_policy_transform_applies_once_and_no_legacy_double_policy` | line 1568 | validator | LOW | Asserts policy transform applied once and no legacy fallback. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_enabled_binary_and_script_refs_dispatch_without_legacy_fallback` | line 1610 | validator | LOW | Asserts enabled binary/script refs dispatch externally without fallback. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_preserves_stdout_bytes_and_maps_stderr_boundary` | line 1664 | validator | LOW | Asserts stdout byte preservation and stderr string boundary. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_nonzero_final_exit_is_execution_result` | line 1685 | validator | LOW | Asserts nonzero final event maps as execution result. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_provider_nonzero_after_final_is_diagnostic_only` | line 1703 | validator | LOW | Asserts provider nonzero after final is diagnostic only. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_malformed_stream_is_protocol_failure_not_model_exit` | line 1721 | validator | LOW | Asserts malformed stream is protocol failure. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_missing_final_is_protocol_failure_not_model_exit` | line 1739 | validator | LOW | Asserts missing final is protocol failure. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_invalid_base64_is_protocol_failure_not_model_exit` | line 1757 | validator | LOW | Asserts invalid base64 is protocol failure. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_timeout_or_host_transport_failure_is_not_model_exit` | line 1775 | validator | LOW | Asserts host transport failure is not model exit. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_host_cancelled_before_final_uses_cancellation_fallback_message` | line 1793 | validator | LOW | Asserts cancellation before final maps to fallback error message. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_provider_nonzero_before_final_is_transport_failure_not_model_exit` | line 1833 | validator | LOW | Asserts provider nonzero before final is transport failure. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_provider_emitted_cancelled_final_event_maps_minimal_cancel_outcome` | line 1851 | validator | LOW | Asserts provider-emitted cancellation maps minimal cancel outcome. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `external_provider_launch_minimal_terminal_scope_uses_final_event_not_standalone_classify` | line 1869 | validator | LOW | Asserts final event controls minimal terminal scope. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `fake_provider` | line 45 | mapper | LOW | Materializes fake provider fixture from options. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `fake_provider_script` | line 59 | formatter | LOW | Formats fake provider shell script text from options. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `write_executable_script` | line 114 | orchestration | LOW | Writes script and applies executable permissions. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `shell_quote` | line 121 | formatter | LOW | Formats path as shell-quoted text. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `provider_ref_path` | line 125 | mapper | LOW | Builds path-flavored provider ref. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `external_model` | line 135 | mapper | LOW | Builds external model fixture. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `execution_request` | line 157 | mapper | LOW | Builds executor facade request. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `provider_registry` | line 168 | mapper | LOW | Builds provider registry fixture. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `execute_with_provider` | line 176 | orchestration | LOW | Sequences model, registry, service, request, and execution. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `assert_terminal` | line 186 | validator | LOW | Asserts terminal signal, reason, and exit code. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `recorded_terminal_request` | line 196 | parser | LOW | Reads and parses terminal classify request JSON. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `recorded_terminal_request_text` | line 200 | accessor | LOW | Retrieves terminal classify request text. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `parse_terminal_request_json` | line 208 | parser | LOW | Parses terminal classify request JSON. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `assert_recorded_bytes` | line 212 | validator | LOW | Asserts recorded stdout/stderr base64 fields. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `assert_terminal_classify_not_invoked` | line 217 | validator | LOW | Asserts terminal classify request was not recorded. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `terminal_mode_expectations` | line 228 | accessor | LOW | Supplies expected terminal-mode table. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `terminal_mode_provider` | line 265 | mapper | LOW | Builds fake provider for one terminal mode. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `classify_failure_provider` | line 273 | mapper | LOW | Builds fake provider configured for classify failure. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `missing_capability_provider` | line 281 | mapper | LOW | Builds fake provider missing terminal capability. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `recording_quota_provider` | line 289 | mapper | LOW | Builds fake provider that records terminal request. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `s6a_nonzero_expected` | line 297 | accessor | LOW | Supplies expected S6a nonzero terminal outcome. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `quota_expected` | line 305 | accessor | LOW | Supplies expected quota terminal outcome. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `external_terminal_classify_maps_quota_maybe_rate_and_cancelled_modes` | line 314 | validator | LOW | Asserts terminal classify mapping for quota/maybe/rate/cancelled modes. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `terminal_classify_failure_after_launch_success_falls_back_to_s6a_mapping` | line 322 | validator | LOW | Asserts classify failure falls back to S6a mapping. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `missing_terminal_capability_after_launch_success_falls_back_to_s6a_mapping` | line 328 | validator | LOW | Asserts missing terminal capability falls back and avoids classify call. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `terminal_classify_request_preserves_raw_stdout_and_stderr_bytes` | line 335 | validator | LOW | Asserts raw stdout/stderr bytes are preserved in request. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `ProviderOptions::provider_session_id` | line 62 | mapper | LOW | Builds provider options for provider-session-id capture. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `ProviderOptions::session_id_without_session_capability` | line 69 | mapper | LOW | Builds provider options for session-id alias without session capability. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::new` | line 78 | orchestration | LOW | Dispatches fixture construction with default provider options. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::new_with_provider_options` | line 82 | orchestration | LOW | Sequences tempdir, path, materialization, and fixture construction helpers. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::run_launch` | line 89 | orchestration | LOW | Dispatches launch with no extra env. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::run_launch_with_env` | line 93 | orchestration | LOW | Constructs and runs launch command with env overrides. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::run_resume` | line 107 | orchestration | LOW | Dispatches resume with no extra env. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::run_resume_with_env` | line 111 | orchestration | LOW | Constructs and runs resume command with env overrides. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::command` | line 129 | mapper | LOW | Builds base runner `Command` with fixture environment. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::db_path` | line 138 | accessor | LOW | Retrieves fixture state database path. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::invocation_session_rows` | line 144 | accessor | LOW | Retrieves invocation session rows through DB helper. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::latest_invocation_outcome` | line 148 | accessor | LOW | Retrieves latest invocation outcome row. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `Fixture::records` | line 152 | accessor | LOW | Retrieves provider records. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `materialize_fixture` | line 157 | orchestration | LOW | Sequences fixture directory/config/provider materialization helpers. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `fixture_paths` | line 164 | mapper | LOW | Maps fixture root path to structured fixture paths. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `create_fixture_directories` | line 178 | orchestration | LOW | Creates required fixture directories. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `write_model_config` | line 184 | orchestration | LOW | Writes model config artifact. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `model_config_toml` | line 192 | formatter | LOW | Formats model config TOML. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `write_providers_config` | line 206 | orchestration | LOW | Writes providers config artifact. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `providers_config_toml` | line 214 | formatter | LOW | Formats providers config TOML. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `fixture_from_paths` | line 224 | mapper | LOW | Maps tempdir and paths into `Fixture`. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `invocation_session_rows_from_db` | line 236 | accessor | LOW | Retrieves invocation session rows from DB path. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `open_invocation_db` | line 241 | accessor | LOW | Opens state database connection. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `query_invocation_session_rows` | line 245 | accessor | LOW | Retrieves invocation session rows via prepared statement helper. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `invocation_session_rows_statement` | line 250 | accessor | LOW | Retrieves prepared statement for invocation session rows. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `collect_invocation_session_rows` | line 260 | mapper | LOW | Maps statement rows into typed row vector. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `invocation_session_row` | line 269 | mapper | LOW | Maps SQLite row to `InvocationSessionRow`. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `latest_invocation_outcome_from_db` | line 279 | accessor | LOW | Retrieves latest invocation outcome row by provider. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `invocation_outcome_row` | line 293 | mapper | LOW | Maps SQLite row to `InvocationOutcomeRow`. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_record_text` | line 302 | accessor | LOW | Retrieves provider record text from path. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_records_from_path` | line 306 | parser | LOW | Reads and parses provider record JSONL. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `parse_provider_records` | line 310 | parser | LOW | Parses record text into provider record values. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_record_lines_with_content` | line 314 | filter | LOW | Filters record lines to non-empty content. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `parse_provider_record_lines` | line 320 | parser | LOW | Parses record lines into JSON values. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_record_line_has_content` | line 324 | predicate | LOW | Answers whether a record line has non-empty content. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `parse_provider_record` | line 328 | parser | LOW | Parses one JSON record line. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `external_provider_resume_without_rotate_uses_external_launch_and_recorded_cwd` | line 333 | validator | LOW | Asserts external resume launch, cwd, and no rotation/migration calls. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `external_launch_session_id_alias_persists_external_capture_method_without_session_capability` | line 381 | validator | LOW | Asserts session-id alias persists external capture method. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `external_provider_launch_terminal_error_exit_zero_finalizes_as_failed` | line 397 | validator | LOW | Asserts external launch provider-error exit-zero finalizes failed. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `external_provider_resume_terminal_error_exit_zero_finalizes_as_failed` | line 407 | validator | LOW | Asserts external resume provider-error exit-zero finalizes failed. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_external_launch_session_capture_rows` | line 417 | validator | LOW | Asserts launch/resume session capture rows. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_external_launch_session_capture_row` | line 432 | validator | LOW | Asserts one external launch session capture row. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_success` | line 446 | validator | LOW | Asserts successful process exit. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_failed_terminal_error_output` | line 456 | validator | LOW | Asserts failed process output envelope fields. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_failed_terminal_error_process` | line 466 | validator | LOW | Asserts process did not exit zero. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_latest_invocation_failed_with_terminal_error` | line 476 | validator | LOW | Asserts latest invocation row records failed terminal error. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `records_for_subcommand` | line 487 | filter | LOW | Filters provider records by subcommand. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_no_rotation_or_migration_provider_calls` | line 494 | validator | LOW | Asserts no forbidden rotation/migration provider calls. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_record_subcommands` | line 499 | mapper | LOW | Maps provider records to subcommand list. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_record_subcommand` | line 503 | accessor | LOW | Retrieves subcommand from provider record. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `assert_no_forbidden_provider_subcommands` | line 507 | validator | LOW | Asserts subcommand list contains no forbidden calls. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_subcommands_are_allowed` | line 514 | predicate | LOW | Answers whether all subcommands are allowed. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `provider_subcommand_is_allowed` | line 520 | predicate | LOW | Answers whether one subcommand is not rotation/migration. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `write_external_provider` | line 524 | orchestration | LOW | Writes provider script artifact and applies executable permissions. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `external_provider_script` | line 534 | formatter | LOW | Formats external provider Python script text from fixture inputs. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| _None_ | _None_ | _None_ | _None_ | No admitted function-like symbol in a touched file inferred two or more A1 categories after applying pure helper-dispatch recognition. | _None_ | _None_ | _None_ | _None_ |

## Residual Ambiguity / Stop-Condition Notes

- `risk_profile_path` was supplied but `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/multi-classifier-risk.md` was not present; the `evidence/` directory contained only `runtime-tests.log`. This was not needed to resolve touched files or classify function bodies from required evidence, so it did not change the verdict.
- The diff touched source and test files only; no Markdown procedure headings, shell snippets as standalone artifacts, or YAML carriers were admitted as function inventory. Embedded shell/Python text inside Rust string literals was treated as fixture data generated by the containing Rust functions, not as separately executable file-local Rust symbols.
- Extern declaration `kill` inside `send_signal` has no Rust body and was excluded from the A5 inventory.

Verdict: LOW

LOW
