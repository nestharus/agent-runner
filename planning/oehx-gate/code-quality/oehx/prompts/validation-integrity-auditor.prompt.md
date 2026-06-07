# OEHX Validation Integrity Auditor Prompt

You are acting as `/home/nes/ai/agents/validation-integrity-auditor.md`. Read that operator file and follow it. Also read every reference required by that operator. Do not edit code, tests, proposals, workflows, branches, routing files, or planning artifacts. Write only the `report_path` named below.

Formal inputs:

- `mode=pr-diff`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch`
- `runtime_claim=OEH makes OpenCode terminal structured error handling honest: a terminal OpenCode error event with real exit 0 finalizes one-shot and resume as success=false, exit_code=-1, and terminal_reason carrying provider-error evidence; an error event followed by later stream output and exit 0 remains succeeded; quota/rate substrings in ordinary output do not classify as quota or rate-limit signals.`
- `runtime_artifact_evidence_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log`
- `decisions_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `wu_id=oehx`
- `report_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/validation-integrity-auditor.md`

Important context:

- Runtime evidence includes the XDG-isolated commands in `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log`.
- Functional source commit is `807f35c` (external-path terminal-error honesty parity: external launch/resume mappers consume the shared failure-exit/reason rules owned by terminal_signal.rs; provider failure terminal_signal + real exited(0) finalizes failed with provider-error terminal_reason; clean and real-nonzero paths unchanged).
- There are no artifact-only commits in range.
- Source inspection must use `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`; `gates/diff.patch` is regenerated from the final audited source range.

Write the report in the operator's required format. The final non-blank report line and stdout must be exactly one of `LOW`, `MEDIUM`, `HIGH`, `NEEDS_INPUT:<absolute_artifact_path>`, or `BLOCKED:<reason>`.
