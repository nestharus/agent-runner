# Cohesion Audit

## Inputs Read

| Input | Path / Value | Notes |
|---|---|---|
| mode | `phase-6` | Phase 6 code-quality gate. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` | Read; identifies PLK touched hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` | Read; lines 1-8 enumerate touched files. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` | Read; PLK behavior intent and proof plan. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` | Read; contains `## Component declared roles`. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | Read as context only; not used to waive cohesion scoring. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/cohesion-auditor.md` | This report. |

## Convention Binding

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Read before scoring. Applied `## Auditor Scope Boundary`, `## Touched-file ownership`, `## Component declared roles (multi-file WU components)`, `## Phase 6 contract visibility for code-quality auditors`, and the `Cohesion by classifications touched` threshold row. |

Metric applied: `Cohesion by classifications touched` is LOW when actual classifications are a subset of the declared role set, or exactly one classification when no declared role set exists. It is HIGH when actual classifications exceed the declared role set or include classifications outside it, or when an undeclared surface has two or more classifications. MEDIUM is not an available score for this row.

## Scope

The PLK touched surfaces are exactly the files in `planning/plk-gate/gates/touched-files.txt` lines 1-8:

| Touched surface | Scope note |
|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/src/dispatch.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/src/dispatch/parent_invocation.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/src/dispatch/predicate.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/src/invocation/mod.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/src/invocation/stale_reconcile.rs` | Whole touched file inspected for cohesion implications. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | Whole touched test harness inspected for cohesion implications. |
| `src-tauri/tests/pr_b_trace_integration.rs` | Whole touched test harness inspected for cohesion implications. |

The Phase 6a contract names one PLK component, `parent invocation linkage and stale-running PID sidecar reconciliation`, and declares component roles: `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter`, `validator`, `filter` (`planning/plk-gate/contracts/plk.contract.md` lines 3-8). Count-only fallback does not apply because the component declared role set is present and parseable.

## Cohesion Classification Inventory

| Surface | Observed classifications | Declared-role coverage | Verdict | Evidence |
|---|---|---|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | `orchestration`, `accessor` | Covered by component roles. | LOW | Source lines 11-24 load trace environment and sessions config through delegated helpers. |
| `src-tauri/src/dispatch.rs` | `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter` | Covered by component roles. | LOW | Source lines 3-5 declare the same full A1 role set; touched test additions at lines 1004-1026 remain validator work. |
| `src-tauri/src/dispatch/parent_invocation.rs` | `orchestration`, `accessor` | Covered by component roles. | LOW | Source lines 5-20 sequence parent env parsing and same-DB UUID lookup through accessor helpers. |
| `src-tauri/src/dispatch/predicate.rs` | `predicate`, `accessor` | Covered by component roles. | LOW | Source lines 6-24 answer diagnostics-model, resume-line, and execution-success predicates. |
| `src-tauri/src/invocation/mod.rs` | `mapper` module-namespace exposure | Covered by component roles. | LOW | Source lines 1-3 expose invocation child modules, including `stale_reconcile`. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter` | Covered by component roles. | LOW | Source lines 33-244 split reconciliation, sidecar access, row mapping, timestamp parsing, liveness predicates, and error formatting into named helpers. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `orchestration`, `accessor`, `mapper`, `parser`, `formatter`, `validator`, `predicate`, `filter` | Covered by component roles. | LOW | Source lines 115-229 add nested `agent-bash` orchestration, command formatting, environment mapping, status access, binary validation, invocation parsing, and marker filtering; source lines 343-399 validate nested parent linkage and malformed/unresolved parent env handling. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `orchestration`, `accessor`, `mapper`, `validator` | Covered by component roles. | LOW | Source lines 78-85 map/access sidecar and DB paths, lines 134-174 seed stale-running and PID sidecar fixtures, and lines 311-342 validate durable stale-running reconciliation. |

## Findings

| ID | Severity | Surface | Evidence | Blocking or residual |
|---|---|---|---|---|
| none | n/a | n/a | All observed A1 classifications in the PLK touched component are a subset of the readable Phase 6a component declared role set. | n/a |

## Stop-Condition Notes

No stop condition fired. The required `contract_path` and `proposal_path` were readable, the contract contains a parseable `## Component declared roles` section, and the audit was limited to the PLK touched surfaces plus immediate cohesion implications.

VERDICT: LOW
