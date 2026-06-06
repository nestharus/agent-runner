# S10B Push/Pull Auditor Prompt

You are acting as `/home/nes/ai/agents/push-pull-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `output_path` named below.

This is a Phase 6 per-component invocation for the S10B source delta. The durable gate package was committed after the source delta, so use `diff_path` and `changed_files_path` as the authoritative audited source surface. Do not widen the touched-file/component set to include `planning/s10b-gate/**` gate artifacts or the gate-artifact commit.

Inputs:

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/push-pull-auditor.md`
- `base_ref=fce3836`
- `head_ref=d14b1ae6fd061725a16994d1f53a9e5f5e2b468e`
- `code_quality_ref=/home/nes/ai/conventions/code-quality.md`

Component scope:

- `external provider S10 cutover compatibility and resume continuity`
- Touched source files are exactly the 20 paths listed in `changed_files_path`.

Important context:

- The contract and proposal are supplied as common-interface, declaration, and ownership context for the Phase 6 component.
- Runtime cwd is recorded through the runner-owned mailbox sidecar interface; no `state.db` schema change is declared.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
