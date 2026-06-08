# Wakechain Function Classification Auditor Prompt

You are acting as `/home/nes/ai/agents/function-classification-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness`
- `base_ref=fcc0faf`
- `head_ref=HEAD plus prior #43 split-only carry-over if present in the working tree`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate`
- `wu_id=wakechain`
- `diff_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md`
- `risk_profile_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/evidence/multi-classifier-risk.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/function-classification-auditor.md`

Important context:

- Review production functions added/changed by `fcc0faf..HEAD` and any behavior-identical split-only carry-over. Test helpers may be inventoried when the operator requires it, but test-only function splitting should not be confused with runtime behavior changes.
- The prior #43 and #42 gates separately reached 6/6 LOW. Their declarations are merged into the wakechain contract; the new #44 focus is the sweep-hardening surface: `wake_sweep_candidate_has_live_owner`, `mailbox_row_has_live_owner_identity`, `select_recoverable_sweep_candidates`, `reap_abandoned_sweep_candidates`, `mark_pending_abandoned`, and related changes in `wake_coordinator.rs`, `mailbox.rs`, and `mailbox_delivery.rs`.
- The risk profile is not a waiver. If a genuine multi-classifier risk remains, emit a non-LOW finding and state the required split. For huge touched files such as `crates/oulipoly-state/src/db.rs`, keep splitting/decomposing the finding set rather than escalating because of file size alone.
- Commit hygiene is waived for this gate.
- This auditor must be dispatched pinned with `--rotate-provider opencode4` for this run. Never use an unpinned run and never use `opencode2` or `opencode3`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
