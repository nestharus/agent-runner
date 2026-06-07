# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| `mode` | `phase-3-proposal` | -- | -- | Valid mode. |
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | -- | -- | Used to resolve referenced source and evidence paths. |
| `operator` | `/home/nes/ai/agents/proof-risk-auditor.md` | 9200 | `788f5bdea5ab` | Operator instructions read. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | 5183 | `d77827e95357` | Readable; exact `## Proof plan` is `proposal.md:29-55`. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | 9166 | `ddedecb08f49` | Readable before scoring; declares runtime obligations and proof scope. |
| `runtime evidence` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | 7622 | `77df12577fdb` | Readable; proposal references this evidence log. |
| `report_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/proof-risk-auditor.md` | -- | -- | Destination for this report only. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | Exact section at `proposal.md:29-55`. |
| `Runtime claim` | Yes | Four runtime-claim entries at `proposal.md:33`, `proposal.md:39`, `proposal.md:45`, and `proposal.md:51`. |
| `Proof method` | Yes | Matching proof-method entries at `proposal.md:35`, `proposal.md:41`, `proposal.md:47`, and `proposal.md:53`. |
| `Evidence-class match` | Yes | Matching evidence-class explanations at `proposal.md:37`, `proposal.md:43`, `proposal.md:49`, and `proposal.md:55`. |
| Self-certification | No | The plan names concrete validation surfaces and does not rely on the proof plan itself or unspecific "tests pass" wording. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | -- | No proof-risk finding. | `proposal.md:29-55` | The plan names per-claim tests and command evidence. | Mixed evidence is explicitly scoped. | Runtime claims are bound to the project runtime binary, result envelope, and `StateDb` invocation row where required. | `proposal.md:33-55`; `contracts/oehx.contract.md:39-48`; `runtime-tests.log:72-87`; `src-tauri/tests/s10_external_provider_resume.rs:138-159`; `src-tauri/tests/s10_external_provider_resume.rs:405-423`; `src-tauri/tests/s10_external_provider_resume.rs:465-493` | No |

## Evidence-class decision

The proposal proof plan is structurally complete. It names four runtime claims: external launch failure-signal plus exited(0) finalization, external resume finalization with the same evidence, unchanged clean external paths, and unchanged in-tree oeh-gate semantics.

The external launch and resume claims are runtime-artifact-bound because they assert host-side launch/resume execution, result-envelope semantics, and durable invocation-row persistence. The proof method names `src-tauri/tests/s10_external_provider_resume.rs` tests that run `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`, isolate runtime config/data homes, drive a fake external provider CLI through the production launch/resume path, and assert failed process/envelope plus `StateDb` row outcomes. The fake provider CLI is a protocol fixture, but the runtime claim is about the runner finalization behavior on provider protocol evidence, and the proof plan explicitly explains that distinction.

The clean external-path claim is also runtime-artifact-bound for unchanged success behavior and real nonzero preservation. Its proof method combines integration fixtures for unchanged success paths with mapper unit tests for non-override branches; the evidence-class match scopes the unit tests to shared-rule seams and uses runtime CLI integration for end-to-end behavior.

The in-tree oeh-gate preservation claim is covered by the same runtime/unit and CLI-integration suites named in the original in-tree proof rows and re-run at this delta, with command evidence recorded in `runtime-tests.log`. The contract's proof table independently aligns the same four claim groups to the S10, opencode, runtime, and mapper proof surfaces.

No proxy-only proof or evidence-class mismatch remains. Mixed evidence is acceptable here because proxy/fixture evidence is scoped to protocol inputs and mapper seams, while runtime finalization claims receive explicit runtime-artifact evidence and persisted-state verification.

## Residual ambiguity / stop-condition notes

No stop-condition blocks report generation: required inputs were supplied, `mode` is valid, `proposal_path` and `contract_path` are readable, the contract was read before scoring, and the report destination parent exists. No `NEEDS_INPUT` is warranted because the claim/evidence identity is resolved from the proposal, contract, evidence log, and referenced source surfaces.

LOW
