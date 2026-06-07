# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb60806` | Read and applied. |
| mode | `pr-diff` | 7 | n/a | Selected diff surface. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | 4096 | n/a | Directory exists and was used to resolve supplied absolute paths. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` | 16561 | `f7fc624de584` | Unified diff inspected by hunks. |
| runtime_claim | `OEH makes OpenCode terminal structured error handling honest: a terminal OpenCode error event with real exit 0 finalizes one-shot and resume as success=false, exit_code=-1, and terminal_reason carrying provider-error evidence; an error event followed by later stream output and exit 0 remains succeeded; quota/rate substrings in ordinary output do not classify as quota or rate-limit signals.` | 392 | `b20438ad255d` | Runtime claim identity for validation-surface comparison. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` | 81794 | `8c0edffa8d3f` | Contains isolated XDG `cargo test -p oulipoly-runtime` and `cargo test -p oulipoly-agent-runner --test opencode_terminal_error_exit_zero --test structural_segmentation`, both `EXIT_STATUS: 0`. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 490293 | `ad14421ed08c` | Read for possible ratification; no OEH-specific weakening ratification was needed. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | 10546 | `4f1e7bc19748` | Phase 6 contract read before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | 5166 | `d4970cd46065` | Proposal read before scoring. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/validation-integrity-auditor.md` | n/a | n/a | This report is the only written path. |
| wu_id | `oeh` | 3 | n/a | Used only for local context; no findings emitted. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|

No validation-integrity findings fired.

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|

No ratification was required because no validation-weakening pattern fired.

## Residual ambiguity / stop-condition notes

The inspected diff changes both runtime code and validation surfaces, but the validation changes add or preserve assertions for the declared OEH runtime claim rather than making the proof easier to pass. The notable changed assertion in `crates/oulipoly-runtime/src/executor/providers/opencode.rs` replaces the prior `NonzeroExit` expectation for a structured OpenCode error with `Unknown` plus evidence assertions, matching the claimed runtime behavior; the diff also adds clean-exit, failure-exit, result-envelope, and StateDb assertions for one-shot and resume paths.

No added pytest or unittest skips were present. No schema or contract relaxation was present. No existing real dependency, adapter, service, container, endpoint, or runtime path was replaced by a mock/stub in a way that weakens an existing validation surface. The new CLI integration fixture uses isolated provider bodies to exercise the runner one-shot/resume paths and is backed by the supplied runtime evidence log; absent a fired weakening pattern, DECISIONS ratification is not required.

LOW
