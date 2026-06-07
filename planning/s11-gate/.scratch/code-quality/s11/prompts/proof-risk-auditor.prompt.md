# S11 Proof Risk Auditor Prompt

You are acting as `/home/nes/ai/agents/proof-risk-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=phase-3-proposal`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/proof-risk-auditor.md`

Important context:

- Audited net source range is `95699d6..7ec42d4`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` because the regenerated diff includes the committed S11 remediation source range.
- The exact `## Proof plan` must bind shipped tests and live evidence without substituting missing live S10 resume evidence for deterministic S10 resume tests.
- Live evidence references are summarized in `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/live-smoke.md`; runtime command evidence is summarized in `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/runtime-tests.log`.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
