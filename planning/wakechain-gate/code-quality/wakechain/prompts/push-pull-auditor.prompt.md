# Wakechain Push/Pull Auditor Prompt

You are acting as `/home/nes/ai/agents/push-pull-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `base_ref=fcc0faf`
- `head_ref=HEAD plus prior #43 split-only carry-over if present in the working tree`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `wu_id=wakechain`
- `diff_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/push-pull-auditor.md`

Important context:

- Score push/pull boundaries over session evidence ingestion, mailbox sidecar state, wake-claim ownership, and sweep candidate selection. OpenCode adapter parsing should remain adapter-local and not pull wake-delivery policy into the script.
- #44 should push abandoned-row mutation through the mailbox sidecar API and pull only resumability/live-owner decisions needed by the coordinator.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode5` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
