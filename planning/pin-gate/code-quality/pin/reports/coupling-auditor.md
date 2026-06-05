# Coupling Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection root. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree root. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate` | Planning artifact root. |
| wu_id | `pin` | Report slug context. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md` | Read before scoring; proposal lines 3-5 define the pin/fallback behavior. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` | Read before scoring; adapter declarations at lines 52-67 and intrinsic declaration at lines 71-84. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch` | Incremental delta evidence over `e8a8e1c..0c8f706`. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md` | Production touched surfaces at lines 5-12; tests listed as context at lines 14-19. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/code-quality/pin/reports/coupling-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | Auditor scope boundary lines 21-27; touched-file ownership lines 143-149; adapter declarations lines 180-210; intrinsic-surface declarations lines 212-253; A1 coupling metric row line 300. |
| `~/ai/conventions/proposer-critic-pattern.md` | Critic independence and non-proposer behavior lines 29-40. |
| `~/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference lines 11-16. |
| `~/ai/workflows/implementation-pipeline.md` | Phase 6 per-component contract/coupling rules lines 403-416 and 489-491. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-state/src/paths.rs` | Touched-surface line 6; new file diff lines 403-427; current source lines 1-19. | Declared adapter in contract lines 55-61 and declared intrinsic surface in contract lines 74-83. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | Touched-surface line 8; diff lines 1-25; current source lines 22-60. | Declared adapter in contract lines 62-67. |
| `crates/oulipoly-state/src/db.rs` | Touched-surface line 7; diff lines 376-388; current source lines 1267-1269. | Reroute-only default path consumer. |
| `crates/oulipoly-state/src/pid_identity.rs` | Touched-surface line 7; diff lines 428-440; current source lines 195-197. | Reroute-only default path consumer. |
| `crates/oulipoly-state/src/lib.rs` | Touched-surface line 7; diff lines 391-400. | Module exposure for `paths`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | Touched-surface line 9; diff lines 26-40; grep evidence current line 401. | Reroute-only helper consumer; existing `XDG_STATE_HOME` branch unchanged. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs` | Touched-surface line 10; diff lines 67-91; current source lines 35-38. | Adds internal app-data helper using `oulipoly_state::paths` constants. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` | Touched-surface line 10; diff lines 43-64; grep evidence current line 191. | Reroute-only consumer of `app_data_dir`. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` | Touched-surface line 10; diff lines 92-104; grep evidence current line 97. | Reroute-only consumer of `app_data_dir`. |
| `crates/oulipoly-runtime/src/quota/mod.rs` | Touched-surface line 10; diff lines 163-175; grep evidence current line 39. | Re-export of shared helper. |
| `crates/oulipoly-runtime/src/services/lock.rs` | Touched-surface line 11; diff lines 179-195; current source lines 183-189. | Reroute-only default lock dir consumer. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs` | Touched-surface line 11; diff lines 196-211; grep evidence current lines 664-665. | Reroute-only default state dir consumer. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs` | Touched-surface line 11; diff lines 212-228; grep evidence current line 1724. | Reroute-only default data root consumer. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | Touched-surface line 11; diff lines 231-250; grep evidence current lines 393 and 397-399. | Reroute-only default session dir consumer. |
| `src-tauri/src/usage/fetcher.rs` | Touched-surface line 12; diff lines 665-686; current source lines 165-167. | Reroute-only consumer through runtime quota re-export. |
| `src-tauri/src/wiring.rs` | Touched-surface line 12; diff lines 689-700; current source lines 204-218. | Reroute-only default runtime paths consumer. |

Test files listed in `touched-surfaces.md` lines 14-19 were treated as proof/context for this production coupling pass, not as separate production component boundaries.

## Per-Pair Coupling

| Source component | Target component | Distinct external symbols/modules referenced | Adapter declaration artifact path | Declared adapter component | `Translates:` contracts | Contract count | Adapter verdict | Intrinsic declaration artifact path | Declared intrinsic component | `Domain:` | `Owns:` set or summary | Domain count | Intrinsic-surface verdict | Final verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|---|---:|---|---|---|---|---|---:|---|---|---|---|
| `crates/oulipoly-state/src/paths.rs` | Data-dir env/platform/app-data contracts | 4 raw references: `std::env::var_os`, `PathBuf::from`, `dirs::data_dir`, `APP_DATA_DIR_NAME`; scored by declared adapter/intrinsic rules | `planning/pin-gate/contracts/pin.contract.md` lines 55-61 | `crates/oulipoly-state/src/paths.rs` | `OULIPOLY_DATA_DIR`; `dirs::data_dir`; `oulipoly-agent-runner` | 3 | LOW | `planning/pin-gate/contracts/pin.contract.md` lines 74-83 | `crates/oulipoly-state/src/paths.rs` | `agent_runner_data_dir_resolution` | `DATA_DIR_ENV`, `APP_DATA_DIR_NAME`, data-dir precedence, `default_data_dir`, canonical error message | 1 | LOW | LOW | blocking | Current source lines 5-18 show all references are subordinate to the declared env/platform/app-data domain. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | Spawn command env + data-dir pin contracts | 5 raw references in the delta/whole command materializer: `Command`, `cmd.env`, `cmd.env_remove`, `std::env::var_os`, `oulipoly_state::paths::{DATA_DIR_ENV,data_dir}`; scored by declared adapter rule | `planning/pin-gate/contracts/pin.contract.md` lines 62-67 | `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | `std::process::Command` child env inheritance/writes; `oulipoly_state::paths::DATA_DIR_ENV` and `data_dir` pin contract | 2 | LOW | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 9 and 14-20 add the pin helper; current source lines 40-48 and 53-59 show env writes and data-dir references subordinate to the declared spawn-env and pin contracts. |
| `crates/oulipoly-state/src/db.rs` | `crate::paths` | 1: `crate::paths::data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 383-388 and current source lines 1267-1269 replace local default derivation with one helper call. |
| `crates/oulipoly-state/src/pid_identity.rs` | `crate::paths` | 1: `crate::paths::data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 435-440 and current source lines 195-197 replace local default derivation with one helper call. |
| `crates/oulipoly-state/src/lib.rs` | `paths` module | 1: `pub mod paths` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 391-400 expose the new module. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `oulipoly_state::paths` | 1: `oulipoly_state::paths::data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 30-39 replace `dirs::data_dir().join("oulipoly-agent-runner")` with one canonical helper call. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs` | `oulipoly_state::paths` | 2: `DATA_DIR_ENV`, `APP_DATA_DIR_NAME` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 84-87 and current source lines 35-38 add `app_data_dir` with exactly two external path constants. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` | `quota::lock_paths` | 1: `app_data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 51-64 reroute the lock dir through `app_data_dir`. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` | `quota::lock_paths` | 1: `app_data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 99-104 reroute the usage lock dir through `app_data_dir`. |
| `crates/oulipoly-runtime/src/quota/mod.rs` | `quota::lock_paths` | 1: `app_data_dir as lock_app_data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 171-175 add one re-export. |
| `crates/oulipoly-runtime/src/services/lock.rs` | `oulipoly_state::paths` | 1: `data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 186-194 and current source lines 183-189 use one canonical data-dir helper. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs` | `oulipoly_state::paths` | 2: `data_dir`, `APP_DATA_DIR_NAME` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 203-210 and grep evidence current lines 664-665 show one helper plus one fallback constant. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs` | `oulipoly_state::paths` | 1: `data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 219-227 and grep evidence current line 1724 use one canonical helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `oulipoly_state::paths` | 2: `data_dir`, `APP_DATA_DIR_NAME` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 243-250 and grep evidence current lines 397-399 show one helper plus one fallback constant. |
| `src-tauri/src/usage/fetcher.rs` | `oulipoly_runtime::quota` | 1: `lock_app_data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 681-686 and current source lines 165-167 use one runtime quota re-export. |
| `src-tauri/src/wiring.rs` | `oulipoly_state::paths` | 1: `data_dir` | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | LOW | blocking | Diff lines 693-700 and current source lines 204-210 use one canonical helper. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | n/a | n/a | n/a | No MEDIUM or HIGH pair was found. |

## Residual Ambiguity / Stop-Condition Notes

No stop condition fired. The contract was readable and non-empty; adapter declarations and intrinsic-surface declarations were well formed and resolved to touched component boundaries. The A1 metric row `Coupling by distinct external symbols/modules referenced` is present in `~/ai/conventions/code-quality.md` line 300.

The audit followed the requested incremental gate focus. Test-only files in the diff were not scored as production coupling surfaces; they were context for the proof and touched-surface enumeration.

Final verdict line: LOW

VERDICT: LOW
