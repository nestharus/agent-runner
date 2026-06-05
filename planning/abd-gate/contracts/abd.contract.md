# A+B+D Phase-6a Code-Quality Contract

Scope: async-bash agent-runner sidecar work covering WU-A PID identity sidecar, WU-B mailbox delivery, and WU-D auto-wake coordination.

## Component declared roles

This WU should not be scored as one cohesive all-role component. The touched surface intentionally splits into focused sub-surfaces, so cohesion should be scored per touched file using the table below.

Sub-surface declared roles:

| Sub-surface | Roles |
|---|---|
| PID identity store | `accessor`, `mapper`, `orchestration`, `parser`, `validator` |
| Mailbox store | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` |
| Spawn identity capture | `formatter`, `mapper`, `orchestration`, `parser` |
| Agent-bash notify resolver | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` |
| Mailbox resume delivery | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` |
| Wake coordinator | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` |
| Resume/repl/balancing hooks | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` |

## Per-file declared roles

| File | Declared roles |
|---|---|
| `crates/oulipoly-runtime/src/executor/cli.rs` | `orchestration` |
| `crates/oulipoly-runtime/src/executor/cli/headless.rs` | `orchestration` |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs` | `formatter`, `mapper`, `orchestration`, `validator` |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs` | `orchestration` |
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs` | `mapper`, `orchestration` |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` | `formatter`, `mapper`, `orchestration`, `parser` |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` | `mapper`, `orchestration`, `predicate` |
| `crates/oulipoly-state/src/lib.rs` | `accessor`, `validator` |
| `crates/oulipoly-state/src/mailbox.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` |
| `crates/oulipoly-state/src/pid_identity.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` |
| `src-tauri/src/commands/mailbox.rs` | `accessor`, `formatter`, `mapper`, `orchestration` |
| `src-tauri/src/commands/mod.rs` | `accessor` |
| `src-tauri/src/commands/notify.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `parser`, `validator` |
| `src-tauri/src/commands/pid_session.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` |
| `src-tauri/src/dispatch.rs` | `formatter`, `mapper`, `orchestration`, `validator` |
| `src-tauri/src/mailbox_delivery.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate` |
| `src-tauri/src/main.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate` |
| `src-tauri/src/migration_providers.rs` | `accessor`, `mapper`, `orchestration` |
| `src-tauri/src/run/balancing/finalization.rs` | `orchestration` |
| `src-tauri/src/run/balancing/orchestration.rs` | `formatter`, `mapper`, `orchestration` |
| `src-tauri/src/run/repl/orchestration.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` |
| `src-tauri/src/run/resume/orchestration.rs` | `accessor`, `filter`, `formatter`, `mapper`, `orchestration`, `predicate`, `validator` |
| `src-tauri/src/usage/cli.rs` | `mapper`, `parser`, `validator` |
| `src-tauri/src/wake_coordinator.rs` | `accessor`, `formatter`, `mapper`, `orchestration`, `parser`, `predicate`, `validator` |

## Function inventory

Production functions only. `#[cfg(test)]` functions and test modules are intentionally excluded.

### `crates/oulipoly-runtime/src/executor/cli.rs`

No production functions; this file is a facade module/re-export surface.

### `crates/oulipoly-runtime/src/executor/cli/headless.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/headless.rs::execute` | `orchestration` | Sequences provider lookup, input resolution, provider execution, spawn identity context, cleanup, and result mapping. MULTI-CLASSIFIER-RISK: includes mapping/cleanup work inline with orchestration. |
| `crates/oulipoly-runtime/src/executor/cli/headless.rs::execute_effective` | `orchestration` | Thin facade routing effective execution through the start-known-session entrypoint. |
| `crates/oulipoly-runtime/src/executor/cli/headless.rs::execute_effective_with_start_known_provider_session_id` | `orchestration` | Thin facade adding optional start-known provider-session context. |
| `crates/oulipoly-runtime/src/executor/cli/headless.rs::execute_effective_with_optional_supervisor_config` | `orchestration` | Sequences input resolution, provider execution, spawn identity context, cleanup, and result mapping. MULTI-CLASSIFIER-RISK: combines orchestration and result/context mapping. |

### `crates/oulipoly-runtime/src/executor/cli/interactive.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive` | `orchestration` | Delegates interactive execution and reduces the result to an exit code. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result` | `orchestration` | Facade that delegates to the model-identity-aware interactive entrypoint. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::execute_interactive_with_result_and_model_identity` | `orchestration` | Sequences arg validation, provider policy, resume args, command build, direct spawn/wait, identity record, and status mapping. MULTI-CLASSIFIER-RISK: validation, command mapping, kernel spawn, and result mapping live in one function. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::validated_interactive_args` | `validator` | Accepts provider configs with `interactive_args` and rejects missing interactive configuration. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::interactive_args_missing_error` | `formatter` | Formats the stable validation error for missing interactive args. |
| `crates/oulipoly-runtime/src/executor/cli/interactive.rs::interactive_result_from_status` | `mapper` | Maps process exit status and recognizer evidence into `InteractiveExecutionResult`. |

### `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs::execute_provider` | `orchestration` | Facade that supplies provider base args and delegates provider execution. |
| `crates/oulipoly-runtime/src/executor/cli/provider_execution.rs::execute_provider_with_arg_parts_and_supervisor_config` | `orchestration` | Assembles launch, runs supervisor, reads IPC return channel, and maps raw result. MULTI-CLASSIFIER-RISK: launch assembly and result mapping are mixed with orchestration. |

### `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs::execute_resume` | `orchestration` | Facade that routes mandatory prompt resume through optional-prompt execution. |
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs::execute_resume_optional_prompt` | `orchestration` | Facade that routes resume execution without explicit model identity. |
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs::execute_resume_optional_prompt_with_model_identity` | `orchestration` | Facade preserving explicit model identity for spawn-sidecar context. |
| `crates/oulipoly-runtime/src/executor/cli/resume_execution.rs::execute_resume_with_optional_supervisor_config` | `orchestration` | Composes resume args, disables capture, executes provider, classifies acceptance, and maps result. MULTI-CLASSIFIER-RISK: orchestration, mapping, and classification live together. |

### `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::SpawnRuntimeMode::as_str` | `formatter` | Converts runtime mode enum to persisted sidecar string token. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::context_from_parent_invocation_env` | `mapper` | Maps parent invocation env plus launch metadata into `SpawnIdentityContext`. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::record_child_identity` | `orchestration` | Records live child identity and then marks session runtime running when a row is available. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::mark_session_running` | `orchestration` | Opens mailbox sidecar and delegates session-runtime running update. |
| `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs::parse_invocation_env_silent` | `parser` | Parses `OULIPOLY_PARENT_INVOCATION` payload while suppressing malformed values. |

### `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::SupervisorConfig::production` | `mapper` | Builds production supervisor config from provider, prompt mode, and prompt payload. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::SupervisorConfig::with_prompt_contract` | `mapper` | Returns an adjusted supervisor config with overridden prompt contract fields. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::run_provider_supervisor` | `orchestration` | Wraps supervised execution and maps supervisor errors for executor callers. |
| `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs::execute_with_supervisor` | `orchestration` | Owns supervised child lifecycle from command setup through drains, live signal handling, termination, and output mapping. MULTI-CLASSIFIER-RISK: IO draining, live classification, and output mapping are all inline. |

### `crates/oulipoly-state/src/lib.rs`

No production functions; this file is a root re-export and compatibility surface.

### `crates/oulipoly-state/src/mailbox.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::default_path` | `accessor` | Exposes the shared sidecar DB default path. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::open_default` | `orchestration` | Resolves the default path and opens an initialized mailbox DB. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::open_default_if_exists` | `orchestration` | Checks for the default sidecar and opens it only when present. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::open` | `orchestration` | Ensures directory, opens SQLite, enables WAL, and ensures PID/mailbox schema. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::open_read_only` | `orchestration` | Opens a read-only sidecar connection. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::path` | `accessor` | Exposes the DB path without changing meaning. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::connection` | `accessor` | Exposes the underlying SQLite connection. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::enqueue_agent_bash_complete` | `orchestration` | Runs enqueue logic inside a transaction and commits it. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::list_pending` | `accessor` | Retrieves undelivered mailbox rows for a session. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::list_mailbox` | `filter` | Selects all rows or pending-only rows based on the `all` flag. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::mark_delivered` | `orchestration` | Transactionally updates mailbox rows as delivered. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::upsert_session_runtime` | `orchestration` | Inserts or updates session runtime metadata in the sidecar. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::mark_session_running` | `orchestration` | Validates running state and persists runtime identity fields. MULTI-CLASSIFIER-RISK: validation and DB mutation are inline. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::mark_session_idle` | `orchestration` | Validates idle state and performs invocation-guarded idle update. MULTI-CLASSIFIER-RISK: validation and DB mutation are inline. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::session_runtime` | `accessor` | Retrieves one session runtime row and checks its stored run state. MULTI-CLASSIFIER-RISK: accessor plus validation. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::session_liveness` | `predicate` | Answers busy/idle by checking recorded runtime state against live process identity. MULTI-CLASSIFIER-RISK: predicate also clears stale DB state. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::try_acquire_wake_claim` | `orchestration` | Delegates wake-claim acquisition without a renewal token. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::try_acquire_or_renew_wake_claim` | `orchestration` | Checks pending rows, liveness, stale claims, then transactionally acquires a claim. MULTI-CLASSIFIER-RISK: filtering, predicates, and mutation are inline. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::wake_claim` | `accessor` | Retrieves the current wake claim for a session. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::release_wake_claim` | `orchestration` | Deletes wake claims with optional token guard. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::record_wake_claim_pid` | `orchestration` | Updates an existing wake claim with a spawned PID. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::validate_wake_claim_for_child` | `validator` | Accepts only matching, non-busy wake claims for a child resume. MULTI-CLASSIFIER-RISK: validation can mutate by releasing a claim. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::list_mailbox_all` | `accessor` | Retrieves all mailbox rows for a session. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::max_mailbox_seq` | `accessor` | Reads the maximum mailbox sequence for a session. |
| `crates/oulipoly-state/src/mailbox.rs::MailboxDb::clear_stale_running_row` | `orchestration` | Updates stale running runtime rows back to idle. |
| `crates/oulipoly-state/src/mailbox.rs::enqueue_agent_bash_complete_in_tx` | `orchestration` | Inserts or detects a mailbox row and classifies the enqueue result. MULTI-CLASSIFIER-RISK: persistence and conflict classification are inline. |
| `crates/oulipoly-state/src/mailbox.rs::query_mailbox_by_kind_handle_tx` | `accessor` | Retrieves a unique mailbox row by kind and handle inside a transaction. |
| `crates/oulipoly-state/src/mailbox.rs::ensure_parent_dir` | `orchestration` | Ensures the sidecar parent directory exists. |
| `crates/oulipoly-state/src/mailbox.rs::set_wal_mode` | `orchestration` | Applies SQLite WAL mode for the sidecar connection. |
| `crates/oulipoly-state/src/mailbox.rs::ensure_mailbox_schema` | `orchestration` | Creates mailbox, session-runtime, and wake-claim sidecar schema. |
| `crates/oulipoly-state/src/mailbox.rs::ensure_session_runtime_columns` | `orchestration` | Adds missing runtime columns to older sidecar tables. MULTI-CLASSIFIER-RISK: schema inspection and migration loop are inline. |
| `crates/oulipoly-state/src/mailbox.rs::table_columns` | `accessor` | Reads SQLite table column names. |
| `crates/oulipoly-state/src/mailbox.rs::validate_run_state` | `validator` | Accepts only `idle` and `running` run-state tokens. |
| `crates/oulipoly-state/src/mailbox.rs::runtime_row_identity` | `mapper` | Maps optional runtime row PID fields into `ProcessIdentity`. |
| `crates/oulipoly-state/src/mailbox.rs::pending_seq_bounds_tx` | `accessor` | Reads pending mailbox min/max sequence bounds inside a transaction. |
| `crates/oulipoly-state/src/mailbox.rs::wake_claim` | `accessor` | Retrieves a wake-claim row by session. |
| `crates/oulipoly-state/src/mailbox.rs::wake_claim_tx` | `accessor` | Retrieves a wake-claim row by session inside a transaction. |
| `crates/oulipoly-state/src/mailbox.rs::claim_is_stale` | `predicate` | Answers whether a claim timestamp exceeds the stale threshold. MULTI-CLASSIFIER-RISK: parses timestamp while answering a predicate. |
| `crates/oulipoly-state/src/mailbox.rs::map_session_runtime_row` | `mapper` | Maps SQLite row columns into `SessionRuntimeRow`. |
| `crates/oulipoly-state/src/mailbox.rs::map_wake_claim_row` | `mapper` | Maps SQLite row columns into `WakeClaimRow`. |
| `crates/oulipoly-state/src/mailbox.rs::map_mailbox_row` | `mapper` | Maps SQLite row columns into `MailboxRow`. |
| `crates/oulipoly-state/src/mailbox.rs::collect_rows` | `mapper` | Collects rusqlite mapped rows into a vector with error conversion. |
| `crates/oulipoly-state/src/mailbox.rs::now_rfc3339` | `formatter` | Formats current UTC time as seconds-precision RFC3339. |

### `crates/oulipoly-state/src/pid_identity.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityRow::identity` | `mapper` | Maps a sidecar row to its process identity subset. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::default_path` | `accessor` | Exposes the default PID identity sidecar path. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::open_default` | `orchestration` | Resolves default path and opens an initialized DB. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::open` | `orchestration` | Ensures directory, opens SQLite, enables WAL, and ensures schema. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::open_default_read_only` | `orchestration` | Resolves default path and opens a read-only DB. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::open_read_only` | `orchestration` | Opens a PID sidecar connection in read-only mode. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::path` | `accessor` | Exposes the DB path. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::connection` | `accessor` | Exposes the underlying SQLite connection. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::record_identity` | `orchestration` | Upserts a PID identity row and rereads it. MULTI-CLASSIFIER-RISK: persistence and readback validation are inline. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::set_session_id` | `orchestration` | Updates session ID guarded by full process identity. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::lookup_by_identity` | `accessor` | Retrieves a row by verified process identity. |
| `crates/oulipoly-state/src/pid_identity.rs::PidIdentityDb::lookup_by_invocation_uuid` | `accessor` | Retrieves sidecar rows for an invocation UUID. |
| `crates/oulipoly-state/src/pid_identity.rs::default_path` | `accessor` | Builds the data-dir path for `pid-identity.db`. |
| `crates/oulipoly-state/src/pid_identity.rs::record_live_process_identity` | `orchestration` | Reads live identity, reads process group, opens sidecar, and records the row. MULTI-CLASSIFIER-RISK: accessor and persistence orchestration are inline. |
| `crates/oulipoly-state/src/pid_identity.rs::read_live_process_identity` | `accessor` | Exposes platform live process identity lookup. |
| `crates/oulipoly-state/src/pid_identity.rs::ensure_parent_dir` | `orchestration` | Ensures the sidecar parent directory exists. |
| `crates/oulipoly-state/src/pid_identity.rs::set_wal_mode` | `orchestration` | Applies SQLite WAL mode. |
| `crates/oulipoly-state/src/pid_identity.rs::ensure_identity_schema` | `orchestration` | Creates the PID identity table when missing. |
| `crates/oulipoly-state/src/pid_identity.rs::map_pid_identity_row` | `mapper` | Maps SQLite row columns into `PidIdentityRow`. |
| `crates/oulipoly-state/src/pid_identity.rs::collect_rows` | `mapper` | Collects rusqlite mapped rows into a vector with error conversion. |
| `crates/oulipoly-state/src/pid_identity.rs::single_or_ambiguous` | `validator` | Accepts zero or one row and rejects ambiguous identity rows. |
| `crates/oulipoly-state/src/pid_identity.rs::read_live_process_identity_impl [cfg(target_os = "linux")]` | `accessor` | Retrieves live identity from `/proc` starttime and boot ID. MULTI-CLASSIFIER-RISK: accessor also maps the final identity struct. |
| `crates/oulipoly-state/src/pid_identity.rs::read_live_process_identity_impl [cfg(not(target_os = "linux"))]` | `accessor` | Exposes no live identity on unsupported platforms. |
| `crates/oulipoly-state/src/pid_identity.rs::read_proc_starttime_ticks [cfg(target_os = "linux")]` | `accessor` | Reads `/proc/<pid>/stat` and returns parsed starttime ticks. MULTI-CLASSIFIER-RISK: filesystem access and proc-stat parsing are inline. |
| `crates/oulipoly-state/src/pid_identity.rs::parse_proc_stat_starttime_ticks [cfg(target_os = "linux")]` | `parser` | Parses the Linux proc-stat starttime field from raw stat text. |
| `crates/oulipoly-state/src/pid_identity.rs::read_boot_id [cfg(target_os = "linux")]` | `accessor` | Reads and trims Linux boot ID from `/proc/sys/kernel/random/boot_id`. MULTI-CLASSIFIER-RISK: file access and empty-value validation are inline. |
| `crates/oulipoly-state/src/pid_identity.rs::process_group_id [cfg(unix)]` | `accessor` | Retrieves a Unix process group ID from the kernel. |
| `crates/oulipoly-state/src/pid_identity.rs::process_group_id [cfg(not(unix))]` | `accessor` | Exposes no process group on unsupported platforms. |

### `src-tauri/src/commands/mailbox.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/commands/mailbox.rs::run_list` | `orchestration` | Retrieves rows and selects JSON or human rendering. MULTI-CLASSIFIER-RISK: access and formatting dispatch are inline. |
| `src-tauri/src/commands/mailbox.rs::list_rows` | `accessor` | Reads mailbox rows from the optional sidecar. |
| `src-tauri/src/commands/mailbox.rs::print_human_rows` | `formatter` | Emits mailbox rows as human text. |
| `src-tauri/src/commands/mailbox.rs::print_json` | `formatter` | Serializes and emits mailbox response JSON. |

### `src-tauri/src/commands/mod.rs`

No production functions; this file exposes command modules.

### `src-tauri/src/commands/notify.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/commands/notify.rs::OwnerSessionSource::as_str` | `formatter` | Formats owner-session source as a response string token. |
| `src-tauri/src/commands/notify.rs::run_agent_bash_complete` | `orchestration` | Routes inner notify outcome or error to the correct renderer. MULTI-CLASSIFIER-RISK: error classification and rendering dispatch are inline. |
| `src-tauri/src/commands/notify.rs::run_agent_bash_complete_inner` | `orchestration` | Sequences metadata parse, owner resolution, payload build, enqueue, and wake trigger. MULTI-CLASSIFIER-RISK: parser/accessor/formatter/orchestration responsibilities are inline. |
| `src-tauri/src/commands/notify.rs::read_metadata` | `parser` | Reads and parses `meta.json` into JSON `Value`. |
| `src-tauri/src/commands/notify.rs::parse_caller_chain` | `parser` | Parses `caller_chain` into caller identity records. MULTI-CLASSIFIER-RISK: parser also validates presence and non-empty shape. |
| `src-tauri/src/commands/notify.rs::parse_caller_identity` | `parser` | Parses one caller-chain entry into `CallerIdentity`. MULTI-CLASSIFIER-RISK: parser also maps into process identity. |
| `src-tauri/src/commands/notify.rs::integer_field` | `parser` | Extracts an integer field from alternate JSON names. MULTI-CLASSIFIER-RISK: parser also validates missing field. |
| `src-tauri/src/commands/notify.rs::string_field` | `parser` | Extracts a non-empty string field from alternate JSON names. MULTI-CLASSIFIER-RISK: parser also validates non-empty value. |
| `src-tauri/src/commands/notify.rs::read_rc` | `parser` | Reads and parses the workload rc file as `i32`. |
| `src-tauri/src/commands/notify.rs::resolve_owner` | `accessor` | Looks up caller-chain ownership through sidecar and state DB. MULTI-CLASSIFIER-RISK: accessor also filters nearest owner source. |
| `src-tauri/src/commands/notify.rs::resolved_owner` | `mapper` | Builds `ResolvedOwner` from caller identity, sidecar row, session ID, and source. |
| `src-tauri/src/commands/notify.rs::resolve_state_invocation_session` | `accessor` | Reads state DB invocation session fallback for a sidecar PID row. |
| `src-tauri/src/commands/notify.rs::resolved_invocation_session_id` | `accessor` | Selects provider session ID, falling back to session ID. |
| `src-tauri/src/commands/notify.rs::open_sidecar_read_only_optional` | `accessor` | Opens optional PID sidecar read-only. |
| `src-tauri/src/commands/notify.rs::open_state_read_only_optional` | `accessor` | Opens optional state DB read-only. |
| `src-tauri/src/commands/notify.rs::payload_json` | `formatter` | Renders the mailbox payload JSON string. MULTI-CLASSIFIER-RISK: formatting and domain-field mapping are inline. |
| `src-tauri/src/commands/notify.rs::render_notify_success` | `formatter` | Emits success responses for enqueue, idempotent enqueue, or no-owner. MULTI-CLASSIFIER-RISK: outcome branching and formatting are inline. |
| `src-tauri/src/commands/notify.rs::render_notify_error` | `formatter` | Emits JSON or human notify error responses. MULTI-CLASSIFIER-RISK: response mapping and rendering are inline. |
| `src-tauri/src/commands/notify.rs::notify_response` | `mapper` | Maps command args, owner, seq, and wake diagnostics into `NotifyResponse`. |
| `src-tauri/src/commands/notify.rs::render_response` | `formatter` | Emits notify response as JSON or human text. |
| `src-tauri/src/commands/notify.rs::path_string` | `mapper` | Converts a filesystem path into an owned lossy string. |

### `src-tauri/src/commands/pid_session.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/commands/pid_session.rs::run_of_pid` | `orchestration` | Coordinates live PID lookup, session resolution, response mapping, and rendering. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/commands/pid_session.rs::run_alive` | `orchestration` | Coordinates live lookup, alive response mapping, and rendering. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/commands/pid_session.rs::run_subtree` | `orchestration` | Coordinates live lookup, DB access, subtree build, response mapping, and rendering. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/commands/pid_session.rs::lookup_verified_live_row` | `accessor` | Retrieves the sidecar row matching the current live process identity. MULTI-CLASSIFIER-RISK: access and live-identity validation are inline. |
| `src-tauri/src/commands/pid_session.rs::live_identity_for_pid` | `accessor` | Reads current live process identity for a PID. |
| `src-tauri/src/commands/pid_session.rs::open_sidecar_read_only_optional` | `accessor` | Opens optional PID sidecar read-only. |
| `src-tauri/src/commands/pid_session.rs::open_sidecar_read_only_required` | `accessor` | Opens required PID sidecar or reports missing. MULTI-CLASSIFIER-RISK: accessor plus required-presence validation. |
| `src-tauri/src/commands/pid_session.rs::open_state_read_only_optional` | `accessor` | Opens optional state DB read-only. |
| `src-tauri/src/commands/pid_session.rs::open_state_read_only_required` | `accessor` | Opens required state DB or reports missing. MULTI-CLASSIFIER-RISK: accessor plus required-presence validation. |
| `src-tauri/src/commands/pid_session.rs::resolve_row_session_id` | `accessor` | Reads session ID from sidecar row or state DB fallback. MULTI-CLASSIFIER-RISK: accessor and fallback selection are inline. |
| `src-tauri/src/commands/pid_session.rs::resolved_invocation_session_id` | `accessor` | Selects provider session ID, falling back to session ID. |
| `src-tauri/src/commands/pid_session.rs::build_subtree_node` | `mapper` | Maps an invocation record plus PID annotation into a subtree node. MULTI-CLASSIFIER-RISK: recursive child orchestration is inline. |
| `src-tauri/src/commands/pid_session.rs::build_subtree_children` | `orchestration` | Iterates child invocations, applies visited guard, and recurses. MULTI-CLASSIFIER-RISK: traversal, filtering, and mapping are inline. |
| `src-tauri/src/commands/pid_session.rs::pid_annotation_for_invocation` | `accessor` | Reads PID rows and chooses live or fallback annotation. MULTI-CLASSIFIER-RISK: accessor, predicate, and selection are inline. |
| `src-tauri/src/commands/pid_session.rs::row_is_alive` | `predicate` | Answers whether a sidecar row still matches live process identity. |
| `src-tauri/src/commands/pid_session.rs::live_identity_for_row_pid` | `accessor` | Reads current live process identity for the row PID. |
| `src-tauri/src/commands/pid_session.rs::of_pid_response` | `mapper` | Maps PID sidecar row data into `PidSessionResponse`. |
| `src-tauri/src/commands/pid_session.rs::render_of_pid_not_found` | `formatter` | Emits not-found response in JSON or human form. |
| `src-tauri/src/commands/pid_session.rs::render_of_pid_found` | `formatter` | Emits found response in JSON or human form. |
| `src-tauri/src/commands/pid_session.rs::render_alive` | `formatter` | Emits alive response and corresponding exit code. |
| `src-tauri/src/commands/pid_session.rs::render_subtree_not_found` | `formatter` | Emits subtree not-found response. |
| `src-tauri/src/commands/pid_session.rs::render_subtree_found` | `formatter` | Emits subtree response as JSON or human tree. MULTI-CLASSIFIER-RISK: formatting dispatch and recursive render delegation are inline. |
| `src-tauri/src/commands/pid_session.rs::render_subtree_node` | `formatter` | Recursively formats subtree nodes as text. |
| `src-tauri/src/commands/pid_session.rs::print_json` | `formatter` | Serializes and prints PID-session JSON. |

### `src-tauri/src/dispatch.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/dispatch.rs::run` | `orchestration` | Top-level CLI lifecycle router across recovery, usage, subcommands, resume, model, and agent paths. MULTI-CLASSIFIER-RISK: many dispatch predicates live inline. |
| `src-tauri/src/dispatch.rs::recover_pending_session_replaces` | `orchestration` | Delegates pending session-replace recovery. |
| `src-tauri/src/dispatch.rs::handle_pending_session_replace_error` | `orchestration` | Emits pending-replace error and maps it to exit code. MULTI-CLASSIFIER-RISK: formatting and mapping are inline. |
| `src-tauri/src/dispatch.rs::emit_pending_session_replace_error` | `formatter` | Emits replacement error JSON to stderr. |
| `src-tauri/src/dispatch.rs::pending_session_replace_exit_code` | `mapper` | Maps replacement error to process exit code. |
| `src-tauri/src/dispatch.rs::run_default_provider_repl` | `orchestration` | Creates default-provider runtime services and runs the default REPL. |
| `src-tauri/src/dispatch.rs::run_usage_command` | `orchestration` | Loads usage context and dispatches usage rendering. |
| `src-tauri/src/dispatch.rs::dispatch_subcommand` | `orchestration` | Routes parsed top-level subcommands to handlers. MULTI-CLASSIFIER-RISK: maps many CLI variants and dispatches them inline. |
| `src-tauri/src/dispatch.rs::dispatch_notify_subcommand` | `orchestration` | Routes notify subcommands to command handlers. |
| `src-tauri/src/dispatch.rs::dispatch_mailbox_subcommand` | `orchestration` | Routes mailbox subcommands to command handlers. |
| `src-tauri/src/dispatch.rs::dispatch_trace_subcommand` | `orchestration` | Maps trace flags into trace options and dispatches the trace command. |
| `src-tauri/src/dispatch.rs::dispatch_resume_subcommand` | `orchestration` | Maps resume subcommand args into `run_resume`. |
| `src-tauri/src/dispatch.rs::dispatch_session_subcommand` | `orchestration` | Routes session subcommands to command handlers. MULTI-CLASSIFIER-RISK: maps many command variants and dispatches them inline. |
| `src-tauri/src/dispatch.rs::dispatch_session_locate_subcommand` | `orchestration` | Delegates session locate command execution. |
| `src-tauri/src/dispatch.rs::dispatch_session_export_subcommand` | `orchestration` | Delegates session export command execution. |
| `src-tauri/src/dispatch.rs::dispatch_top_level_resume` | `orchestration` | Validates top-level resume and routes to headless or interactive resume. MULTI-CLASSIFIER-RISK: validation and dispatch are inline. |
| `src-tauri/src/dispatch.rs::dispatch_headless_top_level_resume` | `orchestration` | Maps top-level resume CLI fields into headless resume execution. |
| `src-tauri/src/dispatch.rs::dispatch_interactive_top_level_resume` | `orchestration` | Maps top-level resume CLI fields into REPL execution. |
| `src-tauri/src/dispatch.rs::validate_top_level_resume_cli` | `validator` | Rejects incompatible top-level resume CLI usage. |
| `src-tauri/src/dispatch.rs::render_resume_error` | `formatter` | Formats and emits resume error. |
| `src-tauri/src/dispatch.rs::resume_session_mismatch_category` | `mapper` | Maps diagnostics category enum to its string representation. |
| `src-tauri/src/dispatch.rs::diagnose_execution_error` | `orchestration` | Builds diagnostic input and invokes diagnostics command. MULTI-CLASSIFIER-RISK: redaction mapping and diagnostic dispatch are inline. |

### `src-tauri/src/mailbox_delivery.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/mailbox_delivery.rs::prepare_headless_resume_delivery` | `orchestration` | Records runtime, reads pending rows, selects batch, formats prefix, and composes answer. MULTI-CLASSIFIER-RISK: access, filtering, formatting, and orchestration are inline. |
| `src-tauri/src/mailbox_delivery.rs::mark_headless_resume_delivered` | `orchestration` | Opens mailbox sidecar and marks selected rows delivered. |
| `src-tauri/src/mailbox_delivery.rs::record_headless_session_runtime` | `orchestration` | Converts resolved resume runtime data into a session-runtime upsert. MULTI-CLASSIFIER-RISK: path mapping and DB mutation are inline. |
| `src-tauri/src/mailbox_delivery.rs::select_batch` | `filter` | Selects a bounded pending-row batch. MULTI-CLASSIFIER-RISK: filtering depends on rendered prefix byte size. |
| `src-tauri/src/mailbox_delivery.rs::render_notification_prefix` | `formatter` | Renders mailbox rows as the resume notification envelope. |
| `src-tauri/src/mailbox_delivery.rs::compose_answer` | `formatter` | Formats notification prefix with optional user resume payload. |
| `src-tauri/src/mailbox_delivery.rs::quote_path` | `formatter` | Formats a sanitized path as quoted text. |
| `src-tauri/src/mailbox_delivery.rs::sanitize` | `formatter` | Escapes backslashes and newlines for notification display. |

### `src-tauri/src/main.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/main.rs::main` | `orchestration` | Delegates process entry to `process_entrypoint`. |
| `src-tauri/src/main.rs::process_entrypoint` | `orchestration` | Initializes tracing and selects GUI vs CLI entrypoint. MULTI-CLASSIFIER-RISK: setup and mode predicate dispatch are inline. |
| `src-tauri/src/main.rs::run_gui_entrypoint` | `orchestration` | Starts the Tauri GUI and returns success. |
| `src-tauri/src/main.rs::run_cli_entrypoint` | `orchestration` | Parses CLI, dispatches it, and maps result to process exit code. MULTI-CLASSIFIER-RISK: parsing, dispatch, and exit mapping are inline. |
| `src-tauri/src/main.rs::initialize_tracing` | `orchestration` | Configures global tracing subscriber. |
| `src-tauri/src/main.rs::should_run_gui` | `predicate` | Answers whether argv shape means GUI mode. |
| `src-tauri/src/main.rs::parse_cli` | `parser` | Parses normalized process args into `Cli`. |
| `src-tauri/src/main.rs::cli_args` | `accessor` | Retrieves process argv. |
| `src-tauri/src/main.rs::arg_count` | `accessor` | Retrieves argument iterator length. |
| `src-tauri/src/main.rs::cli_exit_code` | `mapper` | Maps `CliExit` to `ExitCode`, emitting errors when needed. MULTI-CLASSIFIER-RISK: mapping and formatting side effect are inline. |
| `src-tauri/src/main.rs::cli_exit` | `mapper` | Maps dispatch `Result<i32, String>` into `CliExit`. |
| `src-tauri/src/main.rs::emit_cli_error` | `formatter` | Emits a CLI error line to stderr. |

### `src-tauri/src/migration_providers.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/migration_providers.rs::provider_session_resolved_account` | `accessor` | Retrieves provider session-storage display account for a session. MULTI-CLASSIFIER-RISK: access and storage-variant mapping are split only by helper. |
| `src-tauri/src/migration_providers.rs::provider_session_storage` | `accessor` | Exposes optional provider session storage config. |
| `src-tauri/src/migration_providers.rs::resolved_account_from_session_storage` | `mapper` | Maps session storage variants into account/workspace display strings. |
| `src-tauri/src/migration_providers.rs::load_resume_execution_environment` | `orchestration` | Loads state DB, config root, providers, models, sessions, and packages environment. MULTI-CLASSIFIER-RISK: resource access and environment mapping are inline. |
| `src-tauri/src/migration_providers.rs::resume_execution_models_dir` | `mapper` | Chooses override models dir or default models dir. |
| `src-tauri/src/migration_providers.rs::resume_execution_environment` | `mapper` | Packages loaded pieces into `ResumeExecutionEnvironment`. |

### `src-tauri/src/run/balancing/finalization.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/run/balancing/finalization.rs::finalize_completed_attempt` | `orchestration` | Sequences terminal outcome, child supervision, capture, artifacts, lifecycle finalization, quota, output, and wake recheck. MULTI-CLASSIFIER-RISK: large control path mixes predicates, formatting, persistence, and wake coordination. |
| `src-tauri/src/run/balancing/finalization.rs::mark_balanced_attempt_idle` | `orchestration` | Delegates wake idle marking and warning emission. |
| `src-tauri/src/run/balancing/finalization.rs::mark_balanced_successful_attempt_idle_and_recheck` | `orchestration` | Delegates success idle marking and wake recheck. |
| `src-tauri/src/run/balancing/finalization.rs::record_returned_artifacts_for_completed_attempt` | `orchestration` | Delegates returned-artifact persistence for a completed attempt. |
| `src-tauri/src/run/balancing/finalization.rs::finalize_returned_artifacts_persist_failure` | `orchestration` | Emits artifact error, finalizes failure, emits envelope, and marks idle. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/balancing/finalization.rs::ingest_completed_attempt_session` | `orchestration` | Ingests session metadata and emits fallback known session ID. MULTI-CLASSIFIER-RISK: session ingestion and fallback selection are inline. |

### `src-tauri/src/run/balancing/orchestration.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/run/balancing/orchestration.rs::run_with_balancing` | `orchestration` | Loads balanced execution environment and delegates loop execution. |
| `src-tauri/src/run/balancing/orchestration.rs::run_with_balancing_environment` | `orchestration` | Owns balanced retry loop, routing, execution, zero-turn classification, terminal handling, and finalization. MULTI-CLASSIFIER-RISK: many decision, mapping, and formatting paths are inline. |
| `src-tauri/src/run/balancing/orchestration.rs::select_balanced_provider_index` | `orchestration` | Calls routing service and extracts selected provider index. MULTI-CLASSIFIER-RISK: service call and result mapping are inline. |
| `src-tauri/src/run/balancing/orchestration.rs::exhausted_attempt_reason` | `formatter` | Builds the exhausted-attempt reason string using routing error context. MULTI-CLASSIFIER-RISK: route probing and formatting are inline. |
| `src-tauri/src/run/balancing/orchestration.rs::finalize_spawn_error` | `orchestration` | Sequences spawn-error signal, terminal outcome, invocation finalization, envelope, and wake idle. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/balancing/orchestration.rs::spawn_error_signal` | `mapper` | Maps spawn-error input into a terminal signal. |
| `src-tauri/src/run/balancing/orchestration.rs::apply_spawn_error_terminal_outcome` | `orchestration` | Builds terminal context and applies the spawn-error terminal outcome. |
| `src-tauri/src/run/balancing/orchestration.rs::spawn_error_terminal_signal_context_ids` | `mapper` | Maps invocation ID into terminal signal context IDs. |
| `src-tauri/src/run/balancing/orchestration.rs::finalize_spawn_error_invocation` | `orchestration` | Finalizes invocation for spawn error and emits warning on failure. |
| `src-tauri/src/run/balancing/orchestration.rs::emit_spawn_error_envelope` | `formatter` | Emits spawn-error failure result envelope. |

### `src-tauri/src/run/repl/orchestration.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/run/repl/orchestration.rs::run_repl` | `orchestration` | Coordinates REPL preparation, provider selection, invocation start, binding, emission, execution, and finalization. MULTI-CLASSIFIER-RISK: large lifecycle combines validation, mapping, and orchestration. |
| `src-tauri/src/run/repl/orchestration.rs::prepare_repl_execution` | `orchestration` | Delegates REPL model and resume preparation. |
| `src-tauri/src/run/repl/orchestration.rs::repl_in_flight` | `mapper` | Constructs a quota in-flight tracker. |
| `src-tauri/src/run/repl/orchestration.rs::repl_balance_context` | `mapper` | Packages provider/session config and in-flight references into balance context. |
| `src-tauri/src/run/repl/orchestration.rs::repl_parent_invocation_id` | `accessor` | Retrieves parent invocation ID from the environment state DB. |
| `src-tauri/src/run/repl/orchestration.rs::repl_stderr_is_terminal` | `predicate` | Answers whether stderr is a terminal. |
| `src-tauri/src/run/repl/orchestration.rs::start_selected_repl_invocation` | `orchestration` | Delegates selected-provider invocation startup. |
| `src-tauri/src/run/repl/orchestration.rs::prepare_repl_model_and_resume` | `orchestration` | Loads environment, resolves optional resume, computes fallback target, and resolves model. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/repl/orchestration.rs::fallback_target_for_resume` | `mapper` | Maps an optional resolved resume into an optional execution target. |
| `src-tauri/src/run/repl/orchestration.rs::start_repl_invocation` | `orchestration` | Creates invocation ID, starts lifecycle row, and creates finalizer guard. |
| `src-tauri/src/run/repl/orchestration.rs::serialize_repl_invocation_env` | `formatter` | Serializes invocation ID as JSON for the environment payload. |
| `src-tauri/src/run/repl/orchestration.rs::emit_repl_invocation_line_if_needed` | `formatter` | Emits invocation marker only when the terminal policy says to emit. MULTI-CLASSIFIER-RISK: predicate check and formatting side effect are inline. |
| `src-tauri/src/run/repl/orchestration.rs::execute_and_finalize_repl_attempt` | `orchestration` | Executes interactive provider and routes success or spawn-error finalization. MULTI-CLASSIFIER-RISK: cwd mapping, resume payload mapping, classification, and finalization are inline. |
| `src-tauri/src/run/repl/orchestration.rs::repl_resume_payload` | `mapper` | Maps optional resume session ID into executor resume payload. |
| `src-tauri/src/run/repl/orchestration.rs::repl_execution_cwd` | `mapper` | Selects resume spawn cwd over working dir. |
| `src-tauri/src/run/repl/orchestration.rs::finalize_repl_execution_result` | `orchestration` | Applies terminal disposition and finalizes completed REPL execution. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/repl/orchestration.rs::finalize_completed_repl_execution` | `orchestration` | Adapts REPL execution data into completed REPL finalization. |
| `src-tauri/src/run/repl/orchestration.rs::finalize_repl_spawn_error` | `orchestration` | Delegates REPL spawn-error finalization. |
| `src-tauri/src/run/repl/orchestration.rs::repl_interactive_effective_cwd` | `mapper` | Selects explicit resume cwd or resolves effective spawn cwd. |
| `src-tauri/src/run/repl/orchestration.rs::clear_repl_session_capture_for_unpinned` | `orchestration` | Clears session capture when the REPL was not pinned by resume. MULTI-CLASSIFIER-RISK: predicate and mutation are inline. |
| `src-tauri/src/run/repl/orchestration.rs::resolve_repl_model` | `orchestration` | Selects fallback model or resolves direct/default model path. MULTI-CLASSIFIER-RISK: fallback selection and lookup are inline. |
| `src-tauri/src/run/repl/orchestration.rs::fallback_model` | `accessor` | Retrieves fallback model from a resolved target. |
| `src-tauri/src/run/repl/orchestration.rs::direct_or_default_repl_model` | `mapper` | Maps model source into provider-default or named-model lookup. |
| `src-tauri/src/run/repl/orchestration.rs::repl_model_source` | `mapper` | Maps inputs into provider-default or named model source. |
| `src-tauri/src/run/repl/orchestration.rs::required_repl_model_name` | `validator` | Requires model name when no resume fallback exists. |
| `src-tauri/src/run/repl/orchestration.rs::lookup_repl_model` | `accessor` | Retrieves a model by name from the model map. |
| `src-tauri/src/run/repl/orchestration.rs::unknown_repl_model_message` | `formatter` | Formats the unknown model error. |
| `src-tauri/src/run/repl/orchestration.rs::select_repl_provider` | `orchestration` | Chooses resume-provider or direct-provider selection. MULTI-CLASSIFIER-RISK: option state manipulation and dispatch are inline. |
| `src-tauri/src/run/repl/orchestration.rs::select_repl_resume_provider` | `orchestration` | Emits selected provider, validates target, runs migration, and returns selected tuple. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/repl/orchestration.rs::emit_selected_repl_resume_provider` | `formatter` | Conditionally emits selected resume provider line. |
| `src-tauri/src/run/repl/orchestration.rs::validate_repl_resume_target` | `validator` | Rejects resume targets whose provider lacks resume support. MULTI-CLASSIFIER-RISK: validation emits error text. |
| `src-tauri/src/run/repl/orchestration.rs::repl_resume_target_missing_resume` | `predicate` | Answers whether fallback target lacks resume config. |
| `src-tauri/src/run/repl/orchestration.rs::selected_repl_resume_provider_tuple` | `mapper` | Maps fallback target and resolved resume into selected-provider tuple. MULTI-CLASSIFIER-RISK: emits missing-resume error on invalid provider. |
| `src-tauri/src/run/repl/orchestration.rs::provider_missing_repl_resume_block` | `predicate` | Answers whether a provider lacks a resume block. |
| `src-tauri/src/run/repl/orchestration.rs::emit_repl_missing_resume_block` | `formatter` | Emits the missing resume-block error line. |
| `src-tauri/src/run/repl/orchestration.rs::selected_repl_provider_tuple` | `mapper` | Packages provider index, provider config, and active session ID. |
| `src-tauri/src/run/repl/orchestration.rs::migrate_repl_resume_provider` | `orchestration` | Prepares migration, dispatches migration service, applies result, and renders failures. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/repl/orchestration.rs::prepare_repl_migration` | `mapper` | Builds migration model/effective-cwd bundle. MULTI-CLASSIFIER-RISK: may fail while resolving cwd. |
| `src-tauri/src/run/repl/orchestration.rs::repl_migration_model` | `mapper` | Derives migration pool model for resolved resume. |
| `src-tauri/src/run/repl/orchestration.rs::repl_migration_effective_cwd` | `mapper` | Resolves effective cwd for REPL migration. |
| `src-tauri/src/run/repl/orchestration.rs::dispatch_repl_migration` | `orchestration` | Builds migration service request and invokes migration service. MULTI-CLASSIFIER-RISK: request mapping and service call are inline. |
| `src-tauri/src/run/repl/orchestration.rs::apply_repl_migrated_segment` | `orchestration` | Mutates resolved resume and recomputes fallback target after migration. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/repl/orchestration.rs::render_repl_rotation_failed` | `formatter` | Emits rotation failure and returns false. |
| `src-tauri/src/run/repl/orchestration.rs::render_repl_migration_dependency_failure` | `formatter` | Emits migration dependency failure and returns false. |
| `src-tauri/src/run/repl/orchestration.rs::select_repl_direct_provider` | `orchestration` | Routes direct provider selection and resolves effective provider config. MULTI-CLASSIFIER-RISK: service call and config resolution are inline. |
| `src-tauri/src/run/repl/orchestration.rs::invocation_model_name` | `mapper` | Chooses invocation model name from resolved resume, resume flag, or model. |
| `src-tauri/src/run/repl/orchestration.rs::bind_repl_resume_session` | `orchestration` | Records provider session binding and optional legacy resume input. MULTI-CLASSIFIER-RISK: predicate and mutation are inline. |
| `src-tauri/src/run/repl/orchestration.rs::should_record_repl_legacy_resume_input` | `predicate` | Answers whether legacy resume input should be recorded. |
| `src-tauri/src/run/repl/orchestration.rs::record_repl_legacy_resume_input` | `orchestration` | Persists legacy resume input session ID. |
| `src-tauri/src/run/repl/orchestration.rs::classify_repl_result` | `orchestration` | Applies terminal fixture override and zero-turn classification to result fields. MULTI-CLASSIFIER-RISK: classification and mutation are inline. |
| `src-tauri/src/run/repl/orchestration.rs::terminal_signal_disposition_for_result` | `orchestration` | Builds terminal signal context and applies terminal signal outcome. MULTI-CLASSIFIER-RISK: context mapping and outcome application are inline. |

### `src-tauri/src/run/resume/orchestration.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/run/resume/orchestration.rs::run_resume` | `orchestration` | Coordinates resume validation, auto-wake validation/reset, preparation, and resume loop. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::reject_invalid_resume_input` | `validator` | Validates resume UUID and returns rejection exit code. MULTI-CLASSIFIER-RISK: validation and error formatting are inline. |
| `src-tauri/src/run/resume/orchestration.rs::prepare_headless_resume_execution` | `orchestration` | Resolves answer, environment, resume target, cwd, parent invocation, retry budget, and mailbox delivery. MULTI-CLASSIFIER-RISK: broad preparation with mapping/access. |
| `src-tauri/src/run/resume/orchestration.rs::effective_resume_execution_cwd` | `mapper` | Resolves effective resume execution cwd from state/config and override. |
| `src-tauri/src/run/resume/orchestration.rs::headless_resume_retry_budget` | `mapper` | Derives retry budget from resolved model pool size. |
| `src-tauri/src/run/resume/orchestration.rs::run_resume_loop` | `orchestration` | Owns the headless resume retry loop. |
| `src-tauri/src/run/resume/orchestration.rs::resume_attempts_exhausted` | `predicate` | Answers whether attempts reached max attempts. |
| `src-tauri/src/run/resume/orchestration.rs::resume_attempts_exhausted_exit_code` | `formatter` | Emits exhaustion marker and returns normalized exit code. MULTI-CLASSIFIER-RISK: formatting and exit-code mapping are inline. |
| `src-tauri/src/run/resume/orchestration.rs::normalized_resume_exhausted_exit_code` | `mapper` | Maps zero last-exit to failure exit code. |
| `src-tauri/src/run/resume/orchestration.rs::run_resume_attempt` | `orchestration` | Prepares target, starts invocation, binds session, executes, classifies, records, and handles terminal signal. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::resume_attempt_strategy` | `validator` | Requires provider resume strategy. |
| `src-tauri/src/run/resume/orchestration.rs::missing_resume_block_exit_code` | `formatter` | Emits missing resume-block message and returns failure code. |
| `src-tauri/src/run/resume/orchestration.rs::execute_resume_attempt_command` | `orchestration` | Maps attempt fields into executor resume call and invokes it. MULTI-CLASSIFIER-RISK: mapping and execution are inline. |
| `src-tauri/src/run/resume/orchestration.rs::finalize_resume_spawn_error` | `orchestration` | Finalizes spawn-error invocation, marks guard finalized, and marks session idle. |
| `src-tauri/src/run/resume/orchestration.rs::prepare_resume_attempt_target` | `orchestration` | Builds renderable target and applies migration before execution. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::start_resume_invocation` | `orchestration` | Creates invocation ID/start row/finalizer guard for resume attempt. |
| `src-tauri/src/run/resume/orchestration.rs::bind_resume_attempt_session` | `orchestration` | Binds invocation to provider session and optional legacy resume input. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::should_record_legacy_resume_input` | `predicate` | Answers whether manual migration requires legacy input recording. |
| `src-tauri/src/run/resume/orchestration.rs::record_legacy_resume_input_session_id` | `orchestration` | Persists legacy resume input session ID. |
| `src-tauri/src/run/resume/orchestration.rs::resume_invocation_env` | `formatter` | Serializes invocation ID JSON for environment payload. |
| `src-tauri/src/run/resume/orchestration.rs::apply_resume_attempt_classification` | `orchestration` | Applies terminal fixture override, zero-turn classification, and next-action selection. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::record_resume_acceptance_if_present` | `orchestration` | Conditionally delegates resume-acceptance persistence. |
| `src-tauri/src/run/resume/orchestration.rs::resume_acceptance_result` | `accessor` | Retrieves optional resume acceptance from execution result. |
| `src-tauri/src/run/resume/orchestration.rs::record_resume_acceptance` | `orchestration` | Invokes resume service to persist acceptance. |
| `src-tauri/src/run/resume/orchestration.rs::format_resume_acceptance_service_error` | `formatter` | Formats resume acceptance service failure. |
| `src-tauri/src/run/resume/orchestration.rs::handle_resume_attempt_terminal_signal` | `orchestration` | Applies terminal disposition, finalizes attempt, handles idle, mailbox delivery, and wake recheck. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::mark_resume_attempt_idle` | `orchestration` | Delegates wake idle marking for a resume attempt. |
| `src-tauri/src/run/resume/orchestration.rs::resolve_resume_for_headless_execution` | `orchestration` | Calls resume service and delegates output handling. |
| `src-tauri/src/run/resume/orchestration.rs::headless_resume_resolution_result` | `orchestration` | Converts resume-service output into resolved resume or emitted failure. MULTI-CLASSIFIER-RISK: output mapping and formatting are inline. |
| `src-tauri/src/run/resume/orchestration.rs::render_resume_error` | `formatter` | Emits formatted resume error. |
| `src-tauri/src/run/resume/orchestration.rs::render_resume_service_failure` | `formatter` | Emits resume service failure. |
| `src-tauri/src/run/resume/orchestration.rs::prepare_initial_headless_resume_target` | `orchestration` | Builds initial target, emits short line, and validates resume support. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::render_resume_short_line_if_needed` | `formatter` | Conditionally emits selected-provider resume line. |
| `src-tauri/src/run/resume/orchestration.rs::validate_headless_resume_target` | `validator` | Requires resume support on the target provider. |
| `src-tauri/src/run/resume/orchestration.rs::headless_resume_target_has_resume` | `predicate` | Answers whether target provider has resume config. |
| `src-tauri/src/run/resume/orchestration.rs::headless_missing_resume_block_exit_code` | `formatter` | Emits missing resume-block message and returns failure code. |
| `src-tauri/src/run/resume/orchestration.rs::migrate_resume_target` | `orchestration` | Builds migration model, dispatches migration, applies migrated segment, or emits failure. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::migration_model_for_attempt` | `mapper` | Derives migration model and applies candidate filtering. MULTI-CLASSIFIER-RISK: mapping and filtering are inline. |
| `src-tauri/src/run/resume/orchestration.rs::apply_migration_candidate_filter` | `filter` | Conditionally removes migration candidates based on attempt/manual state. MULTI-CLASSIFIER-RISK: predicate and filter are inline. |
| `src-tauri/src/run/resume/orchestration.rs::migration_model_pool` | `mapper` | Derives migration pool model from resolved resume. |
| `src-tauri/src/run/resume/orchestration.rs::filter_quota_exhausted_migration_model` | `filter` | Removes quota-exhausted migration candidates. |
| `src-tauri/src/run/resume/orchestration.rs::should_filter_migration_candidates` | `predicate` | Answers whether migration candidate filtering should apply. |
| `src-tauri/src/run/resume/orchestration.rs::dispatch_resume_migration` | `orchestration` | Maps resume migration inputs into a service request and invokes migration. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::apply_migrated_segment` | `orchestration` | Mutates resolved active segment and recomputes execution target. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::fail_rotation` | `formatter` | Emits rotation failure and returns failure code. |
| `src-tauri/src/run/resume/orchestration.rs::fail_migration_service` | `orchestration` | Classifies, emits, and returns migration service failure. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::migration_service_error_message` | `mapper` | Maps service error into dependency-or-service error message variant. |
| `src-tauri/src/run/resume/orchestration.rs::emit_migration_service_error` | `formatter` | Emits dependency or service migration failure. |
| `src-tauri/src/run/resume/orchestration.rs::terminal_signal_disposition_for_result` | `orchestration` | Builds terminal signal context and derives terminal disposition. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/run/resume/orchestration.rs::resume_terminal_signal_disposition` | `orchestration` | Confirms maybe-quota signal or applies regular terminal outcome. MULTI-CLASSIFIER-RISK: predicate branch and outcome application are inline. |
| `src-tauri/src/run/resume/orchestration.rs::confirmed_maybe_quota_signal` | `predicate` | Answers with a signal only when zero-turn confirmed maybe-quota applies. |
| `src-tauri/src/run/resume/orchestration.rs::apply_resume_terminal_signal_outcome` | `orchestration` | Delegates terminal signal outcome application. |

### `src-tauri/src/usage/cli.rs`

No production functions; this file declares the clap-derived CLI schema.

### `src-tauri/src/wake_coordinator.rs`

| Function | A1 classification | Justification |
|---|---|---|
| `src-tauri/src/wake_coordinator.rs::WakeDiagnostic::status` | `mapper` | Builds a diagnostic struct from a status token. |
| `src-tauri/src/wake_coordinator.rs::WakeDiagnostic::with_message` | `mapper` | Builds a diagnostic struct from status and message. |
| `src-tauri/src/wake_coordinator.rs::mark_session_idle_after_turn` | `orchestration` | Opens mailbox sidecar and marks a session idle after a turn. |
| `src-tauri/src/wake_coordinator.rs::trigger_notify_wake` | `orchestration` | Starts wake chain with notify-specific defaults. |
| `src-tauri/src/wake_coordinator.rs::mark_successful_turn_idle_and_recheck` | `orchestration` | Marks session idle then triggers turn-end wake recheck. MULTI-CLASSIFIER-RISK: state mutation and wake decision are inline. |
| `src-tauri/src/wake_coordinator.rs::validate_auto_wake_child` | `validator` | Validates auto-wake env/session/token and wake claim before child execution. MULTI-CLASSIFIER-RISK: env parsing and DB validation are inline. |
| `src-tauri/src/wake_coordinator.rs::is_auto_wake_invocation` | `predicate` | Answers whether the auto-wake marker env var is present. |
| `src-tauri/src/wake_coordinator.rs::reset_manual_resume_wake_claim` | `orchestration` | Opens sidecar and releases any manual resume wake claim. |
| `src-tauri/src/wake_coordinator.rs::release_current_auto_wake_claim_for_session` | `orchestration` | Reads current auto-wake env and delegates claim release. |
| `src-tauri/src/wake_coordinator.rs::trigger_turn_end_recheck` | `orchestration` | Checks pending rows, auto-wake count, cap, claim release, and wake spawn. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/wake_coordinator.rs::start_wake_chain` | `orchestration` | Opens sidecar, checks runtime, acquires claim, spawns detached resume, records PID, and handles release on error. MULTI-CLASSIFIER-RISK. |
| `src-tauri/src/wake_coordinator.rs::spawn_detached_resume` | `orchestration` | Builds and spawns detached resume command with auto-wake environment. MULTI-CLASSIFIER-RISK: command mapping and process launch are inline. |
| `src-tauri/src/wake_coordinator.rs::configure_detached [cfg(unix)]` | `orchestration` | Applies Unix `setsid` setup before detached child spawn. |
| `src-tauri/src/wake_coordinator.rs::configure_detached [cfg(not(unix))]` | `orchestration` | No-op detached setup on non-Unix platforms. |
| `src-tauri/src/wake_coordinator.rs::pending_count` | `accessor` | Reads count of pending mailbox rows for a session. |
| `src-tauri/src/wake_coordinator.rs::current_auto_wake` | `accessor` | Reads current auto-wake token/count env values. MULTI-CLASSIFIER-RISK: env access and count parsing/defaulting are inline. |
| `src-tauri/src/wake_coordinator.rs::auto_wake_max` | `accessor` | Reads configured auto-wake max from env or default. MULTI-CLASSIFIER-RISK: env parsing and positive-value validation are inline. |
| `src-tauri/src/wake_coordinator.rs::release_current_auto_wake_claim` | `orchestration` | Opens sidecar and releases a token-guarded wake claim when auto-wake context exists. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-state/src/lib.rs
    role: adapter
    Translates:
      - oulipoly-state public crate re-export contract
      - versioned state DB public API contract
      - PID identity and mailbox sidecar public API contract
  - component: crates/oulipoly-runtime/src/executor/cli.rs
    role: adapter
    Translates:
      - executor public entrypoint contract
      - executor CLI sibling component-set contract
      - executor CLI test fixture contract
      - tempfile Unix permissions test contract
  - component: src-tauri/src/commands/notify.rs
    role: adapter
    Translates:
      - agent-bash async spooler completion contract: meta.json caller_chain, state_dir, log path, rc path, stable handle
      - PID identity sidecar ownership-resolution contract
      - read-only invocation session fallback contract
      - mailbox enqueue and idempotency contract
      - Oulipoly notify CLI JSON/human response and wake diagnostic contract
  - component: src-tauri/src/mailbox_delivery.rs
    role: adapter
    Translates:
      - mailbox row contract for agent_bash_complete notifications
      - provider resume prompt notification-envelope contract
  - component: src-tauri/src/commands/mod.rs
    role: adapter
    Translates:
      - CLI command module registry contract
      - dispatch-visible command handler namespace contract
  - component: src-tauri/src/commands/pid_session.rs
    role: adapter
    Translates:
      - PID identity sidecar query contract
      - read-only invocation session fallback contract
      - pid-session CLI JSON/human response contract
  - component: src-tauri/src/usage/cli.rs
    role: adapter
    Translates:
      - clap-derived process argv contract to public oulipoly-agent-runner CLI command surface
  - component: src-tauri/src/main.rs
    role: adapter
    Translates:
      - OS process argv and exit status contract to CLI dispatch invocation
  - component: src-tauri/src/dispatch.rs
    role: adapter
    Translates:
      - clap-derived CLI command enum contract
      - internal command handler invocation contract
      - run/resume/repl entrypoint contract
      - diagnostics, schema-probe, trace, and migration command contract
      - process exit-code contract
  - component: src-tauri/src/run/resume/orchestration.rs
    role: adapter
    Translates:
      - headless resume lifecycle contract
      - mailbox delivery lifecycle contract
      - auto-wake lifecycle hook contract
      - terminal signal disposition contract
      - resume migration service contract
  - component: src-tauri/src/migration_providers.rs
    role: adapter
    Translates:
      - configured provider/model loading contract
      - provider session storage display contract
      - resume execution environment contract
  - component: crates/oulipoly-runtime/src/executor/cli/interactive.rs
    role: adapter
    Translates:
      - provider interactive launch contract
      - terminal status to interactive execution result contract
      - Unix interactive signal-guard callsite contract
  - component: crates/oulipoly-runtime/src/executor/cli/headless.rs
    role: adapter
    Translates:
      - executor public entrypoint contract
      - effective execute request contract
      - execution result contract
  - component: crates/oulipoly-runtime/src/executor/cli/resume_execution.rs
    role: adapter
    Translates:
      - executor resume entrypoint contract
      - resume payload contract
      - resume acceptance contract
      - execution result contract
  - component: crates/oulipoly-runtime/src/executor/cli/provider_execution.rs
    role: adapter
    Translates:
      - provider launch contract
      - provider supervisor output contract
      - return-channel IPC contract
      - raw execution result contract
  - component: crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs
    role: adapter
    Translates:
      - std process child lifecycle contract
      - stdio pipe drain contract
      - Unix process group contract
      - terminal signal classification contract
      - provider live terminal signal contract
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-state/src/pid_identity.rs
    role: intrinsic-surface
    Domain: pid_identity_sidecar
    Owns:
      - pid-identity.db path and PID identity schema
      - pid_identity row identity key: os_pid, os_boot_id, os_pid_starttime_ticks
      - Linux /proc/<pid>/stat starttime read and parse
      - Linux /proc/sys/kernel/random/boot_id read
      - Unix getpgid process group read
      - live process identity lookup and PID reuse guard
  - component: crates/oulipoly-state/src/mailbox.rs
    role: intrinsic-surface
    Domain: mailbox_sidecar
    Owns:
      - mailbox table and agent_bash_complete enqueue idempotency
      - session_runtime table and running/idle state transitions
      - session_wake_claim table and wake-claim acquire, renew, release, validate operations
      - mailbox pending/all listing and delivered-row marking
      - sidecar-only schema evolution outside versioned state.db
  - component: crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs
    role: intrinsic-surface
    Domain: child_spawn_identity_capture
    Owns:
      - child PID identity capture after provider spawn
      - parent invocation env to spawn identity context mapping
      - spawn runtime mode tokens for headless and PTY interactive executions
      - session_runtime running update from recorded child identity
  - component: src-tauri/src/wake_coordinator.rs
    role: intrinsic-surface
    Domain: auto_wake_lifecycle
    Owns:
      - OULIPOLY_AUTO_WAKE environment variable family
      - detached resume wake process launch and setsid detachment
      - auto-wake cap, claim renewal, claim release, and child claim validation
      - notify-idle and turn-end wake recheck decisions
```

All other touched files: no intrinsic-surface declaration.
