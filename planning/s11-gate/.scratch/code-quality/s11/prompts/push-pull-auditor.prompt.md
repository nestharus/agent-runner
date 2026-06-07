# S11 Push/Pull Auditor Prompt

You are acting as `/home/nes/ai/agents/push-pull-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=95699d6`
- `head_ref=7ec42d4`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/push-pull-auditor.md`

Important context:

- Audited net source range is `95699d6..7ec42d4`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` because the regenerated diff includes the committed S11 remediation source range.
- `scripts/opencode-turns` is bound to the public OpenCode CLI only; private DB fallback content was reverted and is out of the net source behavior.
- Sidecar mailbox/PID state and state-db reads are runner-owned interfaces declared in the S11 contract.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
