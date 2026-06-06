# Coupling Auditor Prompt

Run the coupling auditor for the S10 Phase-6 code-quality gate.

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
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/code-quality/plk/reports/coupling-auditor.md` |

Read `/home/nes/ai/conventions/code-quality.md` before scoring. Audit imports, module boundaries, provider-launch session capture, config-migration coupling, setup prompt carrier coupling, and source-guard path exclusions in the touched S10 surfaces. Do not modify the worktree except to write the report to `output_path`.

End the report with exactly one terminal line: `VERDICT: LOW`, `VERDICT: MEDIUM`, or `VERDICT: HIGH`.
