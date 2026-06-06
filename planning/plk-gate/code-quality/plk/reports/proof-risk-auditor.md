# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | phase-6 | n/a | n/a | Prompt-specific Phase 6 per-component code-quality context; contract was read before scoring. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve supplied evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/proposal.md` | 5330 | `9712bc089ee4a09f` | Readable. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/contracts/plk.contract.md` | 15579 | `af2cf13704422403` | Readable and used for proof scope, runtime obligations, and test-harness context. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/diff.patch` | 20940 | `b987e6befa74ba6c` | Readable; used only to confirm PLK touched surfaces. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/gates/touched-files.txt` | 312 | `56582513e5cd8c63` | Readable; audit confined to listed PLK surfaces. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/evidence/runtime-tests.log` | 4756 | `6ead8aaf698e44ef` | Readable; records passing integration and unit evidence commands. |
| convention | `/home/nes/ai/conventions/code-quality.md` | 30798 | `fa8b6499cc2ec6ed` | Read before scoring. |
| auditor procedure | `/home/nes/ai/agents/proof-risk-auditor.md` | 9200 | `788f5bdea5ab25af` | Read for proof-risk report contract and evidence-class rules. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/plk-gate/code-quality/plk/reports/proof-risk-auditor.md` | n/a | n/a | Destination written by this audit. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| Exact `## Proof plan` section | Yes | `proposal.md:9-47`. |
| `Runtime claim` | Yes | Six entries at `proposal.md:13`, `proposal.md:19`, `proposal.md:25`, `proposal.md:31`, `proposal.md:37`, and `proposal.md:43`. |
| `Proof method` | Yes | Six entries at `proposal.md:15`, `proposal.md:21`, `proposal.md:27`, `proposal.md:33`, `proposal.md:39`, and `proposal.md:45`. |
| `Evidence-class match` | Yes | Six entries at `proposal.md:17`, `proposal.md:23`, `proposal.md:29`, `proposal.md:35`, `proposal.md:41`, and `proposal.md:47`. |
| Self-certification only | No | Proof methods name concrete integration/unit tests, and `runtime-tests.log:8-80` records corresponding `ok` results. |

## Findings
| Finding ID | Severity | Runtime claim | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan fields, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | n/a | n/a | n/a | `proposal.md:9-47`; `runtime-tests.log:8-80`; `plk.contract.md:3-20`; `plk.contract.md:223-249`. | No |

## Evidence-class decision

The nested `agent-bash` parent-inheritance claim is runtime-artifact-bound because it asserts environment propagation through a real `agent-bash` launch into a nested runner command and durable StateDb parent linkage. The proof method uses the real `agent-bash` binary named by `AGENT_BASH_BIN`, starts a nested runner command, waits for `DONE rc=0`, parses the captured child `OULIPOLY_INVOCATION` marker, and verifies the child StateDb row's `parent_invocation_id` equals the parent row id. That directly exercises the PLK runtime path rather than a mock surface. Evidence refs: `proposal.md:13-17`, `runtime-tests.log:8-31`, `src-tauri/tests/pr_a_invocation_integration.rs:115-128`, `src-tauri/tests/pr_a_invocation_integration.rs:343-372`.

The same-DB UUID source-drift claim is scoped to resolver behavior: source-name equality is intentionally removed from parent lookup, and UUID plus supplied StateDb scope is the durable invariant. The proof method is a direct unit test of production `resolve_parent_invocation_id` with a StateDb parent row and serialized `CompositeInvocationId` env value whose source differs from the stored provider name. This is appropriate state-DB evidence for the resolver claim and is not being used as a proxy for nested `agent-bash`, which has separate integration proof. Evidence refs: `proposal.md:19-23`, `runtime-tests.log:60-76`, `src-tauri/src/dispatch/parent_invocation.rs:5-9`, `diff.patch:27-50`, `plk.contract.md:32-38`.

The malformed/unknown parent-env safety claim has mixed integration plus unit proof. The integration suite runs the binary with malformed JSON, a valid but absent UUID, and an invalid UUID string and verifies root invocation rows. The resolver unit tests separately cover unset, malformed, unknown, and invalid UUID cases. This binds production CLI safety behavior to integration evidence while using unit evidence for direct parser/lookup fallbacks. Evidence refs: `proposal.md:25-29`, `runtime-tests.log:16-31`, `runtime-tests.log:68-76`, `src-tauri/tests/pr_a_invocation_integration.rs:374-399`.

The sidecar PID stale-running reconciliation claim is runtime-artifact-bound because it asserts trace-time reconciliation, PID sidecar liveness evidence, and durable StateDb terminal fields. The proof method runs the trace CLI integration fixture with a seeded stale running StateDb row and a PID identity sidecar row for an impossible PID, then verifies both trace JSON and reopened StateDb terminal state. That matches the required runtime class for the positive conclusive-dead sidecar path. Evidence refs: `proposal.md:31-35`, `runtime-tests.log:35-56`, `src-tauri/src/commands/trace/accessor.rs:11-16`, `src-tauri/src/invocation/stale_reconcile.rs:33-48`, `src-tauri/tests/pr_b_trace_integration.rs:311-342`.

The no-conclusive-sidecar and fresh-running trace claims are runtime-artifact-bound to trace CLI behavior and durable StateDb state. Their proof methods run `trace --json` through the runner binary and verify either JSON-only stale-running lift without DB mutation or fresh running rows with null terminal fields and no stale warning. The no-sidecar case is valid evidence for the claim that absent conclusive PID sidecar evidence preserves the existing JSON-only stale lift, while the positive sidecar reconciliation proof separately covers durable finalization when evidence is conclusive. Evidence refs: `proposal.md:37-47`, `runtime-tests.log:43-56`, `src-tauri/tests/pr_b_trace_integration.rs:240-309`.

## Residual ambiguity / stop-condition notes

No stop condition fired. The source-drift proof is unit-level, but the runtime claim is specifically the resolver's same-DB UUID lookup contract and the broader nested `agent-bash` parent-linkage path has integration evidence. The sidecar proof plan includes runtime-artifact evidence for conclusive-dead reconciliation, no-sidecar stale lift preservation, and fresh-running non-stale behavior; broader live/unknown sidecar permutations would be additional validation depth, not an evidence-class mismatch in this proof-risk pass.

VERDICT: LOW
