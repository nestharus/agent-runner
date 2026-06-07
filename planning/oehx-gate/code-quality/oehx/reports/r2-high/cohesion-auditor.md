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
| `/home/nes/ai/agents/cohesion-auditor.md` | Operator procedure and output schema; A1 binding at lines 57-65. |
| `/home/nes/ai/conventions/code-quality.md` | `## Auditor Scope Boundary` lines 21-27; `## Touched-file ownership` lines 143-149; component declared roles lines 161-173; A1 row lines 291-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and proposer/critic separation, lines 29-36. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference, lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6/8 code-quality gate and LOW-only disposition context, lines 528-538 and 605-631. |
| Proposal | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/proposal.md`, especially one-owner rule at lines 17-23. |
| Phase 6a contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oehx-gate/contracts/oehx.contract.md`, declared roles at lines 10-22. |

Metric binding verified: `Cohesion by classifications touched`: LOW when actual classifications are a subset of the declared role set, or exactly 1 classification without declared roles; HIGH when actual classifications exceed the declared role set or include classifications outside it, or 2 or more classifications without declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `touched-files.txt` line 1; `diff.patch` lines 1-81; contract line 14. | Whole file scored. Declared role: `formatter`. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `touched-files.txt` line 2; `diff.patch` lines 82-235; contract line 15. | Whole file scored, including embedded tests. Declared role: `mapper`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `touched-files.txt` line 3; `diff.patch` lines 236-249; contract line 16. | Whole file scored by touched-file ownership. File-local declared role and contract role: `orchestration`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `touched-files.txt` line 4; `diff.patch` lines 250-280; contract line 17. | Whole file scored, including embedded tests. Declared role: `mapper`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `touched-files.txt` line 5; `diff.patch` lines 281-347; contract line 18. | Whole file scored. Declared roles: `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator`. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `touched-files.txt` line 6; `diff.patch` lines 348-503; contract line 19. | Whole file scored, including embedded tests. Declared role: `mapper`. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `touched-files.txt` line 7; `diff.patch` lines 504-518; contract line 20. | Whole test file scored. Contract says `TEST`, which is not an A1 declared-role token, and no file-local role header was present. Count-only fallback applies. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `touched-files.txt` line 8; `diff.patch` lines 519-530; file role header lines 1-4; contract line 21. | Whole test file scored. File-local roles: `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt` line 9; `diff.patch` lines 531-747; contract line 22. | Whole test file scored. Component contract roles: `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/diagnostics/external_provider/reason_format.rs` | `formatter` | LOW | blocking | `fixed_reason_for_kind` returns stable terminal-reason strings from `TerminalSignalKind` at lines 5-17; declared role is `formatter` in file line 1 and contract line 14. |
| `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` | `mapper`, `validator` | HIGH | blocking | Declared role is only `mapper` at file line 1 and contract line 15. Embedded test functions added in `diff.patch` lines 156-235 and source lines 109-154 validate exit-code/reason behavior with assertions, adding `validator`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration`, `mapper`, `formatter`, `validator` | HIGH | blocking | Declared role is only `orchestration` at lines 1-11 and contract line 16. Whole file includes test helpers such as `fixture_script` formatting executable scripts at lines 138-150, `age141_model_for_provider` mapping model structs at lines 160-168, and `age141_signal` asserting terminal-signal evidence at lines 212-220. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper`, `validator` | HIGH | blocking | Declared role is only `mapper` at lines 1-7 and contract line 17. Embedded tests at lines 83-134 assert mapped output behavior, adding `validator`. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `mapper`, `formatter`, `predicate`, `orchestration`, `accessor`, `validator` | LOW | blocking | File-local and contract roles cover the observed classifications: mapper functions at lines 66-81, 126-226, formatter `signal_name` at lines 83-114, predicate `should_forward_interactive_sigterm` at lines 361-363, orchestration in `InteractiveSignalGuard::install` and `Drop` at lines 274-292 and 366-372, accessor `child_signal_pid` at lines 305-308, validator test at lines 392-408. |
| `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` | `mapper`, `validator` | HIGH | blocking | Declared role is only `mapper` at file line 1 and contract line 19. Embedded tests at lines 112-163 validate mapped exit-code/reason behavior with assertions, adding `validator`. |
| `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` | `mapper`, `formatter`, `parser`, `accessor`, `predicate`, `validator`, `orchestration` | HIGH | blocking | No A1 declared role set exists: contract line 20 says `TEST`, not an A1 role token. Count-only fallback applies, and the file has 2 or more classifications: e.g. model/provider mappers at lines 141-238, formatter `shell_quote` at lines 433-435, parser `parse_json_value` at lines 705-707, predicate `openai_env_key_is_absent` at lines 1405-1411, validators/assert helpers at lines 738-774 and tests such as lines 1868-1889. |
| `crates/oulipoly-runtime/tests/age242_terminal_classify_external.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator` | LOW | blocking | File-local roles at lines 1-4 cover observed classifications: script/config formatting at lines 59-112 and 121-123, model/execution mapping at lines 125-184, accessors/parsers at lines 196-210, validators at lines 186-225, orchestration tests at lines 313-340. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, `predicate` | HIGH | blocking | Contract line 22 declares `orchestration`, `formatter`, `mapper`, `accessor`, `parser`, `validator`, but not `predicate`. Source has predicate functions `provider_record_line_has_content` at lines 324-326, `provider_subcommands_are_allowed` at lines 514-518, and `provider_subcommand_is_allowed` at lines 520-522. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| HIGH | blocking | `crates/oulipoly-runtime/src/diagnostics/external_provider/result_mapper.rs` is touched by `touched-files.txt` line 2 and `diff.patch` lines 82-235. | Contract line 15 declares only `mapper`; source lines 109-154 are embedded behavior tests with assertions. | Actual classifications include `validator`, outside the declared role set, so A1 scores HIGH. |
| HIGH | blocking | `crates/oulipoly-runtime/src/executor/cli.rs` is touched by `touched-files.txt` line 3 and `diff.patch` lines 236-249. | File-local role header lines 1-11 and contract line 16 declare only `orchestration`; source lines 138-168 construct scripts/models and lines 212-220 assert terminal-signal evidence. | Whole-file ownership includes pre-existing test helpers. Actual classifications exceed `orchestration`, so A1 scores HIGH. |
| HIGH | blocking | `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` is touched by `touched-files.txt` line 4 and `diff.patch` lines 250-280. | File-local role header lines 1-7 and contract line 17 declare only `mapper`; source lines 83-134 validate mapped output with test assertions. | Embedded test validation is outside the declared role set, so A1 scores HIGH. |
| HIGH | blocking | `crates/oulipoly-runtime/src/executor/external_provider/terminal_cancel_mapper.rs` is touched by `touched-files.txt` line 6 and `diff.patch` lines 348-503. | Contract line 19 declares only `mapper`; source lines 112-163 are embedded behavior tests with assertions. | Actual classifications include `validator`, outside the declared role set, so A1 scores HIGH. |
| HIGH | blocking | `crates/oulipoly-runtime/tests/age217_s6a_policy_launch_dispatch.rs` is touched by `touched-files.txt` line 7 and `diff.patch` lines 504-518. | Contract line 20 says `TEST`, which is not an A1 declared-role token. Source shows mapper/formatter/parser/predicate/validator/orchestration evidence at lines 141-238, 433-435, 705-707, 1405-1411, and 1868-1889. | With no valid declared role set, count-only fallback applies; 2 or more classifications scores HIGH. |
| HIGH | blocking | `src-tauri/tests/s10_external_provider_resume.rs` is touched by `touched-files.txt` line 9 and `diff.patch` lines 531-747. | Contract line 22 omits `predicate`; source predicate functions are at lines 324-326, 514-518, and 520-522. | Actual classifications include `predicate` outside the declared role set, so A1 scores HIGH. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | none | none | none | none | none | No context-only cohesion concerns were scored; all non-LOW evidence is inside touched-file ownership and is blocking. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The Phase 6a contract was readable and parseable for declared roles. Phase 4-only `problem_map_path` and `risk_profile_path` were not supplied; this invocation supplied a Phase 6 contract and proposal, so scoring used the Phase 6 declared-role contract path rather than Phase 4-only context.

Closure expectation: every HIGH row must be closed by making the actual whole-file/component classification set a subset of a valid declared role set, or by decomposing the WU so the touched-file/component ownership target no longer carries the out-of-role classifications.

HIGH
