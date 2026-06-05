# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate`
- `wu_id=cap`
- `mode=phase-6`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/contracts/cap.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/code-quality/cap/reports/function-classification-auditor.md`

## References Read

- `~/ai/conventions/code-quality.md` lines 52-69: A1 single-classification rule and category list.
- `~/ai/conventions/code-quality.md` lines 291-306: `Function categories per function` LOW = 1, MEDIUM = n/a, HIGH >= 2, and `multi-classifier function` failure mode.
- `~/ai/conventions/code-quality.md` lines 21-27 and 143-149: touched-file ownership rule. Caller-supplied delta constraint narrowed this Phase 6 report to production functions added/changed by `42200fb..9ba1275`.
- `planning/cap-gate/contracts/cap.contract.md` lines 1-19: component/per-file declared roles and production/test-sweep split.
- `planning/cap-gate/contracts/cap.contract.md` lines 21-37: declared function inventory and no declared `MULTI-CLASSIFIER-RISK` entries.
- `planning/cap-gate/contracts/cap.contract.md` lines 39-47: test-only sweep note and no new adapter declaration.
- `planning/cap-gate/proposal.md` lines 1-9: capture-time session persistence behavior and test-only isolation-sweep context.
- `planning/cap-gate/gates/diff.patch` lines 41-291: production source delta for `interactive.rs`, `pty_broker.rs`, `spawn_identity.rs`, and `supervision/mod.rs`.
- `planning/cap-gate/gates/touched-surfaces.md` lines 5-19: production substance in commit `9e00408`; commit `9ba1275` treated as test-only isolation sweep.
- Source files under `crates/oulipoly-runtime/src/executor/cli/`: `interactive.rs`, `pty_broker.rs`, `spawn_identity.rs`, `supervision/mod.rs`.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `execute_interactive_with_result_and_model_identity` | lines 76-115; diff lines 45-52 | `orchestration` | LOW | Sequences resume/session context, PTY-or-direct launch, identity recording, child wait, signal guard lifetime, and result mapping via named helpers. The delta at line 107 only discards the optional return from `record_child_identity`; no inlined multi-job body is added. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `execute_interactive_child` | lines 83-110; diff lines 58-64 | `orchestration` | LOW | Sequences PTY setup, control socket, child spawn, identity recording, idle guard, signal guard, relay, and status return. The delta at line 100 only discards the optional return from `record_child_identity`; no added inline classifier work appears. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `record_child_identity` | lines 79-98; diff lines 86-112 | `orchestration` | LOW | Uses an optional-context guard, delegates sidecar row construction to `live_process_identity_record`, calls `record_live_process_identity`, delegates running-state marking to `mark_session_running`, delegates warnings to `warn_child_identity_record_failed`, and returns the recorded identity. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `backfill_captured_session_id` | lines 100-110; diff lines 114-124 | `orchestration` | LOW | Requires both context and process identity, then delegates sidecar session-id backfill to `backfill_pid_identity_session_id` and running-state marking to `mark_session_running_with_session_id`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `mark_session_running` | lines 133-138; diff lines 133-142 | `orchestration` | LOW | Performs a trivial session-id guard and delegates the running-state write path to `mark_session_running_with_session_id`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `mark_session_running_with_session_id` | lines 140-153; diff lines 144-152 | `orchestration` | LOW | Opens the mailbox and applies the named `mark_session_running` update built by `session_runtime_running_update`; failure formatting is delegated to `warn_mark_session_running_failed`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `backfill_pid_identity_session_id` | lines 182-192; diff lines 165-175 | `orchestration` | LOW | Opens the PID identity sidecar, applies the named `set_session_id` operation, and routes missing-row/failure outcomes to warning helpers. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `warn_pid_identity_session_backfill_missing` | lines 194-205; diff lines 177-188 | `formatter` | LOW | Formats a structured warning with invocation UUID, captured session id, PID, and fixed missing-row message. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `warn_pid_identity_session_backfill_failed` | lines 207-219; diff lines 190-202 | `formatter` | LOW | Formats a structured warning with invocation UUID, captured session id, PID, and backfill error text. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `execute_with_supervisor` | lines 122-209; diff lines 225-291 | `orchestration` | LOW | Sequences supervised command setup, spawn, identity recording, drains, stdin writer, live capture observation, terminal outcome handling, final drains, stdin finalization, output construction, and fatal-stdin check through named helpers. Added capture-time backfill is reached by passing `spawn_identity` and `recorded_identity` into `observe_streamed_session_id`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `observe_streamed_session_id` | lines 211-233; diff lines 275-290 | `orchestration` | LOW | Performs single-fire guards, delegates stdout-json parsing to `parse_stdout_json_event_session_id`, delegates sidecar/runtime persistence to `backfill_captured_session_id`, and records the captured id. The parser and persistence work are helper calls, not inlined multi-job logic. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|

No HIGH findings.

## Residual Ambiguity / Stop-Condition Notes

- A1 preservation verified: the source contains the category list, single-classification rule, `Function categories per function` threshold row, and `multi-classifier function` failure mode.
- The caller explicitly requested scoring the incremental delta and production functions added/changed. Test-only `OULIPOLY_DATA_DIR` scrub changes in commit `9ba1275`, including test functions and fixtures, were excluded from production function inventory per `cap.contract.md` lines 1-3 and 39-41 plus `touched-surfaces.md` lines 15-19.
- Type-path/import-only production changes, such as the `ProcessIdentity` import simplification and type-path adjustment in `session_runtime_running_update`, were not inventoried as meaningful function changes per `cap.contract.md` line 37.
- Markdown headings, doc comments, shell snippets inside test fixtures, and YAML declaration carriers were not admitted as executable A5 inventory.
- No unresolved function-boundary ambiguity materially affects the verdict.

VERDICT: LOW
