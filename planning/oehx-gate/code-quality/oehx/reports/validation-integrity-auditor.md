# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb60806` | Read; no additional references required by the operator beyond supplied artifacts. |
| mode | `pr-diff` | 7 | n/a | Selected unified diff inspection. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Directory exists; used to resolve relative paths. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` | n/a | n/a | Caller context; not an operator scoring artifact. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | 29280 | `c75df0f27bea` | Read; parseable unified PR diff. |
| runtime_claim | `OEH makes OpenCode terminal structured error handling honest: a terminal OpenCode error event with real exit 0 finalizes one-shot and resume as success=false, exit_code=-1, and terminal_reason carrying provider-error evidence; an error event followed by later stream output and exit 0 remains succeeded; quota/rate substrings in ordinary output do not classify as quota or rate-limit signals.` | 392 | `b20438ad255d` | Read as supplied text. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | 7622 | `77df12577fdb` | Read; includes XDG-isolated runtime test commands for runtime, external resume, opencode terminal-error parity, and external classifier suites. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 490293 | `ad14421ed08c` | Read; no OEHX validation-surface weakening ratification entry found, but no finding required ratification. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | 8167 | `9c861581e772` | Read before scoring; declares external launch/resume runtime proof surfaces. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | 1964 | `b89c049ae474` | Read before scoring; identifies OEHX proof intent and runtime claim identity. |
| wu_id | `oehx` | 4 | n/a | Used for optional report-local namespacing only; no findings emitted. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/validation-integrity-auditor.md` | n/a | n/a | Output destination; parent directory exists and report was written. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. The changed test expectations in `age217_s6a_policy_launch_dispatch.rs`, `age242_terminal_classify_external.rs`, and `s10_external_provider_resume.rs` either strengthen checks or align assertions with the claimed failed-finalization behavior. | LOW | `diff.patch` hunks add `exit_code = -1`, terminal reason assertions, invocation DB outcome assertions, and incident fixture emission; no skip, mock substitution, fixture-to-stub replacement, schema relaxation, or weaker assertion was found. | OEHX external-path terminal-error honesty parity for OpenCode structured error + real exit 0, later-output success, and quota/rate ordinary-output non-classification. | Not applicable; no finding. | Supplied `runtime-tests.log` covers runtime and high-seam external launch/resume suites. |

No finding records were emitted, so there are no `id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, or `blocks_pipeline` records to enumerate.

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | None required | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | None |

## Residual ambiguity / stop-condition notes

The PR diff is parseable and the Phase 6 contract/proposal were readable before scoring. The validation-surface changes observed are additive or stricter assertions around the runtime behavior being claimed, not easier-to-pass proof surfaces. `VI-007` does not fire because a runtime-artifact evidence path was supplied and read.

LOW
