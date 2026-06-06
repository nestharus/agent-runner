# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-6` | 7 | n/a | Phase-6 per-component code-quality invocation; contract/proposal required and read. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Directory exists; used to resolve supplied paths. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` | n/a | n/a | Directory exists; caller context. |
| code-quality convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2ec6ed` | Read before scoring. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` | 28508 | `3f9dfb06d265b804` | Unified diff inspected by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` | 504 | `8c6b25292b55cbe1` | Touched surfaces list read for context. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` | 5178 | `889db6840b924479` | Proof intent and runtime claim identity read. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` | 15810 | `c8b32f663511a3e4` | Phase-6 contract read; includes test-harness declarations and generated moveout exclusions. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe29511935fb` | Read; no S10 validation-surface weakening ratification entry required because no finding fired. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` | 39079 | `380b0d3ae6e57940` | Runtime-artifact evidence read and considered. |
| runtime_claim | `Shipped tests assert real PLK and S10 behavior: nested agent-bash inherits OULIPOLY_PARENT_INVOCATION and records parent_invocation_id in StateDb; parent resolution uses same-DB UUID and tolerates source-name drift; trace reconciles stale running rows only with conclusive pid-identity sidecar dead-process evidence; external provider launch exit session metadata records external_provider_launch capture and is carried into the next known_provider_session_id resume request.` | 475 | `07d955f8b5bcafbe` | Inline claim supplied by caller. |
| generated moveout scope context | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-moveout/scope.md` | 23991 | `8be44108fbdebb65` | Read to resolve whether source-guard exclusions target generated planning scope rather than runtime proof. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No ACR-254 validation-integrity weakening pattern fired. | LOW | n/a | The supplied PLK/S10 runtime claim is supported by separate runtime evidence. | n/a | `runtime-tests.log` records the named S10 external launch session test and PLK proof commands passing with rc 0. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | n/a | n/a | n/a |

## Residual ambiguity / stop-condition notes

No stop condition fired. Required Phase-6 `contract_path` and `proposal_path` were readable before scoring.

Diff hunk review found assertion additions and strengthened/updated assertions in `crates/oulipoly-runtime/tests/s10_external_launch_session.rs`, `crates/oulipoly-setup/src/context.rs`, and `src-tauri/src/commands/config_migration/tests.rs`. The two removed `assert_eq!(report.model_files_rewritten, 0)` lines were replaced with `assert_eq!(..., 1)` plus provider-binary assertions matching the new migration behavior, so VI-001 did not fire.

Diff hunk review found no added pytest/unittest/runtime-condition skip, no skip-equivalent ignore marker, no mock substitution for a formerly real runtime dependency, no fixture-to-stub replacement, and no schema relaxation.

Source-guard tests added `planning/s10-moveout/**` exclusions in `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs`, `src-tauri/tests/age245_s7c_rotation_source_guard.rs`, and `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs`. This is a validation-surface scope edit, but it did not fire a weakening finding after Phase-6 context resolution: the contract declares these as generated moveout planning-artifact exclusions, `planning/s10-moveout/scope.md` is a planning scope artifact, and the runtime claims have independent runtime-artifact evidence in `runtime-tests.log` rather than relying on the excluded planning path.

Runtime-artifact-bound claim check: VI-007 did not fire because `runtime-tests.log` contains production-path cargo test command evidence for `s10_external_launch_session`, source-guard suites, config/setup suites, nested real `agent-bash` parent inheritance with `AGENT_BASH_BIN`, stale-running PID-sidecar reconciliation, and same-DB UUID source-drift parent resolution. The evidence log ends with rc 0 for required runtime proof commands.

VERDICT: LOW
