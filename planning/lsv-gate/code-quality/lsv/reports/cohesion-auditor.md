# Cohesion Audit

## Inputs Read

| Input | Path / Value | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Repo identity supplied by caller. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate` | Planning artifact root. |
| `wu_id` | `lsv` | Work Unit identifier. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md` | Read before scoring; proposal lines 12-19 describe incremental launch JSONL parsing and bounded retention. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md` | Read before scoring; contract lines 9-21 declare per-file roles. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/touched-files.txt` | Lines 1-9 enumerate the touched files. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/gates/diff.patch` | Diff headers and hunks confirm the same touched files; examples at lines 1-4, 251-254, 810-813, 1560-1563, 1690-1693, 1893-1896, 1965-1968, 2055-2058, 2132-2135. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/code-quality/lsv/reports/cohesion-auditor.md` | Report destination. |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/cohesion-auditor.md` | Operator role/procedure/output shape at lines 7-122. |
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary at lines 21-27, declared roles at lines 133-141, touched-file ownership at lines 143-149, Phase 6 contract visibility at lines 169-174, and A1 numerical row at lines 295-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no self-critique rules at lines 29-36. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership clause at lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 cohesion/coupling role split at line 416 and per-component code-quality rules at lines 489-491. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/proposal.md` | Proposal context and proof plan, lines 12-19 and 25-56. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/lsv-gate/contracts/lsv.contract.md` | Per-file declared roles, lines 9-21. |

Metric row verified: `Cohesion by classifications touched`: LOW = actual classifications are a subset of the declared role set; for components/files without declared roles, exactly 1 classification. HIGH = actual classifications exceed or include classifications outside the declared role set; for components/files without declared roles, 2 or more classifications. Source: `/home/nes/ai/conventions/code-quality.md` lines 295-300.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-provider/src/client.rs` | `touched-files.txt` line 1; `diff.patch` lines 1-4; contract line 13; file-local roles at `client.rs` lines 1-23. | Touched source file scored as whole file. |
| `crates/oulipoly-provider/src/process.rs` | `touched-files.txt` line 2; `diff.patch` lines 251-254; contract line 14; file-local roles at `process.rs` lines 1-23. | Touched source file scored as whole file. |
| `crates/oulipoly-provider/src/stream.rs` | `touched-files.txt` line 3; `diff.patch` lines 810-813; contract line 15; file-local roles at `stream.rs` lines 1-26. | Touched source file scored as whole file. |
| `crates/oulipoly-provider/src/testkit.rs` | `touched-files.txt` line 4; `diff.patch` lines 1560-1563; contract line 16; file-local roles at `testkit.rs` lines 1-25. | Touched source file scored as whole file. |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | `touched-files.txt` line 5; `diff.patch` lines 1690-1693; contract line 19; file-local roles at `fake_provider.rs` lines 1-23. | Touched fixture component scored as whole file. |
| `crates/oulipoly-provider/tests/launch_stream.rs` | `touched-files.txt` line 6; `diff.patch` lines 1893-1896; contract line 17; file-local roles at `launch_stream.rs` lines 1-16. | Touched test component scored as whole file. |
| `crates/oulipoly-provider/tests/launch_stream_protocol.rs` | `touched-files.txt` line 7; `diff.patch` lines 1965-1968; contract line 18; file-local roles at `launch_stream_protocol.rs` lines 1-15. | Touched test component scored as whole file. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `touched-files.txt` line 8; `diff.patch` lines 2055-2058; contract line 20; file-local roles at `launch_result_mapper.rs` lines 1-14. | Touched runtime mapper file scored as whole file. |
| `src-tauri/tests/s10_external_provider_resume.rs` | `touched-files.txt` line 9; `diff.patch` lines 2132-2135; contract line 21; file-local roles at `s10_external_provider_resume.rs` lines 3-10. | Touched E2E test component scored as whole file. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-provider/src/client.rs` | orchestration, validator, parser, mapper, accessor, predicate | LOW | blocking target, whole touched file | Declared set in contract line 13 and file lines 1-23. Actual examples: orchestration in `ProviderClient::{invoke_typed, invoke_json, launch}` lines 244-310; validator functions lines 416-466 and 608-642; parser functions lines 780-899; mapper functions lines 501-606 and 685-757; accessors lines 226-242 and `ProviderEnv::into_env_vec` lines 139-163; predicates lines 670-675 and 902-943. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/src/process.rs` | orchestration, mapper, predicate, accessor, filter, validator | LOW | blocking target, whole touched file | Declared set in contract line 14 and file lines 1-23. Actual examples: orchestration in `ProcessRunner` lines 442-600; mapper functions lines 603-657, 693-708, and 817-837; predicates lines 248-270, 739-785, 973-980, and 1042-1061; accessors lines 66-74 and 390-418; filters/retention lines 87-195 and 263-270; validator tests lines 1063-1279. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/src/stream.rs` | parser, validator, mapper, accessor, filter, orchestration, formatter | LOW | blocking target, whole touched file | Declared set in contract line 15 and file lines 1-26. Actual examples: parser/orchestration in `LaunchJsonlReader::read`, `LaunchStdoutProcessor`, and `LaunchStreamParser` lines 211-513; validators lines 362-467 and 655-679; accessors lines 90-147 and 193-209; filters lines 515-600; formatters lines 631-640; mappers/error translation lines 602-609 and 681-769. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/src/testkit.rs` | orchestration, formatter, mapper, accessor, predicate, parser, filter, validator | LOW | blocking target, whole touched file | Declared set in contract line 16 and file lines 1-25. Actual examples: orchestration in `FakeProvider` and `LeakProbe` lines 66-122 and 370-407; formatter functions lines 518-540 and 599-629; mappers lines 290-363 and 445-462; accessors/predicates/parser/filter over probe data lines 410-443 and 475-493; validator assertions lines 144-146 and 468-510. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/tests/fixtures/provider_client/fake_provider.rs` | orchestration, accessor, mapper, formatter, parser, predicate, validator | LOW | blocking target, whole touched file | Declared set in contract line 19 and file lines 1-23. Actual examples: orchestration/mode dispatch lines 59-155 and launch fixtures lines 959-1077; accessors/parsers lines 63-65, 167-170, 1084-1099; mappers lines 340-420 and 627-734; formatters lines 383-429 and 1101-1153; predicates lines 341-350, 375-377, 499-553, 890-917; validator unknown-mode handling lines 158-165. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/tests/launch_stream.rs` | validator, orchestration, mapper, accessor, predicate | LOW | blocking target, whole touched file | Declared set in contract line 17 and file lines 1-16. Actual examples: tests orchestrate fake-provider/client launch and assert behavior lines 55-254; `launch_client` maps fixture path/options to client lines 256-263; tests access launch result diagnostics/events and recorded invocation lines 63-87, 140-145, 241-253; predicates use `matches!`, `contains`, and nonzero/truncation checks lines 78-85, 184-189, 207-225. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-provider/tests/launch_stream_protocol.rs` | validator, orchestration, formatter, mapper, accessor | LOW | blocking target, whole touched file | Declared set in contract line 18 and file lines 1-15. Actual examples: JSONL fixture construction/orchestration lines 51-89; bounded-retention/accessor assertions lines 91-98; table-driven mapper cases lines 101-131, 134-164, and 167-211; formatter usage through `format!` and JSON line joins lines 53-60, 73-85, 107-111, 169-203. Actual classifications are a subset of declared roles. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | mapper, accessor, predicate | LOW | blocking target, whole touched file | Declared set in contract line 20 and file lines 1-14. Actual examples: mapper `map_launch_result_with_terminal_classification` lines 50-81, `submitted_user_turn_from_marker_value` lines 89-100, and session-capture mapping lines 110-135; accessors `marker_string` and `raw_provider_session_id` lines 102-142; predicate `provider_session_id_is_present` lines 144-150. Actual classifications are a subset of declared roles. |
| `src-tauri/tests/s10_external_provider_resume.rs` | orchestration, formatter, mapper, accessor, parser, validator, predicate, filter | LOW | blocking target, whole touched file | Declared set in contract line 21 and file lines 3-10. Actual examples: fixture/test orchestration lines 110-188 and 373-468; formatter/materialization lines 190-267 and 584-798; accessors/database readers lines 269-371; validators/assertions lines 470-545; filters/predicates over provider records/subcommands lines 547-582; embedded Python fixture parser/formatter/predicate helpers lines 596-786. Actual classifications are a subset of declared roles. |

## Evidence For Non-LOW Scores

| score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| none | none | none | none | No HIGH component score was found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | none | none | none | none | none | No context-only cohesion concern was identified. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The contract was readable and non-blank. The contract does not contain a `## Component declared roles` heading, but this invocation's touched-surface evidence resolves to per-file components rather than one multi-file WU component; the parseable per-file declared roles in contract lines 9-21 and file-local declarations were therefore used before any count-only fallback. No count-only fallback was needed.

Final verdict: LOW

LOW
