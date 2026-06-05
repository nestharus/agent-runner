# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection rooted here. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate` | Planning artifact root. |
| wu_id | `tsb` | Used for report context. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | Read before scoring; proposal names bounded OpenCode turn scans and Rust hard script deadlines at lines 3-5. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | Read before scoring; parseable `## Component declared roles` at lines 3-17. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt` | Read; lists five touched files at lines 1-5. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch` | Read; diff touches the same five files at lines 1-4, 137-140, 428-431, 444-447, and 1093-1097. |
| problem_map_path | not supplied | Phase 6 invocation; not required for this per-component cohesion pass. |
| risk_profile_path | not supplied | Phase 6 invocation; not required for this per-component cohesion pass. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Path | Binding used |
|---|---|---|
| Code quality convention | `/home/nes/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-25; touched-file ownership lines 143-147; component declared roles lines 161-165; A1 cohesion row lines 295-300. |
| Proposer / critic pattern | `/home/nes/ai/conventions/proposer-critic-pattern.md` | Critic independence and no proposer rerun lines 29-35. |
| Risk profile convention | `/home/nes/ai/conventions/risk-profile.md` | Touched-file/component ownership clause lines 11-16. |
| Implementation pipeline | `/home/nes/ai/workflows/implementation-pipeline.md` | Phase 6 context and pipeline ownership references in lines 132-134 and Phase 4 code-quality gate context at lines 360-371. |
| Proposal | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md` | Public CLI adapter and runtime process-deadline ownership claims at lines 3-5; proof plan lines 7-69. |
| Phase 6a contract | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md` | Component declared role set and scoped files at lines 3-17; production function inventory at lines 19-132. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `turn-scan bounding and script deadline safety` | Contract names the component at `planning/tsb-gate/contracts/tsb.contract.md:5`; declared roles at `planning/tsb-gate/contracts/tsb.contract.md:7`; scoped files at `planning/tsb-gate/contracts/tsb.contract.md:9-17`. `touched-files.txt` lists the same five files at lines 1-5. `diff.patch` touches `crates/oulipoly-runtime/src/quota/process.rs` at lines 1-4, `crates/oulipoly-runtime/src/sessions/mod.rs` at lines 137-140, `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` at lines 428-431, `scripts/opencode-turns` at lines 444-447, and `scripts/tests/opencode-turns.test.sh` at lines 1093-1097. | Phase 6 per-component invocation. Per the supplied operator and `code-quality.md` component-declared-role rule, this audit scores the whole touched component identified by the delta, not only hunk lines. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `turn-scan bounding and script deadline safety` | `orchestration`, `parser`, `mapper`, `filter`, `validator`, `predicate`, `accessor`, `formatter` | LOW | blocking target, no blocking finding | The contract declares the same full role set at `planning/tsb-gate/contracts/tsb.contract.md:7`. `scripts/opencode-turns` shows parser work in env/session/export parsing at lines 89-108, 195-211, 494-509; mapper work at lines 111-158 and 562-568; filter work at lines 169-176, 214-238, and 533-534; predicates at lines 224-225, 435-436, 460-461, and 558-559; accessors at lines 79-86; formatters at lines 604-611; orchestration at lines 330-344, 414-432, 588-601, and 614-629. `crates/oulipoly-runtime/src/sessions/mod.rs` declares its file roles at lines 3-5 and shows parser/predicate/accessor/formatter marker handling at lines 177-204, validation and mapping at lines 234-379, and script execution orchestration/predicate/formatter work at lines 512-696. `crates/oulipoly-runtime/src/quota/process.rs` shows orchestration at lines 36-62 and 117-129, mapper command setup at lines 78-94, validation at lines 201-213, and formatting at lines 227-267. The Rust integration test includes accessors/parsers/mappers/validators/formatters/orchestration in fixture/request/assertion helpers at `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs:203-229`, `823-938`, `1036-1411`. The shell test contains validator assertions and orchestration/mock formatting at `scripts/tests/opencode-turns.test.sh:10-50`, `52-143`, and `145-188`. All observed classifications are within the declared component role set. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| None | n/a | n/a | n/a | No non-LOW cohesion scores were found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | n/a | n/a | No context-only cohesion concerns were identified. |

## Residual Ambiguity / Stop-Condition Notes

| Note | Status | Evidence |
|---|---|---|
| A1 metric row present | Clear | `code-quality.md` includes `Cohesion by classifications touched` at lines 295-300. |
| Contract readability | Clear | `planning/tsb-gate/contracts/tsb.contract.md` is non-empty and has parseable component declared roles at lines 3-17. |
| Component boundary ambiguity | None | Contract, touched-files list, and diff agree on the same five-file component. |
| Scope rule | Applied | `code-quality.md` requires whole touched file/component evaluation at lines 21-25 and touched ownership at lines 143-147. |

VERDICT: LOW
