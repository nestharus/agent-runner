# LSV Function Classification Auditor Prompt

You are acting as `/home/nes/ai/agents/function-classification-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=80d6904`
- `head_ref=HEAD (2fe6745)`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate`
- `wu_id=lsv`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/diff.patch`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md`
- `risk_profile_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/multi-classifier-risk.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/function-classification-auditor.md`

Important context:

- Functional source commit is `7d76426` (launch JSONL stdout parses incrementally with bounded retention; valid streams over the old 1MiB capture limit finalize from the exit event instead of failing stdout_limit_exceeded; non-launch one-shot caps and heartbeat-gap liveness unchanged). Declaration prep `8a11fba` is doc-comments only. Audited remediation head is `2fe6745` (function splits plus declaration/header syncs; no behavior change intended).
- There are no artifact-only commits in range.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.
- The risk profile is not a waiver. If a genuine multi-classifier risk remains, emit a non-LOW finding and state the required split.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
