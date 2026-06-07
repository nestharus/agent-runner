# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Repository identity. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate` | Planning artifact root. |
| `wu_id` | `oeh` | Report work-unit identifier. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | Read before scoring; lines 3-8 describe functional/source range and artifact-only exclusions, lines 11-31 provide proof-plan context. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | Read before scoring; lines 3-7 provide component declared roles, lines 17-25 provide touched-file roles, lines 27-65 provide classification inventory. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/touched-files.txt` | Lines 1-5 enumerate the touched files. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/gates/diff.patch` | Lines 1-433 identify touched diff hunks and changed/new files. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/code-quality/oeh/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Path | Notes |
|---|---|---|
| Operator | `/home/nes/ai/agents/cohesion-auditor.md` | Applied the A1 cohesion-only metric binding at lines 57-65 and output requirements at lines 93-109. |
| Code quality convention | `/home/nes/ai/conventions/code-quality.md` | Verified `Auditor Scope Boundary` lines 21-27, `Touched-file ownership` lines 143-149, component declared roles lines 161-173, and A1 row lines 295-300. |
| Proposer/critic pattern | `/home/nes/ai/conventions/proposer-critic-pattern.md` | Read critic independence and no self-certification constraints. |
| Risk profile convention | `/home/nes/ai/conventions/risk-profile.md` | Read touched-file ownership clause at lines 11-16. |
| Implementation pipeline | `/home/nes/ai/workflows/implementation-pipeline.md` | Read Phase 6 per-component code-quality rules, including cohesion/coupling split and contract visibility references surfaced by grep at lines 416 and 489-491. |
| Proposal artifact | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/proposal.md` | Read as required Phase 6 context. |
| Phase 6a contract artifact | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/oeh-gate/contracts/oeh.contract.md` | Read as required declared-role carrier. |

Metric binding applied exactly: `Cohesion by classifications touched`: LOW = actual classifications are a subset of the declared role set (file-local, path default, or component-level declared roles in a Phase 6a contract), or exactly 1 classification for components and files without any declared roles; MEDIUM = n/a; HIGH = actual classifications exceed the declared role set or include classifications outside the declared role set, or 2 or more classifications for components and files without any declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| OpenCode terminal structured error honesty and supervised exit-zero failure finalization | Contract component name at `planning/oeh-gate/contracts/oeh.contract.md:5`; component declared roles at `planning/oeh-gate/contracts/oeh.contract.md:7`; touched files at `planning/oeh-gate/gates/touched-files.txt:1-5`; diff hunks at `planning/oeh-gate/gates/diff.patch:1-433`. | This is a Phase 6 multi-file WU component. Per `code-quality.md` component-declared-role rule, the component role set is the primary declared role set for cohesion scoring. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| OpenCode terminal structured error honesty and supervised exit-zero failure finalization | `formatter`, `mapper`, `validator`, `predicate`, `orchestration`, `accessor`, `parser`, `filter` | LOW | blocking target, no blocking finding | Declared role set includes `mapper`, `parser`, `predicate`, `formatter`, `filter`, `orchestration`, `accessor`, `validator` at `planning/oeh-gate/contracts/oeh.contract.md:7`. Actual classifications are evidenced by the focused production inventory at `planning/oeh-gate/contracts/oeh.contract.md:31-45`, test inventory at `planning/oeh-gate/contracts/oeh.contract.md:49-65`, file-local declarations in `crates/oulipoly-runtime/src/executor/cli/supervision/terminal_outcome.rs:1-6` and `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs:1-20`, and whole-file inspection of `crates/oulipoly-runtime/src/executor/providers/opencode.rs:19-172` plus `src-tauri/tests/opencode_terminal_error_exit_zero.rs:16-123`. The `.gitignore` artifact-hygiene entry at `.gitignore:45-52` is covered by the touched-file role declaration at `planning/oeh-gate/contracts/oeh.contract.md:21` and does not add a classification outside the component role set. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| None | None | None | None | No HIGH cohesion score was found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | None | None | None | None | None | No context-only cohesion residuals were identified. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The supplied `contract_path` was readable and contained a parseable `## Component declared roles` section, so the Phase 6 component declared role set was used instead of count-only fallback. `problem_map_path` and `risk_profile_path` were not supplied; this was not treated as a blocker because this invocation supplied a Phase 6 contract and requested the Phase 6-style output path under `code-quality/oeh/reports`, while the operator requires those inputs only for Phase 4.

LOW
