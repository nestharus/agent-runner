# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | N/A | N/A | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Used to resolve relative evidence paths and source references. |
| operator | `/home/nes/ai/agents/proof-risk-auditor.md` | 9200 | `788f5bdea5ab` | Read as required by caller. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md` | 6351 | `8de5135110da` | Artifact under review. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md` | 13377 | `ad4246068d9d` | Read before scoring for Phase 6 context. |
| referenced evidence log | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/evidence/runtime-tests.log` | 18973 | `eeb1cbc1e7e1` | Referenced by the proposal and contract proof plans. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/proof-risk-auditor.md` | N/A | N/A | Destination written. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:26-99` contains an exact `## Proof plan` section. |
| `Runtime claim` | Yes | Runtime-claim fields appear at `proposal.md:30-31`, `proposal.md:41-42`, `proposal.md:50-51`, `proposal.md:59-61`, `proposal.md:70-71`, `proposal.md:79-80`, and `proposal.md:92`. |
| `Proof method` | Yes | Proof-method fields appear at `proposal.md:33-35`, `proposal.md:44-46`, `proposal.md:53-54`, `proposal.md:63-64`, `proposal.md:73-74`, `proposal.md:82-86`, and `proposal.md:94-95`. |
| `Evidence-class match` | Yes | Evidence-class-match fields appear at `proposal.md:37-39`, `proposal.md:48`, `proposal.md:56-57`, `proposal.md:66-68`, `proposal.md:76-77`, `proposal.md:88-90`, and `proposal.md:97-99`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | N/A | No missing proof-plan fields, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | `proposal.md:26-99`; `contract.md:23-90` | N/A | N/A | N/A | `runtime-tests.log:4-8`, `runtime-tests.log:82-104`, `runtime-tests.log:167-241` | No |

## Evidence-class decision

The proposal has the required `## Proof plan` structure and binds each runtime claim to a proof method with an explicit evidence-class explanation. The Step 6a contract repeats the same proof scope and declares the relevant adapter and intrinsic runtime surfaces for provider client/process/stream handling, external-provider launch-result mapping, and the external launch/resume CLI regression suite.

The stream-volume and truncation claims are runtime-artifact-bound because they assert production launch stream/process behavior. Their proof methods use provider-client subprocess integration against a compiled fake provider binary, plus runtime units for bounded-retention and capped one-shot semantics, which matches the claimed stream/process surfaces rather than a static or mocked-only proxy.

The external launch/resume honesty and clean-path claims are runtime-artifact-bound because they assert production-shaped `oulipoly-agent-runner` external launch/resume outcomes, response envelopes, and state DB invocation rows. Their proof methods name CLI integration tests using the real runner path with fake provider CLI streams and DB row assertions, plus provider-client stream tests for final-exit mapping. That evidence class exercises the named runtime paths and persisted state surfaces.

The in-tree oeh-gate claim is scoped to in-tree runtime mapper and terminal-decision semantics, not deployed-service behavior. The proof method names the runtime crate suite and mapper/terminal rows, and the evidence-class match explains that the direct runtime unit surface exercises the mapper/terminal decision code independent of the external CLI wrapper. This is a matched direct runtime surface for the scoped claim.

## Residual ambiguity / stop-condition notes

All required inputs were readable, the mode was valid, the contract was non-blank and read before scoring, and the report path directory was writable. No `BLOCKED` or `NEEDS_INPUT` stop condition applies.

LOW
