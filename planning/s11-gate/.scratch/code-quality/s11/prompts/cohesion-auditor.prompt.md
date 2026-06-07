# S11 Cohesion Auditor Prompt

You are acting as `/home/nes/ai/agents/cohesion-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate`
- `wu_id=s11`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/touched-files.txt`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/cohesion-auditor.md`

Important context:

- Audited net source range is `95699d6..7ec42d4`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` because the regenerated diff includes the committed S11 remediation source range.
- Historical `planning/s10b-gate/.scratch/**` logs in the touched list are artifacts, not product code components.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
