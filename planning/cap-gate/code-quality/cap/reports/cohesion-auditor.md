# Cohesion Audit

## Inputs Read

| Input | Path |
|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate` |
| wu_id | `cap` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/contracts/cap.contract.md` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/gates/touched-surfaces.md` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/cap-gate/code-quality/cap/reports/cohesion-auditor.md` |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Read lines 21-27 for Auditor Scope Boundary, lines 143-149 for Touched-file ownership, lines 161-173 for Phase 6 component declared roles/contract visibility, and lines 295-300 for the A1 `Cohesion by classifications touched` row. |
| `~/ai/conventions/proposer-critic-pattern.md` | Read lines 29-35 for critic independence and no proposer self-critique. |
| `~/ai/conventions/risk-profile.md` | Read lines 13-16 for touched-file ownership alignment with code-quality auditors. |
| `~/ai/workflows/implementation-pipeline.md` | Read lines 490-491 for Phase 6 contract-read and per-component code-quality blocking semantics, and line 509 for Phase 6 gate-set participation. |
| `planning/cap-gate/proposal.md` | Read lines 1-9 for the production/test-only substance split and lines 11-27 for proof plan claims. |
| `planning/cap-gate/contracts/cap.contract.md` | Read lines 5-24 for component/per-file declared roles, lines 26-42 for the function inventory, and lines 44-46 for the test-infrastructure note. |
| `planning/cap-gate/gates/touched-surfaces.md` | Read lines 5-13 for the production capture-time backfill surfaces and lines 15-19 for the test-only isolation sweep. |
| `planning/cap-gate/gates/diff.patch` | Read changed-file evidence, including production hunks at lines 41-290 and test/fixture hunks starting at lines 292 and 389. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `touched-surfaces.md` lines 5-7 name the None-to-Some streamed-capture transition hook. `diff.patch` lines 207-290 touch this file. `cap.contract.md` line 21 declares per-file roles. | Scored as a whole touched production file under `code-quality.md` lines 21-27 and 143-149. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `touched-surfaces.md` lines 8-9 name the factored mark-running seam. `diff.patch` lines 67-206 touch this file. `cap.contract.md` line 22 declares per-file roles. | Scored as a whole touched production file under `code-quality.md` lines 21-27 and 143-149. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `touched-surfaces.md` lines 10-11 name this as a call-site adjustment. `diff.patch` lines 41-53 touch this file. `cap.contract.md` line 23 declares per-file roles. | Scored as a whole touched production file under `code-quality.md` lines 21-27 and 143-149. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `touched-surfaces.md` lines 10-11 name this as a call-site adjustment. `diff.patch` lines 54-64 touch this file. `cap.contract.md` line 24 declares per-file roles. | Scored as a whole touched production file under `code-quality.md` lines 21-27 and 143-149. |
| Test-only `OULIPOLY_DATA_DIR` isolation sweep | `touched-surfaces.md` lines 15-19 describe commit `9ba1275` as test-only and no-production-change. `cap.contract.md` lines 44-46 state it introduces no production behavior and is not included in the production function inventory. `diff.patch` includes broad test/fixture hunks beginning at lines 292 and 389. | Context-only for this cohesion pass. The supplied Phase 6 contract excludes this sweep from the production function inventory, and this operator is not a test-review auditor. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` | LOW | blocking target | Declared roles are `mapper`, `orchestration`, `predicate` in `cap.contract.md` line 21. Source lines 1-9 declare mapper/orchestration/predicate; `SupervisorConfig` mapping appears at source lines 60-81; supervised lifecycle orchestration appears at source lines 122-209; capture observation and live-signal predicate use appear at source lines 211-233 and 235-256. Actual classifications are a subset of the declared set. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser` | LOW | blocking target | Declared roles are `accessor`, `formatter`, `mapper`, `orchestration`, `parser` in `cap.contract.md` line 22. Source lines 1-8 declare formatter/mapper/orchestration/parser; accessors appear at source lines 43-56; context and runtime-record mapping appears at source lines 59-77, 112-123, and 155-172; orchestration seams appear at source lines 79-110, 133-153, and 182-192; formatter warnings appear at source lines 125-130, 174-179, and 194-219; parsing appears at source lines 221-223. Actual classifications are a subset of the declared set. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `formatter`, `mapper`, `orchestration`, `validator` | LOW | blocking target | Declared roles are `formatter`, `mapper`, `orchestration`, `validator` in `cap.contract.md` line 23. Source lines 1-13 declare those roles; interactive launch orchestration appears at source lines 76-115; provider-arg validation appears at source lines 179-184; stable error formatting appears at source lines 186-191; result mapping appears at source lines 193-210. Actual classifications are a subset of the declared set. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking target | Declared roles are `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` in `cap.contract.md` line 24. Source evidence includes orchestration at lines 83-110 and 565-615, client/control validation at lines 112-126 and 947-967, protocol parsing at lines 142-177 and 878-908, frame/response formatting at lines 179-186 and 935-943, path/hash mapping at lines 421-496, accessors at lines 379-385, and predicates at lines 66-80, 507-523, 669-671, and 763-765. Actual classifications are a subset of the declared set. |

## Evidence For Non-LOW Scores

| score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No non-LOW cohesion score was found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | n/a | n/a | No context-only non-LOW cohesion concerns were scored outside the production component. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6 contract is readable and contains a parseable `## Component declared roles` section at `cap.contract.md` lines 5-15 plus per-file role declarations at lines 17-24. The A1 metric row is present in `code-quality.md` lines 295-300. The broad `9ba1275` test-only sweep is recorded as context because the supplied contract excludes it from the production function inventory at `cap.contract.md` lines 44-46 and the touched-surface artifact states there are no production changes at `touched-surfaces.md` lines 15-19.

Final verdict: LOW

VERDICT: LOW
