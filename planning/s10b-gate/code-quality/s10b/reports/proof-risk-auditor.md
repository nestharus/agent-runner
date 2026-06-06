# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | N/A | N/A | Allowed mode. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md` | 9101 | `59b90d2d2642` | Readable; exact `## Proof plan` found. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` | 18516 | `f457fc8e7fc5` | Read before scoring for supplied code-quality context. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/proof-risk-auditor.md` | N/A | N/A | Report destination. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Used to resolve relative evidence paths. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:9` opens the proof-plan section. |
| `Runtime claim` | Yes | Ten runtime-claim entries at `proposal.md:13`, `proposal.md:19`, `proposal.md:25`, `proposal.md:31`, `proposal.md:37`, `proposal.md:43`, `proposal.md:49`, `proposal.md:55`, `proposal.md:61`, and `proposal.md:67`. |
| `Proof method` | Yes | Ten proof-method entries at `proposal.md:15`, `proposal.md:21`, `proposal.md:27`, `proposal.md:33`, `proposal.md:39`, `proposal.md:45`, `proposal.md:51`, `proposal.md:57`, `proposal.md:63`, and `proposal.md:69`. |
| `Evidence-class match` | Yes | Ten evidence-class entries at `proposal.md:17`, `proposal.md:23`, `proposal.md:29`, `proposal.md:35`, `proposal.md:41`, `proposal.md:47`, `proposal.md:53`, `proposal.md:59`, `proposal.md:65`, and `proposal.md:71`. |
| Self-certification | No | The plan names concrete tests, fixtures, and live logs rather than claiming the plan itself is validation. |
| Relative evidence path resolution | Yes | `planning/s10b-gate/evidence/runtime-tests.log` resolves under `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan field, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | `proposal.md:9-71` | Named registry, provider-client, provider-settings, executor, full CLI integration, source-invariant, and live-smoke surfaces. | Proxy/source evidence is scoped to proxy or source-invariant claims; runtime claims receive integration, CLI, state DB, or live-smoke evidence. | Full CLI launch/resume path and live installed external launch where those runtime artifacts are claimed. | `proposal.md:13-71`; `s10b.contract.md:139-150`; `s10b.contract.md:209-238` | No |

## Evidence-class decision

The proof-plan structure is complete. Each claim names a proof method and an evidence-class match, and the plan does not rely on self-certification or generic “tests pass” language.

The process-PATH binary-resolution claim is exercised by registry tests that materialize a temporary executable, supply resolver PATH entries, and assert success, missing-artifact preservation, and unset-PATH no-panic behavior. The production construction-site claim is now explicitly scoped as a source-guard/source-invariant claim, and the evidence-class match says that source coverage is the intended target while runtime binary resolution and live launch are covered by separate claims. That avoids treating the source guard as proxy proof for an unscoped runtime-artifact claim.

The provider protocol and settings claims are adapter or host-integration claims. Their methods use provider-client subprocess, provider-settings host, and external-provider executor fixtures to exercise typed DTO acceptance, process-status mapping, policy/launch request shape, inherited environment preservation, provider args placement, and LaunchExit session capture behavior. These evidence classes match the contract's adapter and test-harness declarations rather than substituting unrelated static checks.

The provider-ref resume and recorded-cwd claims are runtime-path claims, and their proof method is a full CLI integration fixture with an external provider script, launch-plus-resume command execution, provider request recording, and state DB invocation row assertions. That is a project-specific runtime-shaped proof for the claimed headless resume path, known-session launch request, migration/rotation bypass, capture metadata, and recorded cwd behavior.

The live installed external launch claim is explicitly limited to launch, not resume. Its proof method cites `/tmp/s10-e2e/final.log` and `/tmp/s10-e2e/final2.log`, which the proof plan describes as live smoke logs containing the external provider result marker and final successful `OULIPOLY_RESULT`. Because the proposal expressly excludes live resume from the claim, the deterministic resume integration evidence is not being used as a proxy for an unclaimed live resume result.

## Residual ambiguity / stop-condition notes

No `NEEDS_INPUT` or `BLOCKED` condition is present. The supplied contract was readable and names the relevant production, adapter, intrinsic, and test-harness surfaces. No `state.db` schema migration is declared or claimed, so there is no production DB migration proof class to match.

LOW
