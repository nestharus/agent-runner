# Function Classification Audit

## Inputs Read

- `worktree_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `repo_root=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar`
- `diff_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/diff.patch`
- `touched_surfaces_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/gates/touched-files.txt`
- `proposal_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/proposal.md`
- `contract_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/contracts/tsb.contract.md`
- `output_path=/home/nes/projects/agent-runner/worktrees/age-pid-sidecar/planning/tsb-gate/code-quality/tsb/reports/function-classification-auditor.md`
- `mode=phase-6`

## References Read

- `/home/nes/ai/conventions/code-quality.md` lines 52-69: A1 single-classification rule and category list.
- `/home/nes/ai/conventions/code-quality.md` lines 21-27 and 143-149: auditor scope boundary and touched-file ownership.
- `/home/nes/ai/conventions/code-quality.md` lines 295-300: `Function categories per function` threshold row, LOW = 1 and HIGH >= 2.
- `/home/nes/ai/conventions/code-quality.md` lines 304-310: `multi-classifier function` failure mode.
- `planning/tsb-gate/contracts/tsb.contract.md` lines 3-17: Phase 6 component roles and touched-file set.
- `planning/tsb-gate/contracts/tsb.contract.md` lines 193-256: adapter and intrinsic-surface declarations used as context only.
- `planning/tsb-gate/proposal.md` lines 3-6: bounded OpenCode turn scan and runtime script deadline intent.
- `planning/tsb-gate/proposal.md` lines 17-63: proof claims for bounded enumeration, degraded markers, and process-group timeout behavior.

A1 preservation check: present and non-contradictory. The metric source contains the exact A1 category list (`orchestration`, `filter`, `validator`, `predicate`, `mapper`, `accessor`, `formatter`, `parser`), the single-classification rule, the `Function categories per function` threshold row, touched-file ownership, and the `multi-classifier function` failure mode.

## Functions In Touched Files

| Path | Function / symbol | Line span or diff hunk | Inferred category | Verdict | Evidence |
|---|---|---|---|---|---|
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_refresh_command` | L36-L42 | `orchestration` | LOW | Sequences spawn, stderr drain, wait, join, and success helper dispatch. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_script` | L44-L46 | `orchestration` | LOW | Delegates to bounded runner with the default timeout. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `run_script_with_timeout` | L48-L62 | `orchestration` | LOW | Sequences child spawn, stream drains, wait, success validation helper, and stdout return. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_refresh_command` | L64-L69 | `orchestration` | LOW | Builds and spawns the refresh command, delegating error text to a formatter helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_quota_script` | L71-L76 | `orchestration` | LOW | Builds and spawns the quota script, delegating error text to a formatter helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `shell_command` | L78-L84 | `mapper` | LOW | Maps a shell command string into a configured `Command`. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `configure_script_process_group` unix | L86-L91 | `mapper` | LOW | Maps a `Command` to process-group configured state. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `configure_script_process_group` non-unix | L93-L94 | `mapper` | LOW | No-op mapping for unsupported platforms. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_child_stdout` | L96-L98 | `accessor` | LOW | Takes child stdout and exposes it to the drain helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_child_stderr` | L100-L102 | `accessor` | LOW | Takes child stderr and exposes it to the drain helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `spawn_string_drain` | L104-L109 | `orchestration` | LOW | Starts a drain thread and delegates stream reading to `drain_to_string`. |
| `crates/oulipoly-runtime/src/quota/process.rs` | closure `move || drain_to_string(reader)` | L108 | `orchestration` | LOW | Dispatches only to the named drain helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `drain_to_string` | L111-L115 | `accessor` | LOW | Reads stream contents into a string. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `wait_for_child` | L117-L130 | `orchestration` | LOW | Poll/sleep loop delegates step evaluation and finalization. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `try_wait_child` | L132-L137 | `accessor` | LOW | Retrieves child wait state and maps only error text through helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | closure `|e| format_wait_error(kind, e)` | L136 | `formatter` | LOW | Formats a wait error through a named helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `wait_step` | L139-L149 | `orchestration` | LOW | Sequences child wait result and timeout-step helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `timeout_wait_step` | L151-L156 | `mapper` | LOW | Maps elapsed timeout state to a `WaitStep` enum. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `finish_wait_step` | L158-L169 | `orchestration` | LOW | Dispatches the wait outcome to completion or timeout kill helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_timed_out_child` | L171-L178 | `orchestration` | LOW | Sequences process-group kill and timeout-error helper. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_child_process_group` unix | L180-L189 | `orchestration` | LOW | Sequences process-group kill, child kill, and wait. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `kill_child_process_group` non-unix | L191-L195 | `orchestration` | LOW | Sequences child kill and wait. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `joined_text` | L197-L199 | `accessor` | LOW | Retrieves joined thread output with default. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `ensure_refresh_success` | L201-L206 | `validator` | LOW | Accepts successful status or returns refresh failure. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `ensure_quota_success` | L208-L213 | `validator` | LOW | Accepts successful status or returns quota failure. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_refresh_spawn_error` | L227-L229 | `formatter` | LOW | Formats refresh spawn failure text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_quota_spawn_error` | L231-L233 | `formatter` | LOW | Formats quota spawn failure text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_timeout` | L235-L244 | `formatter` | LOW | Formats auth/quota timeout messages. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_wait_error` | L246-L251 | `formatter` | LOW | Formats auth/quota wait errors. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_refresh_exit` | L253-L259 | `formatter` | LOW | Formats refresh nonzero exit text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `format_quota_exit` | L261-L267 | `formatter` | LOW | Formats quota nonzero exit text. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `quota_script_timeout_is_classified` | L274-L280 | `validator` | LOW | Test validates timeout error tokens. |
| `crates/oulipoly-runtime/src/quota/process.rs` | `quota_script_timeout_kills_process_group_children` | L282-L300 | `validator` | LOW | Test validates timeout token and leaked-marker absence. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_canonical_body_shape` | L76-L81 | `predicate` | LOW | Answers whether a body is a canonical chunk array. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_canonical_body_chunk` | L83-L89 | `predicate` | LOW | Answers whether one chunk has canonical fields. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_provider` | L96-L102 | `orchestration` | LOW | Delegates scan to timeout-aware helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_provider_with_timeout` | L104-L128 | `orchestration` | LOW | Sequences source lookup, state dir creation, script run, batch collection, and persistence helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `provider_session_source` | L130-L135 | `accessor` | LOW | Retrieves provider session source entry. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_report_with_error` | L137-L140 | `mapper` | LOW | Maps a report plus error into an updated report. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `create_session_state_dir` | L142-L145 | `orchestration` | LOW | Calls directory creation and delegates error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|error| format_state_dir_create_error(...)` | L144 | `formatter` | LOW | Formats create-dir error through helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_state_dir_create_error` | L147-L152 | `formatter` | LOW | Formats state-dir creation failure text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `collect_turn_script_batch` | L154-L175 | `orchestration` | LOW | Iterates lines and dispatches marker, parse, record-error, and push helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `degraded_marker_error` | L177-L183 | `orchestration` | LOW | Composes marker parse, predicate, count accessor, and formatter helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_degraded_marker_jsonl` | L185-L187 | `parser` | LOW | Parses one JSONL line as JSON. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `is_degraded_marker` | L189-L194 | `predicate` | LOW | Answers whether JSON is the degraded marker. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `degraded_marker_count` | L196-L201 | `accessor` | LOW | Retrieves optional degraded-marker count. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_degraded_marker_error` | L203-L205 | `formatter` | LOW | Formats degraded-marker error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `non_empty_script_lines` | L207-L209 | `orchestration` | LOW | Composes trim mapper and non-empty filter helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `trimmed_script_lines` | L211-L213 | `mapper` | LOW | Maps stdout lines to trimmed line values. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `non_empty_trimmed_lines` | L215-L217 | `filter` | LOW | Filters empty trimmed lines. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|line| !line.is_empty()` | L216 | `predicate` | LOW | Answers whether a line is non-empty. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_script_line_seen` | L219-L222 | `mapper` | LOW | Updates report line count and returns it. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_optional_scan_error` | L224-L228 | `orchestration` | LOW | Dispatches optional error to the recorder helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `record_scan_error` | L230-L232 | `mapper` | LOW | Adds an error to the report. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `push_script_turn_ingest` | L234-L236 | `mapper` | LOW | Adds an ingest row to the batch. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_line_to_ingest` | L238-L255 | `orchestration` | LOW | Dispatches parse, timestamp parse, body validation, serialization, and ingest mapping helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parsed_script_turn_ingest` | L267-L272 | `mapper` | LOW | Maps ingest plus body error to `ParsedScriptTurnIngest`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_script_turn_line` | L274-L276 | `parser` | LOW | Parses JSONL into `ScriptTurn`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|error| format_malformed_turn_line(...)` | L275 | `formatter` | LOW | Formats malformed-line parse error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_malformed_turn_line` | L278-L280 | `formatter` | LOW | Formats malformed turn line text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `parse_script_turn_timestamp` | L282-L286 | `parser` | LOW | Parses RFC3339 timestamp to UTC. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|timestamp| timestamp.with_timezone(&Utc)` | L284 | `mapper` | LOW | Maps parsed timestamp to UTC. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|error| format_bad_timestamp(...)` | L285 | `formatter` | LOW | Formats timestamp parse error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_bad_timestamp` | L288-L290 | `formatter` | LOW | Formats bad timestamp text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_script_turn_body` | L292-L304 | `validator` | LOW | Accepts missing/valid body or returns rejected body state. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_script_turn_body_shape` | L306-L316 | `validator` | LOW | Accepts canonical body shape or returns validation error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_invalid_body_shape` | L318-L322 | `formatter` | LOW | Formats invalid body-shape error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `serialize_script_turn_body` | L324-L332 | `formatter` | LOW | Serializes body for storage and delegates serialization error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|error| format_body_serialize_error(...)` | L331 | `formatter` | LOW | Formats body serialization error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `selected_script_turn_body` | L334-L339 | `accessor` | LOW | Exposes selected accepted body. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `serialize_selected_script_turn_body` | L341-L349 | `orchestration` | LOW | Dispatches optional body to serialization helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|body| serialize_script_turn_body(...)` | L346 | `formatter` | LOW | Dispatches body to serializer helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_body_serialize_error` | L351-L357 | `formatter` | LOW | Formats body serialization error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_to_ingest` | L359-L374 | `mapper` | LOW | Maps `ScriptTurn` fields into `SessionTurnIngest`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_ingest_from_parts` | L376-L383 | `mapper` | LOW | Maps parts into parsed ingest wrapper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `script_turn_body_error` | L385-L390 | `accessor` | LOW | Retrieves optional body-validation error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `persist_scanned_turns` | L392-L402 | `orchestration` | LOW | Sequences batch ingest and imported-chain persistence/error recording. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `persist_imported_chains` | L404-L422 | `orchestration` | LOW | Sets report count and delegates per-turn chain minting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `scan_all` | L426-L434 | `orchestration` | LOW | Iterates configured providers, scans, records, and sorts results. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|a, b| a.0.cmp(&b.0)` | L432 | `accessor` | LOW | Exposes tuple keys for sort comparison. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `resolve_state_dir` | L436-L442 | `accessor` | LOW | Retrieves configured state dir or default provider path. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `default_app_data_dir` | L444-L447 | `accessor` | LOW | Retrieves app data dir with fallback. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|_| PathBuf::from(...).join(...)` | L446 | `mapper` | LOW | Maps fallback error case to fallback path. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_turn_script` | L449-L455 | `orchestration` | LOW | Delegates to session-script runner with turn-script label. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `locate_transcript` | L457-L476 | `orchestration` | LOW | Sequences source lookup, locator lookup, script run, line validation, and path conversion helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_source_entry` | L478-L483 | `accessor` | LOW | Retrieves provider session source entry. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `transcript_locator_script` | L485-L487 | `accessor` | LOW | Retrieves optional transcript locator script. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `single_transcript_stdout_line` | L489-L495 | `validator` | LOW | Accepts exactly one non-empty stdout line or returns error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `transcript_path_from_line` | L497-L499 | `mapper` | LOW | Maps stdout line to `PathBuf`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_session_script` | L501-L514 | `orchestration` | LOW | Delegates to timeout-aware script runner. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `run_session_script_with_timeout` | L516-L537 | `orchestration` | LOW | Sequences command creation, spawn, stream draining, wait, validation helper, and stdout return. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `validate_session_script_success` | L539-L553 | `validator` | LOW | Accepts success status or returns nonzero error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `session_script_command` | L555-L571 | `mapper` | LOW | Maps script/state/session inputs into a configured `Command`. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `configure_session_script_process_group` unix | L573-L578 | `mapper` | LOW | Maps command to process-group configured state. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `configure_session_script_process_group` non-unix | L580-L581 | `mapper` | LOW | No-op mapping for unsupported platforms. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_session_script_child` | L583-L586 | `orchestration` | LOW | Spawns child and delegates spawn-error formatting. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `|error| format_session_script_spawn_error(...)` | L585 | `formatter` | LOW | Formats spawn error through helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_spawn_error` | L588-L590 | `formatter` | LOW | Formats session script spawn error. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `take_child_stdout` | L592-L594 | `accessor` | LOW | Retrieves child stdout pipe. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `take_child_stderr` | L596-L598 | `accessor` | LOW | Retrieves child stderr pipe. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_stdout_reader` | L600-L602 | `orchestration` | LOW | Dispatches stdout pipe to generic reader spawn helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_stderr_reader` | L604-L606 | `orchestration` | LOW | Dispatches stderr pipe to generic reader spawn helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `spawn_script_reader` | L608-L613 | `orchestration` | LOW | Starts reader thread and dispatches stream drain. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | closure `move || drain_script_reader_to_string(...)` | L612 | `orchestration` | LOW | Dispatches only to named drain helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `drain_script_reader_to_string` | L615-L622 | `accessor` | LOW | Reads script stream to a string. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `wait_for_session_script` | L624-L640 | `orchestration` | LOW | Poll loop delegates poll, pending wait, and error formatting helpers. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `poll_session_script` | L642-L644 | `accessor` | LOW | Retrieves child wait state. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `wait_for_pending_session_script` | L646-L658 | `orchestration` | LOW | Dispatches timeout predicate to failure helper or sleep helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `pending_session_script_timed_out` | L660-L665 | `predicate` | LOW | Answers whether elapsed time exceeded timeout. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `fail_timed_out_pending_session_script` | L667-L677 | `orchestration` | LOW | Sequences timeout kill and timeout message helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_timed_out_pending_session_script` | L679-L681 | `orchestration` | LOW | Delegates timeout kill to process-group helper. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_session_script_process_group` unix | L683-L692 | `orchestration` | LOW | Sequences process-group kill, child kill, and wait. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `kill_session_script_process_group` non-unix | L694-L698 | `orchestration` | LOW | Sequences child kill and wait. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `sleep_before_next_session_script_poll` | L700-L702 | `orchestration` | LOW | Sleeps between polls. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_pending_session_script_timeout` | L704-L706 | `formatter` | LOW | Formats timeout message. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_wait_error` | L708-L713 | `formatter` | LOW | Formats wait error text. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `join_script_reader` | L715-L717 | `accessor` | LOW | Retrieves joined reader output. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `format_session_script_nonzero` | L719-L730 | `formatter` | LOW | Formats nonzero exit message. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | `capitalize_script_kind` | L732-L738 | `formatter` | LOW | Formats script kind with leading uppercase. |
| `crates/oulipoly-runtime/src/sessions/mod.rs` | test helpers and tests | L747-L1137 | `validator` / `mapper` / `orchestration` / `formatter` | LOW | Individual test helpers and test bodies are single-purpose: fixture/config builders map or orchestrate fixtures; test bodies validate expected scan/locator behavior. No multi-classifier test symbol observed in this file. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::new` | L78-L97 | `orchestration` | LOW | Creates temp fixture resources and returns fixture. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::set_mode` | L99-L101 | `mapper` | LOW | Updates fixture mode file. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::registry` | L103-L111 | `mapper` | LOW | Maps fixture paths to provider registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::hostile_registry` | L113-L121 | `mapper` | LOW | Maps fixture paths to hostile registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::timeout_registry` | L123-L133 | `mapper` | LOW | Maps fixture provider path and timeout options to registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::unrelated_registry` | L135-L143 | `mapper` | LOW | Maps unrelated model fixture to registry. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::records` | L145-L147 | `accessor` | LOW | Retrieves provider record values. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::request_records_for` | L149-L154 | `filter` | LOW | Filters fixture records by subcommand. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | closure `|record| record["subcommand"] == subcommand` | L152 | `predicate` | LOW | Answers whether a record matches the subcommand. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::snapshot` | L156-L158 | `accessor` | LOW | Retrieves SQLite snapshot. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::state_path` | L160-L162 | `accessor` | LOW | Retrieves fixture state path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::conn` | L164-L166 | `accessor` | LOW | Opens fixture connection. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::seed_finalized_invocation` | L168-L183 | `orchestration` | LOW | Sequences invocation start/finalize fixture setup. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `Fixture::seed_chain` | L185-L200 | `orchestration` | LOW | Sequences fixture SQL inserts. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_text` | L203-L205 | `accessor` | LOW | Reads provider record text. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_values` | L207-L209 | `parser` | LOW | Parses record text lines through JSON parser helper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_record_value` | L211-L213 | `parser` | LOW | Parses one JSON record line. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `external_model` | L215-L229 | `mapper` | LOW | Maps model/provider inputs into `ModelConfig`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `provider_identity` | L231-L238 | `mapper` | LOW | Maps constants into session provider identity. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `locate_request` | L240-L252 | `mapper` | LOW | Maps registry/session/mode to locate request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `read_request` | L254-L264 | `mapper` | LOW | Maps registry/session to read-turns request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `capture_request` | L266-L276 | `mapper` | LOW | Maps registry/invocation UUID to capture request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | test functions | L278-L824 | `validator` | LOW | Test bodies validate session dispatch, transport, schema, timeout, and host-mutation behavior. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | closures in read-turn assertions | L446-L448, L579, L805 | `accessor` / `mapper` | LOW | Inline closures expose nested values or map records/turns for assertions. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `assert_error_token` | L826-L832 | `validator` | LOW | Validates expected stable token in error text. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::new` | L840-L853 | `orchestration` | LOW | Creates fixture, seeds invocation/chain, stores row id. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::request_without_registry` | L855-L865 | `mapper` | LOW | Maps fixture state/constants into no-ref request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::request_with_registry` | L867-L875 | `mapper` | LOW | Maps registry plus base request into registry-backed request. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `NoRefDispatchProofFixture::snapshot` | L877-L879 | `accessor` | LOW | Retrieves fixture snapshot. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `assert_no_ref_dispatch_fixture_row_id` | L882-L887 | `validator` | LOW | Validates fresh fixture row id. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `hostile_marker` | L889-L891 | `mapper` | LOW | Maps route to hostile marker path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `DataDirOverride::remove` | L898-L906 | `orchestration` | LOW | Captures and removes env override for fixture setup. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `DataDirOverride::drop` | L910-L919 | `orchestration` | LOW | Restores prior env override. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `hostile_markers_json` | L921-L931 | `formatter` | LOW | Formats hostile marker paths as JSON. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | closure `|route| dir.join(format!(...))` | L922 | `formatter` | LOW | Formats one hostile marker path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `assert_request_shape` | L933-L948 | `validator` | LOW | Validates request record shape. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `sqlite_snapshot` | L950-L957 | `mapper` | LOW | Maps connection to snapshot struct. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `invocation_snapshot_rows` | L959-L967 | `accessor` | LOW | Retrieves invocation snapshot rows. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `invocation_snapshot_row` | L969-L977 | `mapper` | LOW | Maps SQLite row into tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_turn_snapshot_rows` | L979-L987 | `accessor` | LOW | Retrieves session turn snapshot rows. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_turn_snapshot_row` | L989-L1000 | `mapper` | LOW | Maps SQLite row into tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_snapshot_rows` | L1002-L1008 | `accessor` | LOW | Retrieves chain snapshot rows. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_snapshot_row` | L1010-L1012 | `mapper` | LOW | Maps SQLite row into tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_segment_snapshot_rows` | L1014-L1021 | `accessor` | LOW | Retrieves chain segment snapshot rows. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `session_chain_segment_snapshot_row` | L1023-L1027 | `mapper` | LOW | Maps SQLite row into tuple. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `query_rows` | L1029-L1038 | `orchestration` | LOW | Sequences query preparation, row mapping, and collection through caller mapper. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | closures in `query_rows` | L1034, L1036 | `mapper` / `validator` | LOW | One applies caller mapper; one unwraps expected row result. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `parse_ts` | L1040-L1044 | `parser` | LOW | Parses RFC3339 timestamp to UTC. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `write_fake_provider` | L1046-L1062 | `orchestration` | LOW | Writes fake provider script and chmods it. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `fake_provider_body` | L1064-L1338 | `formatter` | LOW | Formats fake provider Python source text from fixture values. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `json_string` | L1340-L1342 | `formatter` | LOW | Formats path as JSON string. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `repo_script_path` | L1344-L1349 | `accessor` | LOW | Retrieves repo script path. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `opencode_sessions_config` | L1351-L1372 | `mapper` | LOW | Maps script/bin/root/state inputs into `SessionsConfig`. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `write_fake_opencode` | L1374-L1417 | `orchestration` | LOW | Writes and chmods fake OpenCode executable. |
| `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs` | `shell_single_quote_path` | L1419-L1421 | `formatter` | LOW | Formats shell-quoted path. |
| `scripts/opencode-turns` | `Options.__init__` | L66-L71 | `orchestration` | LOW | Assembles option fields by dispatching to env parser/validator helpers. |
| `scripts/opencode-turns` | `Deadline.__init__` | L74-L77 | `mapper` | LOW | Maps duration into deadline state. |
| `scripts/opencode-turns` | `Deadline.remaining` | L79-L80 | `accessor` | LOW | Retrieves remaining deadline budget. |
| `scripts/opencode-turns` | `Deadline.call_timeout` | L82-L86 | `filter` | LOW | Bounds per-call timeout by remaining deadline. |
| `scripts/opencode-turns` | `env_float` | L89-L90 | `orchestration` | LOW | Delegates env float parsing and positive/default validation to named helpers. |
| `scripts/opencode-turns` | `parsed_env_float` | L93-L100 | `parser` | LOW | Parses an environment value into a float candidate or absence. |
| `scripts/opencode-turns` | `positive_float_or_default` | L103-L106 | `validator` | LOW | Accepts positive float candidates or returns the default. |
| `scripts/opencode-turns` | `env_int` | L109-L110 | `orchestration` | LOW | Delegates env int parsing and positive/default validation to named helpers. |
| `scripts/opencode-turns` | `parsed_env_int` | L113-L120 | `parser` | LOW | Parses an environment value into an integer candidate or absence. |
| `scripts/opencode-turns` | `positive_int_or_default` | L123-L126 | `validator` | LOW | Accepts positive integer candidates or returns the default. |
| `scripts/opencode-turns` | `text_chunk` | L129-L130 | `mapper` | LOW | Maps text into canonical chunk dict. |
| `scripts/opencode-turns` | `canonical_chunk_type` | L133-L138 | `mapper` | LOW | Maps native chunk type names to canonical type names. |
| `scripts/opencode-turns` | `extract_content_chunks` | L141-L148 | `orchestration` | LOW | Dispatches content extraction by value shape to helpers. |
| `scripts/opencode-turns` | `content_chunks_from_text` | L151-L152 | `mapper` | LOW | Maps text to chunk list. |
| `scripts/opencode-turns` | `content_chunks_from_items` | L155-L159 | `orchestration` | LOW | Iterates items and delegates extraction. |
| `scripts/opencode-turns` | `content_chunks_from_obj` | L162-L166 | `orchestration` | LOW | Dispatches object extraction to direct/nested helpers. |
| `scripts/opencode-turns` | `direct_content_chunks_from_obj` | L169-L173 | `orchestration` | LOW | Selects direct content candidate parsing and canonical chunk mapping helpers. |
| `scripts/opencode-turns` | `direct_content_candidate_from_obj` | L176-L182 | `parser` | LOW | Extracts accepted direct text/type candidates from content dictionaries. |
| `scripts/opencode-turns` | `direct_content_chunks_from_candidate` | L185-L186 | `mapper` | LOW | Maps accepted content candidate into canonical chunk dictionaries. |
| `scripts/opencode-turns` | `nested_content_chunks_from_obj` | L189-L194 | `orchestration` | LOW | Iterates known nested keys and delegates recursive extraction. |
| `scripts/opencode-turns` | `unique_values` | L197-L204 | `filter` | LOW | Deduplicates values while preserving order. |
| `scripts/opencode-turns` | `session_ids_from_value` | L207-L220 | `parser` | LOW | Recursively extracts session IDs from external values. |
| `scripts/opencode-turns` | `parse_session_list_stdout` | L223-L232 | `orchestration` | LOW | Coordinates parse, candidate extraction, fallback cap, and candidate selection helpers. |
| `scripts/opencode-turns` | `parse_session_list_json` | L235-L239 | `parser` | LOW | Parses OpenCode session-list stdout JSON or sentinel. |
| `scripts/opencode-turns` | `capped_session_ids_from_value` | L242-L243 | `filter` | LOW | Applies max-session cap after delegated ID extraction. |
| `scripts/opencode-turns` | `session_ids_from_candidates` | L246-L249 | `orchestration` | LOW | Dispatches timestamp-window or max-cap selection helpers. |
| `scripts/opencode-turns` | `candidates_have_timestamps` | L252-L253 | `predicate` | LOW | Answers whether candidates include timestamps. |
| `scripts/opencode-turns` | `recent_session_ids` | L256-L259 | `orchestration` | LOW | Composes recent-candidate filter and ID projection helpers. |
| `scripts/opencode-turns` | `recent_session_candidates` | L262-L268 | `filter` | LOW | Filters timestamped candidates to the recent quota-balancing window. |
| `scripts/opencode-turns` | `capped_candidate_session_ids` | L271-L274 | `orchestration` | LOW | Composes max-cap filter and ID projection helpers. |
| `scripts/opencode-turns` | `capped_session_candidates` | L277-L278 | `filter` | LOW | Applies the max-session cap while preserving candidate shape. |
| `scripts/opencode-turns` | `session_ids_from_candidates_rows` | L281-L282 | `mapper` | LOW | Projects candidate rows to session ID strings. |
| `scripts/opencode-turns` | `unique_session_candidates` | L285-L294 | `filter` | LOW | Deduplicates candidate dictionaries while preserving candidate shape. |
| `scripts/opencode-turns` | `session_candidates_from_value` | L297-L302 | `orchestration` | LOW | Dispatches candidate extraction by value shape. |
| `scripts/opencode-turns` | `session_candidates_from_items` | L305-L309 | `orchestration` | LOW | Iterates items and delegates recursive candidate extraction. |
| `scripts/opencode-turns` | `session_candidates_from_obj` | L312-L315 | `orchestration` | LOW | Routes candidate-shaped object to mapper or recurses into values. |
| `scripts/opencode-turns` | `has_session_candidate_shape` | L318-L319 | `predicate` | LOW | Answers whether object has recognizable session ID. |
| `scripts/opencode-turns` | `session_candidate_from_obj` | L322-L326 | `mapper` | LOW | Maps object to candidate row using parser helpers. |
| `scripts/opencode-turns` | `session_list_session_id` | L329-L337 | `parser` | LOW | Extracts session ID from known fields using regex. |
| `scripts/opencode-turns` | `session_list_timestamp` | L340-L345 | `parser` | LOW | Extracts timestamp from known fields through timestamp parser. |
| `scripts/opencode-turns` | `timestamp_datetime` | L348-L363 | `parser` | LOW | Parses numeric/string timestamp input into UTC datetime. |
| `scripts/opencode-turns` | `numeric_timestamp_datetime` | L366-L371 | `parser` | LOW | Parses numeric seconds/milliseconds into UTC datetime. |
| `scripts/opencode-turns` | `discover_session_ids` | L374-L380 | `orchestration` | LOW | Runs public CLI and delegates timeout/failure/parse handling. |
| `scripts/opencode-turns` | `requested_session_ids` | L383-L388 | `orchestration` | LOW | Selects explicit IDs or discovery helper. |
| `scripts/opencode-turns` | `numeric_timestamp` | L391-L395 | `orchestration` | LOW | Selects numeric timestamp acceptance and formatting helpers. |
| `scripts/opencode-turns` | `accepted_numeric_timestamp` | L398-L401 | `validator` | LOW | Accepts numeric timestamp candidates. |
| `scripts/opencode-turns` | `formatted_numeric_timestamp` | L404-L406 | `formatter` | LOW | Formats accepted numeric timestamps as UTC text. |
| `scripts/opencode-turns` | `timestamp_from_obj` | L409-L417 | `parser` | LOW | Extracts turn timestamp from known fields. |
| `scripts/opencode-turns` | `session_id_from_obj` | L420-L425 | `parser` | LOW | Extracts session ID from known fields. |
| `scripts/opencode-turns` | `role_from_obj` | L428-L435 | `parser` | LOW | Extracts accepted role from top-level or nested message object. |
| `scripts/opencode-turns` | `turn_id_from_obj` | L438-L439 | `orchestration` | LOW | Selects parsed ID or fallback helper. |
| `scripts/opencode-turns` | `turn_id_field_from_obj` | L442-L449 | `parser` | LOW | Extracts turn ID from top-level or nested mapping. |
| `scripts/opencode-turns` | `turn_id_field_from_mapping` | L452-L457 | `parser` | LOW | Extracts turn ID from known fields. |
| `scripts/opencode-turns` | `fallback_turn_id` | L460-L461 | `formatter` | LOW | Formats deterministic fallback turn ID. |
| `scripts/opencode-turns` | `opencode_command` | L464-L466 | `parser` | LOW | Parses configured command string into argv tokens. |
| `scripts/opencode-turns` | `run_opencode` | L469-L487 | `orchestration` | LOW | Sequences deadline check, process spawn, communicate, timeout kill, failure, and result helpers. |
| `scripts/opencode-turns` | `opencode_deadline_expired` | L490-L491 | `predicate` | LOW | Answers whether timeout budget is exhausted. |
| `scripts/opencode-turns` | `spawn_opencode_process` | L494-L502 | `orchestration` | LOW | Spawns OpenCode process with configured stdio/session settings. |
| `scripts/opencode-turns` | `communicate_opencode_process` | L505-L512 | `orchestration` | LOW | Waits for subprocess and reports timeout status. |
| `scripts/opencode-turns` | `opencode_process_failed` | L515-L516 | `predicate` | LOW | Answers whether subprocess returned nonzero. |
| `scripts/opencode-turns` | `degraded_opencode_result` | L519-L520 | `mapper` | LOW | Maps degraded timeout path to result tuple. |
| `scripts/opencode-turns` | `failed_opencode_result` | L523-L524 | `mapper` | LOW | Maps failed CLI path to result tuple. |
| `scripts/opencode-turns` | `successful_opencode_result` | L527-L528 | `mapper` | LOW | Maps stdout to success tuple. |
| `scripts/opencode-turns` | `kill_process_group` | L531-L546 | `orchestration` | LOW | Sequences process-group kill/fallback kill/drain. |
| `scripts/opencode-turns` | `parse_export_stdout` | L549-L553 | `parser` | LOW | Parses export stdout JSON. |
| `scripts/opencode-turns` | `export_session` | L556-L564 | `orchestration` | LOW | Runs export command and delegates timeout/failure/parse handling. |
| `scripts/opencode-turns` | `exported_message_items` | L567-L571 | `orchestration` | LOW | Coordinates message-item extraction and dictionary filtering helpers. |
| `scripts/opencode-turns` | `exported_message_item_values` | L574-L585 | `parser` | LOW | Extracts raw item list from supported export shapes. |
| `scripts/opencode-turns` | `dict_items` | L588-L589 | `filter` | LOW | Keeps only dictionary items. |
| `scripts/opencode-turns` | `record_from_message` | L592-L599 | `orchestration` | LOW | Coordinates field extraction, validation, ID selection, mapping, and optional body helper dispatch. |
| `scripts/opencode-turns` | `message_record_fields` | L602-L607 | `parser` | LOW | Extracts required normalized record fields. |
| `scripts/opencode-turns` | `has_required_message_record_fields` | L610-L611 | `validator` | LOW | Validates required record fields are present. |
| `scripts/opencode-turns` | `message_record_from_fields` | L614-L620 | `mapper` | LOW | Maps validated fields plus turn ID to normalized record. |
| `scripts/opencode-turns` | `record_with_optional_body` | L623-L626 | `mapper` | LOW | Maps base record and optional body into emitted record. |
| `scripts/opencode-turns` | `message_body_chunks` | L629-L634 | `parser` | LOW | Extracts optional body chunks from supported content fields. |
| `scripts/opencode-turns` | `records_from_exported_session` | L637-L643 | `mapper` | LOW | Maps exported message items into normalized records. |
| `scripts/opencode-turns` | `collect_records` | L646-L659 | `orchestration` | LOW | Iterates sessions, exports, stops on timeout, and accumulates record helpers. |
| `scripts/opencode-turns` | `emit_record` | L662-L664 | `formatter` | LOW | Emits compact JSONL record. |
| `scripts/opencode-turns` | `assistant_record_count` | L667-L668 | `filter` | LOW | Counts assistant-role records. |
| `scripts/opencode-turns` | `emit_degraded_marker` | L671-L672 | `formatter` | LOW | Emits degraded marker JSONL. |
| `scripts/opencode-turns` | `has_base_dir_arg` | L675-L676 | `validator` | LOW | Validates argv length for required base-dir slot. |
| `scripts/opencode-turns` | `usage_message` | L679-L680 | `formatter` | LOW | Formats usage text. |
| `scripts/opencode-turns` | `emit_usage` | L683-L684 | `formatter` | LOW | Emits usage text. |
| `scripts/opencode-turns` | `session_args_from_argv` | L687-L688 | `accessor` | LOW | Exposes explicit session ID arguments. |
| `scripts/opencode-turns` | `main` | L691-L707 | `orchestration` | LOW | Coordinates argv handling, options/deadline, collection, record emission, and degraded marker emission. |
| `scripts/tests/opencode-turns.test.sh` | `fail` | L10-L13 | `formatter` | LOW | Emits supplied failure text and exits. |
| `scripts/tests/opencode-turns.test.sh` | `values_equal` | L15-L17 | `predicate` | LOW | Answers whether two assertion values are equal. |
| `scripts/tests/opencode-turns.test.sh` | `assert_eq_failure_message` | L19-L25 | `formatter` | LOW | Formats equality assertion diagnostics. |
| `scripts/tests/opencode-turns.test.sh` | `assert_eq` | L27-L35 | `validator` | LOW | Enforces equality by delegating comparison and failure-message construction. |
| `scripts/tests/opencode-turns.test.sh` | `status_zero` | L37-L39 | `predicate` | LOW | Answers whether captured exit status is zero. |
| `scripts/tests/opencode-turns.test.sh` | `status_zero_failure_message` | L41-L46 | `formatter` | LOW | Formats exit-status diagnostics with stderr evidence. |
| `scripts/tests/opencode-turns.test.sh` | `assert_status_zero` | L48-L55 | `validator` | LOW | Enforces successful adapter exit status. |
| `scripts/tests/opencode-turns.test.sh` | `stdout_contains_pattern` | L57-L59 | `predicate` | LOW | Answers whether stdout contains a fixed-string pattern. |
| `scripts/tests/opencode-turns.test.sh` | `stdout_contains_failure_message` | L61-L66 | `formatter` | LOW | Formats missing-stdout-pattern diagnostics. |
| `scripts/tests/opencode-turns.test.sh` | `assert_stdout_contains` | L68-L75 | `validator` | LOW | Enforces fixed-string presence in captured stdout. |
| `scripts/tests/opencode-turns.test.sh` | `stdout_excludes_pattern` | L77-L79 | `predicate` | LOW | Answers whether stdout excludes a fixed-string pattern. |
| `scripts/tests/opencode-turns.test.sh` | `stdout_unexpected_failure_message` | L81-L86 | `formatter` | LOW | Formats unexpected-stdout-pattern diagnostics. |
| `scripts/tests/opencode-turns.test.sh` | `assert_stdout_not_contains` | L88-L95 | `validator` | LOW | Enforces fixed-string absence from captured stdout. |
| `scripts/tests/opencode-turns.test.sh` | `file_size_bytes` | L97-L105 | `accessor` | LOW | Retrieves file size or zero when absent. |
| `scripts/tests/opencode-turns.test.sh` | `marker_size_sample` | L107-L113 | `accessor` | LOW | Samples descendant-marker file size before and after a wait. |
| `scripts/tests/opencode-turns.test.sh` | `marker_size_stable` | L115-L117 | `predicate` | LOW | Answers whether marker size samples are equal. |
| `scripts/tests/opencode-turns.test.sh` | `marker_growth_failure_message` | L119-L125 | `formatter` | LOW | Formats descendant-marker growth diagnostics. |
| `scripts/tests/opencode-turns.test.sh` | `assert_marker_stopped_growing` | L127-L139 | `validator` | LOW | Enforces descendant-marker stability through named sampling, predicate, and formatter helpers. |
| `scripts/tests/opencode-turns.test.sh` | `process_state` | L141-L149 | `accessor` | LOW | Retrieves descendant process state or absent sentinel. |
| `scripts/tests/opencode-turns.test.sh` | `process_state_allowed` | L151-L155 | `predicate` | LOW | Answers whether process state is allowed after timeout cleanup. |
| `scripts/tests/opencode-turns.test.sh` | `process_running_failure_message` | L157-L163 | `formatter` | LOW | Formats surviving-descendant diagnostics. |
| `scripts/tests/opencode-turns.test.sh` | `assert_process_not_running` | L165-L175 | `validator` | LOW | Enforces descendant process cleanup through named accessor, predicate, and formatter helpers. |
| `scripts/tests/opencode-turns.test.sh` | `write_executable_mock` | L177-L183 | `orchestration` | LOW | Materializes a mock executable by running a body emitter and chmod. |
| `scripts/tests/opencode-turns.test.sh` | `emit_timestampless_cap_mock_body` | L185-L213 | `formatter` | LOW | Emits the timestampless-cap mock OpenCode script body. |
| `scripts/tests/opencode-turns.test.sh` | `write_timestampless_cap_mock` | L215-L219 | `orchestration` | LOW | Materializes the timestampless-cap mock through named helper dispatch. |
| `scripts/tests/opencode-turns.test.sh` | `emit_window_filter_mock_body` | L221-L253 | `formatter` | LOW | Emits the timestamp-window mock OpenCode script body. |
| `scripts/tests/opencode-turns.test.sh` | `write_window_filter_mock` | L255-L259 | `orchestration` | LOW | Materializes the timestamp-window mock through named helper dispatch. |
| `scripts/tests/opencode-turns.test.sh` | `emit_timeout_mock_body` | L261-L289 | `formatter` | LOW | Emits the timeout mock OpenCode script body. |
| `scripts/tests/opencode-turns.test.sh` | `write_timeout_mock` | L291-L295 | `orchestration` | LOW | Materializes the timeout mock through named helper dispatch. |
| `scripts/tests/opencode-turns.test.sh` | `emit_descendant_timeout_mock_body` | L297-L325 | `formatter` | LOW | Emits the descendant-timeout mock OpenCode script body. |
| `scripts/tests/opencode-turns.test.sh` | `write_descendant_timeout_mock` | L327-L331 | `orchestration` | LOW | Materializes the descendant-timeout mock through named helper dispatch. |
| `scripts/tests/opencode-turns.test.sh` | `run_opencode_turns` | L333-L352 | `orchestration` | LOW | Coordinates temp stdout/stderr/export-log setup and adapter invocation. |
| `scripts/tests/opencode-turns.test.sh` | `test_timestampless_session_list_applies_max_sessions_cap` | L354-L371 | `validator` | LOW | Validates capped timestampless session behavior via assertion helpers. |
| `scripts/tests/opencode-turns.test.sh` | `test_exports_only_recent_window_sessions` | L373-L390 | `validator` | LOW | Validates recent-window export selection via assertion helpers. |
| `scripts/tests/opencode-turns.test.sh` | `test_timeout_emits_degraded_best_effort_and_exits_zero` | L392-L413 | `validator` | LOW | Validates degraded timeout behavior and elapsed bound. |
| `scripts/tests/opencode-turns.test.sh` | `test_timeout_kills_opencode_process_group_descendant` | L415-L438 | `validator` | LOW | Validates degraded marker, descendant marker, and descendant process cleanup. |

## Multi-Classifier Findings

| ID | Path | Function / symbol | Categories mixed | Evidence | Suggested split | Blocking or residual | Finding origin | Domain relation |
|---|---|---|---|---|---|---|---|---|
| none | n/a | n/a | n/a | No function in the touched-file inventory inferred two or more A1 categories after applying the pure-orchestrator helper-dispatch rule. | n/a | n/a | n/a | n/a |

## Residual Ambiguity / Stop-Condition Notes

- Diff evidence identified five touched files: `crates/oulipoly-runtime/src/quota/process.rs`, `crates/oulipoly-runtime/src/sessions/mod.rs`, `crates/oulipoly-runtime/tests/age243_s7a_session_dispatch.rs`, `scripts/opencode-turns`, and `scripts/tests/opencode-turns.test.sh`.
- Markdown procedure prose, contract tables, YAML carriers, and heredoc/script literal contents were not admitted as separate A5 function inventory items. Shell functions and named Python/Rust functions in touched files were admitted. Trivial Rust closures found in touched files were included where they are executable closure bodies.
- `crates/oulipoly-runtime/src/sessions/mod.rs` contains many test functions and helpers; their bodies were reviewed as executable symbols. No multi-classifier finding was emitted there because the test bodies primarily validate behavior, while fixture builders and formatters are individually single-classified.
- No `NEEDS_INPUT` condition was encountered. The Phase 6 contract and proposal were readable before scoring.
- The previous split-sensitive shapes in `scripts/opencode-turns` and `scripts/tests/opencode-turns.test.sh` now delegate parser/validator/filter/mapper/formatter work to named helpers, so their remaining caller bodies classify as orchestration only under the pure-orchestrator recognition rule.

Verdict: LOW

VERDICT: LOW
