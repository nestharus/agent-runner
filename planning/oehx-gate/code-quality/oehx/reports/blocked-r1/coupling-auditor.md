# Coupling Audit

## Inputs Read

| Input | Path | Status |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | readable |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | context |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` | readable context |
| `wu_id` | `oehx` | read |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | readable |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | unreadable: file not found |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt` | readable |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | readable |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/coupling-auditor.md` | written |

## References Read

| Reference | Path | Evidence |
|---|---|---|
| Coupling auditor operator | `/home/nes/ai/agents/coupling-auditor.md` | Required Phase 6 contract handling says missing or unreadable `contract_path` is `BLOCKED:unreadable-contract-path` and not permission to infer adapter or intrinsic-surface status. |
| Code quality convention | `/home/nes/ai/conventions/code-quality.md` | `## Auditor Scope Boundary`, `## Touched-file ownership`, `## Adapter declarations`, `## Intrinsic-surface declarations`, and A1 row `Coupling by distinct external symbols/modules referenced`: LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6` were read. |
| Proposer/critic pattern | `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer rerun semantics read. |
| Risk profile convention | `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference and evidence requirements read. |
| Implementation pipeline | `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 per-component code-quality and required `contract_path` semantics read. |
| Proposal under review | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Read before scoring attempt. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| Not scored | `contract_path` input `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` is unreadable. | Phase 6 contract reading is blocking before adapter/intrinsic declaration resolution or raw fallback scoring. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| Not scored | Not scored | n/a | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | n/a | n/a | n/a | `BLOCKED:unreadable-contract-path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | n/a | n/a | n/a | n/a | `BLOCKED:unreadable-contract-path` | `BLOCKED:unreadable-contract-path` | blocking | Supplied Phase 6 contract path cannot be read. The operator and code-quality convention require fail-closed behavior rather than declaration inference or raw generic fallback in Phase 6. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| `BLOCKED:unreadable-contract-path` | blocking | Phase 6 input contract is required before scoring any touched component. | Read attempt for `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` returned file-not-found. `glob planning/oehx-gate/contracts/*` showed `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`, but the supplied `contract_path` is different. | The operator states that in Phase 6, missing or unreadable `contract_path` is `BLOCKED:unreadable-contract-path`, never permission to infer adapter or intrinsic-surface status or fall back to raw generic coupling. |

## Residual Ambiguity / Stop-Condition Notes

Stop condition reached before scoring: the required Phase 6 `contract_path` is unreadable. No adapter declarations or intrinsic-surface declarations were resolved. No per-pair A1 coupling verdict was computed.

BLOCKED:unreadable-contract-path
