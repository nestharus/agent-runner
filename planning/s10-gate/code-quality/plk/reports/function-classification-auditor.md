# Function Classification Audit

## Inputs Read

| Input | Path / value |
|---|---|
| mode | phase-6 |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` |
| wu_id | `s10` |
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/code-quality/plk/reports/function-classification-auditor.md` |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Read before scoring. A1 single-classification rule: one function must classify as exactly one category; multi-classifier functions fail. Category vocabulary: `orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`. |
| `planning/s10-gate/contracts/plk.contract.md` | Read before scoring. The contract is readable and supplies the Phase-6 component roles plus changed-function inventory. |
| `planning/s10-gate/proposal.md` | Read before scoring. The proposal requires any remaining multi-classifier function to be split before gate closure. |
| `planning/s10-gate/gates/diff.patch` and touched source files | Used to identify added or meaningfully changed S10 functions and verify the current code shape. |
| `planning/s10-gate/evidence/runtime-tests.log` | Read as supporting runtime evidence only; passing tests do not waive A1 classification risk. |

## Scope

Audited added or meaningfully changed functions in the touched S10 surfaces listed in `touched-files.txt`, with emphasis on functions introduced or reshaped by `diff.patch`. I did not waive genuine multi-classifier risk. A previous report at this path listed source-guard test functions as multi-classifier, but the current source has split those inline command/status/counting blocks into named helpers, so those stale rows are not carried forward.

## Findings

| ID | Severity | Path | Function | Mixed categories | Evidence | Remediation |
|---|---|---|---|---|---|---|
| None | LOW | n/a | n/a | n/a | No added or meaningfully changed function in the reviewed S10 surfaces carries more than one primary A1 classification in the current tree. | n/a |

## Reviewed Changed Functions

| Path | Function / symbol | Classification | Result |
|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `map_launch_result_with_terminal_classification` | `mapper` | LOW. Maps provider launch output to `ExecutionResult`; terminal cancellation and launch session capture are delegated. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `launch_session_capture` | `mapper` | LOW. Maps optional provider session id to the corresponding `SessionCaptureResult`. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `launch_provider_session_id` | `accessor` | LOW. Retrieves optional exit-session metadata and delegates JSON field extraction. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `provider_session_id_from_value` | `orchestration` | LOW. Sequences raw-id access and accepted-id filtering through named helpers. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `raw_provider_session_id` | `accessor` | LOW. Reads one JSON string field. |
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `accepted_provider_session_id` | `filter` | LOW. Rejects empty ids and returns the accepted id value. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | `SessionCaptureMethod::ExternalProviderLaunch` and `db_value` arm | `mapper` | LOW. Adds one enum variant and maps it to one DB token. |
| `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs` | `grep_scope_args` | `mapper` | LOW. Maps grep options to pathspec arguments, now including the S10 moveout exclusion. |
| `crates/oulipoly-runtime/tests/s10_external_launch_session.rs` | Fixture methods, record accessors/parsers/filters, request/model mappers, fixture formatter helpers, and the integration test | single-role helpers | LOW. Record reading, JSONL parsing, subcommand filtering, request mapping, fixture script formatting, and validation are separated into named helpers. |
| `crates/oulipoly-setup/src/context.rs` | `build_system_prompt`, `build_cli_setup_prompt`, `capabilities_text`, `moved_provider_binary`, `moved_provider_name`, setup prompt tests | `formatter` or `validator` | LOW. Prompt construction, placeholder replacement, token formatting, and assertions remain single-purpose. |
| `src-tauri/src/commands/config_migration/orchestration.rs` | `migrate_model_config_table` and S10 moved-provider backfill helpers | `orchestration`, `predicate`, `mapper`, or `formatter` per helper | LOW. Eligibility, insertion, moved-provider detection, TOML value materialization, and token formatting are split into named single-role functions. |
| `src-tauri/src/commands/config_migration/tests.rs` | `migrated_model_provider_binary`, moved-provider fixture helpers, `moved_model_path`, and changed regression tests | `accessor`, `mapper`, `formatter`, or `validator` per function | LOW. Added helpers and changed assertions delegate parsing/setup work and keep one primary function role. |
| `src-tauri/tests/age245_s7c_rotation_source_guard.rs` | Source-guard helper splits touched by S10 | `orchestration`, `validator`, `filter`, `accessor`, or `predicate` per helper | LOW. Current code separates diff execution, status validation, added-line matching/counting, stdout line counting, and path-ignore predicates. |
| `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs` | Source-guard helper splits touched by S10 | `orchestration`, `validator`, `filter`, `accessor`, or `predicate` per helper | LOW. Current code separates grep/diff execution, status validation, stdout counting, added-line matching/counting, and path-ignore predicates. |

## Stop-Condition Notes

No `BLOCKED` condition was hit: the code-quality convention, proposal, contract, diff, touched-files list, runtime evidence, and source files were readable. No worktree modifications were made except writing this report.

VERDICT: LOW
