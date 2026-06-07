# Push/Pull Coupling Audit

## Inputs Read

- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `base_ref=33775d7`
- `head_ref=HEAD`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch`
- `changed_files_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/push-pull-auditor.md`

## References Read

- `/home/nes/ai/agents/push-pull-auditor.md`
- `/home/nes/ai/conventions/code-quality.md`
- `/home/nes/ai/conventions/agent-questions-and-session-graph.md`
- `/home/nes/ai/workflows/auditor-surface-expansion.md`
- `crates/oulipoly-provider/src/generated.rs`
- `crates/oulipoly-provider/src/terminal.rs`
- `crates/oulipoly-provider/src/schemas.rs`
- `crates/oulipoly-provider/src/stream.rs`
- `src-tauri/tests/age153_support/mod.rs`

A1 preservation check passed: `code-quality.md` contains `## Push-vs-pull system coupling`, the session-graph disambiguator, the `uncontrolled-source coupler` failure mode, and `## Numerical thresholds`; `agent-questions-and-session-graph.md` contains the distinct `## Pull-vs-Push Policy` context-transfer section.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | Runtime terminal-signal vocabulary | `fixed_reason_for_kind` matches `TerminalSignalKind` variants and returns canonical reason strings | LOW source-control/common-interface proof: same repo owns `TerminalSignalKind`; OEHX contract declares `reason_format.rs` as a thin consumer of `terminal-failure-exit-reason-contract`. | LOW | Source lines 3-17; contract lines 14 and 88-92; `crates/oulipoly-provider/src/terminal.rs` lines 3-14. |
| PP-002 | `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | Provider DTOs `ProcessStatus`, `TerminalClassifyResult`, provider `TerminalSignalKind`, shared terminal failure-exit/reason helpers | Maps generated provider enum/status fields into runtime `TerminalClassification` and calls `terminal_exit_code_from_signal` / `terminal_reason_from_signal_status` | LOW common-interface proof: provider DTOs are in-repo generated contract surfaces; OEHX contract declares this component translates external-provider process/status, terminal-signal, execution-result, and terminal-failure-exit/reason contracts. | LOW | Source lines 3-31 and 34-74; diff lines 89-146; contract lines 15, 81-87; `crates/oulipoly-provider/src/generated.rs` lines 7, 215-254. |
| PP-003 | `crates/oulipoly-runtime/src/executor/cli.rs` | Executor CLI component set and terminal helper re-exports | Facade re-exports `terminal_exit_code_from_signal` and `terminal_reason_from_signal_status`; test fixture writes/reads temp scripts and process outputs | LOW source-control proof: facade and sibling modules are in the same repo/component set; local doc and OEHX contract declare `cli.rs` as adapter for executor public entrypoint/component/test fixture contracts. Test filesystem pulls are temp fixtures this test creates and controls. | LOW | Source lines 13-24 and 64-104; diff lines 236-246; contract lines 16 and 53-59; source test fixture lines 138-149. |
| PP-004 | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `ExitStatus`, runtime terminal status/signal DTOs, terminal-signal recognizer, supervised output contract | Reads real child status and optional signal, recognizes signal from stdout/stderr/status, and maps exit code/reason through shared helpers | LOW common-interface proof: OEHX contract declares terminal-outcome as translating terminal-signal classification, std process exit status, and supervised output contracts, plus intrinsic ownership for supervised terminal output mapping. OpenCode structured event fixture uses the public structured stream event contract named in the supplied context, not a private DB fallback. | LOW | Source lines 19-73 and 81-134; diff lines 250-278; contract lines 17, 68-73, 116-123; prompt context states OpenCode classification must use public structured stream event contract only and no private DB fallback exists. |
| PP-005 | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `ExitStatus`, Unix signal names, signal-hook forwarding, runtime terminal DTOs, recognizer contract, provider-error evidence prefix | Converts process status to runtime status/reason, checks `provider error: ` evidence prefix for Unknown signals, derives synthetic failure exit code, and dispatches signal recognizer | LOW common-interface proof: file-local declarations and OEHX contract declare this as adapter/intrinsic owner of terminal-signal vocabulary, status/synthetic-exit mapping, evidence construction, reason canonicalization, and provider-error evidence preservation. OS/Unix signal surfaces are declared adapter contracts. | LOW | Source lines 22-44, 46-63, 64-226, 228-386; diff lines 281-347; contract lines 18, 28-31, 60-67, 102-115. |
| PP-006 | `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | Provider `ProcessStatus`, provider `TerminalSignal`, provider `TerminalSignalKind`, runtime terminal DTOs, shared failure-exit/reason helpers | Matches provider status and terminal-signal variants, copies provider signal evidence, maps into `TerminalCancelOutcome` | LOW common-interface proof: provider DTOs are generated in-repo contract surfaces; OEHX contract declares this component translates external-provider process-status, terminal-signal, executor-terminal-signal DTO, and terminal-failure-exit/reason contracts. | LOW | Source lines 3-92 and 94-164; diff lines 348-431; contract lines 19 and 74-80; `crates/oulipoly-provider/src/generated.rs` lines 215-254. |
| PP-007 | `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | External provider protocol request/response/launch event JSON, generated provider-ref config shape, temp fixture files, env vars | Fake provider emits `oulipoly.provider/v1` envelopes/events; tests read recorded JSON files, request fields, env map fields, argv fields, and record paths | LOW source-control/common-interface proof: provider protocol is in-repo owned by `crates/oulipoly-provider` generated DTOs plus schema registry and launch stream validation; fixture files/env are created and controlled by the test. | LOW | Source lines 480-690, 697-735, 738-755, 914-1017, 1156-1422, 1567-1889; diff lines 504-517; `generated.rs` lines 7, 31-46, 215-254; `schemas.rs` lines 310-395; `stream.rs` lines 106-240. |
| PP-008 | `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | External provider terminal classify protocol, launch event JSON, recorded terminal request fixture | Fake provider emits describe/policy/launch/terminal.classify JSON and test reads recorded request `params.stdout_base64` / `params.stderr_base64` | LOW source-control/common-interface proof: terminal classify and launch events are in-repo provider contract/schema surfaces; record file is created by the fixture. | LOW | Source lines 45-119, 196-226, 313-340; diff lines 518-530; `schemas.rs` lines 328-340 and 389-395; `generated.rs` lines 241-254. |
| PP-009 | `src-tauri/tests/s10_external_provider_resume.rs` | Filesystem config layout, external-provider protocol JSON, result-envelope stdout, StateDb `invocations` terminal fields, provider record JSONL | Test materializes config/model/provider fixture, launches real runner binary, reads state DB rows with SQL, parses `OULIPOLY_RESULT=` JSON, and reads fake-provider record JSONL | LOW source-control/common-interface proof: fixture filesystem and fake provider are test-owned; result envelope helper is same-repo test contract; OEHX contract declares `src-tauri/tests/s10_external_provider_resume.rs` translates Unix CLI fixture, Oulipoly result-envelope JSON, StateDb invocation terminal fields, and fake external provider CLI contracts. StateDb and invocation status are in-repo owned. | LOW | Source lines 77-154, 157-222, 236-330, 332-522, 524-703; diff lines 531-747; contract lines 22, 43-45, 93-100; `src-tauri/tests/age153_support/mod.rs` lines 471-510. |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No HIGH pull site found in the touched files/components. | None | None required. | None |

## Residual Ambiguity / Stop-Condition Notes

No `NEEDS_INPUT` or `BLOCKED` condition remains. The only optional helper read miss was a wrong extension lookup for `src-tauri/tests/age153_support.rs`; the actual module path `src-tauri/tests/age153_support/mod.rs` was read before citation. No deployment-level service, database, cache, filesystem, private endpoint, or topology pull site outside the test-owned local fixtures was touched by this WU.

Verdict: LOW

LOW
