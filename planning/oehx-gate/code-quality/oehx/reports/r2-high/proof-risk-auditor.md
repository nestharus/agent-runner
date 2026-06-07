# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| `mode` | `phase-3-proposal` | -- | -- | Valid mode. |
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | -- | -- | Used to resolve referenced source and evidence paths. |
| `operator` | `/home/nes/ai/agents/proof-risk-auditor.md` | 9200 | `788f5bdea5ab` | Operator instructions read. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | 1964 | `b89c049ae474` | Readable; exact `## Proof plan` is `proposal.md:29-33`. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | 8167 | `9c861581e772` | Readable before scoring; referenced by proposal proof plan. |
| `runtime evidence` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/evidence/runtime-tests.log` | 7622 | `77df12577fdb` | Readable; proposal references this runtime evidence. |
| `report_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/proof-risk-auditor.md` | -- | -- | Destination for this report only. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:29-33` contains an exact section. |
| `Runtime claim` | No | The exact section delegates to `contracts/oehx.contract.md` and says fixtures prove launch/resume failure, clean paths, and in-tree preservation, but it has no `Runtime claim` field. The referenced contract has a `Claim` column at `contracts/oehx.contract.md:41-48`, not the required field. |
| `Proof method` | No | The exact section references the contract and `evidence/runtime-tests.log`, but it has no `Proof method` field. The referenced contract has a `Proof` column at `contracts/oehx.contract.md:41-48`, not the required field. |
| `Evidence-class match` | No | Neither `proposal.md:29-33` nor the referenced contract proof table at `contracts/oehx.contract.md:41-48` explicitly explains why the named evidence class exercises the runtime claim rather than a proxy surface. |
| Self-certification | No | The section does not claim the proof plan itself is validation; the blocking issue is missing required structure and evidence-class explanation. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| PR-001 | HIGH | Missing required `Runtime claim` field. The surrounding artifacts imply runtime finalization claims for external provider launch/resume terminal-error exit-zero handling and in-tree parity, while the caller context also requires recovered-stream success and F4 quota/rate-text parity; those claims are not named in the proposal's exact proof-plan field. | `proposal.md:29-33` | Not reached as a required field because the runtime claim field is absent; the section only delegates to the contract and runtime log. | Unclassified due missing required field. | Cannot be bound from the missing field; implied artifacts include the `oulipoly-agent-runner` launch/resume path, result envelope, and `StateDb` invocation row. | `proposal.md:29-33`; `contracts/oehx.contract.md:39-48`; `src-tauri/tests/s10_external_provider_resume.rs:397-414`; `src-tauri/tests/opencode_terminal_error_exit_zero.rs:17-82`; `crates/oulipoly-runtime/src/executor/cli.rs:328-359`; `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs:1868-1888` | Yes |
| PR-002 | HIGH | Runtime-artifact-bound claims are only implied by the proposal and contract, not captured in a required `Runtime claim` field. | `proposal.md:29-33` | Missing required `Proof method` field. The proposal points to `contracts/oehx.contract.md` and `evidence/runtime-tests.log`; the referenced contract names tests in a `Proof` column, but the proposal proof plan does not contain the required field. | Unscoped mixed evidence by implication: high-seam fake-provider CLI tests, in-tree opencode tests, and unit/runtime tests are named outside the required field. | The method would need to explicitly bind shipped test/runtime artifacts such as `cargo test -p oulipoly-agent-runner --test s10_external_provider_resume`, `cargo test -p oulipoly-agent-runner --test opencode_terminal_error_exit_zero`, and the runtime/classifier parity suites to the corresponding runtime claims. | `proposal.md:29-33`; `contracts/oehx.contract.md:43-48`; `evidence/runtime-tests.log:72-87` | Yes |
| PR-003 | HIGH | The implied claims are runtime-artifact-bound because they assert host-side finalization/classification behavior for production-path launch/resume execution, result envelope emission, and persisted invocation outcomes. | `proposal.md:29-33` | No required `Evidence-class match` field. The referenced proof table names tests and the runtime log records passing commands, but the plan does not explain why real-binary/fake-provider fixture evidence plus StateDb assertions exercise the runtime behavior rather than only a proxy surface. | Mixed/proxy risk remains unaccepted: fake provider CLI and unit tests can be valid only if explicitly scoped, while runtime claims require explicit runtime-artifact matching. | Project runtime binary execution with result-envelope verification and `StateDb` invocation-row evidence for launch/resume finalization; in-tree opencode runtime execution for recovered/failure parity; runtime classifier evidence for quota/rate text parity. | `proposal.md:29-33`; `contracts/oehx.contract.md:43-48`; `evidence/runtime-tests.log:72-87`; `src-tauri/tests/s10_external_provider_resume.rs:129-150`; `src-tauri/tests/s10_external_provider_resume.rs:456-484` | Yes |

## Evidence-class decision

The exact proposal proof-plan section is present but structurally insufficient. It lacks all three required fields: `Runtime claim`, `Proof method`, and `Evidence-class match`.

Using the contract and proposal together, the relevant claims are runtime-artifact-bound: they concern external provider launch/resume finalization, host result envelopes, persisted invocation rows, and in-tree terminal-error parity. The referenced proof evidence appears potentially runtime-shaped in part: `src-tauri/tests/s10_external_provider_resume.rs` runs `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`, asserts process/output failure, and checks the persisted invocation row; `runtime-tests.log` records the S10 and opencode suites passing. However, that cannot make the proof-risk verdict LOW because the exact proof plan does not contain the required fields and does not explicitly match each runtime claim to its evidence class.

The missing evidence-class explanation matters here because the referenced evidence is mixed. Fake external-provider CLI fixtures and unit mapper tests are proxy surfaces unless the proof plan explicitly scopes them, while runtime finalization claims need an explicit project runtime artifact binding. No such binding appears in `proposal.md:29-33` or in the referenced contract proof table.

## Residual ambiguity / stop-condition notes

No stop-condition blocks report generation: required inputs were supplied, `mode` is valid, `proposal_path` and `contract_path` are readable, and the report destination parent exists. No `NEEDS_INPUT` is warranted because the issue is not an unresolved human-owned identity conflict; it is a concrete missing-field and missing evidence-class-match condition under the operator.

HIGH
