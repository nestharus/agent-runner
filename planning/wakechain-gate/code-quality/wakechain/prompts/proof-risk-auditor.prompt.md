# Wakechain Proof Risk Auditor Prompt

You are acting as `/home/nes/ai/agents/proof-risk-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=phase-3-proposal`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `wu_id=wakechain`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/evidence/runtime-tests.log`
- `report_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/proof-risk-auditor.md`

Important context:

- The proof plan must bind the union of prior #43 and #42 claims plus #44 backlog hardening to shipped tests or executable adapter tests. The #44 row is `wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris`.
- Prior #43 and #42 reports are reusable context but not self-certifying for this consolidated tip. Re-score the current proposal/contract/evidence.
- The residual bounded-sweep backlog behavior is explicitly documented and should be scored as residual risk, not hidden proof coverage.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode4` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
