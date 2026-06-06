# S10B Proof Risk Auditor Prompt

You are acting as `/home/nes/ai/agents/proof-risk-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

This is a Phase 6 per-component proof-risk invocation over the S10B proposal/proof plan. The durable gate package was committed after the source delta, so the source diff is available for context at `planning/s10b-gate/gates/diff.patch`, but your scored artifact is the supplied `proposal_path` plus the required Phase 6 `contract_path`.

Inputs:

- `mode=phase-3-proposal`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/proof-risk-auditor.md`

Runtime claim context:

The S10B source changes fix external provider launch and resume compatibility: binary provider references resolve through the production process `PATH`; provider protocol DTOs accept schema-valid free-form `describe.concurrency` metadata; policy and launch request construction preserves inherited environment and provider-compatible model args; external LaunchExit session metadata, including `session_id` alias, persists an external launch capture method; and provider-ref headless resume uses the external launch executor path with `known_provider_session_id` and recorded runtime cwd instead of legacy CLI resume or default migration. The implementation intentionally avoids a `state.db` schema change.

Important context:

- The proposal has a `## Proof plan` section with repeated `Runtime claim`, `Proof method`, and `Evidence-class match` entries.
- The proposal explicitly claims live launch smoke evidence only for installed external launch, not live resume; deterministic integration tests are the claimed resume proof.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
