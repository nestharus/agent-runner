# Validation Integrity Auditor Prompt

Run the validation-integrity auditor for the PLK Phase-6 code-quality gate.

Inputs:

| Key | Value |
|---|---|
| mode | phase-6 |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` |
| runtime_claim | `Shipped tests assert real PLK behavior: a nested agent-bash child inherits OULIPOLY_PARENT_INVOCATION and records parent_invocation_id in StateDb; parent resolution uses same-DB UUID and tolerates source-name drift while malformed/unknown values stay root invocations; trace reconciles stale running rows only with conclusive pid-identity sidecar dead-process evidence and preserves JSON-only non-mutating stale lift without that evidence.` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/validation-integrity-auditor.md` |

Read `/home/nes/ai/conventions/code-quality.md` before scoring. Audit for validation weakening, skips, mock substitution, assertion removal, or mismatched runtime evidence. The runtime artifact evidence must be considered part of the input. Do not modify the worktree except to write the report to `output_path`.

End the report with exactly one terminal line: `VERDICT: LOW`, `VERDICT: MEDIUM`, or `VERDICT: HIGH`.
