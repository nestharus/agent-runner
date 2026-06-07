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
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 code-quality and coupling-decision context at lines 500-509; non-LOW gate disposition at lines 528-538 and 627-631. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | Exact `## Adapter declarations` at lines 49-100 and exact `## Intrinsic-surface declarations` at lines 102-123. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Proposal design claim that `terminal_signal.rs` owns shared rules and external mappers consume them at lines 17-23. |

Metric binding applied exactly: `Coupling by distinct external symbols/modules referenced`: LOW = `0-2`; MEDIUM = `3-5`; HIGH = `>= 6`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `touched-files.txt:1`; `diff.patch:1-81`; source lines 1-17 | Touched whole file; declared adapter in contract lines 88-92. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `touched-files.txt:2`; `diff.patch:82-235`; source lines 1-155 | Touched whole file; declared adapter in contract lines 81-87. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `touched-files.txt:3`; `diff.patch:236-249`; source lines 1-104 | Touched facade file; declared adapter in contract lines 53-59. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt:4`; `diff.patch:250-280`; source lines 1-135 | Touched whole file; declared adapter in contract lines 68-73 and intrinsic surface in contract lines 116-123. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt:5`; `diff.patch:281-347`; source lines 1-409 | Touched whole file; declared adapter in contract lines 60-67 and intrinsic surface in contract lines 106-115. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `touched-files.txt:6`; `diff.patch:348-503`; source lines 1-164 | Touched whole file; declared adapter in contract lines 74-80. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `touched-files.txt:7`; `diff.patch:504-518`; source lines 1-1889 | Touched whole test file; no matching adapter or intrinsic-surface declaration in the contract. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `touched-files.txt:8`; `diff.patch:519-530`; source lines 1-340 | Touched whole test file; no matching adapter or intrinsic-surface declaration in the contract. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt:9`; `diff.patch:531-747`; source lines 1-703 | Touched whole test fixture; declared adapter in contract lines 93-99. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | terminal-signal reason formatting contracts | 1 raw symbol (`TerminalSignalKind`) | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `external-provider-terminal-signal-contract`; `terminal-failure-exit-reason-contract` | 2 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 88-92; source lines 3-17. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | external provider result classification contracts | 9 raw references, adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `external-provider-process-status-contract`; `external-provider-terminal-signal-contract`; `execution-result-dto-contract`; `terminal-failure-exit-reason-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 81-87; source lines 3-10 and 12-74. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | executor CLI facade and component-set contracts | many raw facade/module/test references, adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli.rs` | `executor-public-entrypoint-contract`; `executor-cli-component-set-contract`; `executor-cli-test-fixture-contract`; `tempfile-unix-permissions-test-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 53-59; source facade/component-set lines 64-104 and local adapter declaration lines 13-24. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | supervised terminal output contracts | 8 raw references, adapter/intrinsic-counted as contracts/domain | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `terminal-signal-classification-contract`; `std-process-exit-status-contract`; `supervised-output-contract` | 3 | LOW | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_terminal_output_mapping` | Synthetic failure exit code on real exit 0; real nonzero preservation; supervised terminal reason propagation | 1 | LOW | LOW | blocking-owned touched file | Contract lines 68-73 and 116-123; source lines 19-73. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | terminal signal vocabulary, status, recognizer, and signal-forwarding contracts | many raw references, adapter/intrinsic-counted as contracts/domain | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `std-process-exit-status-contract`; `unix-signal-name-contract`; `signal-hook-forwarding-contract`; `executor-terminal-signal-dto-contract`; `terminal-signal-recognizer-contract` | 5 | LOW | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `runtime terminal-signal vocabulary + reason mapping` | Full `TerminalSignalKind` vocabulary; terminal status and synthetic exit-code mapping; built-in terminal evidence; canonical reason hook; provider-error evidence preservation; shared failure-exit override | 1 | LOW | LOW | blocking-owned touched file | Contract lines 60-67 and 106-115; source lines 46-64, 66-80, 83-226, and 228-386. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | external provider terminal cancel mapping contracts | 8 raw references, adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `external-provider-process-status-contract`; `external-provider-terminal-signal-contract`; `executor-terminal-signal-dto-contract`; `terminal-failure-exit-reason-contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 74-80; source lines 3-11 and 19-92. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | runtime/config/provider/std integration fixture surfaces | >= 6 raw distinct external symbols/modules | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | HIGH | blocking-owned touched file | `touched-files.txt:7`; diff lines 504-518; imports at source lines 3-18 include `oulipoly_config`, `oulipoly_runtime::executor`, `oulipoly_runtime::executor::cli`, `oulipoly_runtime::provider_registry`, `oulipoly_runtime::services`, `serde_json`, and multiple `std::*` modules; source lines 1803-1810 add `oulipoly_provider::client` cancellation/client-option references. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | runtime/config/provider/std integration fixture surfaces | >= 6 raw distinct external symbols/modules | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | HIGH | blocking-owned touched file | `touched-files.txt:8`; diff lines 519-530; imports at source lines 7-21 include `oulipoly_config`, `oulipoly_runtime::executor`, `oulipoly_runtime::executor::terminal_signal::TerminalSignalKind`, `oulipoly_runtime::provider_registry`, `oulipoly_runtime::services`, `serde_json`, and multiple `std::*` modules. |
| `src-tauri/tests/s10_external_provider_resume.rs` | external-provider end-to-end fixture contracts | 9 raw references, adapter-counted as contracts | `planning/oehx-gate/contracts/oehx.contract.md` | `src-tauri/tests/s10_external_provider_resume.rs` | `Unix CLI integration fixture contract`; `Oulipoly result-envelope JSON contract`; `StateDb invocation terminal fields`; `fake external provider CLI contract` | 4 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking-owned touched file | Contract lines 93-99; source lines 3-11, 241-299, 397-485, and 524-703. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| HIGH | blocking | `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` is touched by `touched-files.txt:7` and `diff.patch:504-518`; `code-quality.md` lines 21-27 and 143-149 require whole touched-file scoring. | Source lines 3-18 reference `oulipoly_config::{InputDef, InputType, ModelConfig, PromptMode, ProviderConfig, ProviderImplementationRef}`, `oulipoly_runtime::executor`, `oulipoly_runtime::executor::cli::{self, EffectiveExecuteRequest}`, `oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryOptions}`, `oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest, ServiceError}`, `serde_json::Value`, and `std::*`; lines 1803-1810 reference `oulipoly_provider::client::CancellationToken` and `ProviderClientOptions`. | The touched file has no matching declaration in `oehx.contract.md` lines 49-123, so raw A1 applies. The cited references exceed the HIGH threshold of `>= 6` distinct external symbols/modules. |
| HIGH | blocking | `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` is touched by `touched-files.txt:8` and `diff.patch:519-530`; `code-quality.md` lines 21-27 and 143-149 require whole touched-file scoring. | Source lines 7-21 reference `oulipoly_config::{ModelConfig, PromptMode, ProviderConfig, ProviderImplementationRef}`, `oulipoly_runtime::executor`, `oulipoly_runtime::executor::terminal_signal::TerminalSignalKind`, `oulipoly_runtime::provider_registry::{ProviderRegistry, ProviderRegistryHandle, ProviderRegistryOptions}`, `oulipoly_runtime::services::{ExecutorServicePort, ExecutorServiceRequest}`, `serde_json::Value`, and `std::*`. | The touched file has no matching declaration in `oehx.contract.md` lines 49-123, so raw A1 applies. The cited references exceed the HIGH threshold of `>= 6` distinct external symbols/modules. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6 contract was readable, non-blank, and contained exact `## Adapter declarations` and `## Intrinsic-surface declarations` sections. The A1 metric row `Coupling by distinct external symbols/modules referenced` is present in `code-quality.md` lines 295-300.

All contract declarations that name touched component boundaries are syntactically valid and resolvable. The two HIGH rows are blocking, not residual, because their files are inside the touched set and have no matching explicit adapter or intrinsic-surface declaration.

HIGH
