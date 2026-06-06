# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate` | S10 gate planning root. |
| wu_id | `s10` | Report identity. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/proposal.md` | Phase-6 proposal read; proof plan and scope claims at lines 1-39. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/contracts/plk.contract.md` | Phase-6 contract read; component declared roles at lines 3-21 and inventories at lines 23-128. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/touched-files.txt` | Nine touched source/test files listed at lines 1-9. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/gates/diff.patch` | Diff read; touched hunks at lines 1-752. |
| runtime_artifact_evidence_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10-gate/evidence/runtime-tests.log` | Runtime evidence read for context; all listed suites ended with rc 0 at lines 822-825 and 827-881. |
| Source files | See touched component below | All nine touched files were read from `worktree_path`. |

## References Read

| Reference | Binding Used |
|---|---|
| `/home/nes/ai/conventions/code-quality.md` | `## Auditor Scope Boundary` lines 21-27, `## Touched-file ownership` lines 143-149, Phase-6 contract visibility lines 169-173, A1 cohesion row lines 295-300. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and non-proposer role, especially lines 29-40. |
| `/home/nes/ai/conventions/risk-profile.md` | Touched-file ownership alignment at lines 11-16. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Phase-6 code-quality fanout and contract requirement at lines 403-491. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| S10 external launch session capture and PLK claim carriers | `plk.contract.md` lines 3-21 declares one component and lists all nine files; `touched-files.txt` lines 1-9 matches the same set; `diff.patch` lines 1-752 touches those files only. | Scored as one multi-file Phase-6 component because the contract supplies a parseable `## Component declared roles` section. The declared role set is `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter`, `validator`, `filter` at `plk.contract.md` line 7. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| S10 external launch session capture and PLK claim carriers | `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter`, `validator`, `filter` | LOW | Blocking scope inspected; no blocking cohesion finding. | Actual classifications are a subset of the component declared role set. Evidence includes external launch mapping/accessors in `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` lines 9-68, capture-method DB mapping in `crates/oulipoly-runtime/src/executor/mod.rs` lines 107-124, external-provider test harness orchestration/parsing/formatting/validation in `crates/oulipoly-runtime/tests/s10_external_launch_session.rs` lines 22-293, setup prompt formatting/mapping/validation in `crates/oulipoly-setup/src/context.rs` lines 104-240, config migration orchestration/predicates/mappers/formatters in `src-tauri/src/commands/config_migration/orchestration.rs` lines 39-186, config migration test accessors/parsers/validators in `src-tauri/src/commands/config_migration/tests.rs` lines 9-605, and source-guard filters/predicates/validators in `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs` lines 1225-1255, `src-tauri/tests/age245_s7c_rotation_source_guard.rs` lines 94-219, and `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs` lines 104-243. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| None | None | None | None | No non-LOW cohesion scores were found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | None | None | None | None | None | No context-only cohesion concerns were identified. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The A1 metric row `Cohesion by classifications touched` is present in `/home/nes/ai/conventions/code-quality.md` lines 295-300. The Phase-6 contract was readable, non-blank, and supplied parseable component-level declared roles, so count-only fallback was not used. The component-level actual classification set does not exceed or fall outside the declared role set.

VERDICT: LOW
