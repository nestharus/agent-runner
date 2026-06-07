# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve relative references. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | 1964 | `b89c049ae474` | Readable. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | n/a | n/a | Unreadable: file not found. This is the formal contract input supplied for the code-quality proof-risk run. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/proof-risk-auditor.md` | n/a | n/a | Report destination. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:29-33` contains the exact heading. |
| `Runtime claim` | Not scored | Scoring stopped before proof-plan judgment because the required formal `contract_path` is unreadable. |
| `Proof method` | Not scored | Scoring stopped before proof-plan judgment because the required formal `contract_path` is unreadable. |
| `Evidence-class match` | Not scored | Scoring stopped before proof-plan judgment because the required formal `contract_path` is unreadable. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| STOP-001 | BLOCKED | Not scored. | `proposal.md:29-33` | Not scored. | Not scored. | The supplied Step 6a contract must be readable before runtime/proxy evidence-class judgment. | Formal `contract_path` `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` could not be read. | Yes |

## Evidence-class decision

No evidence-class decision was made. The operator requires the Step 6a contract to be read before scoring in a per-component code-quality proof-risk context; the formal `contract_path` supplied by the caller is unreadable.

## Residual ambiguity / stop-condition notes

The proposal's `## Proof plan` references `contracts/oehx.contract.md`, and a similarly named file exists under the worktree, but it is not the formal `contract_path` input. Substituting a discovered sibling for the supplied required artifact would bypass the operator's fail-closed input handling.

No code, tests, proposals, workflows, branches, routing files, or planning artifacts were modified except this report path.

BLOCKED:unreadable-contract-path
