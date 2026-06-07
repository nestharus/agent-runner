# Cohesion Audit

## Inputs Read

| Input | Path / Value |
|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate` |
| wu_id | `oehx` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/touched-files.txt` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/gates/diff.patch` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/code-quality/oehx/reports/cohesion-auditor.md` |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/cohesion-auditor.md` | Operator procedure, output schema, and A1 binding at lines 57-65. |
| `/home/nes/ai/conventions/code-quality.md` | `## Auditor Scope Boundary` lines 21-27; `## Touched-file ownership` lines 143-149; component declared roles lines 161-173; A1 threshold row lines 291-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and proposer/critic separation at lines 29-36. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference at lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 per-component code-quality context at lines 415-416 and 489-491. |
| Proposal | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`, one-owner rule at lines 17-23 and proof plan at lines 29-55. |
| Phase 6a contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`, touched-file declared roles at lines 10-22. |

Metric binding verified: `Cohesion by classifications touched`: LOW = actual classifications are a subset of the declared role set (file-local, path default, or component-level Phase 6a contract), or exactly 1 classification for components and files without any declared roles; MEDIUM = n/a; HIGH = actual classifications exceed the declared role set or include classifications outside the declared role set, or >= 2 classifications for components and files without any declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `touched-files.txt` line 1; `diff.patch` lines 1-81; contract line 14. | Whole file scored. Declared role: `formatter`. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `touched-files.txt` line 2; `diff.patch` lines 82-239; contract line 15; source lines 1-4. | Whole file scored, including embedded tests. Declared roles: `mapper`, `validator`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `touched-files.txt` line 3; `diff.patch` lines 240-271; contract line 16; source lines 1-14. | Whole file scored. Declared roles: `orchestration`, `mapper`, `formatter`, `validator`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt` line 4; `diff.patch` lines 272-315; contract line 17; source lines 1-8. | Whole file scored, including embedded tests. Declared roles: `mapper`, `validator`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt` line 5; `diff.patch` lines 316-382; contract line 18; source lines 1-20. | Whole file scored. Declared roles: `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator`. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `touched-files.txt` line 6; `diff.patch` lines 383-542; contract line 19; source lines 1-4. | Whole file scored, including embedded tests. Declared roles: `mapper`, `validator`. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `touched-files.txt` line 7; `diff.patch` lines 543-571; contract line 20; source lines 3-9. | Whole test file scored. Declared roles: `orchestration`, `mapper`, `formatter`, `parser`, `accessor`, `predicate`, `validator`. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `touched-files.txt` line 8; `diff.patch` lines 572-583; contract line 21; source lines 1-4. | Whole test file scored. Declared roles: `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt` line 9; `diff.patch` lines 584-809; contract line 22; source lines 3-10. | Whole test file scored. Declared roles: `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `formatter` | LOW | blocking | `fixed_reason_for_kind` formats terminal reason strings from `TerminalSignalKind` at source lines 5-17; declared role is `formatter` at source line 1 and contract line 14. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `mapper`, `validator` | LOW | blocking | Mapper functions at source lines 15-77 construct host terminal classification from provider result/status evidence; embedded tests at source lines 79-158 validate exit-code/reason behavior. Both classifications are declared at source lines 1-4 and contract line 15. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration`, `mapper`, `formatter`, `validator` | LOW | blocking | Facade/re-export orchestration at source lines 67-107; test fixture formatting at lines 141-153; fixture model/provider mapping at lines 155-187; assertion validators at lines 215-329. Declared roles cover these at source lines 1-14 and contract line 16. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper`, `validator` | LOW | blocking | Terminal outcome mapping at source lines 30-75; embedded validator tests at lines 77-137. Declared roles cover both at source lines 1-8 and contract line 17. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator` | LOW | blocking | Mapper functions at source lines 66-81 and 126-252; formatter `signal_name` at lines 83-114; predicate `should_forward_interactive_sigterm` at lines 360-363; signal-guard orchestration at lines 273-292 and 365-386; accessor `child_signal_pid` at lines 305-308; validator test at lines 388-408. Declared roles cover all at source lines 1-20 and contract line 18. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `mapper`, `validator` | LOW | blocking | Mapper functions at source lines 22-95 construct host terminal outcome from provider status/signal; embedded tests at lines 97-167 validate mapped exit-code/reason behavior. Declared roles cover both at source lines 1-4 and contract line 19. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `orchestration`, `mapper`, `formatter`, `parser`, `accessor`, `predicate`, `validator` | LOW | blocking | Fixture model/provider mappers at source lines 149-247; script formatter `fake_provider_script_body` at lines 488-699; JSON parsers/accessors at lines 705-732; predicates at lines 1413-1419; validator assertions and orchestration tests throughout, including lines 784-1897. Declared roles cover all at source lines 3-9 and contract line 20. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator` | LOW | blocking | Script/config formatting at source lines 59-123; model/execution mapping at lines 125-184; accessors/parsers at lines 196-210; validators at lines 186-225; orchestration tests at lines 313-340. Declared roles cover all at source lines 1-4 and contract line 21. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`, `filter` | HIGH | blocking | Declared roles at source lines 3-10 and contract line 22 omit `filter`. Actual filter responsibilities are present in `provider_record_lines_with_content` at source lines 323-327 and `records_for_subcommand` at source lines 496-500. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| HIGH | blocking | `src-tauri/tests/s10_external_provider_resume.rs` is a touched file by `touched-files.txt` line 9 and `diff.patch` lines 584-809; `code-quality.md` `## Auditor Scope Boundary` and `## Touched-file ownership` require whole-file scoring. | Source declared roles at lines 3-10 and contract line 22 are `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate`; source lines 323-327 select non-empty record lines, and source lines 496-500 select records whose `subcommand` matches the requested value. | `filter` is an A1 classification for selecting/excluding existing items without changing their shape. The actual classification set therefore exceeds the declared role set, so the A1 cohesion row scores HIGH. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | none | none | none | none | none | No context-only cohesion concerns were scored; the only non-LOW evidence is inside touched-file ownership and is blocking. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6a contract was readable and the touched-file declared-role table plus file-local declared-role headers were parseable. The contract does not need count-only generic fallback for any touched file. Phase 4-only `problem_map_path` and `risk_profile_path` were not supplied; this invocation supplied a Phase 6 contract and proposal, so scoring used the Phase 6 contract and current source evidence.

Closure expectation: the HIGH row must close by making the actual whole-file/component classification set a subset of a valid declared role set, or by decomposing so the touched-file/component ownership target no longer carries an out-of-role classification.

HIGH
