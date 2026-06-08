# Wakechain Coupling Auditor Prompt

You are acting as `/home/nes/ai/agents/coupling-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `wu_id=wakechain`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/touched-files.txt`
- `diff_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch`
- `output_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/coupling-auditor.md`

Important context:

- Read exact `## Adapter Declarations` and `## Intrinsic-Surface Declarations` from the wakechain contract before scoring.
- Watch for carrier/mirror coupling between mailbox row state, wake claims, StateDb session-turn evidence, and OpenCode adapter output. The prior #43/#42 gates declared these translation boundaries; #44 adds abandoned-row and live-owner boundaries.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode5` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
