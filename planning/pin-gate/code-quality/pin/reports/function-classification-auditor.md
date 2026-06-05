# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/code-quality/pin/reports/function-classification-auditor.md`
- `mode=phase-6`
- Caller delta constraint read: score production functions added/changed in `e8a8e1c..0c8f706`; only inlined multi-job bodies are findings.

## References Read

- `/home/nes/ai/conventions/code-quality.md` lines 52-69: A1 single-classification rule and category list.
- `/home/nes/ai/conventions/code-quality.md` lines 21-27 and 143-149: auditor scope boundary and touched-file ownership.
- `/home/nes/ai/conventions/code-quality.md` lines 291-310: `Function categories per function` threshold and `multi-classifier function` failure mode.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` lines 3-27: Phase 6 component and per-file declared roles.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md` lines 28-50: caller-supplied function inventory and no identified multi-classifier risk.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md` lines 1-35: OULIPOLY_DATA_DIR pin purpose and proof context.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch` lines 1-816: delta evidence for touched files and changed functions.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md` lines 1-19: production/test touched-surface summary.

A1 preservation check: present. The metric source contains the category list `orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`; the single-classification rule; the `Function categories per function` row with LOW = 1 and HIGH >= 2; and the `multi-classifier function` failure mode.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-state/src/paths.rs` | `data_dir` | source lines 8-13; diff lines 416-421 | `mapper` | LOW | Maps `OULIPOLY_DATA_DIR` process env to `PathBuf` or falls through to `default_data_dir`; no inline validation, filtering, or formatting responsibility beyond path resolution. |
| `crates/oulipoly-state/src/paths.rs` | `default_data_dir` | source lines 15-19; diff lines 423-427 | `mapper` | LOW | Maps `dirs::data_dir()` to the canonical `oulipoly-agent-runner` app data path, returning the existing resolution error shape on absence. |
| `crates/oulipoly-state/src/db.rs` | `StateDb::default_path` | source lines 1267-1269; diff lines 383-388 | `mapper` | LOW | Maps shared canonical data-dir resolution to `state.db`; body is one path transformation from helper output. |
| `crates/oulipoly-state/src/pid_identity.rs` | `default_path` | source lines 195-197; diff lines 435-439 | `mapper` | LOW | Maps shared canonical data-dir resolution to `pid-identity.db`; no additional domain job. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | `command_from_parts` | source lines 22-51; diff lines 5-12 | `formatter` | LOW | Materializes a `std::process::Command` from command parts, args, working dir, IPC env, and the data-dir pin helper. The added call is helper dispatch inside the same command-shape materialization responsibility. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` | `pin_agent_data_dir_if_unset` | source lines 53-60; diff lines 14-21 | `formatter` | LOW | Writes the canonical data-dir pin into the child `Command` environment only when the parent process lacks `OULIPOLY_DATA_DIR`; the guard supports one spawn-env formatting/materialization job. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` | `control_socket_dir` | source lines 394-404; diff lines 30-39 | `mapper` | LOW | Maps runtime/state/data-dir environment sources into the PTY control socket directory. The changed body reroutes only the data fallback through `oulipoly_state::paths::data_dir()`. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs` | `app_data_dir` | source lines 35-38; diff lines 84-87 | `mapper` | LOW | Maps optional `OULIPOLY_DATA_DIR` or legacy `data_home()/APP_DATA_DIR_NAME` fallback into the app data directory used by quota locks. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` | `auth_refresh_lock_dir` | source lines 190-192; diff lines 59-64 | `mapper` | LOW | Maps shared app-data-dir helper output to the auth-refresh lock directory. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` | `usage_lock_dir` | source lines 96-98; diff lines 99-103 | `mapper` | LOW | Maps shared app-data-dir helper output to the usage-refresh lock directory. |
| `crates/oulipoly-runtime/src/services/lock.rs` | `default_lock_dir` | source lines 183-189; diff lines 186-194 | `mapper` | LOW | Maps canonical data-dir resolution to the service lock directory or the existing operational error shape; no separate inline policy decision is present. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs` | `default_state_dir` | source lines 663-668; diff lines 203-210 | `mapper` | LOW | Maps canonical data-dir resolution and provider name to the default session metadata directory with the existing current-directory fallback shape. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs` | `default_data_root` | source lines 1723-1727; diff lines 219-227 | `mapper` | LOW | Maps canonical data-dir resolution to the session replacement data root result, preserving the operational error on resolution failure. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `resolve_state_dir` | source lines 389-395; diff lines 235-244 | `mapper` | LOW | Maps an explicit session source directory when present, otherwise maps provider name through `default_app_data_dir()/sessions`; the branch is a single path-resolution responsibility. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `default_app_data_dir` | source lines 397-400; diff lines 247-250 | `mapper` | LOW | Maps canonical data-dir resolution to the sessions module fallback app-data root. |
| `src-tauri/src/usage/fetcher.rs` | `usage_lock_dir` | source lines 165-167; diff lines 681-685 | `mapper` | LOW | Maps runtime quota app-data-dir re-export to the usage-refresh lock directory. |
| `src-tauri/src/wiring.rs` | `default_cli_runtime_paths` | source lines 204-219; diff lines 693-700 | `mapper` | LOW | Maps config/data roots into the structured `RuntimePaths` bundle; the changed data-root expression uses the canonical helper with the existing config-root fallback. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| _None_ | _n/a_ | _n/a_ | _n/a_ | No added or changed production function body in the delta performs two or more A1 categories. | _n/a_ | _n/a_ | _n/a_ | _n/a_ |

## Residual Ambiguity / Stop-Condition Notes

- `crates/oulipoly-state/src/lib.rs` and `crates/oulipoly-runtime/src/quota/mod.rs` changed module/re-export carriers only; no added or changed executable production function body was admitted to this delta inventory.
- Test files touched by the diff were excluded from this report's core function inventory under the caller's explicit `production functions ADDED/CHANGED` constraint: `crates/oulipoly-state/tests/data_dir_precedence.rs`, `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs`, `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs`, `src-tauri/tests/wu_b_mailbox_integration.rs`, `src-tauri/tests/wu_d_proactive_wake_integration.rs`, and `src-tauri/tests/wu_e_pty_delivery_integration.rs`.
- Planning/evidence Markdown/log carriers touched by the diff were excluded because they do not define executable production function-like symbols with inspectable bodies for this A5 delta.
- No unresolved boundary ambiguity materially changes the verdict.

Verdict: LOW

VERDICT: LOW
