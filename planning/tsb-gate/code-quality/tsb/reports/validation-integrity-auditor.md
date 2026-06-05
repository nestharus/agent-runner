# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Resolved absolute repository worktree. |
| runtime_claim | `Shipped tests assert real bounding behavior: the script exports only recent-window sessions and emits degraded output within its deadline when a call wedges (opencode-turns.test.sh); the runtime classifies turn/quota script timeouts, kills the process group (children included), and proceeds without persisting turns (sessions + quota::process tests; age243 dispatch suite stays green).` | 391 | n/a | Caller-supplied runtime claim. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | 17344 | `ad72a7aa6113` | Phase 6 contract read before scoring; declares validation/proof surfaces for `scripts/opencode-turns`, shell tests, sessions, quota process, and AGE-243 dispatch. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | 6179 | `a5f57b3f34e3` | Proposal read before scoring; proof plan identifies public OpenCode CLI adapter, recent-window shell test, timeout/degraded shell test, runtime degraded marker, session timeout, quota timeout, and quota process-group child tests. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | 42598 | `97205fb15f07` | Unified delta inspected by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | 210 | `28440caadc65` | Confirms the five touched surfaces in the contract. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read for ratification availability; no validation-weakening finding required ratification. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/evidence/runtime-tests.log` | 915 | `ca8d54c8300e` | Evidence log references `cargo test --workspace`, `scripts/tests/opencode-turns.test.sh`, targeted sessions/quota tests, and AGE-243 dispatch. |
| code_quality_convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Required Phase 6 convention read; confirms active validation-integrity layer and contract/proposal visibility requirement. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. The delta adds tests/assertions and updates one existing fake OpenCode fixture to require the new public `session list --json` shape; it does not remove assertions, add runtime-condition skips, relax schemas, or replace a previously real dependency with a mock. | LOW | `planning/tsb-gate/gates/diff.patch:1093-1288` adds `scripts/tests/opencode-turns.test.sh`; `diff.patch:432-440` changes the AGE-243 fake CLI from text list output to JSON `--json`; `diff.patch:109-135`, `396-423`, and `1243-1283` add assertions. | Runtime claim names bounded recent-window/degraded script behavior and runtime timeout/classification/process-group behavior. | Not applicable. | `planning/tsb-gate/evidence/runtime-tests.log:7-18` records format/clippy/workspace tests, shell adapter tests, targeted sessions/quota tests, and AGE-243 dispatch passing. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | Not needed; no weakening finding fired. | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/evidence/runtime-tests.log` | None |

## Residual ambiguity / stop-condition notes

No stop condition fired. The Phase 6 contract was readable and non-blank, and the proposal was readable before scoring. The supplied runtime-artifact evidence is a gate log rather than a live OpenCode service/container transcript, but the proposal scopes the adapter proof to the production `scripts/opencode-turns` path exercised through fake public-CLI binaries, and this audit is limited to validation-surface weakening rather than broader proof-depth or coverage-quality review.

Finding records: none. Required fields (`id`, `severity`, `path`, `line_span_or_diff_hunk`, `pattern_id`, `validation_surface_change`, `runtime_fix_claim_ref`, `ratification_ref`, `runtime_artifact_validation_ref`, `closure_expectation`, and `blocks_pipeline`) are not instantiated because no finding was emitted.

VERDICT: LOW
