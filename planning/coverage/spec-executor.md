# spec-executor — Process model and provider CLI dispatch

## Source files

- `crates/oulipoly-runtime/src/executor/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli.rs`
- `crates/oulipoly-runtime/src/executor/cli/headless.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/validate.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/parse.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/format.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/messages.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/schema_access.rs`
- `crates/oulipoly-runtime/src/executor/cli/input_flags/predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/interactive.rs`
- `crates/oulipoly-runtime/src/executor/cli/pty_broker/tui.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_path.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_jsonl.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_parent.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_cleanup.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_warnings.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/return_channel_predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/captured_child_marker.rs`
- `crates/oulipoly-runtime/src/executor/cli/ipc/captured_child_dedupe.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/command.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/command_parse.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/command_validate.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/prompt.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/prompt_file.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/prompt_format.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/prompt_path.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/prompt_predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/capture.rs`
- `crates/oulipoly-runtime/src/executor/cli/launch/supervisor_config.rs`
- `crates/oulipoly-runtime/src/executor/cli/policy.rs`
- `crates/oulipoly-runtime/src/executor/cli/policy/messages.rs`
- `crates/oulipoly-runtime/src/executor/cli/policy/orchestration.rs`
- `crates/oulipoly-runtime/src/executor/cli/policy/predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/policy/validation.rs`
- `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs`
- `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs`
- `crates/oulipoly-runtime/src/executor/cli/provider_lookup.rs`
- `crates/oulipoly-runtime/src/executor/cli/request.rs`
- `crates/oulipoly-runtime/src/executor/cli/result.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume/acceptance.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume/args.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume/messages.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume/output.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume/patterns.rs`
- `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/json_path.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/messages.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/parse_forced_flag.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/parse_stdout_event.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/paths.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs`
- `crates/oulipoly-runtime/src/executor/cli/session_capture/start_known.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/process.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/process_validate.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/stdin.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/drain.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/drain_access.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/drain_chunks.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/errors.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/live_quota.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/status.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/stdin_access.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/stdin_predicates.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs`
- `crates/oulipoly-runtime/src/executor/cli/supervision/termination.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/mod.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/policy/mod.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/policy/host_policy.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/policy/codex.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/session_capture/mod.rs`
- `crates/oulipoly-runtime/src/executor/provider_specific/session_capture/telemetry_scrub.rs`
- `crates/oulipoly-runtime/src/executor/providers/codex.rs`
- `crates/oulipoly-runtime/src/executor/providers/mod.rs`
- `crates/oulipoly-runtime/src/executor/providers/openai_compat.rs`

## Preconditions

- A selection decision from `balancer/mod.rs` carrying provider identity,
  account, and (optionally) a resume session id.
- The provider executable installed and discoverable on PATH (or at the
  configured path).
- A prompt or resume payload to feed via stdin.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Selected provider, fresh invocation. | Spawn child, write prompt to stdin, capture stdout + stderr, await exit; return process outcome. |
| Selected provider, resume invocation. | Spawn child with resume-mode arguments; replay-or-resume per provider semantics. |
| Selected provider, OAuth flow needed (interactive). | Pass stdio through; the executor does not capture interactive streams during auth. |
| Child completes cleanly. | Return the captured outputs to the recognizer for classification. |
| Child writes large stdout/stderr (multiple MB). | Stream incrementally; do NOT buffer the full output if not necessary. |
| Child requires a working directory. | `cli.rs` sets cwd from the resume locator or current invocation context. |
| Child requires env vars (e.g. ANTHROPIC_API_KEY). | Inject from `RuntimeConfig` resolution; do not leak host env beyond the configured allow-set. |

## Edge cases

- Provider executable not on PATH — return a typed "not installed" error
  for the recognizer's `error` signal path.
- Child PID reused after death (Linux PID wrap) — `mod.rs` tracks by
  child handle, not by PID, so reuse does not confuse supervision.
- Child writes a partial OULIPOLY result marker then exits — the
  executor still hands stdout + stderr to the recognizer; the recognizer
  decides classification.
- Child receives SIGINT from a Ctrl-C in the terminal — the executor
  attempts cooperative shutdown then returns the exit status.
- Child blocks on stdin (waiting for input) — the executor closes stdin
  after the configured timeout; the child's response determines whether
  this is `aborted` or `timeout`.

## Error conditions

- `ExecutorSpawnFailed` — child could not be spawned (PATH miss, perms,
  fork failure).
- `ExecutorIoFailed` — stdin/stdout/stderr pipe failure mid-run.
- `ExecutorTerminated` — child killed by signal the executor did not
  send.

These are surfaced as process outcomes, then classified by the
recognizer.

## Boundaries

- Executor does NOT decide WHICH provider to invoke — that is the
  balancer.
- Executor does NOT classify the outcome — that is the recognizer
  (`terminal_signal.rs` + per-provider files).
- Executor does NOT mutate session metadata — it returns outputs; the
  session lifecycle layer ingests.
- Executor does NOT refresh quota — that is `quota/mod.rs`.
- Executor does NOT modify config — it reads `RuntimeConfig` once at
  spawn time.

## Declared test patterns

Per `~/ai/conventions/testing.md`: integration tests with stub provider
executables (see `src-tauri/tests/scripts/`), supervision-shape tests on
SIGINT/SIGTERM paths, stdout/stderr capture tests.

- `crates/oulipoly-runtime/tests/age34_runtime_executor_service_routing.rs`
- `crates/oulipoly-runtime/tests/age34_runtime_launcher_service_routing.rs`
- `crates/oulipoly-runtime/tests/executor_return_channel.rs`
- `src-tauri/tests/age153_captured_child_supervision.rs`
- `src-tauri/tests/age153_one_shot_terminal_signal.rs`
- `src-tauri/tests/age153_repl_terminal_signal.rs`
- `src-tauri/tests/age153_resume_terminal_signal.rs`
- `src-tauri/tests/age27_raw_executor_callsite_scan.rs`
- `src-tauri/tests/pr_d_claude_code_turns.rs`
- `src-tauri/tests/scripts/claude_code_turns_body.rs`
- `src-tauri/tests/scripts/codex_turns_body.rs`
- `src-tauri/tests/scripts.rs`
- `src-tauri/tests/cwd_scripts.rs`

## Cross-references

- `planning/coverage/spec-balancer.md` — upstream selection.
- `planning/coverage/spec-recognizer.md` — downstream classification.
- `planning/coverage/spec-session-lifecycle.md` — passes resume ids in.
- `AGENTS.md` § Rust Workspace Structure.
