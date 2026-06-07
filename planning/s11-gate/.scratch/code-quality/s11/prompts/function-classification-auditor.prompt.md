# S11 Function Classification Auditor Prompt

You are acting as `/home/nes/ai/agents/function-classification-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=95699d6`
- `head_ref=549daaa`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md`
- `risk_profile_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/multi-classifier-risk.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/function-classification-auditor.md`

Important context:

- Audited net source range is `95699d6..549daaa`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` because the regenerated diff includes the committed S11 remediation source range.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/pre-split-multi-classifier-risk.md` preserves the stale pre-remediation HIGH findings. `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/multi-classifier-risk.md` is the current risk profile. Neither artifact is a waiver; current function bodies remain blocking if they mix categories.
- `crates/oulipoly-provider/src/generated.rs` has no generated-artifact exemption and must be audited as hand-maintained DTO code, consistent with S10B.
- Historical `planning/s10b-gate/.scratch/**` logs in the touched list are artifacts with no executable function inventory.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
