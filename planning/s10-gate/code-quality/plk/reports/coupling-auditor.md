# Coupling Audit

## Inputs Read

| Input | Path |
|---|---|
| mode | `phase-6` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` |
| wu_id | `s10` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/code-quality/plk/reports/coupling-auditor.md` |

## Rule Binding

Read `/home/nes/ai/conventions/code-quality.md` before scoring. Applied the Phase 6 contract visibility rule, touched-file/component ownership rule, adapter declaration rule, intrinsic-surface declaration rule, and the coupling threshold row: LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6` distinct external symbols/modules unless a valid adapter or intrinsic-surface declaration changes the counted unit.

The contract at `planning/s10-gate/contracts/plk.contract.md` was readable and contained well-formed `## Adapter declarations` and `## Intrinsic-surface declarations` entries for all nine touched S10 surfaces. Each adapter declaration lists at most five translated contracts, and each intrinsic-surface declaration lists one domain. I therefore scored the touched surfaces against those declared boundaries and checked whether the observed references stay subordinate to the declared `Translates:` or `Owns:` entries.

## Findings

No blocking coupling findings.

## Surface Review

| Surface | Coupling Assessment | Evidence |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | LOW. The file is a declared adapter/intrinsic surface for external-provider launch session capture. References to `LaunchResult`, provider exit-session JSON, `ExecutionResult`, `SessionCaptureResult`, `SessionCaptureMethod::ExternalProviderLaunch`, and terminal classification/cancel mapping are subordinate to the five declared translated contracts and the one declared domain. | Diff lines 1-65; source lines 3-76; contract lines 148-155 and 225-233. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | LOW. The S10 change adds one enum variant and one DB-value mapping under the declared executor facade dispatch/capture contracts. The broader facade dependencies are covered by the contract's adapter and intrinsic declarations for executor service request/output, provider registry/client dispatch, CLI/external-provider dispatch branches, terminal signal mapping, and session capture carrier. | Diff lines 66-85; source lines 16-125 and 154-306; contract lines 156-163 and 234-242. |
| `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs` | LOW. The touched code only extends an existing source-guard pathscope with `planning/s10-moveout/**`. The test harness is declared as an adapter/intrinsic surface for export/replace integration and source-guard pathspec exclusions, with five translated contracts and one domain. | Diff lines 86-97; source lines 1225-1255; contract lines 164-171 and 243-250. |
| `crates/oulipoly-runtime/tests/s10_external_launch_session.rs` | LOW. The new integration harness bridges runtime executor requests, provider registry/model config fixture construction, provider launch JSONL records, temporary executable fixture materialization, and session capture/resume assertions. Those are exactly the five declared test-harness translated contracts and the one declared intrinsic domain; the embedded Python fixture does not pull an undeclared production-private surface. | Diff lines 98-411; source lines 1-308; contract lines 172-179 and 251-259. |
| `crates/oulipoly-setup/src/context.rs` | LOW. Setup prompt carrier coupling is confined to placeholder expansion for the moved provider token/binary and generated setup examples. Detection and memory JSON context rendering are included in the contract declaration, so the prompt builder is not reaching outside the declared setup prompt static template, detection report, memory graph, and moved-provider placeholder carrier. | Diff lines 412-499; source lines 1-241; contract lines 212-219 and 297-305. |
| `src-tauri/src/commands/config_migration/orchestration.rs` | LOW. Config migration coupling is within the declared migration adapter/intrinsic surface: model TOML provider arrays and root provider refs, `providers.toml` runtime provider blocks, legacy session-storage migration sequencing, helper module orchestration, and moved-provider binary carrier backfill. No `state.db` schema or provider-launch runtime surface is introduced here. | Diff lines 500-597; source lines 5-539; contract lines 180-187 and 260-269. |
| `src-tauri/src/commands/config_migration/tests.rs` | LOW. Test fixture coupling is declared for config migration API calls, TOML model/provider fixtures, temporary filesystem paths, moved-provider binary assertions, and legacy session-storage/interactive-args regressions. The added helpers read only the migrated root provider binary and construct expected moved-provider fixture paths. | Diff lines 598-727; source lines 1-350; contract lines 188-195 and 270-278. |
| `src-tauri/tests/age245_s7c_rotation_source_guard.rs` | LOW. Source-guard coupling is intentional and declared: git/rg command-output contracts, production source-reader fixtures, provider-name baseline threshold, generated path/planning-gate exclusions, and rotation/config-migration guard scope. The added `planning/s10-moveout/**` exclusion is subordinate to the generated moveout/planning exclusion contract. | Diff lines 728-799; source lines 1-316; contract lines 196-203 and 279-287. |
| `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs` | LOW. Source-guard coupling is intentional and declared: setup flow/setup-brain host source readers, git command-output contracts, provider-name baseline threshold, generated path/planning-gate exclusions, and setup dispatch guard scope. The added `planning/s10-moveout/**` exclusion is subordinate to the generated moveout/planning exclusion contract. | Diff lines 800-865; source lines 1-289; contract lines 204-211 and 288-296. |

## Focus Checks

| Area | Result |
|---|---|
| Imports and module boundaries | LOW. Production imports remain inside runtime executor/external-provider mapping, setup prompt context, and config-migration orchestration boundaries declared in the Phase 6 contract. Test imports are covered by explicit test-harness adapter/intrinsic declarations. |
| Provider-launch session capture | LOW. `exit.session.provider_session_id` is read from provider launch metadata and mapped to `SessionCaptureMethod::ExternalProviderLaunch`; empty IDs are filtered. The mapping does not couple runtime capture to provider-private storage beyond the declared provider launch exit-session JSON contract. |
| Config-migration coupling | LOW. The moved-provider backfill adds a root `provider = { binary = ... }` only when missing and only for moved-provider model entries. It stays in TOML config migration and does not introduce state DB migration or runtime dispatch coupling. |
| Setup prompt carrier coupling | LOW. The prompt uses placeholders for the moved provider token and binary instead of adding new direct concrete-provider vocabulary. The carrier remains a formatter/mapper over the declared setup prompt template and JSON context surfaces. |
| Source-guard path exclusions | LOW. `planning/s10-moveout/**` is repeated in touched source guards, but the contract declares generated moveout/planning exclusions as part of those guard harnesses. This is not an undeclared private-layout pull for this Phase 6 component. |

## Evidence

Runtime evidence in `planning/s10-gate/evidence/runtime-tests.log` records passing S10 and carried PLK tests, including `s10_external_launch_session`, `age244_s7b_export_replace_dispatch`, setup context tests, config migration tests, and the S7c/S8 source guards. This supports that the declared coupling boundaries are exercised, but the coupling verdict is based on source inspection and the Phase 6 contract declarations rather than test success alone.

VERDICT: LOW
