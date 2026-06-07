# LSV Cohesion Auditor Prompt

You are acting as `/home/nes/ai/agents/cohesion-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate`
- `wu_id=lsv`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/touched-files.txt`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/diff.patch`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/cohesion-auditor.md`

Important context:

- Functional source commit is `8a11fba`; functional commit 7d76426 (incremental bounded launch stream parsing) plus declaration prep 8a11fba, doc-comments only) (external-path terminal-error honesty parity: external launch/resume mappers consume the shared failure-exit/reason rules owned by terminal_signal.rs; provider failure terminal_signal + real exited(0) finalizes failed with provider-error terminal_reason; clean and real-nonzero paths unchanged). Audited remediation head is `2fe6745` (function splits plus declaration/header syncs; no behavior change intended).
- There are no artifact-only commits in range.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
