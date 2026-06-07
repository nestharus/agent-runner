# S11 Validation Integrity Auditor Prompt

You are acting as `/home/nes/ai/agents/validation-integrity-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=pr-diff`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch`
- `runtime_claim=S11 makes detached external-provider wake/resume delivery honest by confirming mailbox delivery only from submitted-turn nonce/hash markers or exact ingested user-turn evidence; failed/rate-limited wake attempts remain pending with delivery_attempts and retry evidence; detached wakes reload launch-time provider/config roots and selected provider settings; external-provider transport timeout and provider unavailable/timeout failures rotate across the pool; no durable state.db schema migration is required.`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence/runtime-tests.log`
- `decisions_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md`
- `wu_id=s11`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/validation-integrity-auditor.md`

Important context:

- Audited net source range is `95699d6..7ec42d4`.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` because the regenerated diff includes the committed S11 remediation source range.
- Runtime evidence includes shipped tests and live smoke notes under `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/evidence`.
- The source scope guard forbids treating the reverted private SQLite fallback as current behavior.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
