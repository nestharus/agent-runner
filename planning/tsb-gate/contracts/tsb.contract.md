# tsb Step-6a Contract

## Component declared roles

Component: turn-scan bounding and script deadline safety.

Declared roles: `orchestration`, `parser`, `mapper`, `filter`, `validator`, `predicate`, `accessor`, `formatter`.

Touched files in scope:

| File | Declared roles | Role notes |
|---|---|---|
| `scripts/opencode-turns` | `orchestration`, `parser`, `mapper`, `filter`, `validator`, `predicate`, `accessor`, `formatter` | Shipped OpenCode turn adapter; translates public OpenCode CLI output into normalized runtime JSONL and owns its OPENCODE_TURNS option parsing. |
| `scripts/tests/opencode-turns.test.sh` | `orchestration`, `validator`, `formatter` | Shell proof harness that builds mock OpenCode CLIs, runs the adapter, and asserts stdout/export/deadline behavior. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `orchestration`, `accessor`, `formatter`, `mapper`, `predicate`, `validator` | Quota/auth script process execution, timeout, stream draining, process-group kill, and error formatting. File-local comments currently omit `validator`; the `ensure_*_success` functions are validator work. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `orchestration`, `accessor`, `filter`, `parser`, `validator`, `mapper`, `formatter`, `predicate` | Session adapter stdout ingestion, degraded-marker recognition, session script execution, transcript locator execution, and StateDb ingest. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `orchestration`, `accessor`, `filter`, `parser`, `validator`, `mapper`, `formatter`, `predicate` | Integration fixture and proof surface for provider/session dispatch and the OpenCode adapter path. |

## Production function inventory

Only added or meaningfully changed production functions are listed for Rust files. The Python adapter is shipped production tooling, so its full function inventory is declared.

### `crates/oulipoly-runtime/src/quota/process.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `run_script` | `orchestration` | Public quota-script entry point that delegates to the bounded runner. | None. |
| `run_script_with_timeout` | `orchestration` | Spawns quota script, drains stdout/stderr, waits with supplied timeout, validates exit, and returns stdout. | None; sequencing is thin around named helpers. |
| `shell_command` | `mapper` | Maps a shell command string into a configured `Command`. | None. |
| `configure_script_process_group` | `mapper` | Adds process-group configuration to the child command on Unix; no-op elsewhere. | None. |
| `kill_timed_out_child` | `orchestration` | Applies timeout kill policy and returns a formatted timeout error. | None. |
| `kill_child_process_group` | `orchestration` | Kills the configured child process group, then waits for the child. | None. |
| `format_timeout` | `formatter` | Formats auth/quota timeout messages, including `script_timeout` for quota scripts. | None. |

### `crates/oulipoly-runtime/src/sessions/mod.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `scan_provider` | `orchestration` | Public scan entry point that applies the default session-script timeout. | None. |
| `scan_provider_with_timeout` | `orchestration` | Resolves session source/state dir, runs turn script with a supplied timeout, collects/persists turns, and returns a `ScanReport`. | None; orchestration over named helpers. |
| `collect_turn_script_batch` | `orchestration` | Iterates script lines, records degraded markers or parse errors, and accumulates valid ingest rows. | None; parser/validator work is delegated. |
| `degraded_marker_error` | `orchestration` | Thinly composes degraded-marker parsing, predicate, count access, and error formatting. | None; delegated to single-role helpers. |
| `parse_degraded_marker_jsonl` | `parser` | Parses one JSONL line into a JSON value when possible. | None. |
| `is_degraded_marker` | `predicate` | Answers whether a parsed JSON value is the degraded marker. | None. |
| `degraded_marker_count` | `accessor` | Reads the optional degraded marker count, defaulting to zero. | None. |
| `format_degraded_marker_error` | `formatter` | Formats the runtime error text for a degraded marker count. | None. |
| `non_empty_script_lines` | `orchestration` | Composes script stdout line trimming and non-empty line selection through named helpers. | None. |
| `trimmed_script_lines` | `mapper` | Maps raw stdout lines into trimmed line values. | None. |
| `non_empty_trimmed_lines` | `filter` | Selects non-empty trimmed script lines. | None. |
| `run_turn_script` | `orchestration` | Runs a provider turn script with caller-supplied timeout. | None. |
| `run_session_script` | `orchestration` | Runs a session script with the default runtime deadline. | None. |
| `run_session_script_with_timeout` | `orchestration` | Spawns session script, drains stdout/stderr, waits with supplied timeout, validates exit, and returns stdout. | None; sequencing is thin around named helpers. |
| `session_script_command` | `mapper` | Maps script/state/session inputs to a configured child `Command`. | None. |
| `configure_session_script_process_group` | `mapper` | Adds process-group configuration to session script commands on Unix; no-op elsewhere. | None. |
| `spawn_script_reader` | `orchestration` | Spawns a reader-drain thread and delegates stream draining to a named helper. | None. |
| `drain_script_reader_to_string` | `accessor` | Reads a script stdout/stderr stream into a string. | None. |
| `wait_for_session_script` | `orchestration` | Polls child process until completion, timeout, or wait error. | None. |
| `wait_for_pending_session_script` | `orchestration` | Applies pending-state timeout/sleep behavior through named timeout helpers. | None; predicate, kill action, and message formatting are delegated. |
| `pending_session_script_timed_out` | `predicate` | Answers whether the pending session-script wait exceeded its deadline. | None. |
| `fail_timed_out_pending_session_script` | `orchestration` | Sequences timeout kill and timeout-result construction through named helpers. | None; thin composition over single-role helpers. |
| `kill_timed_out_pending_session_script` | `orchestration` | Applies session script timeout kill policy. | None. |
| `kill_session_script_process_group` | `orchestration` | Kills the configured session-script process group, then waits for the child. | None. |
| `format_pending_session_script_timeout` | `formatter` | Formats session-script timeout messages with the `script_timeout` token. | None. |

### `scripts/opencode-turns`

| Function or method | A1 class | Meaning | Risk |
|---|---|---|---|
| `Options.__init__` | `orchestration` | Reads and assembles adapter option values from environment helpers. | None. |
| `Deadline.__init__` | `mapper` | Builds deadline state from a duration. | None. |
| `Deadline.remaining` | `accessor` | Exposes remaining wall-clock budget. | None. |
| `Deadline.call_timeout` | `filter` | Bounds a per-call timeout by remaining deadline. | None. |
| `env_float` | `parser` | Parses a positive float env value with fallback default. | None. |
| `env_int` | `parser` | Parses a positive integer env value with fallback default. | None. |
| `text_chunk` | `mapper` | Maps text into the canonical body chunk shape. | None. |
| `canonical_chunk_type` | `mapper` | Maps OpenCode/native chunk type names to canonical chunk type names. | None. |
| `extract_content_chunks` | `orchestration` | Dispatches content extraction by value shape to single-role chunk helpers. | None; recursion and per-shape parsing/mapping are delegated. |
| `content_chunks_from_text` | `mapper` | Maps a plain string content value into one canonical text chunk. | None. |
| `content_chunks_from_items` | `orchestration` | Recursively accumulates chunks from a list of content values. | None. |
| `content_chunks_from_obj` | `orchestration` | Applies direct-object chunk parsing before nested-object recursion. | None; both branches are delegated. |
| `direct_content_chunks_from_obj` | `parser` | Parses one text-bearing content dictionary into a canonical chunk shape. | None; type canonicalization is delegated. |
| `nested_content_chunks_from_obj` | `orchestration` | Tries known nested content fields and returns the first non-empty recursive chunk result. | None. |
| `unique_values` | `filter` | Deduplicates values while preserving order. | None. |
| `session_ids_from_value` | `parser` | Recursively extracts `ses_*` identifiers from arbitrary values. | None. |
| `parse_session_list_stdout` | `orchestration` | Coordinates session-list JSON parsing, candidate extraction, and fallback ID selection. | None; parse, recent-window filtering, and cap filtering are delegated. |
| `parse_session_list_json` | `parser` | Parses OpenCode session-list stdout as JSON or returns the parse-failed sentinel. | None. |
| `capped_session_ids_from_value` | `filter` | Extracts unique session IDs from a value and applies the max-session cap. | None. |
| `session_ids_from_candidates` | `orchestration` | Selects timestamp-window filtering or max-cap filtering for candidate rows. | None; predicate and filters are delegated. |
| `candidates_have_timestamps` | `predicate` | Answers whether any session candidate has a parsed timestamp. | None. |
| `recent_session_ids` | `filter` | Filters timestamped candidates to the recent quota-balancing window. | None. |
| `capped_candidate_session_ids` | `filter` | Applies the max-session cap to candidate session IDs. | None. |
| `unique_session_candidates` | `filter` | Deduplicates candidate session dictionaries by `session_id`. | None. |
| `session_candidates_from_value` | `orchestration` | Dispatches recursive candidate extraction by parsed value shape. | None; candidate-shape predicate and mapping are delegated. |
| `session_candidates_from_items` | `orchestration` | Recursively accumulates candidate rows from iterable parsed values. | None. |
| `session_candidates_from_obj` | `orchestration` | Routes one dictionary either to candidate mapping or recursive value traversal. | None; shape predicate and mapping are delegated. |
| `has_session_candidate_shape` | `predicate` | Answers whether a dictionary has a recognizable OpenCode session ID field. | None. |
| `session_candidate_from_obj` | `mapper` | Maps a recognized session-list object into a session candidate row. | None; field parsing is delegated. |
| `session_list_session_id` | `parser` | Extracts a session ID from known OpenCode session-list fields. | None. |
| `session_list_timestamp` | `parser` | Extracts a session timestamp from known OpenCode session-list fields. | None. |
| `timestamp_datetime` | `parser` | Parses string or numeric timestamps into UTC datetimes. | None. |
| `numeric_timestamp_datetime` | `parser` | Parses seconds/milliseconds numeric timestamps into UTC datetimes. | None. |
| `discover_session_ids` | `orchestration` | Runs public OpenCode session discovery and parses IDs unless the call times out. | None. |
| `requested_session_ids` | `orchestration` | Selects explicit session IDs or implicit discovery. | None. |
| `numeric_timestamp` | `formatter` | Formats numeric timestamps as RFC3339-like UTC strings for emitted turns. | None. |
| `timestamp_from_obj` | `parser` | Extracts a turn timestamp from exported OpenCode message objects. | None. |
| `session_id_from_obj` | `parser` | Extracts a session ID from exported OpenCode message objects. | None. |
| `role_from_obj` | `parser` | Extracts `user` or `assistant` role from message objects. | None. |
| `turn_id_from_obj` | `orchestration` | Selects a parsed turn ID or deterministic fallback ID. | None; field parsing and fallback formatting are delegated. |
| `turn_id_field_from_obj` | `parser` | Extracts a turn ID from top-level or nested message objects. | None. |
| `turn_id_field_from_mapping` | `parser` | Extracts a turn ID from known ID fields on one mapping. | None. |
| `fallback_turn_id` | `formatter` | Formats the deterministic fallback turn ID from session ID and index. | None. |
| `opencode_command` | `parser` | Parses `OPENCODE_BIN` into argv tokens with default fallback. | None. |
| `run_opencode` | `orchestration` | Sequences OpenCode CLI spawn, communicate, timeout handling, and result selection. | None; deadline predicate, process spawn/wait, kill action, and result construction are delegated. |
| `opencode_deadline_expired` | `predicate` | Answers whether the remaining deadline leaves no call budget. | None. |
| `spawn_opencode_process` | `orchestration` | Starts the OpenCode subprocess with bounded stdio/session settings. | None. |
| `communicate_opencode_process` | `orchestration` | Waits for one OpenCode subprocess call and reports stdout or timeout status. | None. |
| `opencode_process_failed` | `predicate` | Answers whether the completed OpenCode subprocess exited non-zero. | None. |
| `degraded_opencode_result` | `mapper` | Constructs the degraded result tuple used for timeout paths. | None. |
| `failed_opencode_result` | `mapper` | Constructs the non-degraded empty result tuple used for failed CLI calls. | None. |
| `successful_opencode_result` | `mapper` | Constructs the successful stdout result tuple. | None. |
| `kill_process_group` | `orchestration` | Kills an OpenCode subprocess group and drains it. | None. |
| `parse_export_stdout` | `parser` | Parses OpenCode export stdout as JSON. | None. |
| `export_session` | `orchestration` | Runs `opencode export <session>` and parses output unless the call times out. | None. |
| `exported_message_items` | `orchestration` | Coordinates supported export-shape extraction and dictionary filtering. | None; shape parsing and dict filtering are delegated. |
| `exported_message_item_values` | `parser` | Extracts raw message item lists from supported OpenCode export shapes. | None. |
| `dict_items` | `filter` | Keeps only dictionary items from a raw message item list. | None. |
| `record_from_message` | `orchestration` | Coordinates required-field validation, turn-ID selection, record mapping, and optional body attachment. | None; validation, fallback ID formatting, and record mapping are delegated. |
| `message_record_fields` | `parser` | Extracts normalized required record fields from one exported message object. | None. |
| `has_required_message_record_fields` | `validator` | Validates that session ID, role, and timestamp are present before emission. | None. |
| `message_record_from_fields` | `mapper` | Maps validated fields and turn ID into the normalized record base. | None. |
| `record_with_optional_body` | `mapper` | Maps a base normalized record plus optional body chunks into the emitted record shape. | None. |
| `message_body_chunks` | `parser` | Extracts optional normalized body chunks from supported message content fields. | None. |
| `records_from_exported_session` | `mapper` | Maps all exported message items into normalized records. | None. |
| `collect_records` | `orchestration` | Iterates sessions, exports each one, stops on timeout, and returns records plus degraded state. | None. |
| `emit_record` | `formatter` | Emits one compact JSONL record. | None. |
| `assistant_record_count` | `filter` | Counts collected assistant records for degraded-marker reporting. | None. |
| `emit_degraded_marker` | `formatter` | Emits compact JSONL degraded marker from a supplied assistant-turn count. | None. |
| `has_base_dir_arg` | `validator` | Validates that argv includes the compatibility base-dir argument. | None. |
| `usage_message` | `formatter` | Formats the adapter CLI usage message. | None. |
| `emit_usage` | `formatter` | Emits the usage message to the supplied stream. | None. |
| `session_args_from_argv` | `accessor` | Exposes explicit session ID arguments from argv after the base-dir slot. | None. |
| `main` | `orchestration` | CLI entry point that delegates argv boundary handling, constructs options/deadline, collects records, emits records, and emits degraded marker when needed. | None. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: scripts/opencode-turns
    role: adapter
    Translates:
      - OpenCode public CLI surface (`opencode session list --json`, `opencode export <sessionID>`)
      - Oulipoly session turn JSONL contract (`session_id`, `turn_id`, `timestamp`, `role`, optional `body`)
      - Oulipoly degraded turn-scan marker contract (`degraded: true`, `count`)
      - Python stdlib process/time surface (`subprocess`, `signal`, `os`, `time`, `datetime`)
      - Python stdlib data/argv parsing surface (`json`, `re`, `shlex`, `sys`)
  - component: crates/oulipoly-runtime/src/sessions/mod.rs
    role: adapter
    Translates:
      - user-configured session script stdout/stderr/exit contract
      - Oulipoly session turn JSONL contract
      - Oulipoly StateDb session-turn ingest contract
  - component: crates/oulipoly-runtime/src/quota/process.rs
    role: adapter
    Translates:
      - user-configured quota/auth shell command stdout/stderr/exit contract
      - std process execution contract (`std::process::Command`, `Child`, `ExitStatus`, `Stdio`)
      - std concurrent stream draining contract (`std::io::Read`, `std::thread`, `JoinHandle`)
```

Runtime-side deadline owner note: `crates/oulipoly-runtime/src/sessions/mod.rs` and `crates/oulipoly-runtime/src/quota/process.rs` own the script-execution deadline contract for scripts they spawn, including timeout token formatting and process-group kill behavior. `scripts/opencode-turns` owns only its internal OpenCode CLI call deadline/degraded-marker behavior.

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: scripts/opencode-turns
    role: intrinsic-surface
    Domain: opencode_turns_adapter_runtime
    Owns:
      - OPENCODE_TURNS_WINDOW_HOURS
      - OPENCODE_TURNS_MAX_SESSIONS
      - OPENCODE_TURNS_CALL_TIMEOUT
      - OPENCODE_TURNS_DEADLINE
      - Python stdlib process spawn contract (`subprocess.Popen`, `subprocess.PIPE`, `subprocess.DEVNULL`, `subprocess.TimeoutExpired`)
      - Python stdlib process-group kill contract (`os.killpg`, `signal.SIGKILL`, fallback `process.kill`)
      - Python stdlib deadline and timestamp contract (`time.monotonic`, `datetime`, `timedelta`, `timezone`)
      - Python stdlib data/argv parsing contract (`json`, `re`, `shlex`, `sys`)
  - component: crates/oulipoly-runtime/src/sessions/mod.rs
    role: intrinsic-surface
    Domain: session_script_execution_deadline
    Owns:
      - SCRIPT_TIMEOUT_SECS for session turn and transcript locator scripts
      - run_session_script_with_timeout timeout_secs contract
      - script_timeout error token for session scripts
      - process-group kill on session script timeout
      - degraded marker recognition in turn-script stdout
  - component: crates/oulipoly-runtime/src/quota/process.rs
    role: intrinsic-surface
    Domain: quota_script_execution_deadline
    Owns:
      - SCRIPT_TIMEOUT_SECS for quota scripts
      - run_script_with_timeout timeout_secs contract
      - script_timeout error token for quota scripts
      - process-group kill on quota script timeout
```

No other intrinsic surfaces are declared for this gate.

## Test-harness declarations

```yaml
test_harness_declarations:
  - component: crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs
    role: test-harness
    Surface:
      - runtime provider/session dispatch integration-test surface
      - oulipoly-config model/provider/session config fixture surface
      - oulipoly-provider client/options/session contract surface
      - oulipoly-state StateDb fixture and SQLite snapshot surface
      - rusqlite query/assertion surface
      - serde_json envelope/request/response fixture surface
      - std filesystem/env/temp-path fixture surface
  - component: scripts/tests/opencode-turns.test.sh
    role: test-harness
    Surface:
      - scripts/opencode-turns adapter invocation surface
      - mock OpenCode CLI shell surface
      - OPENCODE_TURNS env option surface
      - stdout/stderr/export-log assertion surface
      - Python JSON/datetime helper fixture surface
      - shell process deadline and descendant-marker proof surface
```
