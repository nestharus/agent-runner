# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | N/A | N/A | Allowed mode. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md` | 9326 | `d306cf559cbe` | Readable; exact `## Proof plan` found. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` | 29878 | `df6f0c26b082` | Read before scoring for required Phase 6 context. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/proof-risk-auditor.md` | N/A | N/A | Report destination; only written artifact. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | N/A | N/A | Used to resolve relative evidence paths. |
| evidence log | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/evidence/runtime-tests.log` | 6386 | `965254d4d40f` | Referenced by proof plan; read for evidence-class context only. |
| live launch log | `/tmp/s10-e2e/final.log` | 1664 | `df11cb555aeb` | Referenced by proof plan for launch-only live smoke. |
| live launch log | `/tmp/s10-e2e/final2.log` | 1648 | `68f6895785bf` | Referenced by proof plan for launch-only live smoke. |
| source diff context | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch` | 80921 | `fed91f78e958` | Context only; scored artifacts are proposal and contract. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:9` opens the proof-plan section. |
| `Runtime claim` | Yes | Ten runtime-claim entries at `proposal.md:13`, `proposal.md:19`, `proposal.md:25`, `proposal.md:31`, `proposal.md:37`, `proposal.md:43`, `proposal.md:49`, `proposal.md:55`, `proposal.md:61`, and `proposal.md:67`. |
| `Proof method` | Yes | Ten proof-method entries at `proposal.md:15`, `proposal.md:21`, `proposal.md:27`, `proposal.md:33`, `proposal.md:39`, `proposal.md:45`, `proposal.md:51`, `proposal.md:57`, `proposal.md:63`, and `proposal.md:69`. |
| `Evidence-class match` | Yes | Ten evidence-class entries at `proposal.md:17`, `proposal.md:23`, `proposal.md:29`, `proposal.md:35`, `proposal.md:41`, `proposal.md:47`, `proposal.md:53`, `proposal.md:59`, `proposal.md:65`, and `proposal.md:71`. |
| Self-certification | No | The plan names concrete registry, provider-client, provider-settings, executor, CLI integration, source-guard, state-row, and live-smoke surfaces rather than claiming the plan itself is validation. |
| Relative evidence path resolution | Yes | `planning/s10b-gate/evidence/runtime-tests.log` resolves under `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | LOW | No missing proof-plan field, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | `proposal.md:9-71` | Named registry tests, provider-client subprocess fixture, provider-settings host fixture, external-provider executor fixture, source invariant, full CLI launch/resume integration, and launch-only live smoke logs. | Proxy/source evidence is scoped to proxy, adapter, intrinsic, or source-invariant claims; runtime-path claims receive CLI integration, state DB row assertions, or live-smoke evidence. | Actual CLI launch/resume path for provider-ref resume claims; installed external launch logs for the live launch claim; no production DB migration artifact because no schema change is declared. | `proposal.md:13-71`; `s10b.contract.md:219-307`; `runtime-tests.log:48-67`; `/tmp/s10-e2e/final.log`; `/tmp/s10-e2e/final2.log` | No |

## Evidence-class decision

The proof-plan structure is complete. Each proof-plan entry names a runtime claim, proof method, and evidence-class match, and the plan does not rely on generic “tests pass” self-certification.

The binary provider PATH behavior is split into matching classes: registry resolver tests exercise the intrinsic resolver behavior with materialized executable and missing/unset PATH cases, while a separate source-invariant claim covers production registry construction-site opt-in. The live installed launch claim separately supplies runtime smoke evidence that the installed external provider path reached the provider binary.

The DTO, provider-settings, policy/launch request, and LaunchExit session metadata claims are adapter or host-integration claims under the contract. Their proof methods use provider-client subprocess, provider-settings host, and external-provider executor fixtures to exercise the typed protocol surfaces and request/session mapping directly, rather than substituting unrelated static checks.

The provider-ref resume and recorded-cwd claims are runtime-path claims. The proof method is a full CLI launch-plus-resume integration fixture with an external provider script, recorded provider requests, forbidden legacy subcommand assertions, persisted invocation row checks, `known_provider_session_id`, and recorded runtime cwd verification. That is a project-specific runtime-shaped proof for the host resume path being claimed.

The proposal explicitly limits live smoke evidence to installed external launch and does not claim live resume. The absence of `S10-RESUME-OK` in `/tmp/s10-e2e` is therefore not an evidence-class mismatch for the proof plan; deterministic CLI integration is the declared resume proof class.

## Residual ambiguity / stop-condition notes

No `NEEDS_INPUT` or `BLOCKED` condition is present. The supplied contract is readable and declares the relevant adapter, intrinsic-surface, test-harness, and no-`state.db`-migration scope needed for Phase 6 scoring.

LOW
