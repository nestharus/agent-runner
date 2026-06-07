# Cohesion Audit

## Inputs Read

| Input | Path | Evidence |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection resolved paths from this worktree. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Repo identity only; source inspection used `worktree_path`. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate` | Report destination is under this planning tree. |
| wu_id | `abd` | Used for report identity. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-b/proposal.md` | Read lines 1-395 before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/contracts/abd.contract.md` | Read lines 1-648 before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/diff.patch` | Read diff evidence and diff headers. Production source diff headers appear at lines 29, 58, 100, 245, 307, 496, 640, 925, 939, 2582, 4716, 4794, 4809, 5507, 6106, 6189, 6454, 6510, 6553, 6658, 6682, 6700, 6868, and 6975. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/gates/touched-surfaces.md` | Read lines 1-29; identifies the whole-file touched production scope. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/abd-gate/code-quality/abd/reports/cohesion-auditor.md` | Written by this audit. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Read lines 1-328. A1 row at lines 295-300; Auditor Scope Boundary at lines 21-27; Touched-file ownership at lines 143-149; Phase 6 contract visibility at lines 169-173. |
| `~/ai/conventions/proposer-critic-pattern.md` | Read lines 1-67. Critic independence and proposer/critic separation at lines 29-35. |
| `~/ai/conventions/risk-profile.md` | Read lines 1-79. Touched-file ownership clause at lines 11-16. |
| `~/ai/workflows/implementation-pipeline.md` | Read lines 1-652. Phase 6 per-component code-quality fanout and contract-read rules at lines 403-491. |

Metric binding applied exactly: `Cohesion by classifications touched`: LOW when actual classifications are a subset of the declared role set, or exactly 1 classification with no declared roles; HIGH when actual classifications exceed the declared role set or include classifications outside it, or 2 or more classifications with no declared roles. MEDIUM is n/a. Source: `~/ai/conventions/code-quality.md:295-300`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| Per touched file | Contract says this WU should not be scored as one cohesive all-role component and should be scored per touched file using the table below, `planning/abd-gate/contracts/abd.contract.md:5-8`, `:21-48`. | This resolves the Phase 6 component boundary. |
| Touched production files | `planning/abd-gate/gates/touched-surfaces.md:3-20` lists new files; `:22-29` lists existing additive-hook files. | Whole touched files are in scope per `code-quality.md:21-27` and `:143-149`. |
| Diff corroboration | `planning/abd-gate/gates/diff.patch` has production source headers for the same touched files at lines 29, 58, 100, 245, 307, 496, 640, 925, 939, 2582, 4716, 4794, 4809, 5507, 6106, 6189, 6454, 6510, 6553, 6658, 6682, 6700, 6868, and 6975. | Tests, manifests, and planning artifacts in the diff are context, not scored production components for this A1 cohesion pass. |
| Existing wiring files | Caller notes existing run/resume/repl/balancing/dispatch/executor files were touched by minimal wiring and had prior LOW gating. | Whole-file ownership still applies, but pure sequencing of already-named helpers remains `orchestration`; it is not reclassified solely because helpers are predicates, mappers, accessors, or formatters. |

## Per-Component Cohesion

| Component | Declared role set used | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:25`; no production functions/facade in `abd.contract.md:54-56`; source facade role and re-exports at `crates/oulipoly-runtime/src/executor/cli.rs:1-11`, `:64-99`. |
| `crates/oulipoly-runtime/src/executor/cli/headless.rs` | `orchestration` | `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:26`; function inventory `abd.contract.md:58-65`; source functions at `crates/oulipoly-runtime/src/executor/cli/headless.rs:36-101`. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `formatter`, `mapper`, `orchestration`, `validator` | `formatter`, `mapper`, `orchestration`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:27`; function inventory `abd.contract.md:67-76`; source functions at `crates/oulipoly-runtime/src/executor/cli/interactive.rs:41-184`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `orchestration` | `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:28`; function inventory `abd.contract.md:78-83`; source functions at `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs:33-62`. |
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs` | `mapper`, `orchestration` | `mapper`, `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:29`; source role header and functions at `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs:1-3`, `:34-221`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `formatter`, `mapper`, `orchestration`, `parser` | `formatter`, `mapper`, `orchestration`, `parser` | LOW | blocking target, no finding | Contract roles `abd.contract.md:30`; function inventory `abd.contract.md:94-102`; source functions at `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs:21-136`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` | `mapper`, `orchestration`, `predicate` | LOW | blocking target, no finding | Contract roles `abd.contract.md:31`; source role header and functions at `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs:1-3`, `:56-175`. |
| `crates/oulipoly-state/src/lib.rs` | `accessor`, `validator` | `accessor`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:32`; no production functions/root re-export surface in `abd.contract.md:113-115`; source roles, re-exports, and doctest validation at `crates/oulipoly-state/src/lib.rs:1-50`, `:52-80`. |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:33`; function inventory `abd.contract.md:117-163`; source functions at `crates/oulipoly-state/src/mailbox.rs:163-1176`. |
| `crates/oulipoly-state/src/pid_identity.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:34`; function inventory `abd.contract.md:165-196`; source functions at `crates/oulipoly-state/src/pid_identity.rs:36-451`. |
| `src-tauri/src/commands/mailbox.rs` | `accessor`, `formatter`, `mapper`, `orchestration` | `accessor`, `formatter`, `mapper`, `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:35`; function inventory `abd.contract.md:198-205`; source functions at `src-tauri/src/commands/mailbox.rs:15-67`. |
| `src-tauri/src/commands/mod.rs` | `accessor` | `accessor` module exposure only | LOW | blocking target, no finding | Contract roles `abd.contract.md:36`; no production functions in `abd.contract.md:207-209`; source module declarations at `src-tauri/src/commands/mod.rs:1-15`. |
| `src-tauri/src/commands/notify.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:37`; function inventory `abd.contract.md:211-235`; source functions at `src-tauri/src/commands/notify.rs:51-690`. |
| `src-tauri/src/commands/pid_session.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:38`; function inventory `abd.contract.md:237-264`; source functions at `src-tauri/src/commands/pid_session.rs:65-588`. |
| `src-tauri/src/dispatch.rs` | `formatter`, `mapper`, `orchestration`, `validator` | `formatter`, `mapper`, `orchestration`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:39`; function inventory `abd.contract.md:266-291`; source production dispatch functions at `src-tauri/src/dispatch.rs:83-449`; test module starts at `:451`. |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | LOW | blocking target, no finding | Contract roles `abd.contract.md:40`; function inventory `abd.contract.md:293-304`; source functions at `src-tauri/src/mailbox_delivery.rs:17-239`. |
| `src-tauri/src/main.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate` | LOW | blocking target, no finding | Contract roles `abd.contract.md:41`; function inventory `abd.contract.md:306-321`; source functions at `src-tauri/src/main.rs:52-133`. |
| `src-tauri/src/migration_providers.rs` | `accessor`, `mapper`, `orchestration` | `accessor`, `mapper`, `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:42`; function inventory `abd.contract.md:323-332`; source functions at `src-tauri/src/migration_providers.rs:39-106`. |
| `src-tauri/src/run/balancing/finalization.rs` | `orchestration` | `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:43`; function inventory `abd.contract.md:334-343`; source functions at `src-tauri/src/run/balancing/finalization.rs:49-283`. Pure sequencing and delegated helper calls remain orchestration. |
| `src-tauri/src/run/balancing/orchestration.rs` | `formatter`, `mapper`, `orchestration` | `formatter`, `mapper`, `orchestration` | LOW | blocking target, no finding | Contract roles `abd.contract.md:44`; function inventory `abd.contract.md:345-358`; source functions at `src-tauri/src/run/balancing/orchestration.rs:43-489`. |
| `src-tauri/src/run/repl/orchestration.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:45`; function inventory `abd.contract.md:360-414`; source functions at `src-tauri/src/run/repl/orchestration.rs:39-825`. |
| `src-tauri/src/run/resume/orchestration.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:46`; function inventory `abd.contract.md:416-471`; source functions at `src-tauri/src/run/resume/orchestration.rs:43-955`. |
| `src-tauri/src/usage/cli.rs` | `mapper`, `parser`, `validator` | `mapper`, `parser`, `validator` | LOW | blocking target, no finding | Contract roles `abd.contract.md:47`; CLI schema surface in `abd.contract.md:473-475`; source Clap parser/validator/mapper declarations at `src-tauri/src/usage/cli.rs:18-103`, `:105-349`. |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking target, no finding | Contract roles include `validator` at `abd.contract.md:48`; function inventory includes validator/parser/predicate/accessor/orchestration/mapper evidence at `abd.contract.md:477-498`; source functions at `src-tauri/src/wake_coordinator.rs:36-572`. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No non-LOW component scores were found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concerns were needed. |

## Residual Ambiguity / Stop-Condition Notes

| Item | Note |
|---|---|
| Contract readability | `contract_path` was readable and contained `## Component declared roles` plus `## Per-file declared roles`; no `BLOCKED:unreadable-contract-path` or malformed-contract stop condition applied. |
| Boundary ambiguity | No `NEEDS_INPUT` boundary ambiguity: the contract explicitly chooses per-touched-file scoring rather than a single multi-file component. |
| A1 metric source | `Cohesion by classifications touched` was present in `~/ai/conventions/code-quality.md:295-300`; no `BLOCKED:A1-metric-source` applied. |
| Local role comments | Some source file-local comments are broader or narrower than the Phase 6 contract table. The invocation explicitly says the contract declares no blanket component role set and to score per touched file against the contract per-file declared-roles table; that table is the scoring source for this run. |
| Function-shape notes | Contract inventory names several `MULTI-CLASSIFIER-RISK` function-shape notes, but this A6 pass is bound only to A1 cohesion by classifications touched. Those notes do not become cohesion findings when the file's actual classification set remains a subset of its declared role set. |
| Pure-orchestrator rule | Existing orchestration functions that sequence already-named helpers were treated as orchestration and not reclassified solely because their callees are predicates, mappers, accessors, filters, validators, or formatters. |

VERDICT: LOW
