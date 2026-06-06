# plk Step-6a Contract

## Component declared roles

Component: parent invocation linkage and stale-running PID sidecar reconciliation.

Declared roles: `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter`, `validator`, `filter`.

Touched files in scope:

| File | Declared roles | Role notes |
|---|---|---|
| `src-tauri/src/commands/trace/accessor.rs` | `accessor`, `orchestration` | Loads trace state/config and now invokes stale-running reconciliation before trace rendering. |
| `src-tauri/src/dispatch.rs` | `orchestration`, `parser`, `validator`, `accessor`, `formatter`, `mapper`, `predicate`, `filter` | Existing CLI dispatch root; PLK changes are unit tests for parent env resolution with provider source drift. |
| `src-tauri/src/dispatch/parent_invocation.rs` | `orchestration`, `accessor` | Resolves `OULIPOLY_PARENT_INVOCATION` to a same-DB invocation row id by UUID. |
| `src-tauri/src/dispatch/predicate.rs` | `predicate`, `accessor` | The previous provider-source guard predicate was removed; remaining predicates are unchanged. |
| `src-tauri/src/invocation/mod.rs` | `mapper` | Exposes the stale reconciliation module under the invocation namespace. |
| `src-tauri/src/invocation/stale_reconcile.rs` | `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter` | Reconciles stale running invocation rows only when PID sidecar liveness evidence proves the recorded process is dead. |
| `src-tauri/tests/pr_a_invocation_integration.rs` | `orchestration`, `accessor`, `mapper`, `formatter`, `validator`, `predicate`, `filter` | Unix integration harness for direct invocation parent env propagation, nested `agent-bash`, and finalization fences. |
| `src-tauri/tests/pr_b_trace_integration.rs` | `orchestration`, `accessor`, `mapper`, `validator` | Unix integration harness for trace rendering and stale-running reconciliation with a PID identity sidecar. |

## Production function inventory

Only added or meaningfully changed production functions are listed.

### `src-tauri/src/commands/trace/accessor.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `load_trace_environment` | `orchestration` | Opens the default StateDb, reconciles eligible stale running invocations, loads sessions config, and returns the trace environment. | None; trace reconciliation is delegated to `invocation::stale_reconcile`. |

### `src-tauri/src/dispatch/parent_invocation.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `resolve_parent_invocation_id` | `orchestration` | Reads the parent env value, parses the composite invocation id, and returns the same-DB row id for that UUID. | Source-name equality is intentionally not part of the lookup; UUID plus same StateDb scope is the contract. |
| `read_parent_invocation_env` | `accessor` | Reads `OULIPOLY_PARENT_INVOCATION` from the process environment. | None. |
| `lookup_parent_invocation_record` | `accessor` | Retrieves the invocation row matching the composite UUID from the supplied StateDb. | None. |

### `src-tauri/src/dispatch/predicate.rs`

| Symbol | A1 class | Meaning | Risk |
|---|---|---|---|
| `parent_invocation_source_matches` | `predicate` | Removed provider-source equality guard for parent resolution. | Intentional behavior change; source drift is accepted when the parent UUID exists in the same DB. |

### `src-tauri/src/invocation/stale_reconcile.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `reconcile_stale_running_invocations` | `orchestration` | Opens the PID sidecar if present, walks running invocation rows, applies age and liveness predicates, and finalizes proven-dead rows. | Conservative by design; missing/unknown evidence leaves rows untouched. |
| `open_pid_sidecar_read_only_optional` | `orchestration` | Resolves the default PID sidecar path and opens it read-only only when the sidecar exists. | None; no state DB schema migration or sidecar creation is performed. |
| `path_exists` | `predicate` | Answers whether a sidecar path exists. | None. |
| `running_invocations` | `orchestration` | Converts raw running invocation rows into timestamp-parsed domain rows. | None; SQL and parsing are delegated. |
| `running_invocation_rows` | `accessor` | Reads running, unfinished invocation rows from StateDb. | None. |
| `running_invocation_row_values` | `accessor` | Reads raw running invocation row values from StateDb. | None. |
| `running_invocation_row_value` | `mapper` | Maps one SQL row into raw running invocation values. | None. |
| `running_invocation_row_from_values` | `mapper` | Maps raw SQL values into a running invocation row value. | None. |
| `format_stale_running_prepare_error` | `formatter` | Formats stale-running query prepare errors. | None. |
| `format_stale_running_query_error` | `formatter` | Formats stale-running query execution errors. | None. |
| `format_stale_running_row_error` | `formatter` | Formats stale-running row mapping errors. | None. |
| `running_invocation_row` | `mapper` | Maps SQL fields into a raw running invocation row value. | None. |
| `running_invocation_from_row` | `mapper` | Maps a raw row into a timestamp-parsed running invocation value. | None. |
| `parse_running_invocation_created_at` | `parser` | Parses `created_at` as RFC3339 and normalizes it to UTC. | None. |
| `running_invocation_is_stale` | `predicate` | Answers whether the row age meets the trace stale-running threshold. | None. |
| `invocation_has_dead_pid_evidence` | `orchestration` | Reads sidecar rows for an invocation and delegates dead-evidence evaluation. | None; conservative false on no rows, unknown reads, or any live matching identity. |
| `pid_identity_rows_for_invocation` | `accessor` | Reads PID sidecar rows for one invocation UUID. | None. |
| `pid_identity_rows_have_dead_evidence` | `predicate` | Answers whether all supplied sidecar rows prove dead process identity and none prove a live match or unknown state. | None. |
| `pid_identity_row_liveness` | `mapper` | Maps one sidecar row to live, dead, or unknown liveness result. | None. |
| `live_process_identity_state` | `accessor` | Reads live process identity for an OS PID and maps read errors to unknown. | None. |
| `process_identity_matches_row` | `predicate` | Answers whether the live OS process identity matches the recorded sidecar identity. | None. |
| `finalize_stale_invocation` | `orchestration` | Finalizes the StateDb row as failed with stale-running terminal fields, tolerating already-finalized races. | None. |
| `invocation_already_finalized` | `predicate` | Answers whether a finalization error is the benign already-finalized race. | None. |

## Test function inventory

Only added or meaningfully changed test helpers and tests are listed.

### `src-tauri/src/dispatch.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift` | `validator` | Verifies parent resolution returns the row id when the env composite source differs from the stored provider name but the UUID is in the same DB. | None. |

### `src-tauri/tests/pr_a_invocation_integration.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `Fixture::state_home` | `accessor` | Returns the isolated `XDG_STATE_HOME` path for `agent-bash` status files. | None. |
| `run_agent_bash_nested_child` | `orchestration` | Dispatches a nested child through `agent-bash`, parses the handle, and waits for completion. | None; command construction and polling are delegated. |
| `nested_child_command` | `formatter` | Formats the nested runner command line. | None. |
| `shell_quote` | `formatter` | Formats a shell-safe single-quoted argument. | None. |
| `configure_agent_bash_env` | `mapper` | Maps the fixture paths and parent env into the `agent-bash` command environment. | None. |
| `wait_for_agent_bash_done` | `orchestration` | Polls `agent-bash status --full` until completion or timeout. | None. |
| `agent_bash_status` | `accessor` | Reads the full `agent-bash` status output for a handle. | None. |
| `agent_bash_bin_from_env` | `validator` | Accepts a supplied `AGENT_BASH_BIN` or a PATH-discovered `agent-bash` only when it points to a file. | Fails closed when no runnable `agent-bash` binary is available. |
| `find_agent_bash_in_path` | `filter` | Selects the first PATH entry containing an `agent-bash` file. | None. |
| `assert_agent_bash_bin` | `validator` | Verifies the selected `agent-bash` path points to a file. | None. |
| `nested_agent_bash_chain_records_parent_id_from_inherited_env` | `validator` | Verifies a nested `agent-bash` child records `parent_invocation_id` from the inherited parent env. | None. |

### `src-tauri/tests/pr_b_trace_integration.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `Fixture::sidecar_path` | `mapper` | Maps the isolated data home to the PID identity sidecar path. | None. |
| `seed_stale_running_trace_row` | `orchestration` | Seeds a stale running invocation row and returns its row id. | None. |
| `seed_stale_running_trace_row_with_dead_pid` | `orchestration` | Seeds a stale running row plus a sidecar PID identity that cannot match a live process. | None. |
| `trace_reconciles_liveness_stale_running_row_with_dead_pid` | `validator` | Verifies trace reconciles sidecar-proven stale rows to durable failed terminal state. | None. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: src-tauri/src/dispatch/parent_invocation.rs
    role: adapter
    Translates:
      - OULIPOLY_PARENT_INVOCATION environment value
      - oulipoly_state CompositeInvocationId JSON/env grammar
      - same-StateDb invocation UUID lookup
      - StateDb invocation row id used as parent_invocation_id
  - component: src-tauri/src/invocation/stale_reconcile.rs
    role: adapter
    Translates:
      - oulipoly-state PID identity sidecar records
      - OS process liveness identity reads
      - StateDb running invocation rows
      - StateDb terminal invocation finalization fields
  - component: src-tauri/src/dispatch.rs
    role: adapter
    Translates:
      - CLI argument model and subcommand routing
      - runtime execution and resume service entrypoints
      - command-handler and wiring module boundaries
      - dispatch-local parser, predicate, formatter, clock, and failure-marker modules
      - dispatch test parent-env and StateDb fixture surfaces
  - component: src-tauri/tests/pr_a_invocation_integration.rs
    role: adapter
    Translates:
      - Unix runner binary integration fixture and model/provider config files
      - StateDb invocation row assertions and fixture SQL
      - OULIPOLY_PARENT_INVOCATION and OULIPOLY_INVOCATION marker JSON
      - agent-bash run/status interface with isolated XDG_STATE_HOME
      - trace CLI JSON helper used by terminal-state assertions
  - component: src-tauri/tests/pr_b_trace_integration.rs
    role: adapter
    Translates:
      - Unix trace CLI integration fixture and JSON output
      - StateDb running and stale invocation row fixtures
      - PidIdentityDb sidecar records and ProcessIdentity values
      - isolated XDG config/data filesystem roots
      - fixture provider shell command and model config files
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: src-tauri/src/dispatch/parent_invocation.rs
    role: intrinsic-surface
    Domain: parent_invocation_linkage
    Owns:
      - OULIPOLY_PARENT_INVOCATION env var consumption
      - malformed parent env values resolving to no parent rather than panicking
      - unknown parent UUID resolving to no parent
      - same-DB UUID parent lookup tolerating provider/source name drift
  - component: src-tauri/src/invocation/stale_reconcile.rs
    role: intrinsic-surface
    Domain: stale_running_pid_sidecar_reconciliation
    Owns:
      - read-only PID identity sidecar open via PidIdentityDb::default_path
      - stale-running age threshold check through oulipoly_runtime::trace::STALE_RUNNING_THRESHOLD_SECONDS
      - conservative live/dead/unknown process identity handling
      - stale_running error_category and stale_running_liveness terminal_reason finalization
      - no state.db schema change; PID evidence remains in pid-identity.db sidecar
  - component: src-tauri/src/commands/trace/accessor.rs
    role: intrinsic-surface
    Domain: trace_pre_render_reconciliation
    Owns:
      - default StateDb open via StateDb::open_default
      - default sessions.toml path resolution through crate::cli::paths::default_config_root
      - trace SessionsConfig::load
      - trace sessions config load error formatting
      - trace_environment mapper handoff
      - stale-running reconciliation before trace environment construction returns
      - trace rendering after reconciliation sees durable terminal state when evidence is conclusive
  - component: src-tauri/src/dispatch.rs
    role: intrinsic-surface
    Domain: cli_lifecycle_orchestration
    Owns:
      - lifecycle loops
      - run_with_balancing lifecycle loop
      - run_resume lifecycle loop
      - run_repl lifecycle loop
      - top-level --resume dispatch
      - invocation finalization sequencing
      - terminal signal outcome sequencing
      - provider retry and migration sequencing
      - session_replace recovery
      - CLI structs and subcommand enums from crate::usage::cli
      - top-level resume prompt source resolution
      - resume error formatting
      - command, run, usage, and wiring module dispatch
      - dispatch-local clock, formatter, parent_invocation, parser, pre_invocation_failure, predicate, and usage_context modules
      - dispatch test StateDb parent lookup fixtures
      - dispatch test CompositeInvocationId env values
      - dispatch test locked process-environment mutation
  - component: src-tauri/src/dispatch/predicate.rs
    role: intrinsic-surface
    Domain: dispatch_predicates
    Owns:
      - diagnostics model configured predicate over ModelConfig maps
      - agent_runner_lib::load_app_config diagnostics_model read
      - resume short-line emission predicate
      - execution success predicate
  - component: src-tauri/src/invocation/mod.rs
    role: intrinsic-surface
    Domain: invocation_module_namespace
    Owns:
      - finalize child module export
      - result_envelope child module export
      - stale_reconcile child module export
```

## Test-harness declarations

```yaml
test_harness_declarations:
  - component: src-tauri/tests/pr_a_invocation_integration.rs
    role: test-harness
    Surface:
      - Unix runner binary integration fixture
      - isolated XDG_CONFIG_HOME, XDG_DATA_HOME, XDG_STATE_HOME, and env -u OULIPOLY_DATA_DIR
      - fixture model/provider shell command
      - real agent-bash binary supplied by AGENT_BASH_BIN for nested parent propagation
      - StateDb assertions on invocation status, uuid, and parent_invocation_id
  - component: src-tauri/tests/pr_b_trace_integration.rs
    role: test-harness
    Surface:
      - Unix trace CLI integration fixture
      - isolated XDG data/config roots and env -u OULIPOLY_DATA_DIR
      - StateDb running/stale invocation fixtures
      - PidIdentityDb sidecar fixture and dead PID identity record
      - trace JSON and durable StateDb terminal-state assertions
  - component: src-tauri/src/dispatch.rs::tests
    role: test-harness
    Surface:
      - in-memory StateDb parent lookup fixture
      - serialized CompositeInvocationId env values
      - locked process environment mutation for parent env unit tests
```
