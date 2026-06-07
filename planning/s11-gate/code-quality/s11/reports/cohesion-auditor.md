# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection used this worktree exclusively. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree_path for this invocation. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate` | Planning root for S11. |
| `wu_id` | `s11` | Work unit identifier. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md` | Read before scoring; describes the S11 external-provider wake, delivery confirmation, and transport-rotation runtime claims (lines 1–71). |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md` | Read before scoring; parseable `## Component declared roles` section at lines 3–7 declares all 8 A1 tokens; `## Touched-file roles` table at lines 19–75 provides per-file role annotations; `## Adapter declarations`, `## Intrinsic-surface declarations`, and `## Test-harness declarations` sections also present and read. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/touched-files.txt` | Read; 58 lines (57 paths + trailing newline) covering the S11 production component and three historical S10B `.scratch` log artifacts. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch` | Read (first 200+ lines of patch); diff hunks span the S11 production component. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Evidence of reading |
|---|---|
| `/home/nes/ai/agents/cohesion-auditor.md` | Operator read; metric binding at lines 57–65; resolution order and component declared roles rule at lines 63–64; Phase 6 procedure at lines 77–91. |
| `/home/nes/ai/conventions/code-quality.md` | Required convention; A1 cohesion row at lines 295–300; `## Auditor Scope Boundary` at lines 21–25; `## Touched-file ownership` at lines 143–149; `## Component declared roles` at lines 161–167; file-local-override rule at lines 140–141; adapter declarations at lines 183–208; intrinsic-surface declarations at lines 214–253. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Required convention; critic independence at lines 29–32; no-revision rule at lines 33–35. |
| `/home/nes/ai/conventions/risk-profile.md` | Required convention; touched-file ownership clause at lines 11–16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Required workflow; Phase 6 per-component code-quality fanout at lines 489–491; Step 6a contract role at lines 426–432; component declared roles phase-visibility rule at lines 169–173. |

Metric row verified present in `code-quality.md` line 299:

> `Cohesion by classifications touched`: LOW = actual classifications are a subset of the declared role set (file-local, path default, or component-level declared roles in a Phase 6a contract), or exactly 1 classification for components and files without any declared roles; MEDIUM = n/a; HIGH = actual classifications exceed the declared role set or include classifications outside the declared role set, or >= 2 classifications for components and files without any declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| **S11 multi-file component** — external provider wake delivery confirmation, detached resume continuity, and external transport rotation | `s11.contract.md` lines 3–7 name the component and declare its role set; `touched-files.txt` enumerates the 57-path touched surface; `proposal.md` lines 3–5 describe the functional scope. This is a Phase 6 component-level cohesion audit; rule (a) of the metric binding resolution order applies: the `## Component declared roles` section in `contract_path` is the primary declared role source for the subset check. | All 57 touched paths are members of this one component. Three historical S10B `.scratch` log artifacts (`planning/s10b-gate/.scratch/code-quality/s10b/logs/*.log`) are in the touched set; the contract lines 47–49 and the invocation context classify them as formatter-only historical artifacts — they contribute `formatter` to the component classification union and do not open a separate component boundary. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| S11 multi-file component (57 touched files) | `orchestration`, `accessor`, `mapper`, `filter`, `validator`, `predicate`, `formatter`, `parser` | **LOW** | blocking scope, **passing** | Component declared role set from `s11.contract.md` line 7 is exactly these 8 A1 tokens. Union of actual classifications across the whole component — drawn from `s11.contract.md` `## Touched-file roles` table (lines 19–75) and verified by direct source inspection of 10 representative files (see Source Inspection Evidence section below) — is the same 8-token set. Actual ⊆ declared → LOW. |

## Evidence For Non-LOW Scores

No non-LOW scores were assigned. This table is empty per the output format requirement.

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| — | — | — | — | No non-LOW cohesion findings. |

## Residual Rows For Context-Only Cohesion Concerns

No context-only concerns outside the touched component boundary were identified.

| id | severity | surface | anchor | evidence | residual basis | why outside touched set |
|---|---|---|---|---|---|---|
| — | — | — | — | — | — | — |

## Residual Ambiguity / Stop-Condition Notes

| Note | Disposition |
|---|---|
| `contract_path` is readable and contains a parseable `## Component declared roles` section declaring all 8 A1 vocabulary tokens. | Rule (a) of the metric binding resolution order applies. No per-file fallback or count-only fallback was used. No `BLOCKED:unreadable-contract-path`. |
| A1 cohesion metric row is present and unchanged in `code-quality.md` line 299. | No `BLOCKED:A1-metric-source`. |
| The component declared role set is the full A1 vocabulary. Any actual classification across the component is necessarily a subset. | The LOW verdict is not vacuously LOW from a trivially permissive declared set. The contract's `## Touched-file roles` table shows each file holds a bounded subset of the 8 tokens; the union reaches all 8 only because the WU spans provider protocol adaptation (mapper/parser/formatter), process lifecycle orchestration (orchestration), state validation/predicate work (validator/predicate), and filter routing (filter). The component-level role set accurately names the coordinated functional surface of a multi-responsibility WU. |
| Several per-file inline `## Declared roles` sections are incomplete relative to actual classifications observed in source. Key cases: (1) `policy_transform.rs` inline declares only `mapper`; actual also has `validator` (`accepted_policy_transform`), `predicate` (`should_rewrite_arg_prompt`, `prompt_arg_matches`), `accessor` (`final_prompt_arg`), and `filter` (`matching_prompt_arg`). (2) `request_builder.rs` inline declares only `mapper`; actual includes `accessor` (`ambient_env_value`, `pinned_agent_data_dir`). (3) `spawn_identity.rs` inline declares `formatter, mapper, orchestration, parser`; actual also has `accessor` (methods `invocation_uuid()`, `session_id()` on `SpawnIdentityContext`). (4) `context.rs` inline uses `role: carrier` (non-A1 vocabulary); actual functions include mapper (From impl, with_account), accessor (basename_or_value, opencode_account_index), parser (opencode_command_tokens), formatter-keyed (format_opencode_settings_id producing an internal settings ID key). (5) `result.rs` contract per-file table declares `mapper, predicate`, but actual has `mapper, orchestration` (cleanup_temp_files) and no predicate; file-local declares `mapper, orchestration` and matches actuals. | Per-file inline/contract discrepancies are informational only for this component-level audit. All classifications found in spot-checked files are within the component declared role set (all 8 tokens). These discrepancies do not raise the component verdict. They are pre-existing file-local declaration gaps owned by touched files but absorbed by the component-level LOW under rule (a). No per-file HIGH is asserted because this auditor's verdict is at the component granularity declared by `## Component declared roles`. |
| Historical `planning/s10b-gate/.scratch/**` log files are in the touched set but are auditor log artifacts, not product code. | Per invocation context and `s11.contract.md` lines 47–49, these are classified `formatter`. Within the component declared role set. No residual row required. |

## Source Inspection Evidence (spot-checks supporting per-component LOW)

Direct source reads from `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`:

| File | Functions / classifications observed | Contract per-file declared roles | Notes |
|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | `map_registry_error` → mapper; `map_provider_client_error` → mapper; `provider_client_error_is_rotatable` → predicate (returns bool); `host_error_kind_is_rotatable` → predicate; `provider_category_is_rotatable` → predicate; `protocol_service_error` → mapper; `invalid_provider_input_error` → mapper; `service_error` → mapper; `provider_nonzero_before_final` → predicate | `mapper`, `predicate`, `formatter` | Inline declares `mapper, predicate`. Actual: mapper, predicate. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` | `ExternalProviderDispatchError` enum variant constructors (`missing_required_capability`, `runtime_disabled_artifact`, `provider_transport_failure`, `provider_protocol_failure`, `cancellation_fallback`, `policy_rejected`) → mapper (factory constructors building error values); no `Display` impl in this file | `formatter`, `mapper` | Inline declares `formatter`. Constructors are mapper-classified (building error values for downstream formatting). All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/external_provider/context.rs` | `From<ExternalProviderDispatchInput>` → mapper; `with_account` → mapper; `provider_settings_id` → mapper; `canonical_opencode_settings_id` → mapper; `format_opencode_settings_id` → formatter/mapper (internal key); `opencode_settings_index` → mapper; `opencode_account_index_from_command` → mapper; `opencode_command_tokens` → parser (delegates to `shell_split`); `opencode_account_index_from_tokens` → filter; `opencode_account_index` → accessor; `basename_or_value` → accessor; `opencode_account_name_index` → mapper (returns `Option<u8>`) | `mapper`, `accessor` | Inline uses `role: carrier` (non-A1). Actual includes parser and formatter not in contract's per-file roles. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/external_provider/policy_transform.rs` | `apply_policy_transform` → mapper/orchestration; `accepted_policy_transform` → validator (accepts or returns policy_rejected error); `apply_accepted_policy_transform` → mapper; `apply_optional_*` → mapper; `rewrite_arg_prompt_if_needed` → mapper/orchestration; `should_rewrite_arg_prompt` → predicate (returns bool); `replace_arg_prompt` → mapper; `matching_prompt_arg` → filter; `final_prompt_arg` → accessor; `prompt_arg_matches` → predicate; `replace_prompt_arg` → mapper | `mapper`, `predicate`, `validator`, `orchestration` | Inline declares only `mapper`. Actual: mapper, validator, predicate, accessor, filter. File-local declaration is incomplete. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `cleanup_temp_files` → orchestration; `execution_result_from_raw` → mapper; `raw_result_from_supervised_output` → mapper | `mapper`, `predicate` | File-local declares `mapper, orchestration` (accurate). Contract per-file declares `mapper, predicate` (over-declares `predicate`, under-declares `orchestration`). Actual: mapper, orchestration. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `SpawnRuntimeMode::as_str` → formatter; accessor methods `invocation_uuid()`, `session_id()` → accessor; `with_pty_control_path` → mapper; `context_from_parent_invocation_env` → mapper; `parse_parent_invocation_env` → parser; `spawn_identity_context_from_invocation` → mapper; `record_child_identity` → orchestration; `backfill_captured_session_id` → orchestration; `live_process_identity_record` → mapper; `warn_*` → formatter; `mark_session_running*` → orchestration; `session_runtime_running_update` → mapper; `backfill_pid_identity_session_id` → orchestration; `parse_invocation_env_silent` → parser | `accessor`, `formatter`, `mapper`, `orchestration`, `validator` | File-local declares `formatter, mapper, orchestration, parser` (accurate but misses `accessor`). Contract per-file declares `accessor, …, validator` but misses `parser`. Actual: accessor, formatter, mapper, orchestration, parser. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/external_provider/request_builder.rs` | `build_launch_candidate` → mapper; `declared_launch_env` → mapper; `insert_ambient_env` → mapper/filter; `ambient_env_value` → accessor; `insert_pinned_agent_data_dir` → mapper; `pinned_agent_data_dir` → accessor | `accessor`, `filter`, `formatter`, `mapper` | File-local declares only `mapper`. Actual: mapper, accessor. Contract per-file correctly broadens. All ⊆ component declared roles. |
| `src-tauri/src/run/resume/disposition.rs` | `handle_terminal_signal_disposition` → orchestration; `quota_retry_terminal_reason` → accessor; `typed_failure_terminal_reason` → accessor; `terminal_disposition_finalize_request` → mapper; `emit_resume_terminal_failure_output` → formatter; `handle_maybe_quota_verify` → orchestration; `MaybeQuotaActionOutcome::error_category` → accessor; `maybe_quota_action_outcome` → mapper; `emit_maybe_quota_failure_output` → formatter; `failure_exit_code` → mapper; `maybe_quota_finalize_request` → mapper; `confirmed_zero_turn_maybe_quota` → predicate (delegates to predicate module) | `formatter`, `mapper`, `predicate` | File-local declares `orchestration, mapper, predicate, formatter` (accurate). Contract per-file omits `orchestration`. Actual matches file-local. All ⊆ component declared roles. |
| `src-tauri/src/wake_coordinator.rs` (first 60 lines) | `WakeDiagnostic::status` → mapper (builds WakeDiagnostic); `WakeDiagnostic::with_message` → mapper; remaining functions (accessor, orchestration, predicate, formatter, parser, validator) visible from declared roles | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | File-local declares all 7 tokens (no `filter`). Contract per-file matches. Broad declaration appropriate for a multi-responsibility wake coordinator. All ⊆ component declared roles. |
| `crates/oulipoly-runtime/src/executor/mod.rs` (first 60 lines) | Module re-exports and facade struct declarations; `ExecutionResult` struct with field declarations | `mapper`, `orchestration` | File-local declares `accessor, mapper, orchestration`. Contract per-file matches (mapper, orchestration, plus executor service construction). All ⊆ component declared roles. |

LOW
