# Validation Integrity Auditor Prompt

Run the validation-integrity auditor for the S10 Phase-6 code-quality gate.

Inputs:

| Key | Value |
|---|---|
| mode | phase-6 |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` |
| wu_id | `s10` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` |
| runtime_claim | `Shipped tests assert real PLK and S10 behavior: nested agent-bash inherits OULIPOLY_PARENT_INVOCATION and records parent_invocation_id in StateDb; parent resolution uses same-DB UUID and tolerates source-name drift; trace reconciles stale running rows only with conclusive pid-identity sidecar dead-process evidence; external provider launch exit session metadata records external_provider_launch capture and is carried into the next known_provider_session_id resume request.` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/code-quality/plk/reports/validation-integrity-auditor.md` |

Read `/home/nes/ai/conventions/code-quality.md` before scoring. Audit for validation weakening, skips, mock substitution, assertion removal, or mismatched runtime evidence. The runtime artifact evidence must be considered part of the input. Do not modify the worktree except to write the report to `output_path`.

End the report with exactly one terminal line: `VERDICT: LOW`, `VERDICT: MEDIUM`, or `VERDICT: HIGH`.
