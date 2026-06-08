# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` | n/a | n/a | Used to resolve relative evidence paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md` | 6536 | `4ee5401978e5` | Read successfully. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md` | 20817 | `e717e016111a` | Read before scoring. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/evidence/runtime-tests.log` | 286641 | `070c4e3ef1d` | Read as supplied proof evidence context. |
| report_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/proof-risk-auditor.md` | n/a | n/a | Written by this audit. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| Exact `## Proof plan` section | Yes | `proposal.md:28-45`. |
| `Runtime claim` | Yes | Table header at `proposal.md:32`; claim rows at `proposal.md:34-45`. |
| `Proof method` | Yes | Table header at `proposal.md:32`; concrete command/test rows at `proposal.md:34-45`. |
| `Evidence-class match` | Yes | Table header at `proposal.md:32`; runtime CLI, executable adapter, StateDb, SQLite, sidecar, and workspace evidence-class statements at `proposal.md:34-45`. |
| Self-certification only | No | Rows name executable validation surfaces, not the proof plan itself: `cargo test --workspace`, focused `cargo test -p oulipoly-agent-runner --test ...`, and `bash scripts/tests/opencode-turns.test.sh`. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| None | n/a | No missing proof-plan structure, self-certification, proxy-only runtime proof, or evidence-class mismatch found. | `proposal.md:28-45` | Runtime CLI integration, executable adapter tests, StateDb/SQLite unit evidence, sidecar mailbox integration, and full workspace Rust tests. | Fixture/fake-provider evidence is scoped to executable adapter or integration-fixture behavior and is not used as the sole proof for an unscoped production external-service claim. | Compiled runner integration tests, committed `scripts/opencode-turns`, StateDb SQLite-backed tests, PID sidecar mailbox DB, and workspace runtime test suite. | `proposal.md:34-45`; `contract.md:5`, `contract.md:26-69`, `contract.md:169-259`; `runtime-tests.log:12-4550`. | No |

## Evidence-class decision

The proof plan is structurally complete. The proposal names runtime behavior claims for wake delivery confirmation, OpenCode turn ingestion, StateDb nonce lookup, dead-PID wake-claim reclaimability, startup/sweep recovery, live-owner suppression, consumed/unconfirmed suppression, #44 backlog hardening, existing proactive wake behavior, and workspace-wide behavior at the audited head.

The contract binds those claims to runtime and adapter surfaces: StateDb persistence, PID sidecar mailbox and wake-claim storage, committed `scripts/opencode-turns`, CLI dispatch, mailbox delivery, resume orchestration, Tauri startup/maintenance sweep, and wake coordinator planning/reaping (`contract.md:5`, `contract.md:26-69`, `contract.md:169-259`). The proposal's proof methods match that scope with focused compiled-runner integration tests, SQLite/sidecar-backed unit evidence, executable adapter tests, and the full Rust workspace run (`proposal.md:34-45`).

The supplied evidence log contains the named runtime rows: workspace tests pass from `runtime-tests.log:12` through final `PASS` at `runtime-tests.log:4549-4550`; wake-confirm legacy OpenCode integration passes at `runtime-tests.log:4507-4517`; the proactive wake integration suite passes, including `wake_sweep_backlog_recovers_recent_leak_and_reaps_dead_owner_debris`, live-owner suppression, consumed suppression, unconfirmed suppression, and dead-claim recovery at `runtime-tests.log:4520-4543`; `bash scripts/tests/opencode-turns.test.sh` passes at `runtime-tests.log:4546-4547`; StateDb nonce and dead-PID claim unit rows are present at `runtime-tests.log:4077` and `runtime-tests.log:4179`.

The fake OpenCode/fake provider fixture class is not a mismatch here because the claims are framed as committed adapter behavior and runtime CLI integration over stateful sidecar/StateDb evidence, not as real external OpenCode service availability or deployed-container startup. Mixed evidence is explicitly scoped: adapter parsing/targeting receives executable adapter proof, while wake delivery/sweep/state claims receive compiled runner, SQLite, mailbox sidecar, and runtime integration proof.

## Residual ambiguity / stop-condition notes

No stop condition fired. The contract path was readable and non-blank before scoring. Prior #42/#43 reports were not treated as self-certifying; the current proposal, contract, and supplied runtime evidence were re-scored. The residual bounded-sweep backlog behavior is explicitly documented as accepted residual risk at `proposal.md:47-49` and `contract.md:261-263`, not hidden proof coverage.

LOW
