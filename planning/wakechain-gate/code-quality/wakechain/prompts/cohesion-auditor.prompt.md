# Wakechain Cohesion Auditor Prompt

You are acting as `/home/nes/ai/agents/cohesion-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `wu_id=wakechain`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/touched-files.txt`
- `diff_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch`
- `output_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/cohesion-auditor.md`

Important context:

- Score whether the consolidated changes keep responsibilities cohesive across session ingest, StateDb evidence lookup, mailbox sidecar state, delivery preparation, and wake sweep planning.
- #44 should keep abandoned-debris reaping in wake sweep coordination and sidecar mutation, not leak that policy into unrelated OpenCode adapter or resume-confirmation code.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode4` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
