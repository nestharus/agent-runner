# Function Classification Auditor Prompt

Run the function-classification auditor for the OEH Phase-6 code-quality gate.

Inputs:

| Key | Value |
|---|---|
| mode | phase-6 |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/function-classification-auditor.md` |
| base | `549daaa` |
| head | `HEAD` (`48bf5c1`, includes fc multi-classifier splits and the gate-carrier declaration sync remediation) |
| original_head | `3515d31` |
| wu_id | `oeh` |

Delta context: Functional commits are `f58c14f` and `a97e085`; pre-gate validation remediation `bdbb9e3` adds OpenCode F4 unit coverage only; remediation `48bf5c1` splits non_empty_stream_lines and assert_invocation_row into single-class helpers and syncs the contract adapter declarations for terminal_signal.rs (unix-signal-name-contract, signal-hook-forwarding-contract). OEH makes OpenCode terminal structured error events on the stream-terminal line classify as failure with `provider error: ...` evidence; `supervised_exit_code` emits a synthetic failure code when a terminal failure signal coincides with real exit 0, so run/resume finalize `success=false`/`Failed`. Artifact-only commits in the original range are `8db1a02`, `37b6223`, `be9761b`, and `3515d31`, excluded from the functional surface.

Read `/home/nes/ai/conventions/code-quality.md` before scoring, especially the A1 single-classification rule. Audit added or meaningfully changed functions in the touched OEH surfaces. If a genuine multi-classifier risk remains, emit a non-LOW finding and state the required split; do not waive it because a pre-gate split already happened. Do not modify the worktree except to write the report to `output_path`.

End the report with exactly one terminal line: `VERDICT: LOW`, `VERDICT: MEDIUM`, or `VERDICT: HIGH`.
