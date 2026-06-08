# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| Operator | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb608061d91` | Required operator instructions loaded. |
| mode | `pr-diff` | 7 | n/a | Selected PR diff input surface. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` | n/a | n/a | Directory exists and was used to resolve supplied absolute paths. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch` | 117700 | `2814ad98bf231741` | Unified diff inspected by hunks. |
| runtime_claim | `The consolidated wakechain fix confirms delivery only from submitted or ingested user-turn evidence, parses/targets OpenCode current exports, reclaims dead wake claims without stealing live identity-matched owners, caps unconfirmed retry loops, and hardens the sweep so recoverable recent leaks are not starved by dead-owner backlog while abandoned debris is marked instead of retried forever.` | 393 | `10df13264d95d5b1` | Runtime claim used for validation-surface comparison. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md` | 20817 | `e717e016111a0ecf` | Phase 6 contract read before scoring. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md` | 6536 | `4ee5401978e52d06` | Proposal proof intent and residual read before scoring. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/evidence/runtime-tests.log` | 286641 | `070c4e3ef1dacacb` | Evidence includes `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, targeted wake-confirm/proactive-wake tests, and `bash scripts/tests/opencode-turns.test.sh`. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/DECISIONS.md` | 490293 | `ad14421ed08c82ba` | Provided optional ratification source; no downgrade needed because no pattern fired. |
| report_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/validation-integrity-auditor.md` | n/a | n/a | This report is the only written path. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | n/a | No validation-weakening pattern fired. Existing Rust assertions in `crates/oulipoly-runtime/src/sessions/mod.rs` were helper-extracted with equivalent predicates, and new adapter/integration tests add assertions rather than deleting or relaxing prior checks. | LOW | Diff anchors: `diff.patch:273-319`, `diff.patch:1245-1317`, `diff.patch:2573-2698`, `diff.patch:3099-3331`. | Full wakechain runtime claim. | Not required. | `runtime-tests.log:4507-4547` shows targeted wake-confirm, proactive-wake, and opencode-turns executions passed. |

Finding record details: no finding records because no pattern fired; therefore `id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, and `blocks_pipeline` are not applicable.

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | n/a | n/a | n/a |

## Residual ambiguity / stop-condition notes

No added pytest/unittest skip or equivalent runtime-condition skip was found. No removed assertion was found without an equivalent helper-preserved assertion. No schema or contract relaxation was found. No previously real dependency or runtime path was replaced by a mock; fake OpenCode/provider fixtures appear as additive executable test harnesses declared in the contract, not as substitutions for a prior real-dependency validation surface. The documented residual for bounded sweep cycles under pathological backlog is preserved in the proposal and contract and is not expanded by the diff into a broader untested claim.

LOW
