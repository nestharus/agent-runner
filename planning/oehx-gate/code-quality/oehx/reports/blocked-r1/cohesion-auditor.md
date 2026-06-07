# Cohesion Audit

## Inputs Read

| Input | Path | Status |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Read target root supplied; no source scoring performed after stop condition. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Read target identity supplied. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` | Read target planning root supplied. |
| `wu_id` | `oehx` | Read. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Read. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | Unreadable: file not found. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt` | Read. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | Read. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/cohesion-auditor.md` | Written. |

## References Read

| Reference | Path | Status |
|---|---|---|
| Operator | `/home/nes/ai/agents/cohesion-auditor.md` | Read. |
| Code quality convention | `/home/nes/ai/conventions/code-quality.md` | Read. A1 row `Cohesion by classifications touched` is present at `## Numerical thresholds`. |
| Proposer / critic pattern | `/home/nes/ai/conventions/proposer-critic-pattern.md` | Read. |
| Risk profile convention | `/home/nes/ai/conventions/risk-profile.md` | Read. |
| Implementation pipeline | `/home/nes/ai/workflows/implementation-pipeline.md` | Read. |
| Proposal under review | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Read. |
| Phase 6a contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | Unreadable: file not found. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| Not scored | `/home/nes/ai/agents/cohesion-auditor.md` lines 41 and 81 require a readable Phase 6 contract before scoring; `/home/nes/ai/conventions/code-quality.md` lines 169-173 require Phase 6 auditors to receive and read `contract_path` before scoring. | Stop condition fires before component-boundary resolution can be used for a verdict. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| Not scored | Not scored because the required Phase 6 contract is unreadable. | `BLOCKED:unreadable-contract-path` | blocking | `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` returned file not found. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| `BLOCKED:unreadable-contract-path` | blocking | Phase 6 contract is a required input before scoring; this is a stop condition, not a cohesion score. | `/home/nes/ai/agents/cohesion-auditor.md` lines 41 and 81; `/home/nes/ai/conventions/code-quality.md` lines 169-173; unreadable path `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md`. | The operator forbids falling back to generic or count-only judgment when the Phase 6 contract is missing or unreadable. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concerns were scored before the stop condition. |

## Residual Ambiguity / Stop-Condition Notes

The supplied `contract_path` is required for Phase 6 cohesion scoring. It is unreadable because the file does not exist at `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md`. Per the operator and code-quality convention, this requires `BLOCKED:unreadable-contract-path`; the audit must not infer component declared roles or apply count-only fallback.

BLOCKED:unreadable-contract-path
