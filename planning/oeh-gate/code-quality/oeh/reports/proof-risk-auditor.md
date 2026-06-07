# Proof-risk audit report

## Inputs read
| Input | Path or value | Size | SHA excerpt | Notes |
|---|---|---:|---|---|
| mode | `phase-3-proposal` | n/a | n/a | Valid mode. |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | n/a | n/a | Used to resolve proposal evidence and source paths. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | 5166 | `d4970cd46065` | Read successfully; exact `## Proof plan` starts at line 11. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | 10546 | `4f1e7bc19748` | Read successfully before scoring. |
| evidence log | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` | 81794 | `8c0edffa8d3f` | Read as proof-plan referenced runtime command evidence. |
| report_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/proof-risk-auditor.md` | n/a | n/a | Destination path only. |

## Proof-plan parse
| Field | Present | Evidence |
|---|---:|---|
| `## Proof plan` | Yes | `proposal.md:11` is the exact heading. |
| Evidence log | Yes | `proposal.md:13` names `planning/oeh-gate/evidence/runtime-tests.log`. |
| Runtime claim | Yes | `proposal.md:15`, `proposal.md:21`, and `proposal.md:27` name the incident exit-zero failure, recovered stream success, and OpenCode F4 ordinary-output parity claims. |
| Proof method | Yes | `proposal.md:17`, `proposal.md:23`, and `proposal.md:29` name specific runtime crate and CLI integration test functions. |
| Evidence-class match | Yes | `proposal.md:19`, `proposal.md:25`, and `proposal.md:31` explain runtime unit plus CLI integration or runtime unit evidence and the observables each surface exercises. |
| Self-certification | No | The plan does not cite itself as validation and does not merely say tests pass; it names validation surfaces, commands, and expected observables. |

## Findings
| Finding ID | Severity | Runtime claim | Proof plan ref | Proof method | Proxy class | Required runtime artifact | Evidence refs | Blocks pipeline |
|---|---|---|---|---|---|---|---|---|
| No PR findings | LOW | All three proof-plan claims have required structure and matched evidence classes. | `proposal.md:15-31` | Named runtime crate tests plus CLI integration tests. | No unmatched proxy-only proof identified. | `oulipoly-agent-runner` production binary for one-shot/resume CLI paths, durable StateDb invocation rows, production `opencode::Recognizer`, and production `supervised_output_from_terminal` intrinsic surfaces. | `contract.md:33-45`, `contract.md:51-61`, `contract.md:97-124`; `runtime-tests.log:1`, `runtime-tests.log:114-115`, `runtime-tests.log:173-179`, `runtime-tests.log:978-988`; `src-tauri/tests/opencode_terminal_error_exit_zero.rs:17-81`; `src-tauri/tests/age153_support/mod.rs:238-245`; `crates/oulipoly-runtime/src/executor/providers/opencode.rs:235-285`; `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs:89-138`. | No |

## Evidence-class decision

The first runtime claim is runtime-artifact-bound because it asserts one-shot and resume finalization, result-envelope fields, real exit `0` substitution, terminal reason propagation, and durable StateDb invocation fields. The proof method is mixed but matched: recognizer and supervised-output runtime unit tests exercise the production intrinsic mapping surfaces, while the CLI integration tests run the built `oulipoly-agent-runner` binary through `CARGO_BIN_EXE_oulipoly-agent-runner` with isolated XDG roots and assert the emitted envelope plus persisted invocation row. The evidence log records both `cargo test -p oulipoly-runtime` and `cargo test -p oulipoly-agent-runner --test opencode_terminal_error_exit_zero --test structural_segmentation` exiting `0`.

The second runtime claim is also matched. The claim concerns the same runtime finalization path for a recovered stream, and the proof plan binds it to production recognizer classification, supervised-output mapping, the one-shot CLI entrypoint, and a succeeded StateDb row. The evidence class is not a proxy-only import or mock-only assertion because the CLI integration fixture executes the built runner binary and verifies process status, result JSON, and database state.

The third claim is an intrinsic OpenCode terminal-signal classification claim rather than a deployed-service or container-startup claim. The Step 6a contract declares `crates/oulipoly-runtime/src/executor/providers/opencode.rs` as owning ordinary output with quota/rate words preserving process-status classification. The named runtime unit tests directly feed the production `Recognizer` ordinary non-error OpenCode output and assert `CleanExit` and `NonzeroExit`, so the evidence class matches the claimed intrinsic classification behavior.

## Residual ambiguity / stop-condition notes

No stop condition fired. The proposal, contract, and referenced evidence log were readable, and the report destination directory was writable. No missing proof-plan field, self-certification, proxy-only proof for a runtime-artifact-bound claim, or evidence-class mismatch was found.

LOW
