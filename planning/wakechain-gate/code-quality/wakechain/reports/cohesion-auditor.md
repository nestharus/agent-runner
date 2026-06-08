# Cohesion Audit

## Inputs Read

| Input | Path |
|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` |
| repo_root | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate` |
| wu_id | `wakechain` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/touched-files.txt` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/gates/diff.patch` |
| output_path | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/code-quality/wakechain/reports/cohesion-auditor.md` |

## References Read

| Reference | Path |
|---|---|
| Cohesion auditor operator | `/home/nes/ai/agents/cohesion-auditor.md` |
| Code quality convention | `/home/nes/ai/conventions/code-quality.md` |
| Proposer/critic convention | `/home/nes/ai/conventions/proposer-critic-pattern.md` |
| Risk profile convention | `/home/nes/ai/conventions/risk-profile.md` |
| Implementation pipeline workflow | `/home/nes/ai/workflows/implementation-pipeline.md` |
| Phase 6 contract | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/contracts/wakechain.contract.md` |
| Proposal | `/home/nes/projects/agent-runner/worktrees/wake-sweep-robustness/planning/wakechain-gate/proposal.md` |

Metric binding verified from `code-quality.md` line 299: `Cohesion by classifications touched` LOW requires actual classifications to be a subset of declared role set, or exactly one classification when no declared roles exist; HIGH fires when actual classifications exceed declared roles, or when components/files without declared roles have two or more classifications.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `touched-files.txt:1`; `diff.patch:1`; contract `wakechain.contract.md:11` | File-level component from touched-surface enumeration and direct diff hunk. |
| `crates/oulipoly-state/src/db.rs` | `touched-files.txt:2`; `diff.patch:419`; contract `wakechain.contract.md:12` | File-level component from touched-surface enumeration and direct diff hunk. |
| `crates/oulipoly-state/src/mailbox.rs` | `touched-files.txt:3`; `diff.patch:542`; contract `wakechain.contract.md:13` | File-level component from touched-surface enumeration and direct diff hunk. |
| `scripts/opencode-turns` | `touched-files.txt:4`; `diff.patch:997`; contract `wakechain.contract.md:14` | File-level adapter component from touched-surface enumeration and direct diff hunk. |
| `scripts/tests/opencode-turns.test.sh` | `touched-files.txt:5`; contract `wakechain.contract.md:15` | File-level executable test harness component from touched-surface enumeration. |
| `src-tauri/src/dispatch.rs` | `touched-files.txt:6`; contract `wakechain.contract.md:16` | File-level top-level CLI dispatch component. |
| `src-tauri/src/lib.rs` | `touched-files.txt:7`; contract `wakechain.contract.md:17` | Functionless module facade component. |
| `src-tauri/src/mailbox_delivery.rs` | `touched-files.txt:8`; contract `wakechain.contract.md:18` | File-level delivery-preparation component. |
| `src-tauri/src/run/resume/orchestration.rs` | `touched-files.txt:9`; contract `wakechain.contract.md:19` | File-level resume orchestration component. |
| `src-tauri/src/run_tauri.rs` | `touched-files.txt:10`; contract `wakechain.contract.md:20` | File-level Tauri bootstrap component. |
| `src-tauri/src/wake_coordinator.rs` | `touched-files.txt:11`; contract `wakechain.contract.md:21` | File-level wake coordination component. |
| `src-tauri/tests/s11_external_provider_wake.rs` | `touched-files.txt:12`; contract `wakechain.contract.md:22` | File-level integration test fixture component. |
| `src-tauri/tests/wake_confirm_legacy_opencode.rs` | `touched-files.txt:13`; contract `wakechain.contract.md:23` | File-level integration test fixture component. |
| `src-tauri/tests/wu_d_proactive_wake_integration.rs` | `touched-files.txt:14`; contract `wakechain.contract.md:24` | File-level integration test fixture component. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `orchestration`, `accessor`, `filter`, `parser`, `validator`, `mapper`, `formatter`, `predicate` | LOW | blocking target, no finding | File-local roles `mod.rs:3-5`; contract row `wakechain.contract.md:11`; examples include targeted scan orchestration `mod.rs:96-178`, parser/filter/formatter helpers `mod.rs:227-260`, and test helpers `mod.rs:809-1024`. |
| `crates/oulipoly-state/src/db.rs` | `accessor`, `mapper`, `formatter`, `predicate`, `validator`, `parser`, `orchestration`, `filter` | LOW | blocking target, no finding | File-local roles `db.rs:1-12`; contract row `wakechain.contract.md:12`; substring predicate and body parse helpers `db.rs:5850-5927`. |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking target, no finding | File-local roles `mailbox.rs:1-4`; contract row `wakechain.contract.md:13`; sidecar operations and wake selection `mailbox.rs:333-602`, reclaim predicates `mailbox.rs:1085-1156`, schema/parser/mapper helpers `mailbox.rs:1231-1541`. |
| `scripts/opencode-turns` | `orchestration`, `parser`, `mapper`, `filter`, `validator`, `formatter`, `accessor`, `predicate` | LOW | blocking target, no finding | Contract row `wakechain.contract.md:14`; environment access/parsing `scripts/opencode-turns:92-139`, session filtering/parsing `scripts/opencode-turns:236-463`, export mapping/formatting/orchestration `scripts/opencode-turns:592-846`. |
| `scripts/tests/opencode-turns.test.sh` | `orchestration`, `validator`, `formatter`, `parser`, `predicate`, `filter`, `accessor` | LOW | blocking target, no finding | Contract row `wakechain.contract.md:15`; assertions and predicates `opencode-turns.test.sh:15-95`, JSON parser assertions `opencode-turns.test.sh:97-170`, fixture writers and orchestration `opencode-turns.test.sh:278-783`. |
| `src-tauri/src/dispatch.rs` | `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter` | LOW | blocking target, no finding | File-local roles `dispatch.rs:3-5`; contract row `wakechain.contract.md:16`; CLI sequencing and sweep gating `dispatch.rs:83-132`, dispatch mapping `dispatch.rs:171-440`, parser/validator tests `dispatch.rs:480-1106`. |
| `src-tauri/src/lib.rs` | none | LOW | blocking target, no finding | File-local roles `lib.rs:1-3`; contract row `wakechain.contract.md:17`; file contains module declarations and re-exports only `lib.rs:18-48`. |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | LOW | blocking target, no finding | File-local roles `mailbox_delivery.rs:1-3`; contract row `wakechain.contract.md:18`; delivery filtering `mailbox_delivery.rs:75-89`, delivery mapping/orchestration `mailbox_delivery.rs:91-177`, formatting `mailbox_delivery.rs:265-323`. |
| `src-tauri/src/run/resume/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | LOW | blocking target, no finding | File-local roles `orchestration.rs:1-3`; contract row `wakechain.contract.md:19`; resume sequencing `orchestration.rs:47-171`, confirmation predicates and formatting `orchestration.rs:737-1020`, migration mapping/filtering `orchestration.rs:1292-1545`. |
| `src-tauri/src/run_tauri.rs` | `orchestration`, `mapper` | LOW | blocking target, no finding | File-local roles `run_tauri.rs:1-3`; contract row `wakechain.contract.md:20`; app bootstrap orchestration `run_tauri.rs:30-82`, path mapping `run_tauri.rs:84-100`. |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking target, no finding | File-local roles `wake_coordinator.rs:1-4`; contract row `wakechain.contract.md:21`; sweep planning/reaping `wake_coordinator.rs:141-280`, resumability/live-owner predicates `wake_coordinator.rs:283-414`, auto-wake parsing/validation/start orchestration `wake_coordinator.rs:437-1123`. |
| `src-tauri/tests/s11_external_provider_wake.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` | LOW | blocking target, no finding | File-local roles `s11_external_provider_wake.rs:3-11`; contract row `wakechain.contract.md:22`; fixture mapping/accessors `s11_external_provider_wake.rs:61-237`, validation tests `s11_external_provider_wake.rs:239-549`, embedded provider/parser/formatter fixture `s11_external_provider_wake.rs:551-903`. |
| `src-tauri/tests/wake_confirm_legacy_opencode.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` | LOW | blocking target, no finding | File-local roles `wake_confirm_legacy_opencode.rs:3-11`; contract row `wakechain.contract.md:23`; fixture accessors/mappers `wake_confirm_legacy_opencode.rs:65-322`, delivery confirmation tests `wake_confirm_legacy_opencode.rs:324-449`, parsers/formatters/helpers `wake_confirm_legacy_opencode.rs:451-655`. |
| `src-tauri/tests/wu_d_proactive_wake_integration.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` | LOW | blocking target, no finding | File-local roles `wu_d_proactive_wake_integration.rs:3-11`; contract row `wakechain.contract.md:24`; fixture setup/accessors `wu_d_proactive_wake_integration.rs:71-483`, wake/sweep behavior tests `wu_d_proactive_wake_integration.rs:485-1098`, helper mappers/parsers/validators `wu_d_proactive_wake_integration.rs:1100-1453`. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| None | n/a | n/a | n/a | No non-LOW cohesion scores were found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concerns were identified. |

## Residual Ambiguity / Stop-Condition Notes

| Note | Disposition |
|---|---|
| The contract has a `## Declared Roles` table covering every touched file rather than a separate `## Component declared roles` section. | No stop condition. The touched-surface enumeration lists concrete files, so the audit resolved file-level components and used their file-local/contract declared role sets instead of count-only fallback. |
| The proposal and contract keep abandoned-debris reaping in `src-tauri/src/wake_coordinator.rs` planning/reaping and `crates/oulipoly-state/src/mailbox.rs` sidecar mutation. | LOW. The touched OpenCode adapter and resume-confirmation surfaces retain their adapter/confirmation responsibilities and do not own the abandoned-debris policy. |

LOW
