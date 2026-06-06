# Coupling Audit

## Inputs Read

| Input | Path / Value | Notes |
|---|---|---|
| mode | `phase-6` | Phase 6 per-component code-quality gate. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` | Read lines 1-615. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` | Read lines 1-8. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` | Read lines 1-47. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` | Read lines 1-249, including exact adapter and intrinsic-surface declaration carriers. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | Read lines 1-80 as validation context. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/coupling-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/prompts/coupling-auditor.prompt.md` | Read lines 1-20; applied the PLK Phase-6 inputs and exact terminal verdict-line requirement. |
| `/home/nes/ai/agents/coupling-auditor.md` | Read lines 1-247; applied Phase 6 contract visibility, adapter/intrinsic-surface declaration validation, and report shape. |
| `/home/nes/ai/conventions/code-quality.md` | Read lines 1-328; applied `## Auditor Scope Boundary`, `## Touched-file ownership`, `## Adapter declarations`, `## Intrinsic-surface declarations`, LOW-only disposition, and the A1 coupling row at line 300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Read lines 1-67; maintained critic-only posture. |
| `/home/nes/ai/conventions/risk-profile.md` | Read lines 1-79; applied touched-file ownership cross-reference and evidence requirement for non-LOW scores. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Read lines 1-554; applied Phase 6 code-quality fanout and LOW-only blocking semantics at lines 489-491. |

Metric binding verified: `/home/nes/ai/conventions/code-quality.md:300` contains `Coupling by distinct external symbols/modules referenced` with LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | Touched file list line 1; diff lines 1-18; source lines 1-24; contract lines 13 and 174-184. | Whole file is touched-owned. Declared intrinsic surface for `trace_pre_render_reconciliation`. |
| `src-tauri/src/dispatch.rs` | Touched file list line 2; diff lines 19-54; source lines 1-1089; contract lines 14, 127-134, and 185-205. | Whole file is touched-owned. Declared adapter and intrinsic surface in the Phase 6 contract. |
| `src-tauri/src/dispatch/parent_invocation.rs` | Touched file list line 3; diff lines 55-70; source lines 1-20; contract lines 15, 113-119, and 157-164. | Declared adapter and intrinsic surface for parent invocation linkage. |
| `src-tauri/src/dispatch/predicate.rs` | Touched file list line 4; diff lines 73-95; source lines 1-24; contract lines 16 and 206-213. | Declared intrinsic surface for dispatch predicates. |
| `src-tauri/src/invocation/mod.rs` | Touched file list line 5; diff lines 96-103; source lines 1-3; contract lines 17 and 214-220. | Declared intrinsic surface for invocation namespace exports. |
| `src-tauri/src/invocation/stale_reconcile.rs` | Touched file list line 6; diff lines 104-353; source lines 1-244; contract lines 18, 120-126, and 165-173. | New touched file. Declared adapter and intrinsic surface for stale-running PID sidecar reconciliation. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | Touched file list line 7; diff lines 354-512; source lines 1-700; contract lines 19 and 135-142. | Touched Unix integration harness. Declared adapter with five translated contracts. |
| `src-tauri/tests/pr_b_trace_integration.rs` | Touched file list line 8; diff lines 513-615; source lines 1-438; contract lines 20 and 143-150. | Touched Unix trace integration harness. Declared adapter with five translated contracts. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | Trace StateDb/session/reconciliation setup | 7 raw references: `StateDb::open_default`, `reconcile_stale_running_invocations`, default config root, `SessionsConfig::load`, trace mapper, trace formatter, `Path` | n/a | n/a | n/a | n/a | n/a | `planning/plk-gate/contracts/plk.contract.md:174-184` | `src-tauri/src/commands/trace/accessor.rs` | `trace_pre_render_reconciliation` | Default StateDb open, default sessions path, sessions load, error formatting, mapper handoff, stale reconciliation before rendering | 1 | LOW | LOW | blocking | Source lines 11-23 are subordinate to the declared `Owns:` items at contract lines 178-184. |
| `src-tauri/src/dispatch.rs` | CLI dispatch, runtime/service wiring, and dispatch test fixtures | >= 16 raw references/modules across CLI structs, runtime services, command handlers, dispatch-local modules, env mutation, StateDb fixtures, and UUID parsing | `planning/plk-gate/contracts/plk.contract.md:127-134` | `src-tauri/src/dispatch.rs` | CLI argument model/subcommand routing; runtime execution/resume service entrypoints; command-handler/wiring module boundaries; dispatch-local parser/predicate/formatter/clock/failure-marker modules; dispatch test parent-env and StateDb fixture surfaces | 5 | LOW | `planning/plk-gate/contracts/plk.contract.md:185-205` | `src-tauri/src/dispatch.rs` | `cli_lifecycle_orchestration` | Lifecycle loops, session replacement recovery, CLI structs, resume prompt/error formatting, command/run/usage/wiring dispatch, dispatch-local modules, StateDb parent lookup fixtures, CompositeInvocationId env values, locked process-env mutation | 1 | LOW | LOW | blocking | Source lines 56-81, 83-449, and 451-1089 are subordinate to the five adapter contracts and the intrinsic `Owns:` set. |
| `src-tauri/src/dispatch/parent_invocation.rs` | Parent env value, CompositeInvocationId grammar, same-StateDb UUID lookup | 5 raw references: `CompositeInvocationId`, `InvocationRecord`, `StateDb`, `std::env`, parent-env parser | `planning/plk-gate/contracts/plk.contract.md:113-119` | `src-tauri/src/dispatch/parent_invocation.rs` | `OULIPOLY_PARENT_INVOCATION` environment value; `CompositeInvocationId` JSON/env grammar; same-StateDb invocation UUID lookup; StateDb row id used as `parent_invocation_id` | 4 | LOW | `planning/plk-gate/contracts/plk.contract.md:157-164` | `src-tauri/src/dispatch/parent_invocation.rs` | `parent_invocation_linkage` | Env consumption, malformed values resolving to no parent, unknown UUID resolving to no parent, same-DB UUID lookup tolerating source drift | 1 | LOW | LOW | blocking | Source lines 3-19 are subordinate to the declared env, grammar, and StateDb lookup contracts. |
| `src-tauri/src/dispatch/predicate.rs` | Dispatch predicate config/model surfaces | 3 raw references: `ModelConfig`, `HashMap`, `agent_runner_lib::load_app_config` | n/a | n/a | n/a | n/a | n/a | `planning/plk-gate/contracts/plk.contract.md:206-213` | `src-tauri/src/dispatch/predicate.rs` | `dispatch_predicates` | Diagnostics model configured predicate, diagnostics model config read, resume short-line predicate, execution success predicate | 1 | LOW | LOW | blocking | Source lines 3-23 are subordinate to the intrinsic predicate domain. |
| `src-tauri/src/invocation/mod.rs` | Invocation module namespace | 3 raw module exports: `finalize`, `result_envelope`, `stale_reconcile` | n/a | n/a | n/a | n/a | n/a | `planning/plk-gate/contracts/plk.contract.md:214-220` | `src-tauri/src/invocation/mod.rs` | `invocation_module_namespace` | Child module exports for `finalize`, `result_envelope`, and `stale_reconcile` | 1 | LOW | LOW | blocking | Source lines 1-3 exactly match the declared invocation namespace `Owns:` set. |
| `src-tauri/src/invocation/stale_reconcile.rs` | PID identity sidecar, OS liveness identity, StateDb running rows, StateDb terminal finalization | >= 15 raw references/modules including `chrono`, `StateDb`, `PidIdentityDb`, `PidIdentityRow`, `ProcessIdentity`, `read_live_process_identity`, `Path`, stale threshold, SQL running-row columns, sidecar lookup, process identity comparison, and StateDb finalization | `planning/plk-gate/contracts/plk.contract.md:120-126` | `src-tauri/src/invocation/stale_reconcile.rs` | PID identity sidecar records; OS process liveness identity reads; StateDb running invocation rows; StateDb terminal invocation finalization fields | 4 | LOW | `planning/plk-gate/contracts/plk.contract.md:165-173` | `src-tauri/src/invocation/stale_reconcile.rs` | `stale_running_pid_sidecar_reconciliation` | Read-only sidecar open, stale threshold, conservative live/dead/unknown handling, stale terminal fields, PID evidence remaining in sidecar storage | 1 | LOW | LOW | blocking | Source lines 33-244 are subordinate to the four adapter contracts and intrinsic `Owns:` items; sidecar access is read-only unless tests seed fixtures, and StateDb writes are limited to stale terminal finalization. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | Unix invocation integration harness, parent env, agent-bash, StateDb assertions, trace helper | >= 10 raw references/modules including `oulipoly_state`, `rusqlite`, `serde_json`, filesystem, Unix permissions, paths, process spawning, time polling, runner binary, env vars, and `agent-bash` status | `planning/plk-gate/contracts/plk.contract.md:135-142` | `src-tauri/tests/pr_a_invocation_integration.rs` | Unix runner fixture/config files; StateDb invocation assertions/fixture SQL; parent/invocation marker JSON; agent-bash run/status with isolated `XDG_STATE_HOME`; trace CLI JSON helper | 5 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Source lines 3-260 and 262-700 are subordinate to the five declared integration-harness contracts; contract count is at the adapter threshold, not above it. |
| `src-tauri/tests/pr_b_trace_integration.rs` | Unix trace integration harness, StateDb fixtures, PID sidecar fixtures, JSON trace output | >= 10 raw references/modules including `chrono`, `PidIdentityDb`, `PidIdentityRecord`, `ProcessIdentity`, `StateDb`, `rusqlite`, JSON, filesystem, Unix permissions, paths, process spawning, and runner binary | `planning/plk-gate/contracts/plk.contract.md:143-150` | `src-tauri/tests/pr_b_trace_integration.rs` | Unix trace CLI fixture and JSON output; StateDb running/stale row fixtures; PidIdentityDb sidecar records and ProcessIdentity values; isolated XDG roots; fixture provider shell command/model config files | 5 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Source lines 3-209 and 212-438 are subordinate to the five declared integration-harness contracts; contract count is at the adapter threshold, not above it. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No MEDIUM or HIGH coupling scores were found in the touched PLK surfaces after applying the current Phase 6 adapter and intrinsic-surface declarations. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The prompt, code-quality convention, proposal, contract, diff, touched-surface list, runtime evidence, and required coupling-auditor references were readable.

The exact `## Adapter declarations` section at `planning/plk-gate/contracts/plk.contract.md:109-151` is structurally valid: every entry names a touched component, uses `role: adapter`, and has a non-empty `Translates:` list. No declared adapter exceeds the `N = 5` contract threshold.

The exact `## Intrinsic-surface declarations` section at `planning/plk-gate/contracts/plk.contract.md:153-221` is structurally valid: every entry names a touched component, uses `role: intrinsic-surface`, has exactly one `Domain:`, and has a non-empty `Owns:` list. No declared intrinsic surface exceeds the `N = 5` domain threshold.

The PLK-specific coupling concerns named by the prompt are covered by the current declarations: parent-env consumption resolves through same-StateDb UUID lookup, sidecar access is limited to `PidIdentityDb` records and OS liveness reads, StateDb access is limited to declared parent lookup, running-row reads, and terminal finalization fields, and module-boundary additions are declared under the invocation namespace or dispatch adapter surfaces.

VERDICT: LOW
