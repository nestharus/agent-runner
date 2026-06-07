# Coupling Audit

## Inputs Read

| Input | Path / Value | Status |
|---|---|---|
| mode | phase-6 | read |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | read |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` | read |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` | read |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | read |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | read |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` | read |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/coupling-auditor.md` | written |
| base | `549daaa` | read as context |
| head | `HEAD` / `46181c6` | read as context |
| original_head | `3515d31` | read as context |
| wu_id | `oeh` | read |
| problem_map_path | not supplied | not required for Phase 6 coupling scoring |
| risk_profile_path | not supplied | not required for Phase 6 coupling scoring |
| code_trace_paths | not supplied | not needed; source and diff evidence were sufficient |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Lines 21-27 define `## Auditor Scope Boundary`; lines 143-149 define `## Touched-file ownership`; lines 180-204 define adapter declarations; lines 212-253 define intrinsic-surface declarations; lines 295-300 include A1 row `Coupling by distinct external symbols/modules referenced` with LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6`. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Lines 29-35 require critic independence and prohibit proposer self-critique. |
| `/home/nes/ai/conventions/risk-profile.md` | Lines 13-16 require evidence for non-LOW scores and bind touched-file ownership to code-quality scope. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Lines 403-416 define Phase 6 and the coupling/cohesion split; lines 489-491 require per-component code-quality auditors to read the Step 6a contract and treat only LOW as passing. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | Lines 68-98 carry exact `## Adapter declarations`; lines 100-129 carry exact `## Intrinsic-surface declarations`. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | Lines 3-9 define the OEH functional delta and excluded surfaces; lines 11-31 define the runtime proof plan. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` | Lines 1-461 identify touched files and hunks. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` | Lines 1-5 enumerate the touched surfaces. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/evidence/runtime-tests.log` | Lines 118-119, 176-182, and later CLI integration entries confirm the OEH proof surfaces ran. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `.gitignore` | `touched-files.txt` line 1; `diff.patch` lines 1-9; source `.gitignore` lines 1-52. | File-level touched surface. Artifact-hygiene ignore patterns only; no code imports or module references. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt` line 2; `diff.patch` lines 10-103; source lines 19-77 and tests lines 79-139; contract adapter lines 72-77; contract intrinsic lines 113-119. | File-level declared adapter and declared intrinsic surface. Whole-file ownership includes test module references. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt` line 3; `diff.patch` lines 104-151; source lines 46-63, 154-203, 234-352, and tests lines 355-376; contract adapter lines 78-85; contract intrinsic lines 120-129. | File-level declared adapter and declared intrinsic surface. Carrier now mirrors the local recognizer contract and full intrinsic ownership set. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `touched-files.txt` line 4; `diff.patch` lines 152-328; source lines 8-177 and tests lines 179-292; contract adapter lines 86-91; contract intrinsic lines 103-112. | File-level declared adapter and declared intrinsic surface. Whole-file coupling is concentrated on OpenCode JSON stream parsing and Oulipoly terminal-signal recognizer/evidence DTOs. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `touched-files.txt` line 5; `diff.patch` lines 329-461; source lines 1-127; contract adapter lines 92-98. | File-level declared test adapter. No intrinsic declaration. |

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---:|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `.gitignore` | repository ignore-pattern surface | 0 raw references | none | none | none | 0 | n/a | none | none | none | none | 0 | n/a | LOW | blocking-owned but LOW | Source `.gitignore` lines 1-52 contains ignore patterns only. No source module, symbol, import, or call edge exists. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | terminal signal classification, std process exit status, supervised output | 11 raw references before adapter collapse | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `terminal-signal-classification-contract`; `std-process-exit-status-contract`; `supervised-output-contract` | 3 | LOW: declared adapter bridges `<= 5` contracts and observed references are subordinate | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `supervised_terminal_output_mapping` | synthetic failure exit code when terminal failure coincides with real exit 0; real nonzero exit preservation; supervised terminal reason propagation | 1 | LOW: one declared domain and observed references are subordinate | LOW | blocking-owned but LOW | Source lines 19-26 import `SupervisedOutput`, `SupervisedTerminalOutcome`, `ProviderRecognizer`, terminal-signal helpers and DTOs, and `ExitStatus`; lines 36-77 only map those surfaces. Tests lines 82-99 use `ProviderRecognizer`, `TerminalSignalKind`, `TerminalStatusEvidence`, and `ExitStatusExt` as subordinate fixture references to the same contracts. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | std process exit status, Unix signal naming, signal-hook forwarding, executor terminal-signal DTO, terminal-signal recognizer | 20+ raw references before adapter collapse | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `std-process-exit-status-contract`; `unix-signal-name-contract`; `signal-hook-forwarding-contract`; `executor-terminal-signal-dto-contract`; `terminal-signal-recognizer-contract` | 5 | LOW: declared adapter bridges `<= 5` contracts and observed references are subordinate | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `runtime terminal-signal vocabulary + reason mapping` | full `TerminalSignalKind` vocabulary; terminal status and synthetic exit-code mapping; built-in terminal evidence construction; terminal reason canonicalization hook; provider-error terminal-reason evidence preservation for `Unknown` signals | 1 | LOW: one declared domain and observed references are subordinate to the mirrored `Owns:` set | LOW | blocking-owned but LOW | Source lines 46-57 import `signal_hook`, `std::process`, sync/thread, and `SystemTime`; lines 59-62 import `ProviderRecognizer` and terminal-signal DTOs; lines 66-203 map process status, terminal reason, synthetic exit code, and recognizer evidence; lines 234-352 implement signal forwarding. Contract lines 78-85 declare the five adapter contracts including `terminal-signal-recognizer-contract`; lines 120-129 declare the full intrinsic Owns set covering the terminal vocabulary, status/exit-code mapping, evidence construction, reason canonicalization, and provider-error evidence preservation. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | OpenCode JSON stream event, Oulipoly terminal-signal recognizer, Oulipoly terminal-signal evidence | 10 raw references before adapter collapse | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `OpenCode json stream event contract`; `Oulipoly terminal-signal-recognizer contract`; `Oulipoly terminal-signal evidence contract` | 3 | LOW: declared adapter bridges `<= 5` contracts and observed references are subordinate | `planning/oeh-gate/contracts/oeh.contract.md` | `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `opencode_terminal_signal_recognition` | structured `type:error` terminal event recognition; last non-empty stream line terminality rule; provider-error evidence formatting; structured-event-only rate/quota classification; ordinary output with quota/rate words preserving process-status classification | 1 | LOW: one declared domain and observed references are subordinate | LOW | blocking-owned but LOW | Source lines 8-13 import terminal-signal DTO/helper symbols, `serde_json::Value`, and `Cow`; lines 19-38 implement `TerminalSignalRecognizer`; lines 41-177 parse OpenCode JSON event shape and map to terminal-signal/evidence. The serde JSON field/path references are subordinate to the declared OpenCode JSON stream event contract. |
| `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | Unix CLI integration fixture, Oulipoly result-envelope JSON, StateDb invocation terminal fields | 8 raw references before adapter collapse | `planning/oeh-gate/contracts/oeh.contract.md` | `src-tauri/tests/opencode_terminal_error_exit_zero.rs` | `Unix CLI integration fixture contract`; `Oulipoly result-envelope JSON contract`; `StateDb invocation terminal fields` | 3 | LOW: declared adapter bridges `<= 5` contracts and observed references are subordinate | none | none | none | none | 0 | n/a | LOW | blocking-owned but LOW | Source lines 3-6 import `age153_support::{Age153Fixture, assert_result_envelope_shape}` and `oulipoly_state::InvocationStatus`; lines 23-57 assert result-envelope fields; lines 62-88 use CLI fixture setup/run/resume methods; lines 101-127 query `invocations` fields `status`, `success`, `exit_code`, `terminal_reason`, and `provider_name`. Those references are subordinate to the three declared test-adapter contracts. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | none | All touched file/component rows scored LOW. | No MEDIUM or HIGH per-pair score was found after applying explicit adapter and intrinsic declarations from `contract_path`. | Not applicable. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition was triggered. The Phase 6 contract and proposal were readable, exact `## Adapter declarations` and `## Intrinsic-surface declarations` sections were present in the contract, all declaration entries resolved to touched component boundaries where required, and the A1 coupling metric row was present in `code-quality.md` line 300.

No residual-only coupling findings were used. All assessed surfaces were blocking-owned by the current WU, and each owned surface scored LOW.

VERDICT: LOW
