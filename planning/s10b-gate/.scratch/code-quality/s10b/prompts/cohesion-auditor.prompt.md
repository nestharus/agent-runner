# S10B Cohesion Auditor Prompt

You are acting as `/home/nes/ai/agents/cohesion-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

This is a Phase 6 per-component invocation for the S10B source delta. The durable gate package was committed after the source delta, so use `diff_path` and `touched_surfaces_path` as the authoritative audited source surface. Do not widen the touched-file/component set to include `planning/s10b-gate/**` gate artifacts or the gate-artifact commit.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate`
- `wu_id=s10b`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/touched-files.txt`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/cohesion-auditor.md`

Component scope:

- `external provider S10 cutover compatibility and resume continuity`
- Touched source files are exactly the 20 paths listed in `touched_surfaces_path`.

Important context:

- The contract's component declared role set is the source-backed union for this multi-file component and contains only valid A1 role tokens.
- Score the component against the component-level declared role set before applying count-only fallback, per `/home/nes/ai/conventions/code-quality.md` Phase 6 contract visibility rules.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
