# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root supplied by caller. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same repository identity as `worktree_path`. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate` | Inferred from supplied S10B planning artifacts. |
| `wu_id` | `S10B` | Work Unit identifier supplied by caller. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md` | Read before Phase 6 scoring decision. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` | Read before Phase 6 scoring decision. The consumed component-role declaration is malformed for A1 cohesion. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/touched-files.txt` | Read; lists 20 touched files. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch` | Read; regenerated from base range `fce3836..HEAD` per evidence log. |
| `evidence_log` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/evidence/runtime-tests.log` | Read as supplemental currentness and touched-diff evidence. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/cohesion-auditor.md` | This report, overwritten as requested. |
| `problem_map_path` | not supplied | Not required for this Phase 6 contract-based invocation. |
| `risk_profile_path` | not supplied | Not required for this Phase 6 contract-based invocation. |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Read `## Auditor Scope Boundary`, `## Touched-file ownership`, `## Declared roles`, `### Component declared roles`, `### Phase 6 contract visibility for code-quality auditors`, and `## Numerical thresholds`. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Read critic/proposer separation and acceptance semantics. |
| `/home/nes/ai/conventions/risk-profile.md` | Read touched-file ownership clause tying risk profiles and code-quality auditors to whole touched files/components. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Read Phase 6 per-component code-quality rules, including lines 489-491, after the initial workflow reference read. |

Metric binding verified: `/home/nes/ai/conventions/code-quality.md` lines 295-300 still contain `Cohesion by classifications touched`: LOW when actual classifications are a subset of the declared role set, or exactly 1 classification for components/files without declared roles; HIGH when actual classifications exceed/include classifications outside the declared role set, or 2 or more classifications without declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `external provider S10 cutover compatibility and resume continuity` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` lines 3-8 name `## Component declared roles`, the component, and the declared role list. `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` lines 9-32 list the touched files in scope. `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/touched-files.txt` lines 1-20 list the same 20 files. `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/evidence/runtime-tests.log` lines 3-5 state the diff and touched-file list were regenerated from `git diff fce3836..HEAD`. | Phase 6 multi-file WU component boundary is unambiguous, but the component-level declared role set required for the cohesion subset check is malformed because it includes `test-harness`, which is outside the A1 vocabulary. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `external provider S10 cutover compatibility and resume continuity` | Not scored. | `BLOCKED:malformed-contract-path` | blocking | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` line 7 declares `test-harness` in the component declared role set. `/home/nes/ai/conventions/code-quality.md` lines 133-135 require declared role tokens to come from the A1 category vocabulary: `orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`. `/home/nes/ai/conventions/code-quality.md` lines 161-165 require Phase 6 component declared roles to be consumed before fallback scoring. `/home/nes/ai/conventions/code-quality.md` lines 169-173 require `BLOCKED:<reason>` when the Phase 6 contract is malformed for the declaration family being consumed, rather than inferred role scoring. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| `BLOCKED:malformed-contract-path` | blocking | The malformed declaration is in the Phase 6 contract for the touched component itself, not in context-only evidence. | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` lines 3-8. | The consumed `## Component declared roles` section includes `test-harness`; A1 cohesion declared roles cannot include non-A1 tokens, so the required component-level subset check cannot be performed. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | No context-only cohesion concerns were scored. | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

Stop condition fired: `BLOCKED:malformed-contract-path`.

The contract is readable and non-blank, but the exact Phase 6 component declared role set needed by this auditor contains a non-A1 token. Per `/home/nes/ai/conventions/code-quality.md` lines 169-173 and `/home/nes/ai/workflows/implementation-pipeline.md` lines 489-491, this is not permission to ignore the invalid token or fall back to count-only generic scoring. Closure expectation: rerun this cohesion auditor after the Phase 6 contract carries a parseable A1 component declared role set.

OVERALL: BLOCKED:malformed-contract-path
