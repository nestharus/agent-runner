# OULIPOLY_DATA_DIR Pin Contract

## Component declared roles

- accessor
- formatter
- mapper

## Per-file declared roles

- `crates/oulipoly-state/src/paths.rs` — mapper.
- `crates/oulipoly-state/src/db.rs` — mapper.
- `crates/oulipoly-state/src/pid_identity.rs` — mapper.
- `crates/oulipoly-state/src/lib.rs` — accessor.
- `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs` — formatter.
- `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs` — mapper.
- `crates/oulipoly-runtime/src/quota/lock_paths.rs` — mapper.
- `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs` — mapper.
- `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs` — mapper.
- `crates/oulipoly-runtime/src/quota/mod.rs` — accessor.
- `crates/oulipoly-runtime/src/services/lock.rs` — mapper.
- `crates/oulipoly-runtime/src/session_metadata/locator.rs` — mapper.
- `crates/oulipoly-runtime/src/session_replace/mod.rs` — mapper.
- `crates/oulipoly-runtime/src/sessions/mod.rs` — mapper.
- `src-tauri/src/usage/fetcher.rs` — mapper.
- `src-tauri/src/wiring.rs` — mapper.

## Function inventory

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-state/src/paths.rs::data_dir` | mapper | Maps the process pin/default platform data source into the canonical app data directory path. |
| `crates/oulipoly-state/src/paths.rs::default_data_dir` | mapper | Maps `dirs::data_dir()` into the app-specific `oulipoly-agent-runner` directory or returns the resolution error. |
| `crates/oulipoly-state/src/db.rs::StateDb::default_path` | mapper | Maps the canonical data directory to the default `state.db` path. |
| `crates/oulipoly-state/src/pid_identity.rs::default_path` | mapper | Maps the canonical data directory to the PID identity sidecar database path. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs::command_from_parts` | formatter | Materializes the spawned provider `Command`, including arguments, working directory, IPC env, and the data-dir spawn env shape. |
| `crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs::pin_agent_data_dir_if_unset` | formatter | Writes the canonical data-dir pin into the child process command environment when the parent process is not already pinned. |
| `crates/oulipoly-runtime/src/executor/cli/pty_broker.rs::control_socket_dir` | mapper | Maps runtime/state/default data roots into the PTY control socket directory path. |
| `crates/oulipoly-runtime/src/quota/lock_paths.rs::app_data_dir` | mapper | Maps the optional data-dir pin or legacy data-home fallback into the app data directory used by quota locks. |
| `crates/oulipoly-runtime/src/quota/auth_refresh_lock.rs::auth_refresh_lock_dir` | mapper | Maps the shared app data directory to the auth-refresh lock directory. |
| `crates/oulipoly-runtime/src/quota/marker_verification/lock.rs::usage_lock_dir` | mapper | Maps the shared app data directory to the usage-refresh lock directory. |
| `crates/oulipoly-runtime/src/services/lock.rs::default_lock_dir` | mapper | Maps the canonical data directory to the session lock directory or its operational error. |
| `crates/oulipoly-runtime/src/session_metadata/locator.rs::default_state_dir` | mapper | Maps the canonical data directory and provider name to the default session metadata directory. |
| `crates/oulipoly-runtime/src/session_replace/mod.rs::default_data_root` | mapper | Maps canonical data-dir resolution into the session replacement service's data root result. |
| `crates/oulipoly-runtime/src/sessions/mod.rs::resolve_state_dir` | mapper | Maps a session source entry and provider name to either the explicit state directory or the canonical default directory. |
| `crates/oulipoly-runtime/src/sessions/mod.rs::default_app_data_dir` | mapper | Maps canonical data-dir resolution to the sessions module's app-data fallback path. |
| `src-tauri/src/usage/fetcher.rs::usage_lock_dir` | mapper | Maps the runtime quota app data directory to the usage-refresh lock directory used by the Tauri usage fetcher. |
| `src-tauri/src/wiring.rs::default_cli_runtime_paths` | mapper | Maps default config/data roots into the structured runtime path bundle. |

MULTI-CLASSIFIER-RISK: none identified in the added or meaningfully changed production functions.

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-state/src/paths.rs
    role: adapter
    Translates:
      - process data-dir environment contract (`OULIPOLY_DATA_DIR`)
      - platform user-data directory contract (`dirs::data_dir`, including XDG-derived defaults)
      - agent-runner canonical app-data directory contract (`oulipoly-agent-runner`)
  - component: crates/oulipoly-runtime/src/executor/cli/launch/command_format.rs
    role: adapter
    Translates:
      - provider child process spawn environment contract (`std::process::Command` env inheritance and env writes)
      - agent-runner canonical data-dir pin contract (`oulipoly_state::paths::DATA_DIR_ENV`, `oulipoly_state::paths::data_dir`)
```

Reroute-only files consume the internal `oulipoly_state::paths` or `quota::lock_app_data_dir` helper and do not add a separate external-contract translation for this delta.

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-state/src/paths.rs
    role: intrinsic-surface
    Domain: agent_runner_data_dir_resolution
    Owns:
      - DATA_DIR_ENV (`OULIPOLY_DATA_DIR`)
      - APP_DATA_DIR_NAME (`oulipoly-agent-runner`)
      - data_dir precedence (`OULIPOLY_DATA_DIR` before default platform data directory)
      - default_data_dir fallback through `dirs::data_dir()`
      - canonical data-dir resolution error message (`Could not determine data directory`)
```
