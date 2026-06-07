# OEHX Push/Pull Auditor Prompt

You are acting as `/home/nes/ai/agents/push-pull-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=33775d7`
- `head_ref=HEAD`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate`
- `wu_id=oehx`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/push-pull-auditor.md`

Important context:

- Functional source commit is `807f35c` (external-path terminal-error honesty parity: external launch/resume mappers consume the shared failure-exit/reason rules owned by terminal_signal.rs; provider failure terminal_signal + real exited(0) finalizes failed with provider-error terminal_reason; clean and real-nonzero paths unchanged).
- There are no artifact-only commits in range.
- OpenCode classification must use the public structured stream event contract only. No private OpenCode DB fallback exists in this delta.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
