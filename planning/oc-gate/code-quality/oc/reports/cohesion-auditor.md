# Cohesion Audit.

## Inputs Read

| Input | Path / Value | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same repository identity. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate` | Planning artifact root. |
| `wu_id` | `oc` | Work Unit identifier. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | Read before scoring; OpenCode P0/P1 scope and proof-plan context. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | Read before scoring; parseable `## Component declared roles` and `## Per-file declared roles` sections present. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch` | Read as changed-file and hunk evidence. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md` | Read as canonical touched production surface list. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/cohesion-auditor.md` | This report. |
| `mode` | `phase-6` | Per-component Phase 6 code-quality audit. |

`problem_map_path` and `risk_profile_path` were not supplied. This invocation is Phase 6, where `contract_path` and `proposal_path` are required for this auditor.

## References Read

| Reference | Evidence Used |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Scope boundary `code-quality.md:21-25`; file-local/path/default role resolution and touched-file ownership `code-quality.md:133-147`; Phase 6 contract visibility `code-quality.md:161-173`; A1 row `code-quality.md:295-300`. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer self-critique `proposer-critic-pattern.md:29-35`. |
| `/home/nes/ai/conventions/risk-profile.md` | Evidence requirement and touched-file ownership tie-in `risk-profile.md:11-16`. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 cohesion/coupling role split `implementation-pipeline.md:403-416`; per-component code-quality fanout and contract-read requirement `implementation-pipeline.md:489-491`. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | P0/P1 OpenCode worklist `gap-matrix.md:105-121`; code-change context `gap-matrix.md:131-157`; proof-plan scope `gap-matrix.md:159-169`. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | Component role guidance `oc.contract.md:10-27`; per-file declared-role table `oc.contract.md:28-54`; function inventory `oc.contract.md:58-111`; adapter/intrinsic context `oc.contract.md:113-201`. |

## Metric Binding Verified

`/home/nes/ai/conventions/code-quality.md` still contains the bound A1 row `Cohesion by classifications touched`: LOW when actual classifications are a subset of the declared role set, or exactly one classification with no declared roles; HIGH when actual classifications exceed the declared role set, include classifications outside it, or when files/components without roles have two or more classifications (`code-quality.md:295-300`). MEDIUM is n/a for this row.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| Per touched production file | `touched-surfaces.md:3-26`; contract says this WU spans multiple surfaces and should not be scored as one all-purpose component (`oc.contract.md:10-13`). | Scored each touched production file as its own component. |
| Contract per-file declared-role surfaces | Contract table lists declared roles for all touched production surfaces except `src-tauri/src/run/resume/mod.rs` (`oc.contract.md:28-54`). | Used the contract rows as Phase 6 evidence; where a source file has a file-local role header, that remains a declared role source under `code-quality.md:133-141`. |
| `src-tauri/src/run/resume/mod.rs` | Present in `touched-surfaces.md:24`; diff adds a `validate_resume_input` re-export at `diff.patch:5316-5324`; source is module wiring only `src-tauri/src/run/resume/mod.rs:1-15`. | No contract/file-local role set; no-declared-roles fallback applies. Actual classification is exactly one module-exposure/accessor classification, so LOW. |
| `scripts/opencode-turns` | Present in `touched-surfaces.md:20`; contract per-file row declares `parser`, `accessor`, `mapper`, `formatter`, `filter`, `validator`, `orchestration` at `oc.contract.md:54`; contract also states it is a non-Rust adapter with explicit A6 role declaration `oc.contract.md:56`. | A5 Rust inventory exclusion does not remove A6 scope, and the no-declared-roles fallback does not apply because `oc.contract.md:54` declares roles for this touched file. |
| Tests, reports, `.gitignore`, and `DECISIONS.md` in diff | Diff contains non-production/report/test hunks, including `.gitignore` and `DECISIONS.md` at `diff.patch:1-43`, report hunks at `diff.patch:1875-2555`, and test hunks throughout. | Not in `touched-surfaces.md:3-26`; treated as context only, not blocking cohesion targets. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Declared role set used | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-config/src/model.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate` | Same six roles from file-local header and contract row. | LOW | none | File roles `crates/oulipoly-config/src/model.rs:1-4`; contract `oc.contract.md:32`; changed validator body `crates/oulipoly-config/src/model.rs:239-305`. |
| `crates/oulipoly-config/src/providers.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate`, `filter`, `orchestration` | All eight roles from contract row. | LOW | none | Contract `oc.contract.md:33`; parsing/validation/mapping/orchestration surface `crates/oulipoly-config/src/providers.rs:172-345`; migration/filtering context `crates/oulipoly-config/src/providers.rs:347-380`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | `orchestration` | LOW | none | File roles and facade boundary `crates/oulipoly-runtime/src/executor/cli.rs:1-12`; contract `oc.contract.md:34`; facade exports `crates/oulipoly-runtime/src/executor/cli.rs:64-99`. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `mapper`, `predicate`, `accessor`, `formatter`, `orchestration` | Same five roles. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/capture_result.rs:1-13`; contract `oc.contract.md:35`; mapping and restoration orchestration `crates/oulipoly-runtime/src/executor/cli/capture_result.rs:34-55`, `crates/oulipoly-runtime/src/executor/cli/capture_result.rs:108-180`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `orchestration` | `orchestration` | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs:1-7`; contract `oc.contract.md:36`; launch/supervisor/result sequence `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs:62-107`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `parser`, `mapper`, `predicate`, `accessor`, `filter`, `orchestration` | Same six roles, using contract row for changed recognizer dispatch role. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs:1-17`; contract `oc.contract.md:37`; provider mapping and recognizer dispatch `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs:85-111`; parser/filter/accessor helpers begin `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs:114-130`. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `mapper`, `orchestration` | Same two roles. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/result.rs:1-10`; contract `oc.contract.md:38`; temp cleanup and raw-result mapping `crates/oulipoly-runtime/src/executor/cli/result.rs:36-99`. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs` | `formatter` | `formatter` | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs:1-4`; contract `oc.contract.md:39`; argv formatting functions `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs:18-41`. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `mapper`, `orchestration`, `validator` | Same three roles. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs:1-4`; contract `oc.contract.md:40`; capture-plan orchestration/mapping/validation `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs:40-141`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` | Same three roles. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs:1-9`; contract `oc.contract.md:41`; supervisor config mapping and child lifecycle orchestration `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs:57-79`, `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs:102-232`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper` | `mapper` | LOW | none | File roles `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs:1-7`; contract `oc.contract.md:42`; supervised-output mapping `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs:28-68`. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | `accessor`, `mapper`, `orchestration` | Same three roles. | LOW | none | File roles `crates/oulipoly-runtime/src/executor/mod.rs:1-4`; contract `oc.contract.md:43`; facade accessors/DTOs `crates/oulipoly-runtime/src/executor/mod.rs:16-53`; service orchestration/mapping `crates/oulipoly-runtime/src/executor/mod.rs:152-227`. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs` | `predicate` | `predicate` | LOW | none | File roles `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs:1-4`; contract `oc.contract.md:44`; phrase predicate `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs:30-33`. |
| `crates/oulipoly-runtime/src/executor/providers/mod.rs` | `accessor` | `accessor` | LOW | none | Contract `oc.contract.md:45`; module exposure only `crates/oulipoly-runtime/src/executor/providers/mod.rs:1-4`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `orchestration`, `parser`, `filter`, `formatter`, `validator`, `mapper`, `accessor`, `predicate` | Same eight roles from contract row. | LOW | none | Contract `oc.contract.md:46`; recognizer orchestration `crates/oulipoly-runtime/src/executor/providers/opencode.rs:19-45`; parser/filter/formatter/validator/mapper/accessor/predicate helpers `crates/oulipoly-runtime/src/executor/providers/opencode.rs:48-154`. |
| `crates/oulipoly-runtime/src/executor/terminal_signal.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `validator` | Same five roles. | LOW | none | Declared roles `crates/oulipoly-runtime/src/executor/terminal_signal.rs:18-20`; contract `oc.contract.md:47`; DTO re-export/accessor and helpers `crates/oulipoly-runtime/src/executor/terminal_signal.rs:13-91`. |
| `crates/oulipoly-state/src/db.rs` | `accessor`, `mapper`, `formatter`, `predicate`, `validator`, `parser`, `orchestration`, `filter` | Same eight roles. | LOW | none | File roles `crates/oulipoly-state/src/db.rs:1-12`; contract `oc.contract.md:48`; changed resume-resolution orchestration and validation/predicate helpers `crates/oulipoly-state/src/db.rs:6293-6332`. |
| `scripts/opencode-turns` | `parser`, `accessor`, `mapper`, `formatter`, `filter`, `validator`, `orchestration` | Same seven roles from contract row. | LOW | none | Contract `oc.contract.md:54` and `oc.contract.md:56`; public OpenCode CLI adapter description `scripts/opencode-turns:3-17`; parser/filter/mapper/accessor/formatter/validator/orchestration functions `scripts/opencode-turns:34-65`, `scripts/opencode-turns:78-119`, `scripts/opencode-turns:122-195`, `scripts/opencode-turns:198-255`. |
| `src-tauri/src/error_emit.rs` | `formatter`, `mapper`, `orchestration` | `formatter`, `mapper`, `orchestration` from contract row; file-local header also permits validation but no validator classification is needed for the changed helper. | LOW | none | File roles `src-tauri/src/error_emit.rs:1-4`; contract `oc.contract.md:49`; error disposition orchestration/mapping `src-tauri/src/error_emit.rs:12-29`, `src-tauri/src/error_emit.rs:58-122`; changed formatter `src-tauri/src/error_emit.rs:260-262`. |
| `src-tauri/src/resume_cli.rs` | `orchestration`, `mapper`, `predicate`, `validator`, `formatter`, `accessor` | File-local declared role set; contract row confirms changed formatter/mapper/orchestration/validator surface. | LOW | none | File roles `src-tauri/src/resume_cli.rs:3-22`; contract `oc.contract.md:50`; predicate/accessor/mapper examples `src-tauri/src/resume_cli.rs:96-140`, `src-tauri/src/resume_cli.rs:457-472`; rendering/orchestration examples `src-tauri/src/resume_cli.rs:474-493`. |
| `src-tauri/src/run/repl/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | Same seven roles from contract row. | LOW | none | File roles `src-tauri/src/run/repl/orchestration.rs:1-4`; contract `oc.contract.md:51`; changed early validation orchestration `src-tauri/src/run/repl/orchestration.rs:39-118`; broader mapping/predicate/orchestration context `src-tauri/src/run/repl/orchestration.rs:338-477`. |
| `src-tauri/src/run/resume/mod.rs` | `accessor` | No declared roles; exactly one actual classification fallback. | LOW | none | Touched surface `touched-surfaces.md:24`; diff re-export hunk `diff.patch:5316-5324`; module wiring/re-export `src-tauri/src/run/resume/mod.rs:1-15`. |
| `src-tauri/src/run/resume/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | Same seven roles. | LOW | none | File roles `src-tauri/src/run/resume/orchestration.rs:1-4`; contract `oc.contract.md:52`; changed invalid-input delegation `src-tauri/src/run/resume/orchestration.rs:43-96`; broader resume resolution/formatting/predicate context `src-tauri/src/run/resume/orchestration.rs:652-740`. |
| `src-tauri/src/run/resume/validator.rs` | `validator`, `predicate` | Same two roles from contract row. | LOW | none | Contract `oc.contract.md:53`; validation function and provider-session predicate `src-tauri/src/run/resume/validator.rs:8-25`. |

## Evidence For Non-LOW Scores

| score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No MEDIUM or HIGH cohesion score was found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concern was used to set this verdict. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired: `contract_path`, `proposal_path`, `diff_path`, and `touched_surfaces_path` were readable; the Phase 6 contract contained parseable component and per-file role sections; and A1 still contains the `Cohesion by classifications touched` row.

The diff contains an earlier cohesion report that treated `scripts/opencode-turns` as missing declared roles. Current contract evidence contradicts that: `oc.contract.md:54` supplies an explicit per-file A6 role declaration for the script, and `oc.contract.md:56` explains the Rust A5 inventory exclusion without excluding A6.

VERDICT: LOW
