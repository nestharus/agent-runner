# LSV Validation Integrity Auditor Prompt

You are acting as `/home/nes/ai/agents/validation-integrity-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=pr-diff`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/diff.patch`
- `runtime_claim=LSV makes launch streams volume-safe: a valid launch stream larger than the transport capture limit completes from its exit event instead of failing stdout_limit_exceeded, with bounded host retention honestly recorded; truncation without a valid final exit stays a transport error; non-launch one-shot invocations keep capped stdout semantics; external launch/resume finalization and terminal-error honesty semantics are unchanged for ordinary streams.`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/runtime-tests.log`
- `decisions_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md`
- `wu_id=lsv`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/validation-integrity-auditor.md`

Important context:

- Runtime evidence includes the XDG-isolated commands in `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/runtime-tests.log`.
- Functional source commit is `7d76426` (launch JSONL stdout parses incrementally with bounded retention; valid streams over the old 1MiB capture limit finalize from the exit event instead of failing stdout_limit_exceeded; non-launch one-shot caps and heartbeat-gap liveness unchanged). Declaration prep `8a11fba` is doc-comments only. Audited remediation head is `2fe6745` (function splits plus declaration/header syncs; no behavior change intended).
- There are no artifact-only commits in range.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
