# S10B Function Classification Split 1 Prompt

You are acting as `/home/nes/ai/agents/function-classification-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

This is an intentional split of the S10B Phase 6 function-classification audit. This split audits only the files listed in `changed_files_path`; sibling split reports cover the remaining S10B touched files. Whole-file ownership applies to the listed files for this split.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/.scratch/code-quality/s10b/fc-splits/diffs/fc-1-provider.diff`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/.scratch/code-quality/s10b/fc-splits/touched/fc-1-provider.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/fc-splits/fc-1-provider.md`
- `base_ref=fce3836`
- `head_ref=d14b1ae6fd061725a16994d1f53a9e5f5e2b468e`
- `code_quality_ref=/home/nes/ai/conventions/code-quality.md`

Split files:

- `crates/oulipoly-provider/src/error.rs`
- `crates/oulipoly-provider/src/generated.rs`
- `crates/oulipoly-provider/tests/client_invoke.rs`

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
