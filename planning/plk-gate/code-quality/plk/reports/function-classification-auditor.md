# Function Classification Audit

## Inputs Read

| Input | Path / value |
|---|---|
| mode | `phase-6` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/function-classification-auditor.md` |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Read before scoring. Applied A1 single-classification rule, category vocabulary, `Function categories per function` threshold, and `multi-classifier function` failure mode. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` | Read before scoring. Used Phase-6 component declared roles and function inventory as context, not as a waiver. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` | Read before scoring. Used parent-linkage and stale-running reconciliation intent as context. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` | Read to identify touched PLK surfaces and added or meaningfully changed functions. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` | Read as the touched-surface boundary. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | Read as runtime proof context. Runtime success was not used to waive function-classification defects. |

## Scope

Audited only PLK touched surfaces listed in `touched-files.txt`, with scoring focused on added or meaningfully changed functions from `diff.patch` and the contract inventory. Removed symbols were considered only to confirm they are absent from the post-diff function inventory.

## Function Classification

| Path | Function / symbol | Lines | Inferred category | Verdict | Evidence |
|---|---|---:|---|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | `load_trace_environment` | 11-17 | `orchestration` | LOW | Sequences state open, stale reconciliation, sessions config load, and mapper handoff; the reconciliation work is delegated. |
| `src-tauri/src/dispatch/parent_invocation.rs` | `resolve_parent_invocation_id` | 5-9 | `orchestration` | LOW | Sequences env read, parser helper, DB lookup helper, and trivial row-id extraction. |
| `src-tauri/src/dispatch/predicate.rs` | `parent_invocation_source_matches` | removed | n/a | LOW | Removed provider-source guard is not present in post-diff code and has no remaining function body to classify. |
| `src-tauri/src/invocation/mod.rs` | `stale_reconcile` module export | 3 | n/a | LOW | Module declaration only; no executable function-like body. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `reconcile_stale_running_invocations` | 33-48 | `orchestration` | LOW | Opens optional sidecar, iterates helper-returned rows, delegates stale and liveness predicates, and delegates finalization. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `open_pid_sidecar_read_only_optional` | 50-56 | `orchestration` | LOW | Resolves the sidecar path, delegates existence check, and opens read-only when present. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `path_exists` | 58-60 | `predicate` | LOW | Answers whether a path exists. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocations` | 62-67 | `orchestration` | LOW | Sequences running-row retrieval and conversion through named helpers. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_rows` | 69-74 | `orchestration` | LOW | Sequences raw value retrieval and row conversion through named helpers. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_row_values` | 76-98 | `accessor` | LOW | Performs the SQL read of running invocation row values; row value construction and error text are delegated to named helpers. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_row_value` | 106-116 | `mapper` | LOW | Maps SQL column values into the raw row-value struct. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_row_from_values` | 118-126 | `mapper` | LOW | Maps raw row-value struct into `RunningInvocationRow` via a named mapper. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `format_stale_running_prepare_error` | 128-130 | `formatter` | LOW | Formats prepare errors for presentation. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `format_stale_running_query_error` | 132-134 | `formatter` | LOW | Formats query errors for presentation. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `format_stale_running_row_error` | 136-138 | `formatter` | LOW | Formats row mapping errors for presentation. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_row` | 140-150 | `mapper` | LOW | Maps raw field values into `RunningInvocationRow`. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_from_row` | 152-159 | `mapper` | LOW | Maps raw row plus delegated timestamp parsing into `RunningInvocation`. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `parse_running_invocation_created_at` | 161-166 | `parser` | LOW | Parses RFC3339 timestamp text and normalizes to UTC; parse failure reporting is subordinate to parser failure handling. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `running_invocation_is_stale` | 168-174 | `predicate` | LOW | Answers whether invocation age reaches the stale-running threshold. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `invocation_has_dead_pid_evidence` | 176-182 | `orchestration` | LOW | Sequences PID sidecar row access and dead-evidence predicate through named helpers. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `pid_identity_rows_for_invocation` | 184-189 | `accessor` | LOW | Retrieves PID identity rows for one invocation UUID from the sidecar. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `pid_identity_rows_have_dead_evidence` | 191-204 | `predicate` | LOW | Answers whether rows conclusively prove dead process evidence and reject live or unknown states. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `pid_identity_row_liveness` | 206-214 | `mapper` | LOW | Maps one sidecar row into live/dead/unknown liveness result. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `live_process_identity_state` | 216-222 | `accessor` | LOW | Reads live process identity for an OS PID and exposes unavailable/error reads as `Unknown`. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `process_identity_matches_row` | 224-226 | `predicate` | LOW | Answers whether live process identity equals the recorded row identity. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `finalize_stale_invocation` | 228-240 | `orchestration` | LOW | Dispatches finalization with stale-running constants and delegates benign race detection. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `invocation_already_finalized` | 242-244 | `predicate` | LOW | Answers whether a finalization error string is the benign already-finalized race. |
| `src-tauri/src/dispatch.rs` | `resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift` | 1004-1026 | `validator` | LOW | Seeds source-drift parent fixture and asserts resolver returns the same-DB row id. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `Fixture::state_home` | 89-91 | `accessor` | LOW | Exposes isolated `XDG_STATE_HOME` path for fixture use. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `run_agent_bash_nested_child` | 115-129 | `orchestration` | LOW | Dispatches nested `agent-bash` run and delegates command formatting, environment mapping, and polling. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `nested_child_command` | 131-137 | `formatter` | LOW | Formats the nested runner command line. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `shell_quote` | 139-141 | `formatter` | LOW | Formats a shell-safe single-quoted argument. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `configure_agent_bash_env` | 143-149 | `mapper` | LOW | Maps fixture paths and parent env value onto a `Command` environment. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `wait_for_agent_bash_done` | 151-162 | `orchestration` | LOW | Polls a named status accessor until completion or timeout. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `agent_bash_status` | 164-171 | `accessor` | LOW | Retrieves full `agent-bash` status output for a handle. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `agent_bash_bin_from_env` | 173-182 | `orchestration` | LOW | Selects an env-supplied or PATH-discovered binary and delegates file validation. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `find_agent_bash_in_path` | 184-189 | `filter` | LOW | Selects the first PATH entry containing an `agent-bash` file. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `assert_agent_bash_bin` | 191-196 | `validator` | LOW | Fails unless the selected `agent-bash` path points to a file. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `nested_agent_bash_chain_records_parent_id_from_inherited_env` | 343-372 | `validator` | LOW | Verifies nested `agent-bash` child records `parent_invocation_id` from inherited parent env. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `Fixture::sidecar_path` | 78-82 | `mapper` | LOW | Maps isolated data home to the PID identity sidecar path. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `Fixture::seed_stale_running_trace_row` | 134-153 | `orchestration` | LOW | Sequences stale running row fixture setup and returns its row id. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `Fixture::seed_stale_running_trace_row_with_dead_pid` | 155-174 | `orchestration` | LOW | Sequences stale row setup and sidecar PID identity fixture setup. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `trace_reconciles_liveness_stale_running_row_with_dead_pid` | 312-342 | `validator` | LOW | Verifies trace reconciliation persists durable failed terminal state from sidecar-proven dead PID evidence. |

## Multi-Classifier Findings

No non-LOW multi-classifier findings remain in the prompt-scoped PLK touched surfaces.

## Prior Report Recheck

The prior report's stale `FC-001` shape is no longer present: `running_invocation_rows` now delegates SQL access, row materialization, and error formatting across `running_invocation_row_values`, mapper helpers, and formatter helpers. The prior report's stale `FC-002` shape is no longer present: `invocation_has_dead_pid_evidence` now delegates sidecar row retrieval to `pid_identity_rows_for_invocation` and dead-evidence evaluation to `pid_identity_rows_have_dead_evidence`.

## Stop Conditions

No stop condition fired. `contract_path`, `proposal_path`, `diff_path`, `touched_surfaces_path`, and runtime evidence were readable before scoring. `src-tauri/src/invocation/mod.rs` contains no function-like body. Runtime evidence passed, but the LOW result above is based on code-shape classification rather than runtime proof.

VERDICT: LOW
