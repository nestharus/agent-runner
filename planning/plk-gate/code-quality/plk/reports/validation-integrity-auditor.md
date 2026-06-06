# Validation-integrity audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 | 7 | n/a | Phase-6 code-quality gate invocation; scored with contract and proposal loaded before pattern judgment. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve supplied paths and PLK touched surfaces. |
| runtime_claim | `Shipped tests assert real PLK behavior: a nested agent-bash child inherits OULIPOLY_PARENT_INVOCATION and records parent_invocation_id in StateDb; parent resolution uses same-DB UUID and tolerates source-name drift while malformed/unknown values stay root invocations; trace reconciles stale running rows only with conclusive pid-identity sidecar dead-process evidence and preserves JSON-only non-mutating stale lift without that evidence.` | 439 | `0ed00dbeba7d` | Artifact-bound claim names nested `agent-bash`, StateDb parent linkage, PID sidecar reconciliation, and trace behavior. |
| auditor_prompt | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/prompts/validation-integrity-auditor.prompt.md` | 1993 | `6420f546817b` | Caller prompt with output path and PLK inputs. |
| code_quality_convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2e` | Read before scoring; confirms Phase-6 contract visibility and ACR-254 validation-integrity layer. |
| validation_integrity_auditor | `/home/nes/ai/agents/validation-integrity-auditor.md` | 11070 | `6983abb60806` | Read for VI pattern definitions and report contract. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` | 20940 | `b987e6befa74` | Unified PLK diff inspected by hunks. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` | 312 | `56582513e5cd` | Lists eight PLK touched production/test surfaces. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` | 5330 | `9712bc089ee4` | Proof intent and runtime claim identity loaded before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` | 15579 | `af2cf1370442` | Step 6a declarations loaded before scoring; includes adapter, intrinsic-surface, and test-harness declarations. |
| decisions_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/DECISIONS.md` | 487336 | `3dd8fe295119` | Read as optional ratification input; no ratification required because no pattern fired. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | 4756 | `6ead8aaf698e` | Runtime artifact evidence present; records real `agent-bash` path and targeted integration/unit tests passing. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/commands/trace/accessor.rs` | 935 | `dc550ead16e7` | Read to inspect trace reconciliation hook. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/dispatch.rs` | 35187 | `0e24da99d40e` | Read to inspect parent resolver assertions and dispatch test surface. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/dispatch/parent_invocation.rs` | 708 | `c734723e3812` | Read to inspect same-DB UUID parent lookup behavior. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/dispatch/predicate.rs` | 844 | `1ad87c32f5b9` | Read to inspect removed source-match predicate surface. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/invocation/mod.rs` | 89 | `a4accbba0e0d` | Read to inspect stale reconciliation module export. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/src/invocation/stale_reconcile.rs` | 7045 | `4c94b02a36b1` | Read to inspect PID sidecar liveness reconciliation. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/pr_a_invocation_integration.rs` | 24705 | `0342837ba020` | Read to inspect parent-env, malformed/unknown-env, and nested `agent-bash` assertions. |
| touched source | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/src-tauri/tests/pr_b_trace_integration.rs` | 14987 | `21d7aa934a40` | Read to inspect trace stale-running and sidecar reconciliation assertions. |

## Patterns detected
| Finding ID | Pattern ID | Pattern shape | Severity | Code line or excerpt | Runtime claim ref | Ratification status | Runtime-artifact evidence |
|---|---|---|---|---|---|---|---|
| None | None | No validation-weakening pattern fired. The PLK diff adds assertions and runtime-path checks, removes no assertion, adds no skip/skip marker, substitutes no mock/stub for the claimed runtime dependency, and does not rely on proxy-only proof for the artifact-bound claim. | LOW | `pr_a_invocation_integration.rs` now fails closed through `agent_bash_bin_from_env()` when `AGENT_BASH_BIN`/PATH lacks a real `agent-bash`; `pr_b_trace_integration.rs` asserts both JSON output and durable StateDb fields after PID-sidecar reconciliation. | Full runtime claim from prompt; proposal.md lines 13-47. | Not applicable. | `runtime-tests.log` lines 3-5 and 8-80 show isolated commands, `AGENT_BASH_BIN=/home/nes/.local/bin/agent-bash`, 13/13 `pr_a`, 11/11 `pr_b`, and 6/6 parent-resolution unit tests passed. |

## Ratification evidence
| Finding ID | DECISIONS heading | Runtime-artifact path | Downgrade |
|---|---|---|---|
| None | Not applicable; no validation-integrity finding fired. | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | None needed. |

## Residual ambiguity / stop-condition notes

No input was missing or unreadable, and the diff was inspectable. The source-name equality guard removal in `dispatch/parent_invocation.rs` is the declared runtime fix, not a proof-surface weakening: same-DB UUID lookup is still tested for malformed JSON, invalid UUID format, unknown UUID, existing parent row, and source-name drift. The nested `agent-bash` proof is fail-closed rather than skip-based in the current touched file, and the runtime artifact evidence records the real `agent-bash` integration run. The trace sidecar path adds durable StateDb assertions for conclusive dead-PID evidence while preserving the pre-existing JSON-only non-mutating stale lift without sidecar evidence.

VERDICT: LOW
