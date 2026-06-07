# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Repo identity; source inspection used `worktree_path`. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` | Planning artifact root. |
| wu_id | `oehx` | Work Unit id. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` | Read before scoring; proposal lines 17-23 declare the shared owner/rule movement. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` | Read before scoring; contract lines 10-22 declare touched-file roles. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt` | Lines 1-9 enumerate the touched file set. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` | Diff hunks confirm the same touched file set at lines 1, 82, 240, 272, 316, 383, 543, 571, and 584. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/cohesion-auditor.md` | Operator procedure and output contract, especially lines 57-65 and 77-91. |
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-27, touched-file ownership lines 143-149, Phase 6 contract visibility lines 169-173, and A1 metric row line 299. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer self-certification lines 29-35. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership linkage lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 code-quality fanout and contract-reading requirements lines 403-490. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `touched-files.txt` line 1; `diff.patch` lines 1-81; contract line 14. | File-level component; file-local role header at source line 1. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `touched-files.txt` line 2; `diff.patch` lines 82-239; contract line 15. | File-level component; file-local role header at source lines 1-4. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `touched-files.txt` line 3; `diff.patch` lines 240-271; contract line 16. | File-level facade component; file-local role header at source lines 1-14. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt` line 4; `diff.patch` lines 272-315; contract line 17. | File-level component; file-local role header at source lines 1-8. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt` line 5; `diff.patch` lines 316-382; contract line 18. | File-level intrinsic terminal-signal owner; file-local role header at source lines 1-20. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `touched-files.txt` line 6; `diff.patch` lines 383-542; contract line 19. | File-level component; file-local role header at source lines 1-4. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `touched-files.txt` line 7; `diff.patch` lines 543-570; contract line 20. | Whole test file component; file-local role header at source lines 3-9. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `touched-files.txt` line 8; `diff.patch` lines 571-583; contract line 21. | Whole test file component; file-local role header at source lines 1-4. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt` line 9; `diff.patch` lines 584-809; contract line 22. | Whole test file component; file-local role header at source lines 3-10. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `formatter` | LOW | blocking-scope owned, no finding | Actual classification is only fixed terminal-reason formatting at source lines 5-17; declared role is `formatter` at source line 1 and contract line 14. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `mapper`, `validator` | LOW | blocking-scope owned, no finding | Mapper functions are source lines 15-77; embedded validator tests are source lines 79-158; declared roles match at source lines 1-4 and contract line 15. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration`, `mapper`, `formatter`, `validator` | LOW | blocking-scope owned, no finding | Facade orchestration/re-exports are source lines 67-107; fixture/script/test helper roles are in the embedded test module source lines 115-433; declared roles match at source lines 1-14 and contract line 16. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper`, `validator` | LOW | blocking-scope owned, no finding | Supervised terminal outcome mapping is source lines 30-75; embedded validator tests are source lines 77-137; declared roles match at source lines 1-8 and contract line 17. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator` | LOW | blocking-scope owned, no finding | Mapping is source lines 66-80, 126-252; formatting is source lines 83-114; predicates/accessors/orchestration are source lines 267-386; validator test is source lines 388-409; declared roles match at source lines 1-20 and contract line 18. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `mapper`, `validator` | LOW | blocking-scope owned, no finding | Provider status/signal mapping is source lines 22-95; embedded validator tests are source lines 97-167; declared roles match at source lines 1-4 and contract line 19. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `orchestration`, `mapper`, `formatter`, `parser`, `accessor`, `predicate`, `validator` | LOW | blocking-scope owned, no finding | Whole-file test roles are declared at source lines 3-9 and contract line 20; examples include fixture/model mappers at source lines 189-247, script formatters at source lines 132-147 and 441-699, JSON/line parsers at source lines 705-744, validators/predicates at source lines 746-782 and 1089-1429, and orchestrated tests through source lines 784-1897. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator` | LOW | blocking-scope owned, no finding | Whole-file test roles are declared at source lines 1-4 and contract line 21; examples include fixture script formatting at source lines 59-112, execution/result mapping at source lines 135-184, record access/parsing at source lines 196-210, validators at source lines 186-194 and orchestrated tests at source lines 313-340. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` | LOW | blocking-scope owned, no finding | Whole-file test roles are declared at source lines 3-10 and contract line 22; examples include fixture/config mappers and materializers at source lines 86-243, DB accessors/parsers at source lines 245-339, validators and tests at source lines 341-494, predicate/filter helpers at source lines 496-531, and provider script formatting at source lines 543-712. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| None | n/a | n/a | n/a | No component scored HIGH. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concerns were identified. |

## Residual Ambiguity / Stop-Condition Notes

The bound A1 row is present in `/home/nes/ai/conventions/code-quality.md` line 299: `Cohesion by classifications touched` is LOW when actual classifications are a subset of the declared role set, and HIGH when they exceed it or, without declared roles, reach two or more classifications.

The touched set is resolved as nine file-level components from `touched-files.txt` lines 1-9 and matching `diff.patch` hunks. The Phase 6 contract is readable and carries per-file declared roles at contract lines 10-22; file-local declared roles are also present in the source files. Because file-level declared roles cover the resolved components, no count-only fallback or component-level generic judgment was used.

There are no malformed or unreadable required inputs, no A1 metric-source conflict, and no unresolved boundary question that would materially change the verdict.

LOW
