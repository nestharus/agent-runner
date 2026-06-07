# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| Operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb60806` | Required operator reference read. |
| mode | `pr-diff` | 7 | `n/a` | Selected PR diff input surface. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | `n/a` | Directory exists and was used to resolve paths. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | 29280 | `c75df0f27bea` | Unified diff was readable, but not scored because the required contract is unreadable. |
| runtime_claim | `OEH makes OpenCode terminal structured error handling honest: a terminal OpenCode error event with real exit 0 finalizes one-shot and resume as success=false, exit_code=-1, and terminal_reason carrying provider-error evidence; an error event followed by later stream output and exit 0 remains succeeded; quota/rate substrings in ordinary output do not classify as quota or rate-limit signals.` | 392 | `b20438ad255d` | Runtime claim value supplied inline. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | 7622 | `77df12577fdb` | Runtime-test evidence readable. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 490293 | `ad14421ed08c` | Ratification file readable. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` | n/a | `UNREADABLE` | Required Phase 6 contract path is missing or unreadable. Operator requires `BLOCKED:unreadable-contract-path` before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | 1964 | `b89c049ae474` | Proposal readable. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/validation-integrity-auditor.md` | n/a | `n/a` | Output destination; parent directory exists and is writable. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | n/a | Not scored because required `contract_path` is unreadable. Required finding fields: `id=n/a`; `severity=n/a`; `path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md`; `line_span_or_diff_hunk=n/a`; `pattern_id=n/a`; `validation_surface_change=not evaluated`; `runtime_fix_claim_ref=inline runtime_claim`; `ratification_ref=n/a`; `runtime_artifact_validation_ref=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log`; `closure_expectation=provide readable Phase 6 contract at supplied path or rerun with corrected contract_path`; `blocks_pipeline=true`. | n/a | Required contract unreadable before validation-integrity scoring. | Inline runtime claim. | n/a | Evidence readable but not evaluated due stop condition. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | n/a | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | n/a |

## Residual ambiguity / stop-condition notes

Phase 6 per-component code-quality requires reading `contract_path` before scoring validation integrity. The supplied path `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oeh.contract.md` could not be read, so the operator mandates fail-closed termination with `BLOCKED:unreadable-contract-path` instead of judging validation weakening from the diff, proposal, runtime evidence, or generic context.

BLOCKED:unreadable-contract-path
