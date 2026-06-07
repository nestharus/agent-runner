# OEHX Proof Risk Auditor Prompt

You are acting as `/home/nes/ai/agents/proof-risk-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=phase-3-proposal`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate`
- `wu_id=oehx`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/proof-risk-auditor.md`

Important context:

- The exact `## Proof plan` must bind the four runtime claims to shipped tests: external launch with failure terminal_signal plus exited(0) finalizes failed with provider-error reason (envelope + invocation row); external resume same honesty; clean external paths unchanged (clean_exit+0 succeeded, real nonzero preserved); in-tree oeh-gate semantics unchanged. Audited head is `f646b13` (functional commit 807f35c plus declared-role-header/carrier sync, zero executable change).
- Runtime command evidence is summarized in `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
