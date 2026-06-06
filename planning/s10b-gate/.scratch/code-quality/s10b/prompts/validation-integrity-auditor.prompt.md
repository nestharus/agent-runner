# S10B Validation Integrity Auditor Prompt

You are acting as `/home/nes/ai/agents/validation-integrity-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

This is a Phase 6 per-component `pr-diff` invocation for the S10B source delta. The durable gate package was committed after the source delta, so use `diff_path` as the authoritative audited source diff. Do not widen the touched-file/component set to include `planning/s10b-gate/**` gate artifacts or the gate-artifact commit.

Inputs:

- `mode=pr-diff`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/validation-integrity-auditor.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/evidence/runtime-tests.log`
- `wu_id=s10b`

Runtime claim:

The S10B source changes fix external provider launch and resume compatibility: binary provider references resolve through the production process `PATH`; provider protocol DTOs accept schema-valid free-form `describe.concurrency` metadata; policy and launch request construction preserves inherited environment and provider-compatible model args; external LaunchExit session metadata, including `session_id` alias, persists an external launch capture method; and provider-ref headless resume uses the external launch executor path with `known_provider_session_id` and recorded runtime cwd instead of legacy CLI resume or default migration. The implementation intentionally avoids a `state.db` schema change.

Important context:

- The proposal and evidence log distinguish deterministic resume integration evidence from live launch smoke evidence. Do not infer live resume evidence that is not claimed.
- Treat validation-surface changes in tests and protocol fixtures according to the operator's weakening patterns, using the runtime claim above and the proof/evidence context supplied.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
