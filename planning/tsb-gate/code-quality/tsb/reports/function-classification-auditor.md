# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `planning_dir=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate`
- `wu_id=tsb`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/function-classification-auditor.md`
- `mode=phase-6`

## References Read

- `/home/nes/ai/conventions/code-quality.md`: A1 source of truth. Confirmed `orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, and `parser` category list at lines 60-69; single-classification rule at lines 54-58; touched-file ownership at lines 143-149; `Function categories per function` threshold row at lines 295-300; `multi-classifier function` failure mode at lines 304-310.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md`: Phase 6 contract. Confirmed component declared roles and touched files at lines 3-18, production function inventory at lines 19-131, adapter declarations at lines 133-157, intrinsic-surface declarations at lines 159-188.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md`: proposal context and proof claims at lines 1-69.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch`: touched-file and changed-body evidence.
- `/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt`: touched-file list.
- Source files under `worktree_path`: `crates/oulipoly-runtime/src/quota/process.rs`, `crates/oulipoly-runtime/src/sessions/mod.rs`, `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs`, `scripts/opencode-turns`, `scripts/tests/opencode-turns.test.sh`.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_refresh_command` | 36-42 | `orchestration` | LOW | Sequences spawn, drain, wait, join, and success validation through named helpers. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_script` | 44-46 | `orchestration` | LOW | Delegates to timeout-aware runner with the configured quota timeout. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_script_with_timeout` | 48-62 | `orchestration` | LOW | Sequences quota script spawn, stream drains, bounded wait, output joins, and success validation through helpers. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_refresh_command` | 64-69 | `orchestration` | LOW | Configures a command via helper and spawns it for auth refresh. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_quota_script` | 71-76 | `orchestration` | LOW | Configures a command via helper and spawns it for quota. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `shell_command` | 78-84 | `mapper` | LOW | Maps shell text into a configured `Command`. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `configure_script_process_group` | 87-91 | `mapper` | LOW | Maps Unix child command configuration to process-group settings. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `configure_script_process_group` | 94 | `mapper` | LOW | Non-Unix no-op command configuration branch. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_child_stdout` | 96-98 | `accessor` | LOW | Takes child stdout and exposes a drain handle. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_child_stderr` | 100-102 | `accessor` | LOW | Takes child stderr and exposes a drain handle. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_string_drain` | 104-109 | `orchestration` | LOW | Dispatches reader draining into a spawned thread using a named helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_to_string` | 111-115 | `accessor` | LOW | Reads a stream into a string without changing meaning. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `wait_for_child` | 117-130 | `orchestration` | LOW | Sequences wait-step polling, sleep, and finalization helpers. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `try_wait_child` | 132-137 | `accessor` | LOW | Exposes child wait status with formatted error delegation. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `wait_step` | 139-149 | `orchestration` | LOW | Coordinates try-wait and timeout-step helpers. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `timeout_wait_step` | 151-156 | `mapper` | LOW | Maps elapsed time versus timeout into a `WaitStep`. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `finish_wait_step` | 158-169 | `orchestration` | LOW | Dispatches completed, timed-out, or impossible pending wait-step outcomes. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_timed_out_child` | 171-178 | `orchestration` | LOW | Sequences process-group kill and timeout error helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_child_process_group` | 181-189 | `orchestration` | LOW | Performs Unix timeout cleanup actions for the child process group. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_child_process_group` | 192-195 | `orchestration` | LOW | Performs non-Unix timeout cleanup actions for the child. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `joined_text` | 197-199 | `accessor` | LOW | Exposes joined thread text with default fallback. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `ensure_refresh_success` | 201-206 | `validator` | LOW | Validates successful auth-refresh exit status or returns failure. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `ensure_quota_success` | 208-213 | `validator` | LOW | Validates successful quota exit status or returns failure. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_refresh_spawn_error` | 227-229 | `formatter` | LOW | Formats auth-refresh spawn error text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_quota_spawn_error` | 231-233 | `formatter` | LOW | Formats quota spawn error text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_timeout` | 235-244 | `formatter` | LOW | Formats timeout messages by process kind. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_wait_error` | 246-251 | `formatter` | LOW | Formats wait errors by process kind. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_refresh_exit` | 253-259 | `formatter` | LOW | Formats auth-refresh non-zero exit text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_quota_exit` | 261-267 | `formatter` | LOW | Formats quota non-zero exit text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `tests::quota_script_timeout_is_classified` | 275-280 | `validator` | LOW | Test body asserts timeout error classification tokens. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `tests::quota_script_timeout_kills_process_group_children` | 284-300 | `validator` | LOW | Test body asserts timeout classification and no leaked process marker. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_canonical_body_shape` | 76-81 | `predicate` | LOW | Answers whether body is an array of canonical chunks. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_canonical_body_chunk` | 83-89 | `predicate` | LOW | Answers whether one chunk has canonical fields. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_provider` | 96-102 | `orchestration` | LOW | Delegates public scan to timeout-aware scan helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_provider_with_timeout` | 104-128 | `orchestration` | LOW | Sequences source lookup, state-dir setup, script run, batch collection, persistence, and report return through helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `provider_session_source` | 130-135 | `accessor` | LOW | Retrieves provider session source from config. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_report_with_error` | 137-140 | `mapper` | LOW | Maps a report plus error into an updated report. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `create_session_state_dir` | 142-145 | `orchestration` | LOW | Dispatches directory creation and error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_state_dir_create_error` | 147-152 | `formatter` | LOW | Formats state-dir creation error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `collect_turn_script_batch` | 154-175 | `orchestration` | LOW | Iterates script lines and delegates degraded-marker, parse, error recording, and batch push work to helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `degraded_marker_error` | 177-183 | `orchestration` | LOW | Pure helper dispatch for marker parse, marker predicate, count access, and error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_degraded_marker_jsonl` | 185-187 | `parser` | LOW | Parses JSONL text into a JSON value. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_degraded_marker` | 189-194 | `predicate` | LOW | Answers whether JSON value has `degraded=true`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `degraded_marker_count` | 196-201 | `accessor` | LOW | Retrieves optional degraded count with fallback. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_degraded_marker_error` | 203-205 | `formatter` | LOW | Formats degraded scan error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `non_empty_script_lines` | 207-213 | `mapper`, `filter` | HIGH | FC-001: splits stdout lines, trims line values, filters empty values, and collects. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_script_line_seen` | 215-218 | `accessor` | LOW | Updates and exposes the next script line count. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_optional_scan_error` | 220-224 | `orchestration` | LOW | Delegates optional error recording through named helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_scan_error` | 226-228 | `mapper` | LOW | Maps an error into the report's error list. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `push_script_turn_ingest` | 230-232 | `mapper` | LOW | Maps one ingest item into the batch collection. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_line_to_ingest` | 234-251 | `orchestration` | LOW | Delegates parse, timestamp parse, body validation, body serialization, error extraction, and ingest construction. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parsed_script_turn_ingest` | 263-268 | `mapper` | LOW | Maps ingest plus body error into `ParsedScriptTurnIngest`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_script_turn_line` | 270-272 | `parser` | LOW | Parses one JSONL turn line into `ScriptTurn`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_malformed_turn_line` | 274-276 | `formatter` | LOW | Formats malformed turn line error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_script_turn_timestamp` | 278-282 | `parser` | LOW | Parses RFC3339 timestamp text into UTC `DateTime`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_bad_timestamp` | 284-286 | `formatter` | LOW | Formats invalid timestamp error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_script_turn_body` | 288-300 | `validator` | LOW | Accepts absent or valid body and rejects invalid body with error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_script_turn_body_shape` | 302-312 | `validator` | LOW | Validates canonical body shape or returns formatted validation error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_invalid_body_shape` | 314-318 | `formatter` | LOW | Formats canonical body shape validation error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `serialize_script_turn_body` | 320-328 | `formatter` | LOW | Serializes body value to text and delegates error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `selected_script_turn_body` | 330-335 | `accessor` | LOW | Selects accepted body reference from validation result. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `serialize_selected_script_turn_body` | 337-345 | `orchestration` | LOW | Dispatches optional body serialization through named helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_body_serialize_error` | 347-353 | `formatter` | LOW | Formats body serialization error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_to_ingest` | 355-370 | `mapper` | LOW | Maps parsed script turn fields into `SessionTurnIngest`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_ingest_from_parts` | 372-379 | `mapper` | LOW | Maps turn parts into `ParsedScriptTurnIngest`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_body_error` | 381-386 | `accessor` | LOW | Retrieves body validation error when present. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `persist_scanned_turns` | 388-398 | `orchestration` | LOW | Dispatches DB ingest result into chain persistence or error recording. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `persist_imported_chains` | 400-418 | `orchestration` | LOW | Sequences new-turn report update and imported-chain minting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_all` | 422-430 | `orchestration` | LOW | Iterates providers, dispatches scans, records reports, and sorts output. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `resolve_state_dir` | 432-438 | `mapper` | LOW | Maps provider source entry and provider name into a state directory path. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `default_app_data_dir` | 440-443 | `accessor` | LOW | Retrieves configured data dir with fallback. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_turn_script` | 445-451 | `orchestration` | LOW | Delegates turn script execution to timeout-aware session script runner. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `locate_transcript` | 453-472 | `orchestration` | LOW | Sequences entry lookup, locator lookup, state-dir creation, script run, line validation, and path mapping through helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_source_entry` | 474-479 | `accessor` | LOW | Retrieves provider session source entry. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `transcript_locator_script` | 481-483 | `accessor` | LOW | Retrieves optional locator script text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `single_transcript_stdout_line` | 485-491 | `validator` | LOW | Validates transcript locator stdout has exactly one line. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `transcript_path_from_line` | 493-495 | `mapper` | LOW | Maps one stdout line into `PathBuf`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_session_script` | 497-510 | `orchestration` | LOW | Delegates to timeout-aware session script runner with default timeout. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_session_script_with_timeout` | 512-533 | `orchestration` | LOW | Sequences command construction, spawn, stream readers, wait, join, and success validation through helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_session_script_success` | 535-549 | `validator` | LOW | Validates session script exit status or returns non-zero error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_script_command` | 551-567 | `mapper` | LOW | Maps script, state dir, and optional session ID into configured `Command`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `configure_session_script_process_group` | 570-574 | `mapper` | LOW | Maps Unix session script command configuration to process-group settings. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `configure_session_script_process_group` | 577 | `mapper` | LOW | Non-Unix no-op command configuration branch. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_session_script_child` | 579-582 | `orchestration` | LOW | Spawns configured command and delegates spawn-error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_spawn_error` | 584-586 | `formatter` | LOW | Formats session script spawn error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `take_child_stdout` | 588-590 | `accessor` | LOW | Retrieves child stdout pipe. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `take_child_stderr` | 592-594 | `accessor` | LOW | Retrieves child stderr pipe. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_stdout_reader` | 596-598 | `orchestration` | LOW | Dispatches stdout reader spawning to common helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_stderr_reader` | 600-602 | `orchestration` | LOW | Dispatches stderr reader spawning to common helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_script_reader` | 604-613 | `orchestration`, `accessor` | HIGH | FC-002: spawns a thread and inlines stream-to-string draining inside the spawned closure. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `wait_for_session_script` | 615-631 | `orchestration` | LOW | Polls session script and delegates pending, success, and wait-error handling. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `poll_session_script` | 633-635 | `accessor` | LOW | Retrieves child wait status. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `wait_for_pending_session_script` | 637-649 | `orchestration` | LOW | Dispatches timeout predicate, timeout failure helper, and sleep helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `pending_session_script_timed_out` | 651-656 | `predicate` | LOW | Answers whether pending wait exceeded timeout. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `fail_timed_out_pending_session_script` | 658-668 | `orchestration` | LOW | Sequences timeout kill and timeout error construction through helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_timed_out_pending_session_script` | 670-672 | `orchestration` | LOW | Delegates timeout kill to process-group cleanup helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_session_script_process_group` | 675-683 | `orchestration` | LOW | Performs Unix timeout cleanup actions for the session script process group. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_session_script_process_group` | 686-689 | `orchestration` | LOW | Performs non-Unix timeout cleanup actions for the child. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `sleep_before_next_session_script_poll` | 691-693 | `orchestration` | LOW | Performs bounded polling sleep action. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_pending_session_script_timeout` | 695-697 | `formatter` | LOW | Formats session script timeout text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_wait_error` | 699-704 | `formatter` | LOW | Formats session script wait error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `join_script_reader` | 706-708 | `accessor` | LOW | Exposes joined reader thread text with fallback. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_nonzero` | 710-721 | `formatter` | LOW | Formats non-zero session script exit text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `capitalize_script_kind` | 723-729 | `formatter` | LOW | Formats script-kind casing for messages. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::db` | 737-739 | `accessor` | LOW | Opens in-memory state DB fixture. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::fixture_script` | 749-757 | `mapper` | LOW | Maps script body text into an executable fixture path wrapper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::cfg_with` | 759-770 | `mapper` | LOW | Maps provider and script path into `SessionsConfig`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::cfg_with_locator` | 772-787 | `mapper` | LOW | Maps provider, locator path, and state dir into `SessionsConfig`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::ingests_assistant_turns_and_advances_count` | 790-805 | `validator` | LOW | Test body asserts ingest and assistant-count behavior. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::duplicate_turns_are_idempotent_per_unique_constraint` | 808-820 | `validator` | LOW | Test body asserts duplicate turns dedupe by unique constraint. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::script_turn_legacy_json_deserializes_with_none_defaults` | 823-835 | `validator` | LOW | Test body asserts legacy JSON defaults. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::script_turn_full_json_deserializes_parent_and_sidechain_fields` | 838-848 | `validator` | LOW | Test body asserts optional parent and sidechain fields. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::scan_provider_persists_body_encoding_edge_cases` | 851-879 | `validator` | LOW | Test body asserts body encoding persistence. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::scan_provider_rejects_non_canonical_body_shape` | 882-914 | `validator` | LOW | Test body asserts invalid body shape errors and persisted empty bodies. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::malformed_lines_collect_as_errors_but_dont_abort` | 917-930 | `validator` | LOW | Test body asserts malformed lines collect errors without abort. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::nonzero_exit_is_an_error` | 933-941 | `validator` | LOW | Test body asserts non-zero script exit is recorded. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::turn_script_timeout_is_classified_and_does_not_persist_turns` | 944-956 | `validator` | LOW | Test body asserts timeout classification and no persisted turns. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::degraded_marker_is_reported_without_malformed_turn_error` | 959-970 | `validator` | LOW | Test body asserts degraded marker is not treated as malformed turn JSON. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::script_receives_state_dir_env` | 973-995 | `validator` | LOW | Test body asserts state-dir env delivery. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::locate_transcript_returns_none_when_no_locator_is_configured` | 998-1004 | `validator` | LOW | Test body asserts missing locator returns none. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::locate_transcript_returns_script_stdout_path` | 1007-1026 | `validator` | LOW | Test body asserts locator stdout path and session env. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::locate_transcript_returns_error_on_nonzero_exit` | 1029-1036 | `validator` | LOW | Test body asserts non-zero locator error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::locate_transcript_returns_error_on_empty_stdout` | 1043-1053 | `validator` | LOW | Test body asserts empty locator stdout is rejected. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::cfg_from_adapter_fixture` | 1055-1069 | `mapper` | LOW | Maps fixture name into script fixture and sessions config. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::turn_script_optional_compaction_field_defaults_false` | 1073-1086 | `validator` | LOW | Test body asserts compaction default behavior. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `tests::turn_script_compaction_field_propagates_to_session_turns` | 1090-1103 | `validator` | LOW | Test body asserts compaction boundary propagation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::new` | 78-97 | `mapper` | LOW | Maps tempdir, state DB, file paths, and fake provider into fixture struct. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::set_mode` | 99-101 | `orchestration` | LOW | Performs mode-file write action. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::registry` | 103-111 | `mapper` | LOW | Maps fixture provider path and roots into provider registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::hostile_registry` | 113-121 | `mapper` | LOW | Maps fixture provider path and hostile roots into provider registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::timeout_registry` | 123-133 | `mapper` | LOW | Maps fixture provider path and timeout client options into provider registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::unrelated_registry` | 135-143 | `mapper` | LOW | Maps unrelated model configuration into provider registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::records` | 145-147 | `accessor` | LOW | Retrieves provider record values from record path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::request_records_for` | 149-154 | `filter` | LOW | Selects provider records matching a subcommand. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::snapshot` | 156-158 | `accessor` | LOW | Retrieves SQLite snapshot for fixture connection. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::state_path` | 160-162 | `accessor` | LOW | Exposes fixture state DB path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::conn` | 164-166 | `accessor` | LOW | Opens fixture SQLite connection. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::seed_finalized_invocation` | 168-183 | `orchestration` | LOW | Sequences invocation start and finalize fixture setup. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::seed_chain` | 185-200 | `orchestration` | LOW | Sequences chain and chain segment fixture inserts. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_text` | 203-205 | `accessor` | LOW | Reads provider record file text. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_values` | 207-209 | `parser` | LOW | Parses record text lines into JSON values through a helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_value` | 211-213 | `parser` | LOW | Parses one provider record JSON line. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_model` | 215-229 | `mapper` | LOW | Maps model name and provider path into `ModelConfig`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_identity` | 231-238 | `mapper` | LOW | Maps constants into `SessionProviderIdentity`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `locate_request` | 240-252 | `mapper` | LOW | Maps registry, session ID, and lookup mode into locate request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `read_request` | 254-264 | `mapper` | LOW | Maps registry and session ID into read-turns request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `capture_request` | 266-276 | `mapper` | LOW | Maps registry and invocation UUID into capture request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `no_ref_dispatch_aware_lifecycle_path_preserves_session_capture_marker_and_sqlite_bytes` | 279-312 | `validator` | LOW | Test body asserts no-ref dispatch lifecycle and SQLite equivalence. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_locate_dispatch_maps_success_and_request_identity` | 315-341 | `validator` | LOW | Test body asserts locate success mapping and request identity. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_locate_failure_matrix_does_not_fall_back_to_private_layouts` | 344-392 | `validator` | LOW | Test body asserts locate failure matrix tokens and no host mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_locate_unknown_format_maps_to_other_storage_class` | 395-420 | `validator` | LOW | Test body asserts unknown format storage classification. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_read_turns_maps_transport_into_owned_turn_interface_before_persistence` | 423-478 | `validator` | LOW | Test body asserts read-turn mapping before persistence. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_read_turns_provider_transport_and_schema_failures_do_not_mutate_sqlite` | 481-514 | `validator` | LOW | Test body asserts read-turn failure tokens and no mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_read_turns_rejects_invalid_or_mismatched_provider_evidence_without_mutation` | 517-540 | `validator` | LOW | Test body asserts invalid read evidence is rejected without mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_read_turns_complete_partial_idempotency_and_turn_count_are_evidence_only` | 543-591 | `validator` | LOW | Test body asserts complete/partial evidence and idempotency. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_read_turns_ingest_uses_owned_interface_and_host_idempotency` | 594-633 | `validator` | LOW | Test body asserts owned ingest and host idempotency. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `opencode_read_turns_ingests_normalized_jsonl` | 636-659 | `validator` | LOW | Test body asserts OpenCode adapter JSONL ingest and repeat scan behavior. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_capture_maps_facts_without_mutating_capture_rows` | 662-690 | `validator` | LOW | Test body asserts capture facts without SQLite mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_capture_provider_transport_and_schema_failures_do_not_mutate_sqlite` | 693-728 | `validator` | LOW | Test body asserts capture failure tokens and no mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_timeout_is_stable_transport_token_without_mutation` | 731-750 | `validator` | LOW | Test body asserts timeout token and no mutation. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_provider_error_tokens_are_stable_by_failure_class` | 753-775 | `validator` | LOW | Test body asserts stable error tokens by failure class. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `hostile_provider_cannot_discover_or_mutate_runner_sqlite_through_session_dispatch` | 778-821 | `validator` | LOW | Test body asserts hostile provider cannot discover or mutate runner SQLite. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `assert_error_token` | 823-829 | `validator` | LOW | Assertion helper validates stable token presence. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::new` | 836-850 | `mapper`, `validator` | HIGH | FC-003: constructs/seeds proof fixture and asserts row-id invariant in the same body. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::request_without_registry` | 852-862 | `mapper` | LOW | Maps fixture state and constants into no-ref proof request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::request_with_registry` | 864-872 | `mapper` | LOW | Maps registry override into no-ref proof request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::snapshot` | 874-876 | `accessor` | LOW | Exposes fixture snapshot. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `hostile_marker` | 879-881 | `mapper` | LOW | Maps fixture and route into marker path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `DataDirOverride::remove` | 888-896 | `orchestration` | LOW | Sequences env capture and temporary env removal. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `DataDirOverride::drop` | 900-908 | `orchestration` | LOW | Restores or removes env value during drop cleanup. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `hostile_markers_json` | 911-921 | `formatter` | LOW | Formats hostile marker paths into JSON text. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `assert_request_shape` | 923-938 | `validator` | LOW | Assertion helper validates request JSON shape. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `sqlite_snapshot` | 940-947 | `mapper` | LOW | Maps database rows into `SqliteSnapshot`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `invocation_snapshot_rows` | 949-957 | `accessor` | LOW | Retrieves invocation snapshot rows via query helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `invocation_snapshot_row` | 959-967 | `mapper` | LOW | Maps a SQLite row into invocation snapshot tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_turn_snapshot_rows` | 969-977 | `accessor` | LOW | Retrieves session turn snapshot rows via query helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_turn_snapshot_row` | 979-990 | `mapper` | LOW | Maps a SQLite row into session-turn snapshot tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_snapshot_rows` | 992-998 | `accessor` | LOW | Retrieves session chain snapshot rows via query helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_snapshot_row` | 1000-1002 | `mapper` | LOW | Maps a SQLite row into session-chain tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_segment_snapshot_rows` | 1004-1011 | `accessor` | LOW | Retrieves session chain segment snapshot rows via query helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_segment_snapshot_row` | 1013-1017 | `mapper` | LOW | Maps a SQLite row into chain segment tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `query_rows` | 1019-1028 | `accessor` | LOW | Retrieves query rows and applies supplied row mapper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `parse_ts` | 1030-1034 | `parser` | LOW | Parses RFC3339 timestamp into UTC `DateTime`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `write_fake_provider` | 1036-1052 | `orchestration` | LOW | Sequences fake provider body formatting, file write, chmod, and path return. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `fake_provider_body` | 1054-1328 | `formatter` | LOW | Formats fake provider Python source text from paths and constants; embedded Python functions are string payload, not Rust inventory symbols. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `json_string` | 1330-1332 | `formatter` | LOW | Formats path display text as JSON string. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `repo_script_path` | 1334-1339 | `mapper` | LOW | Maps script name into repository script path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `opencode_sessions_config` | 1341-1362 | `mapper` | LOW | Maps script/opencode/state paths into `SessionsConfig`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `write_fake_opencode` | 1364-1407 | `orchestration` | LOW | Sequences fake OpenCode script file write, chmod, and path return. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `shell_single_quote_path` | 1409-1411 | `formatter` | LOW | Formats shell-quoted path text. |
| `scripts/opencode-turns` | `Options.__init__` | 67-71 | `orchestration` | LOW | Dispatches option parsing to env helpers and assigns option fields. |
| `scripts/opencode-turns` | `Deadline.__init__` | 75-77 | `mapper` | LOW | Maps duration into deadline state. |
| `scripts/opencode-turns` | `Deadline.remaining` | 79-80 | `accessor` | LOW | Exposes remaining deadline budget. |
| `scripts/opencode-turns` | `Deadline.call_timeout` | 82-86 | `filter` | LOW | Bounds max call timeout by remaining deadline. |
| `scripts/opencode-turns` | `env_float` | 89-97 | `parser` | LOW | Parses positive float env value with fallback. |
| `scripts/opencode-turns` | `env_int` | 100-108 | `parser` | LOW | Parses positive integer env value with fallback. |
| `scripts/opencode-turns` | `text_chunk` | 111-112 | `mapper` | LOW | Maps text into canonical text chunk. |
| `scripts/opencode-turns` | `canonical_chunk_type` | 115-120 | `mapper` | LOW | Maps native chunk type names to canonical names. |
| `scripts/opencode-turns` | `extract_content_chunks` | 123-130 | `orchestration` | LOW | Dispatches content extraction by value shape to helper functions. |
| `scripts/opencode-turns` | `content_chunks_from_text` | 133-134 | `mapper` | LOW | Maps plain text into canonical chunk list. |
| `scripts/opencode-turns` | `content_chunks_from_items` | 137-141 | `orchestration` | LOW | Recursively dispatches content extraction for list items. |
| `scripts/opencode-turns` | `content_chunks_from_obj` | 144-148 | `orchestration` | LOW | Delegates direct object parsing before nested recursion. |
| `scripts/opencode-turns` | `direct_content_chunks_from_obj` | 151-158 | `parser` | LOW | Parses a text-bearing content dictionary into canonical chunk shape. |
| `scripts/opencode-turns` | `nested_content_chunks_from_obj` | 161-166 | `orchestration` | LOW | Tries known nested content fields and returns first recursive chunk result. |
| `scripts/opencode-turns` | `unique_values` | 169-176 | `filter` | LOW | Deduplicates values while preserving order. |
| `scripts/opencode-turns` | `session_ids_from_value` | 179-192 | `parser` | LOW | Recursively extracts session IDs from arbitrary value shapes. |
| `scripts/opencode-turns` | `parse_session_list_stdout` | 195-204 | `orchestration` | LOW | Coordinates session-list JSON parsing, candidate extraction, fallback ID extraction, and bounded selection through helpers. |
| `scripts/opencode-turns` | `parse_session_list_json` | 207-211 | `parser` | LOW | Parses session list stdout as JSON or returns sentinel. |
| `scripts/opencode-turns` | `capped_session_ids_from_value` | 214-215 | `filter` | LOW | Extracts unique session IDs through helper and applies cap. |
| `scripts/opencode-turns` | `session_ids_from_candidates` | 218-221 | `orchestration` | LOW | Selects timestamp-window filtering or cap filtering through helpers. |
| `scripts/opencode-turns` | `candidates_have_timestamps` | 224-225 | `predicate` | LOW | Answers whether candidates include parsed timestamps. |
| `scripts/opencode-turns` | `recent_session_ids` | 228-234 | `filter` | LOW | Selects candidate session IDs within recent window. |
| `scripts/opencode-turns` | `capped_candidate_session_ids` | 237-238 | `filter` | LOW | Applies max-session cap to candidate session IDs. |
| `scripts/opencode-turns` | `unique_session_candidates` | 241-250 | `filter` | LOW | Deduplicates session candidates by session ID. |
| `scripts/opencode-turns` | `session_candidates_from_value` | 253-258 | `orchestration` | LOW | Dispatches candidate extraction by value shape. |
| `scripts/opencode-turns` | `session_candidates_from_items` | 261-265 | `orchestration` | LOW | Recursively accumulates candidates from iterable values. |
| `scripts/opencode-turns` | `session_candidates_from_obj` | 268-271 | `orchestration` | LOW | Routes dictionary candidate mapping or recursive traversal through helpers. |
| `scripts/opencode-turns` | `has_session_candidate_shape` | 274-275 | `predicate` | LOW | Answers whether an object has a recognizable session ID. |
| `scripts/opencode-turns` | `session_candidate_from_obj` | 278-282 | `mapper` | LOW | Maps recognized session-list object into candidate row. |
| `scripts/opencode-turns` | `session_list_session_id` | 285-293 | `parser` | LOW | Parses a session ID from known fields. |
| `scripts/opencode-turns` | `session_list_timestamp` | 296-301 | `parser` | LOW | Parses a timestamp from known session-list fields. |
| `scripts/opencode-turns` | `timestamp_datetime` | 304-319 | `parser` | LOW | Parses numeric or string timestamp values into UTC datetime. |
| `scripts/opencode-turns` | `numeric_timestamp_datetime` | 322-327 | `parser` | LOW | Parses numeric seconds or milliseconds into UTC datetime. |
| `scripts/opencode-turns` | `discover_session_ids` | 330-336 | `orchestration` | LOW | Runs session discovery and delegates timeout/result parsing. |
| `scripts/opencode-turns` | `requested_session_ids` | 339-344 | `orchestration` | LOW | Selects explicit session IDs or implicit discovery through helpers. |
| `scripts/opencode-turns` | `numeric_timestamp` | 347-351 | `formatter` | LOW | Formats numeric timestamp as UTC ISO text. |
| `scripts/opencode-turns` | `timestamp_from_obj` | 354-362 | `parser` | LOW | Parses turn timestamp from known object fields. |
| `scripts/opencode-turns` | `session_id_from_obj` | 365-370 | `parser` | LOW | Parses session ID from exported message object. |
| `scripts/opencode-turns` | `role_from_obj` | 373-380 | `parser` | LOW | Parses supported role from top-level or nested message object. |
| `scripts/opencode-turns` | `turn_id_from_obj` | 383-384 | `orchestration` | LOW | Selects parsed turn ID or fallback through helpers. |
| `scripts/opencode-turns` | `turn_id_field_from_obj` | 387-394 | `parser` | LOW | Parses turn ID from top-level or nested message object. |
| `scripts/opencode-turns` | `turn_id_field_from_mapping` | 397-402 | `parser` | LOW | Parses turn ID from known ID fields. |
| `scripts/opencode-turns` | `fallback_turn_id` | 405-406 | `formatter` | LOW | Formats deterministic fallback turn ID. |
| `scripts/opencode-turns` | `opencode_command` | 409-411 | `parser` | LOW | Parses `OPENCODE_BIN` shell words with fallback. |
| `scripts/opencode-turns` | `run_opencode` | 414-432 | `orchestration` | LOW | Sequences deadline, spawn, communicate, timeout cleanup, failure classification, and result construction through helpers. |
| `scripts/opencode-turns` | `opencode_deadline_expired` | 435-436 | `predicate` | LOW | Answers whether no per-call timeout budget remains. |
| `scripts/opencode-turns` | `spawn_opencode_process` | 439-447 | `orchestration` | LOW | Starts OpenCode subprocess with bounded stdio/session settings. |
| `scripts/opencode-turns` | `communicate_opencode_process` | 450-457 | `orchestration` | LOW | Waits for process output and reports timeout status. |
| `scripts/opencode-turns` | `opencode_process_failed` | 460-461 | `predicate` | LOW | Answers whether subprocess exit code is non-zero. |
| `scripts/opencode-turns` | `degraded_opencode_result` | 464-465 | `mapper` | LOW | Maps timeout condition into degraded result tuple. |
| `scripts/opencode-turns` | `failed_opencode_result` | 468-469 | `mapper` | LOW | Maps failed process condition into non-degraded empty result tuple. |
| `scripts/opencode-turns` | `successful_opencode_result` | 472-473 | `mapper` | LOW | Maps stdout into successful result tuple. |
| `scripts/opencode-turns` | `kill_process_group` | 476-491 | `orchestration` | LOW | Sequences process-group kill, fallback process kill, and drain. |
| `scripts/opencode-turns` | `parse_export_stdout` | 494-498 | `parser` | LOW | Parses export stdout as JSON or returns none. |
| `scripts/opencode-turns` | `export_session` | 501-509 | `orchestration` | LOW | Runs export command and delegates timeout/result parsing. |
| `scripts/opencode-turns` | `exported_message_items` | 512-516 | `orchestration` | LOW | Coordinates export-shape extraction and dictionary filtering through helpers. |
| `scripts/opencode-turns` | `exported_message_item_values` | 519-530 | `parser` | LOW | Parses raw message item list from supported export shapes. |
| `scripts/opencode-turns` | `dict_items` | 533-534 | `filter` | LOW | Keeps only dictionary items from a raw item list. |
| `scripts/opencode-turns` | `record_from_message` | 537-547 | `orchestration`, `mapper` | HIGH | FC-004: coordinates field parsing/validation/record mapping and also mutates the mapped record with optional body inline. |
| `scripts/opencode-turns` | `message_record_fields` | 550-555 | `parser` | LOW | Extracts normalized required fields from message object. |
| `scripts/opencode-turns` | `has_required_message_record_fields` | 558-559 | `validator` | LOW | Validates that required message record fields are present. |
| `scripts/opencode-turns` | `message_record_from_fields` | 562-568 | `mapper` | LOW | Maps validated fields and turn ID into normalized record base. |
| `scripts/opencode-turns` | `message_body_chunks` | 571-576 | `parser` | LOW | Extracts optional body chunks from supported message content fields. |
| `scripts/opencode-turns` | `records_from_exported_session` | 579-585 | `mapper` | LOW | Maps exported message items into normalized records, omitting non-record results. |
| `scripts/opencode-turns` | `collect_records` | 588-601 | `orchestration` | LOW | Iterates sessions, exports each, stops on timeout, and accumulates records through helpers. |
| `scripts/opencode-turns` | `emit_record` | 604-606 | `formatter` | LOW | Emits one compact JSONL record. |
| `scripts/opencode-turns` | `emit_degraded_marker` | 609-611 | `filter`, `formatter` | HIGH | FC-005: counts assistant records with inline predicate/filter logic and emits the degraded JSONL marker. |
| `scripts/opencode-turns` | `main` | 614-629 | `validator`, `formatter`, `orchestration` | HIGH | FC-006: validates argv and prints usage inline, then orchestrates options/deadline, record collection, record emission, and degraded marker emission. |
| `scripts/tests/opencode-turns.test.sh` | `fail` | 10-13 | `validator` | LOW | Test failure helper reports and exits on validation failure. |
| `scripts/tests/opencode-turns.test.sh` | `assert_eq` | 15-23 | `validator` | LOW | Assertion helper validates equality. |
| `scripts/tests/opencode-turns.test.sh` | `assert_status_zero` | 25-32 | `validator` | LOW | Assertion helper validates zero exit status. |
| `scripts/tests/opencode-turns.test.sh` | `assert_stdout_contains` | 34-41 | `validator` | LOW | Assertion helper validates stdout contains fixed text. |
| `scripts/tests/opencode-turns.test.sh` | `assert_stdout_not_contains` | 43-50 | `validator` | LOW | Assertion helper validates stdout excludes fixed text. |
| `scripts/tests/opencode-turns.test.sh` | `write_window_filter_mock` | 52-88 | `orchestration` | LOW | Writes and chmods mock OpenCode executable for window-filter proof. |
| `scripts/tests/opencode-turns.test.sh` | `write_timeout_mock` | 90-122 | `orchestration` | LOW | Writes and chmods mock OpenCode executable for timeout proof. |
| `scripts/tests/opencode-turns.test.sh` | `run_opencode_turns` | 124-143 | `orchestration` | LOW | Runs adapter with fixture env and captures stdout/stderr/status. |
| `scripts/tests/opencode-turns.test.sh` | `test_exports_only_recent_window_sessions` | 145-162 | `validator` | LOW | Test body asserts recent-window export behavior and non-degraded output. |
| `scripts/tests/opencode-turns.test.sh` | `test_timeout_emits_degraded_best_effort_and_exits_zero` | 164-185 | `validator` | LOW | Test body asserts timeout-degraded best-effort behavior and elapsed bound. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| FC-001 | `crates/oulipoly-runtime/src/sessions/mod.rs` | `non_empty_script_lines` | `mapper`, `filter` | `failure_mode: multi-classifier function`. Lines 208-212 call `.lines()`, `.map(str::trim)`, `.filter(|line| !line.is_empty())`, and `.collect()`: the body both changes each line to a trimmed representation and excludes empty lines. | Split line normalization from empty-line selection. Keep one helper that maps stdout to trimmed line values, and a separate helper that filters non-empty values. Convergence: current finding FC-001 is closed because the current mixed trim-plus-filter body disappears; introduced helpers are each audited under the same overlay and must remain single-class. | blocking | pre_existing_in_touched_file | same_domain |
| FC-002 | `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_script_reader` | `orchestration`, `accessor` | `failure_mode: multi-classifier function`. Lines 608-612 spawn a thread and inline stream draining inside the closure (`read_to_string` into `buf`, then return `buf`), so the function both orchestrates thread execution and performs stream-to-string data retrieval. | Split stream draining into a named accessor/reader helper and leave `spawn_script_reader` as the thread-spawn dispatcher. Convergence: current finding FC-002 is closed because the orchestration body no longer performs inline read/drain work; introduced helper is audited as the single accessor under overlay. | blocking | pre_existing_in_touched_file | same_domain |
| FC-003 | `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::new` | `mapper`, `validator` | `failure_mode: multi-classifier function`. Lines 837-849 construct and seed a `NoRefDispatchProofFixture`, while lines 840-843 assert `invocation_row_id == 1`; the body combines fixture construction/mapping with invariant validation. | Split proof-fixture construction/seeding from the row-id assertion. Keep constructor-like work in a mapper/setup helper and move the assertion to the calling test or a validator helper. Convergence: current finding FC-003 is closed because the fixture constructor no longer validates an invariant inline; any assertion helper is audited separately as validator. | blocking | pre_existing_in_touched_file | same_domain |
| FC-004 | `scripts/opencode-turns` | `record_from_message` | `orchestration`, `mapper` | `failure_mode: multi-classifier function`. Lines 538-543 coordinate field parsing, required-field validation, record mapping, and turn-ID selection through helpers; lines 544-546 then perform inline optional-body mapping by mutating `record["body"] = body`. | Split optional body attachment into a named mapper helper, leaving `record_from_message` as orchestration over field extraction, validation, base-record mapping, body extraction, and attachment helper. Convergence: current finding FC-004 is closed because the orchestrator stops mutating the record shape inline; introduced body-attachment helper is audited separately as mapper. | blocking | changed_function | same_domain |
| FC-005 | `scripts/opencode-turns` | `emit_degraded_marker` | `filter`, `formatter` | `failure_mode: multi-classifier function`. Line 610 computes `count = sum(1 for record in records if record.get("role") == "assistant")`, selecting/counting assistant records; line 611 formats/emits the degraded marker JSONL. | Split assistant-count selection into a filter/accessor-style helper and keep `emit_degraded_marker` focused on marker formatting/emission from a supplied count. Convergence: current finding FC-005 is closed because the formatter no longer performs inline assistant-role filtering; introduced count helper is audited separately as filter. | blocking | changed_function | same_domain |
| FC-006 | `scripts/opencode-turns` | `main` | `validator`, `formatter`, `orchestration` | `failure_mode: multi-classifier function`. Lines 615-617 validate CLI argv shape and print usage text; lines 619-628 construct options/deadline, collect records, emit records, and conditionally emit degraded marker. The body mixes entrypoint validation/formatting with runtime orchestration. | Split argv validation/usage rendering into a validator/formatter boundary that returns an accepted argument set or usage failure, leaving `main` to orchestrate accepted options, collection, and emission. Convergence: current finding FC-006 is closed because `main` no longer performs inline argv validation or usage formatting; introduced validation/formatting helpers are each audited under the same overlay. | blocking | changed_function | same_domain |

## Residual Ambiguity / Stop-Condition Notes

- Embedded shell/Python/YAML/text payloads inside heredocs or Rust string literals were excluded from the A5 inventory because they are fixture/document payloads in the touched source file rather than language-level function symbols of the touched file itself. The source functions that produce those payloads, such as `fake_provider_body`, `write_window_filter_mock`, and `write_timeout_mock`, were included.
- Inline method-argument closures were inspected as body evidence for their containing function. They did not change any LOW/HIGH conclusion beyond the explicit closure-backed finding for `spawn_script_reader`, where the closure body performs non-trivial stream draining inline.
- No `NEEDS_INPUT` condition was found. Required Phase 6 `contract_path` and `proposal_path` were readable before scoring.

Verdict: HIGH

VERDICT: HIGH
