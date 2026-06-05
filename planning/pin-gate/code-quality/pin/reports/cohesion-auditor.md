# Cohesion Audit

## Inputs Read

| Input | Path | Notes |
|---|---|---|
| worktree_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Source inspection used this root. |
| repo_root | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar` | Same as worktree. |
| planning_dir | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate` | Planning artifact root. |
| wu_id | `pin` | Report target is the supplied output path. |
| proposal_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md` | Read before scoring; lines 3-6 define the data-dir pin design intent. |
| contract_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` | Read before scoring; lines 3-8 declare component roles and lines 9-26 declare per-file roles. |
| diff_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch` | Read to identify touched production deltas and line anchors. |
| touched_surfaces_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md` | Read to identify the production surface list at lines 5-12. |
| output_path | `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/code-quality/pin/reports/cohesion-auditor.md` | This report. |

## References Read

| Reference | Evidence |
|---|---|
| `~/ai/conventions/code-quality.md` | `## Auditor Scope Boundary` lines 21-27, `## Touched-file ownership` lines 143-149, Phase 6 contract visibility lines 169-173, and A1 row `Cohesion by classifications touched` lines 295-300. |
| `~/ai/conventions/proposer-critic-pattern.md` | Critic independence and non-proposer role read at lines 29-40. |
| `~/ai/conventions/risk-profile.md` | Touched-file ownership cross-reference read at lines 11-16. |
| `~/ai/workflows/implementation-pipeline.md` | Phase 6 process-tree/per-component code-quality context read at lines 500-510 and disposition context at lines 627-631. |
| `planning/pin-gate/proposal.md` | Runtime claims and proof plan read at lines 3-35. |
| `planning/pin-gate/contracts/pin.contract.md` | Component declared roles, per-file declared roles, and function inventory read at lines 3-50. |

## Component Boundaries

| Component | Evidence | Notes |
|---|---|---|
| `crates/oulipoly-state/src/paths.rs` | Touched-surface line 6; diff lines 403-427; contract lines 11 and 32-33. | New canonical data-dir resolution production file. |
| `crates/oulipoly-state/src/db.rs` | Touched-surface line 7; diff lines 376-388; contract lines 12 and 34. | Default state DB path reroute. |
| `crates/oulipoly-state/src/pid_identity.rs` | Touched-surface line 7; diff lines 428-440; contract lines 13 and 35. | Default PID sidecar path reroute. |
| `crates/oulipoly-state/src/lib.rs` | Touched-surface line 7; diff lines 391-400; contract line 14. | Root module exposure for the helper. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | Touched-surface line 8; diff lines 1-24; contract lines 15 and 36-37. | Spawn command formatting plus data-dir env pin. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | Touched-surface line 9; diff lines 26-40; contract line 16 and 38. | PTY control socket fallback reroute. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs` | Touched-surface line 10; diff lines 67-91; contract lines 17 and 39. | Shared quota app-data helper. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` | Touched-surface line 10; diff lines 43-64; contract lines 18 and 40. | Auth-refresh lock directory reroute. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` | Touched-surface line 10; diff lines 92-104; contract lines 19 and 41. | Usage-refresh lock directory reroute. |
| `crates/oulipoly-runtime/src/quota/mod.rs` | Touched-surface line 10; diff lines 163-175; contract line 20. | Public accessor export for the shared helper. |
| `crates/oulipoly-runtime/src/services/lock.rs` | Touched-surface line 11; diff lines 179-195; contract lines 21 and 42. | Service lock directory default reroute. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs` | Touched-surface line 11; diff lines 196-211; contract lines 22 and 43. | Session metadata default state dir reroute. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs` | Touched-surface line 11; diff lines 212-228; contract lines 23 and 44. | Session replacement default data root reroute. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | Touched-surface line 11; diff lines 231-250; contract lines 24 and 45-46. | Sessions state-dir default reroute. |
| `src-tauri/src/usage/fetcher.rs` | Touched-surface line 12; diff lines 665-686; contract line 25 and 47. | Tauri usage lock directory reroute. |
| `src-tauri/src/wiring.rs` | Touched-surface line 12; diff lines 689-700; contract line 26 and 48. | Runtime path bundle data-root reroute. |
| Test/proof files in diff | Touched-surface lines 14-19; diff lines 107-162, 255-371, 443-545, 704-816. | Not scored by this cohesion operator; this pass is the Phase 6 production code-quality cohesion audit, not a test-review pass. |

## Per-Component Cohesion

| Component | Classifications in the touched production delta | Declared role set used | Verdict | blocking_or_residual | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-state/src/paths.rs` | `mapper` | `mapper` | LOW | blocking-owned | `data_dir` and `default_data_dir` map env/platform data roots to the canonical app data dir at `crates/oulipoly-state/src/paths.rs:8-19`; contract declares mapper at `pin.contract.md:11` and inventory at `pin.contract.md:32-33`. |
| `crates/oulipoly-state/src/db.rs` | `mapper` | `mapper` | LOW | blocking-owned | `StateDb::default_path` maps canonical data dir to `state.db` at `crates/oulipoly-state/src/db.rs:1267-1269`; contract declares mapper at `pin.contract.md:12` and inventory at `pin.contract.md:34`. |
| `crates/oulipoly-state/src/pid_identity.rs` | `mapper` | `mapper` | LOW | blocking-owned | `default_path` maps canonical data dir to `SIDECAR_DB_NAME` at `crates/oulipoly-state/src/pid_identity.rs:195-197`; contract declares mapper at `pin.contract.md:13` and inventory at `pin.contract.md:35`. |
| `crates/oulipoly-state/src/lib.rs` | `accessor` | `accessor` | LOW | blocking-owned | The delta exposes `pub mod paths` at `crates/oulipoly-state/src/lib.rs:19`; contract declares accessor at `pin.contract.md:14`. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | `formatter` | `formatter` | LOW | blocking-owned | `command_from_parts` materializes a `Command`, and `pin_agent_data_dir_if_unset` writes the data-dir env shape into that command at `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs:22-60`; file-local role is formatter at lines 1-6 and contract declares formatter at `pin.contract.md:15` with inventory at `pin.contract.md:36-37`. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `mapper` | `mapper` | LOW | blocking-owned | `control_socket_dir` maps runtime/state/canonical data roots to the PTY socket directory at `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:394-404`; contract declares mapper at `pin.contract.md:16` and inventory at `pin.contract.md:38`. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs` | `mapper` | `mapper` | LOW | blocking-owned | `app_data_dir` maps `OULIPOLY_DATA_DIR` or legacy data-home fallback into the app data dir at `crates/oulipoly-runtime/src/quota/lock_paths.rs:35-38`; contract declares mapper at `pin.contract.md:17` and inventory at `pin.contract.md:39`. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` | `mapper` | `mapper` | LOW | blocking-owned | `auth_refresh_lock_dir` maps app data dir to `auth-refresh-locks` at `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs:190-192`; contract declares mapper at `pin.contract.md:18` and inventory at `pin.contract.md:40`. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` | `mapper` | `mapper` | LOW | blocking-owned | `usage_lock_dir` maps app data dir to `usage-refresh-locks` at `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs:96-98`; contract declares mapper at `pin.contract.md:19` and inventory at `pin.contract.md:41`. |
| `crates/oulipoly-runtime/src/quota/mod.rs` | `accessor` | `accessor` | LOW | blocking-owned | The delta re-exports `lock_app_data_dir` at `crates/oulipoly-runtime/src/quota/mod.rs:38-41`; contract declares accessor at `pin.contract.md:20`. |
| `crates/oulipoly-runtime/src/services/lock.rs` | `mapper` | `mapper` | LOW | blocking-owned | `default_lock_dir` maps canonical data dir to `locks` or maps resolution failure to `LockError` at `crates/oulipoly-runtime/src/services/lock.rs:183-189`; contract declares mapper at `pin.contract.md:21` and inventory at `pin.contract.md:42`. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs` | `mapper` | `mapper` | LOW | blocking-owned | `default_state_dir` maps canonical data dir and provider name to `sessions/<provider>` at `crates/oulipoly-runtime/src/session_metadata/locator.rs:663-668`; contract declares mapper at `pin.contract.md:22` and inventory at `pin.contract.md:43`. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs` | `mapper` | `mapper` | LOW | blocking-owned | `default_data_root` maps canonical data-dir resolution into a `ReplaceError` result at `crates/oulipoly-runtime/src/session_replace/mod.rs:1723-1727`; contract declares mapper at `pin.contract.md:23` and inventory at `pin.contract.md:44`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `mapper` | `mapper` | LOW | blocking-owned | `resolve_state_dir` and `default_app_data_dir` map session source/provider inputs to explicit or canonical default state roots at `crates/oulipoly-runtime/src/sessions/mod.rs:389-400`; contract declares mapper at `pin.contract.md:24` and inventory at `pin.contract.md:45-46`. |
| `src-tauri/src/usage/fetcher.rs` | `mapper` | `mapper` | LOW | blocking-owned | `usage_lock_dir` maps runtime quota app data dir to `usage-refresh-locks` at `src-tauri/src/usage/fetcher.rs:165-167`; contract declares mapper at `pin.contract.md:25` and inventory at `pin.contract.md:47`. |
| `src-tauri/src/wiring.rs` | `mapper` | `mapper` | LOW | blocking-owned | `default_cli_runtime_paths` maps config/data roots into `RuntimePaths` at `src-tauri/src/wiring.rs:204-218`; contract declares mapper at `pin.contract.md:26` and inventory at `pin.contract.md:48`. |

## Evidence For Non-LOW Scores

| Score | blocking_or_residual | Touched-file/component ownership proof or residual basis | Evidence | Why it supports the verdict |
|---|---|---|---|---|
| none | none | none | none | No production delta classification exceeded the declared role set. |

## Residual Rows For Context-Only Cohesion Concerns

| id | severity | surface | anchor | evidence | residual basis | why the concern is outside the touched file/component set |
|---|---|---|---|---|---|---|
| none | none | none | none | none | none | No context-only cohesion concerns were identified. |

## Residual Ambiguity / Stop-Condition Notes

| Note | Disposition |
|---|---|
| A1 metric row present | Verified `Cohesion by classifications touched` at `~/ai/conventions/code-quality.md:295-300`; no `BLOCKED:A1-metric-source`. |
| Phase 6 contract readable | `## Component declared roles` is present and parseable at `pin.contract.md:3-8`; no `BLOCKED:unreadable-contract-path` or `BLOCKED:malformed-contract-path`. |
| Invocation scope | Caller requested an incremental Phase 6 per-file cohesion audit over `e8a8e1c..0c8f706`; touched-surfaces records prior surfaces as already gated LOW at `touched-surfaces.md:3`. This report scores the production delta against the supplied per-file declared roles. |
| Test files in diff | Test/proof deltas are not scored here because this operator is not a test-review pass; the production surfaces carrying contract-declared roles are scored above. |

VERDICT: LOW
