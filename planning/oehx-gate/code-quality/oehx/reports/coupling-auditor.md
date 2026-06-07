# Coupling Audit

## Inputs Read

| Input | Path | Status |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | read for source inspection |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | context |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` | context |
| `wu_id` | `oehx` | context |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | read |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | read |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt` | read |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | read |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/coupling-auditor.md` | written |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/coupling-auditor.md` | Operator loaded; A1 coupling binding at lines 63-81 and output format at lines 117-132. |
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary at lines 21-27, touched-file ownership at lines 143-149, adapter declarations at lines 180-210, intrinsic-surface declarations at lines 212-253, and A1 row at lines 295-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer-rerun semantics at lines 29-35. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-surface risk ownership at lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 coupling-decision context at lines 500-509 and non-LOW residual prohibition at lines 627-631. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | Exact `## Adapter declarations` at lines 49-116 and exact `## Intrinsic-surface declarations` at lines 118-139. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Proposal design claim that `terminal_signal.rs` owns shared rules and external mappers consume them at lines 17-23. |

Metric binding applied exactly: `Coupling by distinct external symbols/modules referenced`: LOW = `0-2`; MEDIUM = `3-5`; HIGH = `>= 6`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `touched-files.txt:1`; `diff.patch:1-81`; source lines 1-17 | Touched whole file; declared adapter in contract lines 88-92. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `touched-files.txt:2`; `diff.patch:82-239`; source lines 1-158 | Touched whole file; declared adapter in contract lines 81-87. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `touched-files.txt:3`; `diff.patch:240-271`; source lines 1-433 | Touched whole facade file; declared adapter in contract lines 53-59. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt:4`; `diff.patch:272-315`; source lines 1-137 | Touched whole file; declared adapter in contract lines 68-73 and intrinsic surface in contract lines 132-139. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt:5`; `diff.patch:316-382`; source lines 1-409 | Touched whole file; declared adapter in contract lines 60-67 and intrinsic surface in contract lines 122-131. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `touched-files.txt:6`; `diff.patch:383-542`; source lines 1-167 | Touched whole file; declared adapter in contract lines 74-80. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `touched-files.txt:7`; `diff.patch:543-570`; source lines 1-1897 | Touched whole test file; declared adapter in contract lines 93-100. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `touched-files.txt:8`; `diff.patch:571-583`; source lines 1-340 | Touched whole test file; declared adapter in contract lines 101-108. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt:9`; `diff.patch:584-809`; source lines 1-712 | Touched whole test fixture; declared adapter in contract lines 109-115. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | external provider terminal-signal reason formatting | 1 raw symbol; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `external-provider-terminal-signal-contract`; `terminal-failure-exit-reason-contract` | 2 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 88-92; source lines 3-17 reference `TerminalSignalKind` and reason strings subordinate to declared contracts. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | external provider result classification and execution-result DTOs | raw mapper references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `external-provider-process-status-contract`; `external-provider-terminal-signal-contract`; `execution-result-dto-contract`; `terminal-failure-exit-reason-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 81-87; source lines 6-13, 15-34, 37-77, and 79-158 map provider status/signal into terminal classification and tests within declared contracts. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | executor CLI facade and component-set contracts | raw facade/module/test references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli.rs` | `executor-public-entrypoint-contract`; `executor-cli-component-set-contract`; `executor-cli-test-fixture-contract`; `tempfile-unix-permissions-test-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 53-59; source lines 67-107 re-export CLI component surfaces and source lines 115-433 contain facade-local fixture tests subordinate to declared contracts. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | supervised terminal output contracts | raw terminal-output references; adapter/intrinsic-counted as contracts/domain | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `terminal-signal-classification-contract`; `std-process-exit-status-contract`; `supervised-output-contract` | 3 | LOW | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_terminal_output_mapping` | Synthetic failure exit code on real exit 0; real nonzero preservation; supervised terminal reason propagation | 1 | LOW | LOW | blocking-owned touched file | Contract lines 68-73 and 132-139; source lines 21-75 and 77-137 use terminal signal/status and supervised output references subordinate to those surfaces. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | terminal signal vocabulary, status, recognizer, and signal-forwarding contracts | raw terminal/signal references; adapter/intrinsic-counted as contracts/domain | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `std-process-exit-status-contract`; `unix-signal-name-contract`; `signal-hook-forwarding-contract`; `executor-terminal-signal-dto-contract`; `terminal-signal-recognizer-contract` | 5 | LOW | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `runtime terminal-signal vocabulary + reason mapping` | Full `TerminalSignalKind` vocabulary; terminal status and synthetic exit-code mapping; built-in terminal evidence; canonical reason hook; provider-error evidence preservation; shared failure-exit override | 1 | LOW | LOW | blocking-owned touched file | Contract lines 60-67 and 122-131; source lines 46-64, 66-226, 228-386, and 388-409 keep signal-hook, process-status, recognizer, DTO, and owned reason-mapping references within declared surfaces. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | external provider cancel status/signal to host terminal outcome | raw mapper references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `external-provider-process-status-contract`; `external-provider-terminal-signal-contract`; `executor-terminal-signal-dto-contract`; `terminal-failure-exit-reason-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 74-80; source lines 6-14, 16-95, and 97-167 map provider `ProcessStatus`/`TerminalSignal` into host outcome via declared contracts. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | executor service-port and external provider fixture contracts | raw integration-test references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `executor-service-port test harness contract`; `provider-registry fixture contract`; `external-provider client cancellation contract`; `Unix fixture script and environment contract`; `serde_json fixture parsing contract` | 5 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 93-100; source lines 11-26, 80-147, 249-260, 705-728, 1803-1816, and 1879-1894 reference only the declared harness, registry fixture, cancellation, Unix/env, and JSON fixture surfaces. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | executor service-port, provider-registry, terminal-vocabulary, Unix fixture, and JSON contracts | raw integration-test references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `executor-service-port test harness contract`; `provider-registry fixture contract`; `executor terminal-signal vocabulary contract`; `Unix fixture script and environment contract`; `serde_json fixture parsing contract` | 5 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 101-108; source lines 7-21, 45-123, 157-184, 186-210, and 228-340 stay subordinate to the declared harness, registry, terminal-vocabulary, Unix/env, and JSON fixture surfaces. |
| `src-tauri/tests/s10_external_provider_resume.rs` | external-provider end-to-end fixture contracts | raw integration-test references; adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `src-tauri/tests/s10_external_provider_resume.rs` | `Unix CLI integration fixture contract`; `Oulipoly result-envelope JSON contract`; `StateDb invocation terminal fields`; `fake external provider CLI contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 109-115; source lines 12-20, 98-164, 245-309, 405-494, and 533-712 reference the declared CLI, result envelope, StateDb row, and fake provider surfaces. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| n/a | n/a | n/a | No MEDIUM or HIGH per-pair scores. | Every touched component has an explicit, resolvable adapter declaration with `<= 5` `Translates:` contracts, and the two intrinsic-surface declarations each declare one domain with non-empty `Owns:` sets. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6 contract was readable, non-blank, and contained exact `## Adapter declarations` and `## Intrinsic-surface declarations` sections. The A1 metric row `Coupling by distinct external symbols/modules referenced` is present in `code-quality.md` lines 295-300.

All adapter declarations in `oehx.contract.md` lines 53-115 name touched component boundaries, set `role: adapter`, and provide non-empty `Translates:` lists. All intrinsic-surface declarations in lines 122-139 name touched component boundaries, set `role: intrinsic-surface`, provide exactly one `Domain:`, and provide non-empty `Owns:` lists.

LOW
