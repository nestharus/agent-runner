# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| `repo_root` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Supplied repository root. |
| `worktree_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| `planning_dir` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate` | Supplied planning artifact directory. |
| `wu_id` | `s10b` | Supplied Work Unit id. |
| `proposal_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/proposal.md` | Read before scoring; describes the external provider cutover and resume-continuity claims. |
| `contract_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/contracts/s10b.contract.md` | Read before scoring; contains the Phase 6 component declared roles and touched file table. |
| `touched_surfaces_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/touched-files.txt` | Read; authoritative touched source surface list with exactly 20 paths. |
| `diff_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/gates/diff.patch` | Read; authoritative source delta for this invocation. |
| `output_path` | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/s10b-gate/code-quality/s10b/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `/home/nes/ai/agents/cohesion-auditor.md` | Read the operator procedure, non-negotiables, metric binding, stop conditions, and required output format. |
| `/home/nes/ai/conventions/code-quality.md` | Read `## Auditor Scope Boundary`, `## Touched-file ownership`, `## Declared roles`, `### Component declared roles`, `### Phase 6 contract visibility for code-quality auditors`, and `## Numerical thresholds`. |
| `/home/nes/ai/conventions/proposer-critic-pattern.md` | Read critic/proposer separation and acceptance semantics. |
| `/home/nes/ai/conventions/risk-profile.md` | Read the touched-file ownership clause tying risk profiles and code-quality auditors to whole touched files/components. |
| `/home/nes/ai/workflows/implementation-pipeline.md` | Read Phase 6 per-component code-quality rules, especially the per-component code-quality fanout and verdict semantics. |

Metric binding verified: `/home/nes/ai/conventions/code-quality.md` contains `Cohesion by classifications touched`, with LOW when actual classifications are a subset of the declared role set, or exactly 1 classification for components/files without declared roles; HIGH when actual classifications exceed/include classifications outside the declared role set, or 2 or more classifications without declared roles.

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `external provider S10 cutover compatibility and resume continuity` | `planning/s10b-gate/contracts/s10b.contract.md` lines 3-8 name the component and declare `orchestration`, `accessor`, `mapper`, `filter`, `validator`, `predicate`, `formatter`, and `parser`. `planning/s10b-gate/contracts/s10b.contract.md` lines 9-32 list the 20 files in scope. `planning/s10b-gate/gates/touched-files.txt` lines 1-20 list the same 20 source paths. `planning/s10b-gate/gates/diff.patch` touches those source paths and does not make planning gate artifacts part of the audited source component. | Phase 6 multi-file WU component boundary is unambiguous. Per caller instruction and the diff/touched-file artifacts, `planning/s10b-gate/**` gate artifacts are not part of the audited touched source component. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `external provider S10 cutover compatibility and resume continuity` | `orchestration`, `accessor`, `mapper`, `filter`, `validator`, `predicate`, `formatter`, `parser` | LOW | n/a | The component declared role set in `planning/s10b-gate/contracts/s10b.contract.md` lines 3-8 is exactly the eight valid A1 role tokens. Actual classification evidence is source-backed by the contract inventory: production functions in lines 38-195 and test functions/helpers in lines 197-217 classify the touched source work into the same eight roles. Representative source checks match those classifications: `crates/oulipoly-provider/src/error.rs` lines 387-499 contain parser/mapper/accessor/predicate work; `crates/oulipoly-provider/src/generated.rs` lines 68-84 and 705-718 contain parser/validator/formatter work; `crates/oulipoly-runtime/src/session_metadata/mod.rs` lines 538-619 contain orchestration/accessor/validator/mapper/predicate/formatter work; `src-tauri/src/run/resume/orchestration.rs` lines 335-418 and 772-885 contain orchestration/mapper/validator/accessor/predicate/formatter work; `src-tauri/tests/s10_external_provider_resume.rs` lines 140-238, 241-287, 344-375, and 387-535 cover mapper/formatter/parser/orchestration/filter/accessor/predicate/validator test-harness responsibilities. Since the actual classification set is a subset of the component-level declared role set, the A1 cohesion row scores LOW before any count-only fallback. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | touched-file/component ownership proof or residual basis | evidence | why it supports the verdict |
|---|---|---|---|---|
| None | n/a | n/a | No non-LOW cohesion score was assigned. | n/a |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| None | n/a | n/a | n/a | No context-only cohesion concerns were scored. | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

No residual ambiguity or stop condition was encountered. The Phase 6 contract was readable, non-blank, and parseable for component declared roles, and the declared role set covers all observed A1 classifications in the requested multi-file component.

LOW
