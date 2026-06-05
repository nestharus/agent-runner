# Cohesion Audit

## Inputs Read

| Input | Path / Value | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate` | Planning root. |
| wu_id | `oc` | Report target WU. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | Read before scoring. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | Read before scoring; parseable `## Component declared roles` and `## Per-file declared roles` present. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/diff.patch` | Read as changed-file/hunk evidence. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/gates/touched-surfaces.md` | Read as canonical touched production surface list. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/code-quality/oc/reports/cohesion-auditor.md` | This report. |
| mode | `phase-6` | Per-component Phase 6 code-quality audit. |

## References Read

| Reference | Evidence Used |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-25; touched-file ownership lines 143-147; component declared roles and Phase 6 contract visibility lines 161-173; A1 metric row lines 295-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and non-proposer role lines 29-35. |
| `/home/nes/ai/conventions/risk-profile.md` | Non-LOW evidence requirement and touched-file ownership tie-in lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 cohesion/coupling split lines 403-416; per-component code-quality fanout and contract-read requirement lines 489-491. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/opencode-contract/gap-matrix.md` | OpenCode P0/P1 scope and required capture/resume/terminal/turn-script work lines 105-121 and code-change summary lines 131-157. |
| `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oc-gate/contracts/oc.contract.md` | Component declared roles lines 10-27; per-file declared roles lines 28-52; function inventory lines 54-92; adapter/intrinsic declarations lines 94-170. |

## Metric Binding Verified

`/home/nes/ai/conventions/code-quality.md` still contains the bound A1 row: `Cohesion by classifications touched`, with LOW when actual classifications are a subset of the declared role set, or exactly one classification with no declared roles, and HIGH when classifications exceed the declared role set or when files/components without roles have two or more classifications (`code-quality.md:295-300`). MEDIUM is n/a for this row.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| Per touched production file | `touched-surfaces.md:3-22`; contract says cohesion should be scored per touched file or focused sub-surface, not as one all-purpose component (`oc.contract.md:10-13`). | I scored per file, using contract per-file roles where present. |
| Rust production files | Contract per-file declared roles table lists Rust files and role sets (`oc.contract.md:28-50`). | These are blocking touched-file targets under `code-quality.md:21-25` and `code-quality.md:143-147`. |
| `scripts/opencode-turns` | Touched production surface at `touched-surfaces.md:20`; diff adds file at `diff.patch:1546-1758`; contract names it as a touched non-Rust adapter and excludes it from the Rust A5 inventory (`oc.contract.md:52`, `oc.contract.md:112-123`). | A5 Rust inventory exclusion does not create A6 cohesion declared roles. A1 is language-neutral per `code-quality.md:9-15`, so this touched file is still scored by the no-declared-roles fallback. |
| Tests and `.gitignore` in diff | Diff includes `.gitignore` and test hunks (`diff.patch:1-12`, `diff.patch:777-2180`). | Not in `touched-surfaces.md`; treated as evidence/context, not production cohesion targets. |

## Per-Component Cohesion

| Component | Classifications in touched file/component | Declared role set used | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-config/src/model.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate` | Same six roles from file-local header and contract. | LOW | none | File-local roles `model.rs:1-4`; contract row `oc.contract.md:32`; changed validator inventory `oc.contract.md:60`; changed validation body `model.rs:239-283`. |
| `crates/oulipoly-config/src/providers.rs` | `parser`, `validator`, `mapper`, `formatter`, `accessor`, `predicate`, `filter`, `orchestration` | All eight roles from contract. | LOW | none | Contract row `oc.contract.md:33`; provider loader/validation/mapping functions show parsing, validation, mapping, access, filtering, and orchestration `providers.rs:172-345`, `providers.rs:447-655`; diff only adds tests in this file `diff.patch:48-169`. |
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | `orchestration` | LOW | none | File-local role declaration `cli.rs:1-12`; contract row `oc.contract.md:34`; file is facade/module orchestration and re-exports `cli.rs:64-99`; diff only updates test fixture field `diff.patch:170-181`. |
| `crates/oulipoly-runtime/src/executor/cli/capture_result.rs` | `mapper`, `predicate`, `accessor`, `formatter`, `orchestration` | Same five roles. | LOW | none | File-local declaration `capture_result.rs:1-13`; contract row `oc.contract.md:35`; changed capture mapping and stdout restoration selection `capture_result.rs:34-55`, `capture_result.rs:108-152`; inventory `oc.contract.md:64-66`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `orchestration` | `orchestration` | LOW | none | File-local declaration `provider_execution.rs:1-7`; contract row `oc.contract.md:36`; function sequences launch, supervisor, IPC cleanup, result mapping `provider_execution.rs:62-107`; inventory `oc.contract.md:67`. |
| `crates/oulipoly-runtime/src/executor/cli/provider_identity.rs` | `parser`, `mapper`, `predicate`, `accessor`, `filter`, `orchestration` | Same six roles from contract; file-local header omits `orchestration` but Phase 6 contract per-file row covers changed delegating recognizer dispatch. | LOW | none | File-local roles `provider_identity.rs:1-17`; contract row adds `orchestration` for recognizer dispatch `oc.contract.md:37`; changed dispatch body `provider_identity.rs:85-111`; inventory `oc.contract.md:68-69`. |
| `crates/oulipoly-runtime/src/executor/cli/result.rs` | `mapper`, `orchestration` | Same two roles. | LOW | none | File-local roles `result.rs:1-10`; contract row `oc.contract.md:38`; changed raw-result mapping `result.rs:74-99`; inventory `oc.contract.md:70`. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/args.rs` | `formatter` | `formatter` | LOW | none | File-local roles `args.rs:1-4`; contract row `oc.contract.md:39`; argv formatting functions `args.rs:18-41`; inventory `oc.contract.md:61`. |
| `crates/oulipoly-runtime/src/executor/cli/session_capture/plan.rs` | `mapper`, `orchestration`, `validator` | Same three roles. | LOW | none | File-local roles `plan.rs:1-4`; contract row `oc.contract.md:40`; capture plan mapping/validation `plan.rs:35-122`; inventory `oc.contract.md:62-63`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` | Same three roles. | LOW | none | File-local roles `supervision/mod.rs:1-9`; contract row `oc.contract.md:41`; supervisor orchestration and streamed ID observation `supervision/mod.rs:102-232`; inventory `oc.contract.md:71-73`. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs` | `mapper` | `mapper` | LOW | none | File-local roles `terminal_outcome.rs:1-7`; contract row `oc.contract.md:42`; output mapping `terminal_outcome.rs:28-68`; inventory `oc.contract.md:74`. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | `accessor`, `mapper`, `orchestration` | Same three roles. | LOW | none | File-local roles `executor/mod.rs:1-4`; contract row `oc.contract.md:43`; facade re-exports and executor service mapping/orchestration `executor/mod.rs:16-53`, `executor/mod.rs:152-415`; diff only adds OpenCode recognizer re-export `diff.patch:535-546`. |
| `crates/oulipoly-runtime/src/executor/provider_specific/resume_acceptance.rs` | `predicate` | `predicate` | LOW | none | File-local roles `resume_acceptance.rs:1-4`; contract row `oc.contract.md:44`; phrase predicate body `resume_acceptance.rs:33-41`; inventory `oc.contract.md:75`. |
| `crates/oulipoly-runtime/src/executor/providers/mod.rs` | `accessor` | `accessor` | LOW | none | Contract row `oc.contract.md:45`; module exposure only `providers/mod.rs:1-4`; diff adds module line `diff.patch:576-584`. |
| `crates/oulipoly-runtime/src/executor/providers/opencode.rs` | `orchestration`, `parser`, `accessor`, `predicate` | Same four roles from contract. | LOW | none | Contract row `oc.contract.md:46`; recognizer orchestration `opencode.rs:18-45`; JSON parsing/classification helpers `opencode.rs:47-77`; accessors `opencode.rs:79-107`; predicates `opencode.rs:109-120`; inventory `oc.contract.md:76-85`. |
| `crates/oulipoly-runtime/src/executor/terminal_signal.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `validator` | Same five roles. | LOW | none | Declared roles comment `terminal_signal.rs:18-20`; contract row `oc.contract.md:47`; DTO/helper functions `terminal_signal.rs:13-91`; diff only adds test `diff.patch:761-776`. |
| `crates/oulipoly-state/src/db.rs` | `accessor`, `mapper`, `formatter`, `predicate`, `validator`, `parser`, `orchestration`, `filter` | All eight roles. | LOW | none | File-local roles `db.rs:1-12`; contract row `oc.contract.md:48`; changed resume orchestration delegates to DB sub-steps `db.rs:6290-6309`; inventory `oc.contract.md:86`. |
| `src-tauri/src/run/resume/orchestration.rs` | `orchestration`, `validator`, `accessor`, `mapper`, `filter`, `predicate`, `formatter` | Same seven roles. | LOW | none | File-local roles `resume/orchestration.rs:1-4`; contract row `oc.contract.md:49`; changed validator delegation `resume/orchestration.rs:43-96`; inventory `oc.contract.md:87`. |
| `src-tauri/src/run/resume/validator.rs` | `validator` | `validator` | LOW | none | Contract row `oc.contract.md:50`; validator body accepts or rejects input `resume/validator.rs:1-8`; inventory `oc.contract.md:88`. |
| `scripts/opencode-turns` | `parser`, `accessor`, `mapper`, `formatter`, `filter`, `validator`, `orchestration` | No cohesion declared roles found for this touched non-Rust file in the Phase 6 contract; no file-local `## Declared roles` section. | HIGH | blocking | Touched production surface `touched-surfaces.md:20`; no per-file declared role row in `oc.contract.md:28-52`; actual parser/accessor/mapper/formatter/filter/validator/orchestration evidence in `scripts/opencode-turns:37-56`, `scripts/opencode-turns:59-82`, `scripts/opencode-turns:85-97`, `scripts/opencode-turns:99-164`, `scripts/opencode-turns:167-203`. |

## Evidence For Non-LOW Scores

| score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| HIGH | blocking | `scripts/opencode-turns` is explicitly in the touched production surface list (`touched-surfaces.md:20`) and added by the diff (`diff.patch:1546-1758`), so whole-file ownership applies under `code-quality.md:21-25` and `code-quality.md:143-147`. | `scripts/opencode-turns` has no per-file declared roles in contract rows `oc.contract.md:28-52` and no file-local `## Declared roles`; the contract only states it is a non-Rust adapter excluded from Rust A5 inventory (`oc.contract.md:52`, `oc.contract.md:112-123`). Actual classifications include parser/accessor/mapper/formatter/filter/validator/orchestration: recursive content extraction `scripts/opencode-turns:37-56`, path/root discovery `scripts/opencode-turns:59-82`, cursor read/write `scripts/opencode-turns:85-97`, timestamp/session/role/record mapping `scripts/opencode-turns:99-164`, JSON file parse plus output formatting `scripts/opencode-turns:167-178`, and main argv validation/filter/orchestration `scripts/opencode-turns:181-203`. | A1 says files/components without declared roles are LOW only at exactly one classification and HIGH at two or more classifications (`code-quality.md:295-300`). This touched file has multiple actual classifications and no declared role set, so the fallback is HIGH. The Rust A5 inventory exclusion does not remove A6 scope because the code-quality convention is language-neutral (`code-quality.md:9-15`) and touched-file ownership applies to touched files/components. Closure expectation: rerun this A6 audit against current evidence after the touched non-Rust adapter has an applicable cohesion role declaration or is decomposed/scoped by a valid Phase 6 contract surface. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concern was used to set this verdict. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired: `contract_path`, `proposal_path`, `diff_path`, and `touched_surfaces_path` were readable; the Phase 6 contract contained parseable component/per-file role sections for the Rust files; and A1 still contains the `Cohesion by classifications touched` row.

The only ambiguity is intentional and scored rather than blocked: `scripts/opencode-turns` is declared a non-Rust adapter and excluded from the Rust A5 function inventory (`oc.contract.md:52`, `oc.contract.md:123`), but no A6 cohesion declared role set is supplied for it. Under A1, that means the no-declared-roles fallback applies to this touched file.

VERDICT: HIGH
