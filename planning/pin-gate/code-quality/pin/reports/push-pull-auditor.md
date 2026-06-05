# Push/Pull Coupling Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/contracts/pin.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/gates/touched-surfaces.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/pin-gate/code-quality/pin/reports/push-pull-auditor.md`
- `mode=phase-6`

## References Read

- `/home/nes/ai/conventions/code-quality.md:21-27` for auditor scope boundary.
- `/home/nes/ai/conventions/code-quality.md:106-131` for Push-vs-pull system coupling and `uncontrolled-source coupler` binding.
- `/home/nes/ai/conventions/code-quality.md:143-149` for touched-file ownership.
- `/home/nes/ai/conventions/code-quality.md:169-173` for Phase 6 contract visibility.
- `/home/nes/ai/conventions/code-quality.md:291-310` for numerical thresholds and failure modes.
- `/home/nes/ai/conventions/agent-questions-and-session-graph.md:230-242` for terminology disambiguation only; this audit is system-coupling A4, not session-graph context transfer.
- `planning/pin-gate/proposal.md:3-6` for the intended pin/fallback behavior.
- `planning/pin-gate/proposal.md:17-35` for spawn-env and shadow-XDG runtime claims.
- `planning/pin-gate/contracts/pin.contract.md:52-69` for declared adapter/common-interface context.
- `planning/pin-gate/contracts/pin.contract.md:71-84` for the declared `paths.rs` intrinsic owner of data-dir resolution.

A1 preservation verified: the Push-vs-pull system coupling section exists, the session-graph Pull-vs-Push Policy disambiguator exists, the `uncontrolled-source coupler` failure mode exists, and the numerical thresholds section exists.

## Pull Sites Inspected

| ID | Puller | Source | Pull mechanism | Ownership/interface evidence | Verdict | Evidence |
|---|---|---|---|---|---|---|
| PP-001 | `crates/oulipoly-state/src/paths.rs::data_dir` | Process data-dir environment plus platform data directory fallback | Reads `OULIPOLY_DATA_DIR`; otherwise maps `dirs::data_dir()` to `oulipoly-agent-runner` | LOW common-interface/source-control proof: Phase 6 contract declares `paths.rs` as the adapter for process env, platform user-data, and canonical app-data contracts, and intrinsic owner of `DATA_DIR_ENV`, `APP_DATA_DIR_NAME`, precedence, fallback, and error message | LOW | `crates/oulipoly-state/src/paths.rs:5-18`; `planning/pin-gate/contracts/pin.contract.md:56-61`; `planning/pin-gate/contracts/pin.contract.md:75-83` |
| PP-002 | `crates/oulipoly-state/src/db.rs::StateDb::default_path` | Canonical data-dir helper | Calls `crate::paths::data_dir()` and appends `state.db` | LOW common-interface proof: consumer pulls from owner helper, not raw env or platform path layout | LOW | `crates/oulipoly-state/src/db.rs:1258-1269`; `planning/pin-gate/contracts/pin.contract.md:32-35` |
| PP-003 | `crates/oulipoly-state/src/pid_identity.rs::default_path` | Canonical data-dir helper | Calls `crate::paths::data_dir()` and appends `SIDECAR_DB_NAME` | LOW common-interface proof: consumer pulls from owner helper, not raw env or platform path layout | LOW | `crates/oulipoly-state/src/pid_identity.rs:195-197`; `planning/pin-gate/contracts/pin.contract.md:34-35` |
| PP-004 | `crates/oulipoly-state/src/lib.rs` | State crate public module surface | Exposes `pub mod paths` | LOW source-control proof: same crate publishes the declared helper as the common interface | LOW | `crates/oulipoly-state/src/lib.rs:17-20`; `planning/pin-gate/contracts/pin.contract.md:11-15` |
| PP-005 | `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs::pin_agent_data_dir_if_unset` | Provider child process environment | Checks whether parent `OULIPOLY_DATA_DIR` is set; otherwise resolves `oulipoly_state::paths::data_dir()` and pushes it into `Command.env` | LOW common-interface proof: contract declares this file as the adapter translating `std::process::Command` env writes and the agent-runner canonical data-dir pin contract; the delta pushes the owner-resolved value into the child env rather than making descendants re-derive it | LOW | `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs:40-59`; `planning/pin-gate/contracts/pin.contract.md:62-67`; `planning/pin-gate/proposal.md:17-19` |
| PP-006 | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_dir` | Runtime/state env overrides and canonical data-dir helper fallback | Reads `XDG_RUNTIME_DIR`/`XDG_STATE_HOME` for PTY runtime placement, otherwise calls `oulipoly_state::paths::data_dir()` | LOW common-interface proof for the delta: fallback data-dir resolution is pulled from the shared helper; no duplicated `OULIPOLY_DATA_DIR` or `dirs::data_dir()` derivation remains in this consumer | LOW | `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs:394-403`; `planning/pin-gate/gates/diff.patch:30-39` |
| PP-007 | `crates/oulipoly-runtime/src/quota/lock_paths.rs::app_data_dir` | Quota lock app-data root | Reads the shared `DATA_DIR_ENV` constant; otherwise maps the quota `data_home()` compatibility root to the shared `APP_DATA_DIR_NAME` | LOW source-control/common-interface proof: this file is the quota lock-root helper and uses constants from the `paths.rs` owner for the new pin/name surface; downstream quota consumers pull from `app_data_dir()`/`lock_app_data_dir()` instead of re-deriving the new env precedence | LOW | `crates/oulipoly-runtime/src/quota/lock_paths.rs:25-38`; `crates/oulipoly-runtime/src/quota/lock_paths.rs:6-15`; `planning/pin-gate/contracts/pin.contract.md:39-40`; `planning/pin-gate/contracts/pin.contract.md:69` |
| PP-008 | `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs::auth_refresh_lock_dir` | Quota lock app-data helper | Calls `app_data_dir().join("auth-refresh-locks")` | LOW common-interface proof: consumer pulls from quota helper, not raw env/platform data layout | LOW | `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs:186-192`; `planning/pin-gate/gates/diff.patch:56-64` |
| PP-009 | `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs::usage_lock_dir` | Quota lock app-data helper | Calls `lock_paths::app_data_dir().join("usage-refresh-locks")` | LOW common-interface proof: consumer pulls from quota helper, not raw env/platform data layout | LOW | `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs:88-101`; `planning/pin-gate/gates/diff.patch:96-104` |
| PP-010 | `crates/oulipoly-runtime/src/quota/mod.rs` and `src-tauri/src/usage/fetcher.rs::usage_lock_dir` | Runtime quota lock-root helper exported to Tauri usage fetcher | Re-exports `app_data_dir as lock_app_data_dir`; Tauri usage fetcher calls `quota::lock_app_data_dir()` | LOW common-interface proof: Tauri usage code pulls from the runtime quota helper instead of re-deriving env/path layout | LOW | `crates/oulipoly-runtime/src/quota/mod.rs:37-41`; `src-tauri/src/usage/fetcher.rs:165-167`; `planning/pin-gate/gates/diff.patch:665-686` |
| PP-011 | `crates/oulipoly-runtime/src/services/lock.rs::default_lock_dir` | Canonical data-dir helper | Calls `oulipoly_state::paths::data_dir()` and appends `locks` | LOW common-interface proof: consumer pulls from owner helper, not raw env or platform path layout | LOW | `crates/oulipoly-runtime/src/services/lock.rs:183-189`; `planning/pin-gate/gates/diff.patch:183-194` |
| PP-012 | `crates/oulipoly-runtime/src/session_metadata/locator.rs::default_state_dir` and `crates/oulipoly-runtime/src/sessions/mod.rs::default_app_data_dir` | Canonical data-dir helper plus explicit local fallback | Calls `oulipoly_state::paths::data_dir()`; fallback appends shared `APP_DATA_DIR_NAME` under `.` for existing operational fallback behavior | LOW common-interface proof for the delta: both consumers pull the pin/XDG decision and app-dir name from the shared owner constants/helper rather than duplicating env parsing or string literals | LOW | `crates/oulipoly-runtime/src/session_metadata/locator.rs:663-668`; `crates/oulipoly-runtime/src/sessions/mod.rs:389-400`; `planning/pin-gate/contracts/pin.contract.md:43-46` |
| PP-013 | `crates/oulipoly-runtime/src/session_replace/mod.rs::default_data_root` | Canonical data-dir helper | Calls `oulipoly_state::paths::data_dir()` | LOW common-interface proof: consumer pulls from owner helper, not raw env or platform path layout | LOW | `crates/oulipoly-runtime/src/session_replace/mod.rs:1723-1727`; `planning/pin-gate/gates/diff.patch:219-227` |
| PP-014 | `src-tauri/src/wiring.rs::default_cli_runtime_paths` | Canonical data-dir helper | Calls `oulipoly_state::paths::data_dir()` and falls back to config root only on resolution error | LOW common-interface proof: default data-root resolution is delegated to the shared helper rather than duplicating `OULIPOLY_DATA_DIR`/XDG parsing | LOW | `src-tauri/src/wiring.rs:204-218`; `planning/pin-gate/gates/diff.patch:693-700` |
| PP-015 | `crates/oulipoly-state/tests/data_dir_precedence.rs` | Declared data-dir behavior under test | Test sets `OULIPOLY_DATA_DIR`/`XDG_DATA_HOME` and asserts shipped default path APIs | LOW canonical contract probe: tests exercise proposal/contract-declared env precedence and public default path APIs; they do not create production pull coupling | LOW | `crates/oulipoly-state/tests/data_dir_precedence.rs:10-28`; `crates/oulipoly-state/tests/data_dir_precedence.rs:46-77`; `planning/pin-gate/proposal.md:9-15` |
| PP-016 | Spawn/integration harnesses in `age_pid_sidecar_spawn.rs`, `wu_d_proactive_wake_integration.rs`, `wu_b_mailbox_integration.rs`, and `wu_e_pty_delivery_integration.rs` | Test child process env and fixture data-root layout | Harnesses remove/set `OULIPOLY_DATA_DIR` and `XDG_DATA_HOME`; provider fixture verifies inherited pin and shadow-XDG behavior | LOW canonical contract probe: tests validate the declared spawn-env push and fallback behavior from the proposal; production code under test owns the common interface | LOW | `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:23-67`; `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs:122-160`; `src-tauri/tests/wu_d_proactive_wake_integration.rs:69-80`; `src-tauri/tests/wu_d_proactive_wake_integration.rs:664-705`; `src-tauri/tests/wu_b_mailbox_integration.rs:79-85`; `src-tauri/tests/wu_e_pty_delivery_integration.rs:90-143`; `planning/pin-gate/proposal.md:17-35` |
| PP-017 | `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs` | Quota lock-root env precedence under test | Test sets `OULIPOLY_DATA_DIR`, `OULIPOLY_DATA_HOME`, and `XDG_DATA_HOME`, then asserts `lock::usage_lock_dir()` | LOW canonical contract probe: test targets the quota helper's declared compatibility precedence and verifies downstream users do not re-derive lock roots | LOW | `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs:59-68`; `crates/oulipoly-runtime/src/quota/marker_verification/tests.rs:292-356`; `planning/pin-gate/contracts/pin.contract.md:39-41` |

## Uncontrolled-Source Coupler Findings

| ID | Puller | Source | Implicit contract evidence | Missing proof | Decoupling direction | Failure mode |
|---|---|---|---|---|---|---|
| None | None | None | No HIGH pull site found in the delta. | None | None | None |

## Residual Ambiguity / Stop-Condition Notes

- No `BLOCKED` condition: required inputs were readable and A1 metric source was intact.
- No `NEEDS_INPUT` condition: missing ownership/interface proof was not encountered at a concrete delta pull site.
- The quota `lock_paths.rs` helper remains a distinct compatibility surface for `OULIPOLY_DATA_HOME`-based quota lock roots. For this delta it uses `oulipoly_state::paths::DATA_DIR_ENV` and `APP_DATA_DIR_NAME` for the new pin/name surface, while downstream quota/Tauri call sites pull from `app_data_dir()` or `lock_app_data_dir()` rather than re-parsing the env layout.

Verdict: LOW

VERDICT: LOW
