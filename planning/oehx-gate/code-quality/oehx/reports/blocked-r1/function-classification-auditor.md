# Function Classification Audit

## Inputs Read

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=33775d7`
- `head_ref=HEAD`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` read successfully.
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` read successfully.
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` unreadable: file not found.
- `risk_profile_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/multi-classifier-risk.md` unreadable: file not found.
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/function-classification-auditor.md`

## References Read

- `/home/nes/ai/agents/function-classification-auditor.md`
- `/home/nes/ai/conventions/code-quality.md`

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|

## Residual Ambiguity / Stop-Condition Notes

- Stop condition reached before scoring: Phase 6 supplied `contract_path` is required and must be read before classification. The exact supplied path `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` is unreadable.
- Directory inspection found a sibling artifact at `planning/oehx-gate/contracts/oehx.contract.md`, but the auditor contract does not permit substituting a different contract for the caller-supplied `contract_path`.
- A1 source was readable and contains the category list, single-classification rule, `Function categories per function` threshold row, and `multi-classifier function` failure mode. The metric was not applied because the required Phase 6 contract input was unreadable.

BLOCKED:unreadable-contract-path
