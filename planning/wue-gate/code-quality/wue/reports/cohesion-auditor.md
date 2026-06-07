# Cohesion Audit

## Inputs Read

| Input | Path | Evidence |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source files read from this tree. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree path. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate` | Output written under this planning tree. |
| wu_id | `wue` | Used for report identity. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wu-e/proposal.md` | Read lines 1-577. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/contracts/wue.contract.md` | Read lines 1-177, including `## Component declared roles` and `## Per-file declared roles`. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/diff.patch` | Read lines 1-1549; production touch evidence includes `pty_broker.rs` new file at lines 115-120 and module exposure at lines 1-13. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/gates/touched-surfaces.md` | Read lines 1-11. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/wue-gate/code-quality/wue/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Read lines 1-328. A1 scope boundary at lines 21-27, touched-file ownership at lines 143-149, Phase 6 contract visibility at lines 169-173, and A1 `Cohesion by classifications touched` row at lines 295-300. |
| `~/ai/conventions/proposer-critic-pattern.md` | Read lines 1-67. Critic independence and no proposer self-critique are stated at lines 29-35. |
| `~/ai/conventions/risk-profile.md` | Read lines 1-79. Touched-file ownership linkage to code-quality scope is at lines 13-16. |
| `~/ai/workflows/implementation-pipeline.md` | Read lines 1-374 and 403-512. Phase 6 per-component code-quality contract/read requirements and LOW-only semantics are at lines 489-491. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | Touched surface list line 3; contract per-file role line 15; diff lines 1-13. | Production facade/module exposure file. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | Touched surface list line 4; contract per-file role line 16; diff lines 14-114. | Production interactive entrypoint file. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | Touched surface list line 5; contract per-file role line 17; diff lines 115-120. | New Unix PTY broker production file; substantive WU-E surface. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | Touched surface list line 6; contract per-file role line 18; diff lines 1251-1297. | Production spawn identity/runtime sidecar threading file. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | Touched surface list line 7; contract per-file role line 19; diff lines 1298-1394. | Production terminal signal forwarding/mapping file. |
| `crates/oulipoly-state/src/mailbox.rs` | Touched surface list line 8; contract per-file role line 20. | Production mailbox/session-runtime sidecar storage file. |
| `src-tauri/src/commands/notify.rs` | Touched surface list line 9; contract per-file role line 21. | Production notify command and live delivery integration file. |
| `src-tauri/src/mailbox_delivery.rs` | Touched surface list line 10; contract per-file role line 22. | Production mailbox delivery preparation/rendering file. |
| `src-tauri/src/wake_coordinator.rs` | Touched surface list line 11; contract per-file role line 23. | Production wake orchestration and PTY liveness file. |

## Per-Component Cohesion

| Component | Classifications in the touched file/component | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` | LOW | blocking scope, no finding | Contract declares `orchestration` at `planning/wue-gate/contracts/wue.contract.md:15`; source declares facade orchestration at `crates/oulipoly-runtime/src/executor/cli.rs:1-12`; production body is module composition/re-export at `crates/oulipoly-runtime/src/executor/cli.rs:64-101`. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `formatter`, `mapper`, `orchestration`, `validator` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:16`; source declares same roles at `crates/oulipoly-runtime/src/executor/cli/interactive.rs:1-13`; orchestration appears in `execute_interactive_with_result_and_model_identity` at lines 76-115; mapper evidence at lines 130-143 and 193-209; validator/formatter evidence at lines 179-190. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:17`; client and child relay orchestration appears at `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:49-63`, `85-112`, and `554-604`; validator evidence at lines 114-128 and 935-961; parser evidence at lines 144-179 and 867-897; formatter evidence at lines 181-188, 475-498, and 924-932; accessor evidence at lines 235-236, 252-255, and 722-734; mapper evidence at lines 396-406, 423-468, and 613-656; predicate evidence at lines 66-68, 658-660, 752-754, 774-779, and 815-821. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:18`; accessor/mapper evidence at `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs:41-55`; parser/mapper evidence at lines 57-75 and 155-157; orchestration evidence at lines 77-88 and 111-126; formatter evidence at lines 21-28 and 103-109; mapper evidence at lines 90-101 and 128-145. |
| `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:19`; source declares same roles at `crates/oulipoly-runtime/src/executor/cli/terminal_signal.rs:1-20`; mapper evidence at lines 63-78, 118-149, and 184-208; formatter evidence at lines 80-106 and 256-259; accessor evidence at lines 261-264; orchestration evidence at lines 223-249, 273-297, and 321-342; predicate evidence at lines 299-319. |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking scope, no finding | Contract declares all observed roles at `planning/wue-gate/contracts/wue.contract.md:20`; source declares the full role set at `crates/oulipoly-state/src/mailbox.rs:1-4`; accessors and orchestration appear at lines 164-210 and 212-348; filtering/selection appears at lines 247-253 and 815-823; mapper evidence appears at lines 656-668, 671-765, and 1111-1179; validator evidence appears at lines 786-799 and 1031-1036; parser evidence appears at lines 1100-1104; formatter evidence appears at lines 1016-1018 and 1181-1183; predicate evidence appears at lines 801-823, 870-872, and 1093-1108. |
| `src-tauri/src/commands/notify.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:21`; source declares same roles at `src-tauri/src/commands/notify.rs:1-4`; orchestration appears at lines 88-167, 186-237, and 239-331; filter evidence appears at lines 340-346, 560-564, 577-590, and 623-632; parser/validator evidence appears at lines 472-490, 492-543, and 596-614; mapper evidence appears at lines 387-408, 432-454, 687-724, 744-782, 794-829, and 891-914; formatter evidence appears at lines 95-135, 831-889, 916-945; accessor evidence appears at lines 410-412, 726-742, and 943-945. |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:22`; source declares same roles at `src-tauri/src/mailbox_delivery.rs:1-4`; orchestration/accessor evidence appears at lines 22-52, 36-38, 40-52, and 118-130; predicate evidence appears at lines 87-89; mapper evidence appears at lines 91-116 and 147-165; filter/selection evidence appears at lines 167-216; formatter evidence appears at lines 222-264. |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` | LOW | blocking scope, no finding | Contract declares these roles at `planning/wue-gate/contracts/wue.contract.md:23`; source declares same roles at `src-tauri/src/wake_coordinator.rs:1-4`; orchestration/validator evidence appears at lines 63-110, 147-172, 178-214, and 261-291; predicate evidence appears at lines 117-119, 143-145, 224-230, and 331-348; mapper evidence appears at lines 39-61, 293-318, 379-421, 454-483, and 571-572; accessor evidence appears at lines 125-130, 216-222, 320-329, 434-447, 516-520, and 523-555; parser evidence appears at lines 557-568; formatter evidence appears at lines 438-452 and 592-601. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | none | none | none | No HIGH cohesion score was found. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | none | none | none | none | none | No context-only cohesion concern was found. |

## Residual Ambiguity / Stop-Condition Notes

| Note | Disposition |
|---|---|
| A1 metric row was present and matched the bound metric. | No stop condition. Evidence: `~/ai/conventions/code-quality.md:295-300`. |
| Phase 6 contract was readable and contained parseable `## Component declared roles` plus `## Per-file declared roles`. | No stop condition. Evidence: `planning/wue-gate/contracts/wue.contract.md:3-23`. |
| The caller requested per-file scoring against contract per-file declared roles. | Used per-file declared roles from `planning/wue-gate/contracts/wue.contract.md:11-23`, not component-level fallback. |
| The touched-surface artifact is explicitly a production-surface list, and the operator is not for reviewing tests. | Test files and in-file `#[cfg(test)]` fixtures were treated as out of the cohesion verdict target; production touched files are the nine rows in `planning/wue-gate/gates/touched-surfaces.md:3-11`. |

VERDICT: LOW
