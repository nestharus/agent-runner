# Wakechain Validation Integrity Auditor Prompt

You are acting as `/home/nes/ai/agents/validation-integrity-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=pr-diff`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `diff_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch`
- `runtime_claim=The consolidated wakechain fix confirms delivery only from submitted or ingested user-turn evidence, parses/targets OpenCode current exports, reclaims dead wake claims without stealing live identity-matched owners, caps unconfirmed retry loops, and hardens the sweep so recoverable recent leaks are not starved by dead-owner backlog while abandoned debris is marked instead of retried forever.`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/evidence/runtime-tests.log`
- `decisions_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/DECISIONS.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `wu_id=wakechain`
- `report_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/validation-integrity-auditor.md`

Important context:

- Runtime evidence includes XDG-isolated `cargo fmt --check`, `cargo test --workspace`, the targeted wake-confirm and proactive-wake suites, and `bash scripts/tests/opencode-turns.test.sh` in `runtime-tests.log`.
- The #44 coverage claim is `wu_d_proactive_wake_integration::wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris`.
- The documented residual is bounded sweep cycles under pathological backlog; do not treat that residual as a validation weakening unless the diff creates a broader untested claim.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode5` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
