# Coupling Audit

## Inputs Read

| Input | Path |
|---|---|
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate` |
| `wu_id` | `s11` |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/proposal.md` |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/contracts/s11.contract.md` |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/touched-files.txt` |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/gates/diff.patch` |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s11-gate/code-quality/s11/reports/coupling-auditor.md` |

Audited net source range: `95699d6..549daaa`.

## References Read

| Reference | Purpose |
|---|---|
| `/home/nes/ai/agents/coupling-auditor.md` | Operator, stop conditions, output format, declaration handling |
| `/home/nes/ai/conventions/code-quality.md` | `## Auditor Scope Boundary`, `## Touched-file ownership`, adapter/intrinsic rules, A1 row |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposal revision |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-surface ownership cross-reference |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 contract visibility and gate context |
| `planning/s11-gate/contracts/s11.contract.md` | Exact `## Adapter declarations` and `## Intrinsic-surface declarations` carrier |
| `planning/s11-gate/proposal.md` | Proposal context and proof claims |

Metric binding verified in `/home/nes/ai/conventions/code-quality.md` line 300: `Coupling by distinct external symbols/modules referenced` = LOW `0-2`, MEDIUM `3-5`, HIGH `>= 6`.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `DECISIONS.md` | touched-files.txt line 1; contract lines 391–400 | Declared intrinsic surface; `project_decision_log_evidence` domain. |
| `crates/oulipoly-provider/src/client.rs` | touched-files.txt line 2; contract lines 114–118 | Declared adapter; 2 `Translates:` contracts. |
| `crates/oulipoly-provider/src/error.rs` | touched-files.txt line 3; contract lines 119–123 | Declared adapter; 2 contracts. |
| `crates/oulipoly-provider/src/generated.rs` | touched-files.txt line 4; contract lines 124–128 | Declared adapter; 2 contracts. |
| `crates/oulipoly-provider/src/process.rs` | touched-files.txt line 5; contract lines 129–133 | Declared adapter; 2 contracts. |
| `crates/oulipoly-provider/src/testkit.rs` | touched-files.txt line 6; contract lines 134–139 | Declared adapter; 3 contracts. |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | touched-files.txt line 7; contract lines 140–145 | Declared adapter; 3 contracts. |
| `crates/oulipoly-provider/tests/launch_stream_lifecycle.rs` | touched-files.txt line 8; contract lines 146–150 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | touched-files.txt line 9; contract lines 103–108 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | touched-files.txt line 10; contract lines 109–113 | Declared adapter; 2 contracts. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | touched-files.txt line 11; contract lines 408–414 | Declared intrinsic surface; `external_provider_process_identity` domain. |
| `crates/oulipoly-runtime/src/executor/external_provider/context.rs` | touched-files.txt line 12; contract lines 156–161 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/dispatch.rs` | touched-files.txt line 13; contract lines 167–173 | Declared adapter; 4 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/error_formatter.rs` | touched-files.txt line 14; contract lines 174–179 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/error_mapper.rs` | touched-files.txt line 15; contract lines 180–186 | Declared adapter; 4 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` | touched-files.txt line 16; source inspected directly | Non-declared raw component; no adapter or intrinsic-surface entry in contract. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | touched-files.txt line 17; contract lines 162–166 | Declared adapter; 2 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/policy_transform.rs` | touched-files.txt line 18; contract lines 187–191 | Declared adapter; 2 contracts. |
| `crates/oulipoly-runtime/src/executor/external_provider/request_builder.rs` | touched-files.txt line 19; contract lines 151–155 | Declared adapter; 2 contracts. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | touched-files.txt line 20; contract lines 192–197 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/provider_registry/client_factory.rs` | touched-files.txt line 21; contract lines 198–203 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/src/provider_settings/mod.rs` | touched-files.txt line 22; contract lines 204–209 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | touched-files.txt line 23; contract lines 210–215 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/tests/age246_external_transport_rotation.rs` | touched-files.txt line 24; contract lines 216–222 | Declared adapter; 4 contracts. |
| `crates/oulipoly-runtime/tests/provider_registry.rs` | touched-files.txt line 25; contract lines 223–228 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/tests/provider_settings_host.rs` | touched-files.txt line 26; contract lines 229–234 | Declared adapter; 3 contracts. |
| `crates/oulipoly-runtime/usage-refresh-locks/age222-marker-a.lock` | touched-files.txt line 27; source confirmed empty | Static fixture artifact; no executable code or symbol references. |
| `crates/oulipoly-state/src/db.rs` | touched-files.txt line 28; contract lines 415–423 | Declared intrinsic surface; `state_db_repository_surface` domain. |
| `crates/oulipoly-state/src/mailbox.rs` | touched-files.txt line 29; contract lines 401–407 | Declared intrinsic surface; `sidecar_mailbox_delivery_state` domain. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/cohesion-auditor.log` | touched-files.txt line 30; caller context | Historical `.scratch` log artifact; not product code; no executable symbol references. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.log` | touched-files.txt line 31; caller context | Historical `.scratch` log artifact; not product code; no executable symbol references. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.rerun2.log` | touched-files.txt line 32; caller context | Historical `.scratch` log artifact; not product code; no executable symbol references. |
| `scripts/opencode-turns` | touched-files.txt line 33; contract lines 235–241 | Declared adapter; 4 contracts. |
| `scripts/tests/opencode-turns.test.sh` | touched-files.txt line 34; contract lines 242–247 | Declared adapter; 3 contracts. |
| `src-tauri/Cargo.toml` | touched-files.txt line 35; contract lines 424–430 | Declared intrinsic surface; `tauri_crate_manifest` domain. |
| `src-tauri/src/commands/direct_model.rs` | touched-files.txt line 36; contract lines 248–253 | Declared adapter; 3 contracts. |
| `src-tauri/src/commands/provider_settings.rs` | touched-files.txt line 37; contract lines 254–259 | Declared adapter; 3 contracts. |
| `src-tauri/src/mailbox_delivery.rs` | touched-files.txt line 38; contract lines 260–265 | Declared adapter; 3 contracts. |
| `src-tauri/src/migration_providers.rs` | touched-files.txt line 39; contract lines 266–271 | Declared adapter; 3 contracts. |
| `src-tauri/src/resume_cli.rs` | touched-files.txt line 40; contract lines 272–277 | Declared adapter; 3 contracts. |
| `src-tauri/src/run/balancing/accessor.rs` | touched-files.txt line 41; contract lines 278–283 | Declared adapter; 3 contracts. |
| `src-tauri/src/run/balancing/diagnostics_tests.rs` | touched-files.txt line 42; contract lines 284–289 | Declared adapter; 3 contracts. |
| `src-tauri/src/run/balancing/finalization.rs` | touched-files.txt line 43; contract lines 290–296 | Declared adapter; 4 contracts. |
| `src-tauri/src/run/balancing/mapper.rs` | touched-files.txt line 44; contract lines 297–302 | Declared adapter; 3 contracts. |
| `src-tauri/src/run/balancing/orchestration.rs` | touched-files.txt line 45; contract lines 303–309 | Declared adapter; 4 contracts. |
| `src-tauri/src/run/resume/disposition.rs` | touched-files.txt line 46; contract lines 310–315 | Declared adapter; 3 contracts. |
| `src-tauri/src/run/resume/orchestration.rs` | touched-files.txt line 47; contract lines 316–323 | Declared adapter; 5 contracts — highest in set, still `<= 5`. |
| `src-tauri/src/session_ingest_cli.rs` | touched-files.txt line 48; contract lines 324–329 | Declared adapter; 3 contracts. |
| `src-tauri/src/terminal_outcome_adapter.rs` | touched-files.txt line 49; contract lines 330–335 | Declared adapter; 3 contracts. |
| `src-tauri/src/wake_coordinator.rs` | touched-files.txt line 50; contract lines 336–342 | Declared adapter; 4 contracts. |
| `src-tauri/tests/age100_resume_quota_migration.rs` | touched-files.txt line 51; contract lines 343–348 | Declared adapter; 3 contracts. |
| `src-tauri/tests/age166_zero_turn_classifier.rs` | touched-files.txt line 52; contract lines 349–354 | Declared adapter; 3 contracts. |
| `src-tauri/tests/age166_zero_turn_orchestration_e2e.rs` | touched-files.txt line 53; contract lines 355–360 | Declared adapter; 3 contracts. |
| `src-tauri/tests/age240_relocated_support.rs` | touched-files.txt line 54; contract lines 361–366 | Declared adapter; 3 contracts. |
| `src-tauri/tests/s10_external_provider_resume.rs` | touched-files.txt line 55; contract lines 367–372 | Declared adapter; 3 contracts. |
| `src-tauri/tests/s11_external_provider_wake.rs` | touched-files.txt line 56; contract lines 373–378 | Declared adapter; 3 contracts. |
| `src-tauri/tests/structural_segmentation.rs` | touched-files.txt line 57; contract lines 379–384 | Declared adapter; 3 contracts. |
| `src-tauri/tests/wu_b_mailbox_integration.rs` | touched-files.txt line 58; contract lines 385–391 | Declared adapter; 3 contracts. |

## Per-Pair Coupling

| source component | target component | distinct external symbols/modules referenced | adapter declaration artifact path | declared adapter component | `Translates:` contracts | contract count | adapter verdict | intrinsic declaration artifact path | declared intrinsic component | `Domain:` | `Owns:` set or summary | domain count | intrinsic-surface verdict | final verdict | blocking_or_residual | evidence |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 48 declared adapter components (see Component Boundaries) | Declared translated contract surfaces | Raw import counts exceed 2 in many files; adapter scoring counts named contracts, not field references | `planning/s11-gate/contracts/s11.contract.md` lines 99–392 | 48 components listed under `adapter_declarations:` in contract | 2–5 contracts per component; highest is `src-tauri/src/run/resume/orchestration.rs` with 5 | 2–5 | LOW | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | All 48 adapter entries have `role: adapter` and a non-empty `Translates:` list. Every contract count is `<= 5`. Source responsibilities align with provider protocol, runtime executor, sidecar identity, Tauri IPC, CLI, test fixture, and shell harness contracts named in contract lines 99–392. Spot-checked: `dispatch.rs` (4 contracts) reaches `oulipoly_provider::*` and `oulipoly_state::pid_identity::ProcessIdentity` — all subordinate to declared Translates surfaces; `wake_coordinator.rs` (4 contracts) reaches `oulipoly_state::mailbox::*` and `oulipoly_runtime::executor::cli::pty_broker` — subordinate; `resume/orchestration.rs` (5 contracts) reaches `oulipoly_runtime::*` and `sha2::Sha256` for delivery confirmation — subordinate to declared `mailbox delivery confirmation and retry outcomes` surface. |
| `DECISIONS.md` | `project_decision_log_evidence` domain | 0 code-external references; prose-only Markdown | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` lines 391–400 | `DECISIONS.md` | `project_decision_log_evidence` | Decision-log ledger; historical project decision entries; S11/S10B gate entries; validation/proposal/contract/evidence-log references; live smoke references; audited source range, mode-remediation, revert, and transport-rotation rationale | 1 | LOW | LOW | blocking | Markdown prose file; no executable coupling. One declared domain; all internal decision references are subordinate to the declared `Owns:` ledger set. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `external_provider_process_identity` domain | Raw process/session identity imports from `oulipoly_state`; all subordinate to declared Owns | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` lines 408–414 | `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `external_provider_process_identity` | Provider child PID identity observation; sidecar owner/session backfill; launch-time sidecar identity validation | 1 | LOW | LOW | blocking | Source lines 10–14: imports `oulipoly_state::CompositeInvocationId`, `oulipoly_state::mailbox::{MailboxDb, SessionRuntimeRunningUpdate}`, `oulipoly_state::pid_identity::{self, LiveProcessIdentityRecord, PidIdentityDb, ProcessIdentity}`. All are subordinate to the three declared Owns entries covering PID observation, sidecar backfill, and launch-time validation. One declared domain. |
| `crates/oulipoly-state/src/db.rs` | `state_db_repository_surface` domain | Raw serde/uuid/path/time/rusqlite/chrono refs within StateDb scope; explicitly covered by Owns set | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` lines 415–423 | `crates/oulipoly-state/src/db.rs` | `state_db_repository_surface` | StateDb connection/migration/schema/repository accessors; invocation/session/quota/lifecycle/mailbox persistence helpers; `session_turns` ingest and body lookup; exact user text match predicate; serde/uuid/path/time/result/transaction value mapping used by StateDb operations | 1 | LOW | LOW | blocking | Contract Owns set explicitly names utility-crate mapping (serde, uuid, path, time, result, transaction) as subordinate. Proposal lines 67–71 confirm S11 only reads existing `session_turns` body data with no schema migration. One declared domain. |
| `crates/oulipoly-state/src/mailbox.rs` | `sidecar_mailbox_delivery_state` domain | Raw chrono/rusqlite/serde refs used for mailbox row timestamps, SQLite storage, and serialization; all subordinate to Owns | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` lines 401–407 | `crates/oulipoly-state/src/mailbox.rs` | `sidecar_mailbox_delivery_state` | Pending/delivered/failed rows; delivery_attempts and delivery_error updates; wake claims and sidecar session runtime metadata | 1 | LOW | LOW | blocking | Source lines 10–15: imports `chrono`, `rusqlite`, `serde::Serialize`, `std::path`, `crate::pid_identity`. `chrono` timestamps are subordinate to `pending/delivered/failed rows` (enqueued_at, delivered_at). `rusqlite` is subordinate to the sidecar SQLite backing store. `serde::Serialize` is subordinate to `MailboxRow` delivery-state representation. One declared domain. |
| `src-tauri/Cargo.toml` | `tauri_crate_manifest` domain | No executable references; manifest TOML only | n/a | n/a | n/a | n/a | n/a | `planning/s11-gate/contracts/s11.contract.md` lines 424–430 | `src-tauri/Cargo.toml` | `tauri_crate_manifest` | Package dependency declarations; workspace crate dependency declarations; Tauri crate dev-dependency and test-target declarations | 1 | LOW | LOW | blocking | TOML manifest; no executable coupling surface. One declared domain; all manifest entries subordinate to declared Owns dependency/test-target set. |
| `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` | `oulipoly_provider::generated` | 1 raw distinct external symbol: `oulipoly_provider::generated::Diagnostic` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Source lines 3–13: only external import is `use oulipoly_provider::generated::Diagnostic`. Used as `Vec<Diagnostic>` in `PolicyRejected` variant constructor. All other types (String, &'static str, Vec, Debug, Clone) are Rust prelude/std. Raw count = 1 = LOW (0–2 threshold). |
| `crates/oulipoly-runtime/usage-refresh-locks/age222-marker-a.lock` | none | 0 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | File is empty (confirmed source read); no symbols or modules referenced. |
| `planning/s10b-gate/.scratch/code-quality/s10b/logs/cohesion-auditor.log`, `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.log`, `planning/s10b-gate/.scratch/code-quality/s10b/logs/coupling-auditor.rerun2.log` | historical artifact content | 0 product-code references | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Caller context confirms these are historical `.scratch` log artifacts, not product code; no executable dependency edges; audit treats serialized log text as artifact content only. |

## Evidence For Non-LOW Scores

| score | blocking_or_residual | ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | No MEDIUM or HIGH component-pair scores found. | All declared adapter components have 2–5 contracts (`<= 5`). All intrinsic-surface components have 1 declared domain (`<= 5`). The one undeclared code component (`errors.rs`) has 1 raw external reference. Non-code artifacts have 0. Overall verdict remains LOW. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. `contract_path` was readable before scoring and `## Adapter declarations` and `## Intrinsic-surface declarations` sections were present and well-formed. The A1 metric row `Coupling by distinct external symbols/modules referenced` (LOW `0-2`; MEDIUM `3-5`; HIGH `>= 6`) is confirmed in `conventions/code-quality.md` line 300.

Declaration validation:
- 48 adapter entries: each has `component`, `role: adapter`, and a non-empty `Translates:` list with 2–5 entries. All 48 component names resolve to files listed in touched-files.txt. No entry is malformed.
- 5 intrinsic-surface entries: `DECISIONS.md`, `crates/oulipoly-state/src/mailbox.rs`, `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs`, `crates/oulipoly-state/src/db.rs`, `src-tauri/Cargo.toml`. Each has `component`, `role: intrinsic-surface`, exactly one `Domain:`, and a non-empty `Owns:` list. All resolve to touched-files.txt entries. No entry is malformed.
- Undeclared components: `crates/oulipoly-runtime/src/executor/external_provider/errors.rs` (1 raw external ref = LOW); `crates/oulipoly-runtime/usage-refresh-locks/age222-marker-a.lock` (empty file = LOW); three historical `.scratch` log artifacts (no code refs = LOW).

Note: `dispatch.rs` source contains an inline doc-comment YAML block claiming `intrinsic_surface_declarations`. This embedded declaration is not the contract carrier. Per operator procedure Step 7, declarations are loaded from the exact `## Adapter declarations` and `## Intrinsic-surface declarations` sections of `contract_path`. The contract declares `dispatch.rs` as an adapter (4 contracts), which is the operative declaration used for scoring.

LOW
