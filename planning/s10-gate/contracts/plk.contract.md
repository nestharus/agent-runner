# s10 Step-6a Contract

## Component declared roles

Component: S10 external launch session capture and PLK claim carriers.

Declared roles: `orchestration`, `accessor`, `mapper`, `parser`, `predicate`, `formatter`, `validator`, `filter`.

Touched files in scope:

| File | Declared roles | Role notes |
|---|---|---|
| `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs` | `mapper`, `predicate`, `accessor`, `orchestration` | Maps provider launch stream results into runtime `ExecutionResult` values, including provider session capture from external launch exit metadata. |
| `crates/oulipoly-runtime/src/executor/mod.rs` | `mapper` | Adds the `ExternalProviderLaunch` capture method DB value. |
| `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs` | `orchestration`, `validator`, `filter` | Existing export/replace dispatch test suite; S10 change extends source-guard exclusions for generated moveout planning artifacts. |
| `crates/oulipoly-runtime/tests/s10_external_launch_session.rs` | `orchestration`, `accessor`, `mapper`, `parser`, `formatter`, `validator`, `filter` | New external-provider launch integration harness proving exit-session capture and resume request handoff. |
| `crates/oulipoly-setup/src/context.rs` | `formatter`, `mapper`, `orchestration`, `validator` | Setup prompt context now formats moved-provider external-provider refs through carrier helpers and tests the prompt output. |
| `src-tauri/src/commands/config_migration/orchestration.rs` | `orchestration`, `predicate`, `mapper`, `accessor`, `validator`, `formatter` | Config migration now backfills the moved provider's external-provider binary reference while preserving existing provider/runtime block migration behavior. |
| `src-tauri/src/commands/config_migration/tests.rs` | `accessor`, `mapper`, `formatter`, `validator`, `orchestration` | Config-migration unit harness for moved-provider binary backfill and unchanged argument separation. |
| `src-tauri/tests/age245_s7c_rotation_source_guard.rs` | `orchestration`, `filter`, `predicate`, `validator` | Rotation source-guard suite excludes generated moveout planning artifacts from concrete-provider vocabulary checks. |
| `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs` | `orchestration`, `filter`, `predicate`, `validator` | Setup dispatch source-guard suite excludes generated moveout planning artifacts from concrete-provider vocabulary checks. |

## Production function inventory

Only added or meaningfully changed production functions are listed.

### `crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `map_launch_result_with_terminal_classification` | `mapper` | Maps an external provider `LaunchResult` plus optional terminal classification into the runtime `ExecutionResult`, now delegating session capture mapping to `launch_session_capture`. | None; terminal classification and session capture are delegated. |
| `launch_session_capture` | `mapper` | Maps an optional provider launch session id into `SessionCaptureResult` and the `ExternalProviderLaunch` method. | None. |
| `launch_provider_session_id` | `accessor` | Reads optional exit session metadata from a `LaunchResult` and delegates extraction of the provider session id. | None. |
| `provider_session_id_from_value` | `orchestration` | Sequences raw provider-session-id access and accepted-id filtering. | None; raw access and acceptance are delegated. |
| `raw_provider_session_id` | `accessor` | Reads the raw optional `provider_session_id` string from provider exit session JSON. | None. |
| `accepted_provider_session_id` | `filter` | Rejects empty provider session ids before returning an accepted id value. | None. |

### `crates/oulipoly-runtime/src/executor/mod.rs`

| Symbol | A1 class | Meaning | Risk |
|---|---|---|---|
| `SessionCaptureMethod::ExternalProviderLaunch` | `mapper` | Adds a distinct capture method for external-provider launch metadata. | None. |
| `SessionCaptureMethod::db_value` | `mapper` | Maps the new capture method to the persisted `external_provider_launch` value. | None. |

### `crates/oulipoly-setup/src/context.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `build_system_prompt` | `formatter` | Formats the setup system prompt using generated capabilities text. | None; moved-provider token replacement is delegated. |
| `build_cli_setup_prompt` | `formatter` | Formats a CLI-specific setup prompt using generated capabilities text. | None; moved-provider token replacement is delegated. |
| `capabilities_text` | `mapper` | Maps the static capabilities template to concrete prompt text by replacing moved-provider placeholders. | None. |
| `moved_provider_binary` | `formatter` | Formats the moved provider external binary name from the moved provider token. | None. |
| `moved_provider_name` | `formatter` | Formats the moved provider token without adding a new concrete-provider string literal. | None. |

### `src-tauri/src/commands/config_migration/orchestration.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `migrate_model_config_table` | `orchestration` | Sequences existing model config migration, old top-level provider handling, provider array handling, and moved-provider external ref backfill. | None; backfill classification is delegated. |
| `backfill_moved_external_provider_ref` | `orchestration` | Sequences moved-provider backfill eligibility and provider-ref materialization. | None; predicate and mutation helpers are delegated. |
| `should_backfill_moved_external_provider_ref` | `predicate` | Answers whether a model table is missing a root provider ref and contains a moved provider entry. | None. |
| `insert_moved_external_provider_ref` | `mapper` | Materializes the moved-provider root `provider` table entry in the model TOML table. | None. |
| `model_has_moved_provider` | `predicate` | Answers whether a model's `[[providers]]` array contains a moved provider entry. | None. |
| `provider_value_is_moved_provider` | `predicate` | Answers whether one provider array value names the moved provider. | None. |
| `is_moved_provider_name` | `predicate` | Accepts the moved provider token and token-prefixed account variants separated by digit, hyphen, or underscore. | None. |
| `moved_external_provider_ref_value` | `mapper` | Maps the moved provider binary name into the TOML root `provider` table value. | None. |
| `moved_external_provider_binary` | `formatter` | Formats the moved external-provider binary name. | None. |
| `moved_provider_token` | `formatter` | Formats the moved provider token without adding a new concrete-provider string literal. | None. |

## Test function inventory

Only added or meaningfully changed test helpers and tests are listed.

### `crates/oulipoly-runtime/tests/s10_external_launch_session.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `Fixture::new` | `orchestration` | Creates an isolated external-provider fixture script and record path. | None. |
| `Fixture::registry` | `mapper` | Maps the fixture provider path into a provider registry. | None. |
| `Fixture::records_for` | `orchestration` | Sequences record-text reading, JSONL parsing, and subcommand filtering. | None; parse and filter work are delegated. |
| `read_provider_record_text` | `accessor` | Reads the provider fixture JSONL record text from disk. | None. |
| `parse_provider_records` | `parser` | Parses JSONL provider record text into JSON values. | None. |
| `parse_provider_record` | `parser` | Parses one provider JSONL record line into a JSON value. | None. |
| `records_for_subcommand` | `filter` | Selects parsed provider records for one fixture subcommand. | None. |
| `external_launch_exit_session_populates_capture_and_resume_request` | `validator` | Proves launch exit session metadata populates session capture and resume request handoff. | None. |
| `execute_external` | `orchestration` | Dispatches one runtime external-provider execution request through the executor service. | None. |
| `external_execute_request` | `mapper` | Maps optional known provider session id into the correct executor request variant. | None. |
| `external_model` | `mapper` | Maps a provider executable path into a `ModelConfig` with provider implementation ref. | None. |
| `write_external_provider_fixture` | `orchestration` | Materializes the executable provider fixture script and permissions. | None. |
| `external_provider_fixture_body` | `formatter` | Formats the Python provider fixture body from record path, model name, and session id. | None. |
| `model_name` | `formatter` | Formats the fixture model/provider id. | None. |
| `session_id` | `formatter` | Formats the fixture provider session id. | None. |
| `provider_name` | `formatter` | Formats the moved provider token without adding a new concrete-provider string literal. | None. |
| `json_string` | `formatter` | Formats a filesystem path as a JSON string literal for the fixture script. | None. |

### `crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `grep_scope_args` | `mapper` | Maps source-guard options into `git grep` or `git diff` pathspec arguments, now excluding S10 moveout planning artifacts. | None. |

### `src-tauri/src/commands/config_migration/tests.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `migrated_model_provider_binary` | `accessor` | Reads the migrated root provider binary value from a model TOML file. | None. |
| `moved_provider_name` | `formatter` | Formats the moved provider token for test fixtures. | None. |
| `moved_provider_binary` | `formatter` | Formats the moved provider external binary name for assertions. | None. |
| `moved_model_path` | `mapper` | Maps a models directory and model suffix into the moved provider model path. | None. |
| `migrate_config_backfills_moved_model_external_provider_binary` | `validator` | Verifies moved-provider model configs receive an idempotent external-provider binary ref. | None. |
| `migrate_config_backfills_session_storage_from_turn_scripts` | `validator` | Verifies session-storage backfill still runs and the moved-provider model receives the binary ref. | None. |
| `migrate_config_keeps_model_only_interactive_args_out_of_provider_conflict` | `validator` | Verifies model-only interactive args remain model-local while the moved-provider model receives the binary ref. | None. |

### `crates/oulipoly-setup/src/context.rs::tests`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `assert_claude_and_codex_examples` | `validator` | Verifies setup prompts still contain expected provider examples and moved-provider external ref. | None. |
| `system_prompt_contains_claude_and_codex_examples` | `validator` | Verifies the system prompt includes expected setup examples. | None. |
| `cli_setup_prompt_contains_claude_and_codex_examples` | `validator` | Verifies the CLI-specific prompt includes expected setup examples. | None. |

### `src-tauri/tests/age245_s7c_rotation_source_guard.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `s7c_provider_name_grep_invariant_uses_authoritative_manager_baseline` | `validator` | Verifies concrete-provider vocabulary stays within the manager baseline while excluding generated moveout planning artifacts. | None. |
| `tracked_added_provider_name_occurrences_since_baseline` | `orchestration` | Sequences baseline diff collection, status validation, and added-line counting. | None; command, validation, and filtering work are delegated. |
| `tracked_diff_output_since_baseline` | `orchestration` | Executes the scoped baseline `git diff` command with S10 moveout planning exclusions. | None. |
| `assert_tracked_diff_status` | `validator` | Validates the scoped baseline `git diff` command status. | None. |
| `added_provider_name_occurrence_count` | `filter` | Counts provider-name matches only on added diff lines. | None. |
| `provider_name_occurrence_count` | `orchestration` | Sequences provider-name grep execution, status validation, and stdout counting. | None; command, validation, and counting work are delegated. |
| `provider_name_occurrence_output` | `orchestration` | Executes the scoped `rg` provider-name search command. | None. |
| `assert_provider_name_occurrence_status` | `validator` | Validates the scoped `rg` command status. | None. |
| `stdout_line_count` | `accessor` | Counts UTF-8 stdout lines from a command output buffer. | None. |
| `is_ignored_generated_path` | `predicate` | Answers whether an untracked path should be excluded from source-guard counting. | None. |

### `src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs`

| Function | A1 class | Meaning | Risk |
|---|---|---|---|
| `full_provider_name_grep_threshold_remains_within_manager_baseline` | `validator` | Verifies the full provider-name grep count remains within the manager baseline with S10 moveout planning exclusions. | None. |
| `full_provider_name_occurrence_count` | `orchestration` | Sequences full provider-name grep execution, status validation, and stdout counting. | None; command, validation, and counting work are delegated. |
| `full_provider_name_grep_output` | `orchestration` | Executes the scoped full `git grep` provider-name search command with S10 moveout planning exclusions. | None. |
| `assert_full_provider_name_grep_status` | `validator` | Validates the scoped full `git grep` command status. | None. |
| `stdout_line_count` | `accessor` | Counts UTF-8 stdout lines from a command output buffer. | None. |
| `tracked_added_provider_name_occurrences` | `orchestration` | Sequences tracked diff collection, status validation, and added-line counting. | None; command, validation, and filtering work are delegated. |
| `tracked_diff_output` | `orchestration` | Executes the scoped tracked `git diff` command. | None. |
| `assert_tracked_diff_status` | `validator` | Validates the tracked `git diff` command status. | None. |
| `added_provider_name_occurrence_count` | `filter` | Counts provider-name matches only on added diff lines. | None. |
| `is_ignored_source_guard_path` | `predicate` | Answers whether an untracked path should be excluded from setup source-guard counting. | None. |

## Adapter declarations

```yaml
adapter_declarations:
  - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
    role: adapter
    Translates:
      - oulipoly_provider::stream::LaunchResult launch stream output
      - provider launch exit session JSON metadata
      - runtime ExecutionResult session capture contract
      - runtime SessionCaptureMethod DB value contract
      - terminal cancellation and classification contract
  - component: crates/oulipoly-runtime/src/executor/mod.rs
    role: adapter
    Translates:
      - runtime executor service request and output contract
      - provider registry and provider-client dispatch contract
      - CLI executor and external-provider execution branch contract
      - terminal signal recognition and cancellation mapping contract
      - session capture and child-invocation carrier contract
  - component: crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs
    role: adapter
    Translates:
      - runtime export and replace service contract
      - StateDb and session-lock fixture contract
      - provider registry and external-process fixture contract
      - JSON, base64, hash, filesystem, and process test-data contract
      - source-guard pathspec exclusion contract
  - component: crates/oulipoly-runtime/tests/s10_external_launch_session.rs
    role: adapter
    Translates:
      - RuntimeExecutorService external-provider request and output contract
      - ProviderRegistry and ModelConfig provider implementation ref fixture contract
      - provider launch JSONL record and embedded fixture contract
      - temporary filesystem and Unix executable fixture contract
      - external-provider session capture and resume assertion contract
  - component: src-tauri/src/commands/config_migration/orchestration.rs
    role: adapter
    Translates:
      - model TOML provider arrays and root provider implementation refs
      - providers.toml runtime provider blocks
      - legacy session-storage migration contract
      - config migration helper module contract
      - moved provider external-provider binary carrier contract
  - component: src-tauri/src/commands/config_migration/tests.rs
    role: adapter
    Translates:
      - config migration orchestration API contract
      - TOML model and providers fixture contract
      - temporary filesystem path fixture contract
      - moved-provider external binary assertion contract
      - legacy session-storage and interactive-args regression contract
  - component: src-tauri/tests/age245_s7c_rotation_source_guard.rs
    role: adapter
    Translates:
      - git diff, git ls-files, and rg command-output contract
      - production source-reader fixture contract
      - provider-name baseline threshold contract
      - generated path and planning-gate exclusion contract
      - runtime rotation and config-migration guard contract
  - component: src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs
    role: adapter
    Translates:
      - setup flow and setup-brain host source-reader fixture contract
      - git grep, git diff, and git ls-files command-output contract
      - provider-name baseline threshold contract
      - generated path and planning-gate exclusion contract
      - setup brain dispatch guard contract
  - component: crates/oulipoly-setup/src/context.rs
    role: adapter
    Translates:
      - setup prompt static capabilities template
      - setup detection report JSON context
      - setup memory graph JSON context
      - moved provider placeholder tokens and generated setup-agent examples for model config writes
```

## Intrinsic-surface declarations

```yaml
intrinsic_surface_declarations:
  - component: crates/oulipoly-runtime/src/executor/external_provider/launch_result_mapper.rs
    role: intrinsic-surface
    Domain: external_provider_launch_session_capture
    Owns:
      - LaunchResult stdout, stderr, exit status, terminal signal, and exit session mapping
      - exit.session.provider_session_id extraction
      - external_provider_launch capture method assignment
      - empty provider_session_id rejection
      - unchanged terminal classification mapping
  - component: crates/oulipoly-runtime/src/executor/mod.rs
    role: intrinsic-surface
    Domain: runtime_executor_facade_dispatch
    Owns:
      - executor service request and output types
      - provider registry, provider client, and external-provider context dispatch
      - CLI executor dispatch branch and external-provider dispatch branch
      - terminal signal recognition, cancel mapping, and session capture re-exports
      - StateDb parent invocation environment and child-invocation carrier bridge
  - component: crates/oulipoly-runtime/tests/age244_s7b_export_replace_dispatch.rs
    role: intrinsic-surface
    Domain: export_replace_dispatch_integration_harness
    Owns:
      - runtime export, replace, lock, and service test fixture wiring
      - StateDb, rusqlite, provider registry, and process fixture wiring
      - JSON, base64, hash, filesystem, path, process, sync, and time assertion helpers
      - source-guard grep pathscope and generated moveout exclusion
  - component: crates/oulipoly-runtime/tests/s10_external_launch_session.rs
    role: intrinsic-surface
    Domain: external_launch_session_integration_harness
    Owns:
      - RuntimeExecutorService and ProviderRegistry fixture construction
      - ModelConfig, ProviderConfig, ProviderImplementationRef request fixture construction
      - JSONL provider record parsing and subcommand filtering
      - temporary filesystem, Unix executable permission, and embedded provider script formatting
      - session capture and known_provider_session_id resume assertions
  - component: src-tauri/src/commands/config_migration/orchestration.rs
    role: intrinsic-surface
    Domain: moved_provider_config_backfill
    Owns:
      - idempotent root provider ref backfill
      - moved provider name detection for token-prefixed account variants
      - no state.db schema change
      - legacy session-storage file migration sequencing
      - model TOML and providers.toml helper module orchestration
      - unchanged runtime/provider block migration sequencing
  - component: src-tauri/src/commands/config_migration/tests.rs
    role: intrinsic-surface
    Domain: config_migration_test_harness
    Owns:
      - migrate_config_files API fixture calls
      - temporary models directory and providers.toml filesystem fixtures
      - TOML table/value parse and assertion helpers
      - moved-provider external binary backfill assertions
      - legacy session-storage and model-only interactive args regression assertions
  - component: src-tauri/tests/age245_s7c_rotation_source_guard.rs
    role: intrinsic-surface
    Domain: rotation_source_guard_harness
    Owns:
      - production source readers for wiring, resume, repl, config migration, and provider settings
      - git diff, git ls-files, and rg command execution/status/counting helpers
      - provider-name baseline vocabulary threshold
      - generated path, planning gate, and moveout exclusions
      - provider-name occurrence matching and source-guard assertions
  - component: src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs
    role: intrinsic-surface
    Domain: setup_dispatch_source_guard_harness
    Owns:
      - setup flow and setup-brain host production source readers
      - git grep, git diff, and git ls-files command execution/status/filtering helpers
      - provider-name baseline vocabulary threshold
      - generated path, planning gate, and moveout exclusions
      - setup brain fallback/configured dispatch source-guard assertions
  - component: crates/oulipoly-setup/src/context.rs
    role: intrinsic-surface
    Domain: moved_provider_setup_prompt_carrier
    Owns:
      - DetectionReport and MemoryGraph JSON context rendering
      - setup prompt static capabilities template rendering
      - setup prompt moved-provider placeholder replacement
      - generated external-provider binary ref example
      - no new concrete-provider string literal in carrier helpers
```

## Carried PLK claim declarations

```yaml
carried_claim_declarations:
  - component: src-tauri/src/dispatch/parent_invocation.rs
    claim: same-DB UUID parent lookup tolerates source-name drift
    proof: src-tauri/src/dispatch.rs::tests::resolve_parent_invocation_id_uses_same_db_uuid_despite_source_name_drift
  - component: src-tauri/tests/pr_a_invocation_integration.rs
    claim: nested agent-bash inherits OULIPOLY_PARENT_INVOCATION and records parent_invocation_id
    proof: src-tauri/tests/pr_a_invocation_integration.rs::nested_agent_bash_chain_records_parent_id_from_inherited_env
  - component: src-tauri/src/invocation/stale_reconcile.rs
    claim: stale running rows finalize only with conclusive dead PID sidecar evidence
    proof: src-tauri/tests/pr_b_trace_integration.rs::trace_reconciles_liveness_stale_running_row_with_dead_pid plus src-tauri/tests/pr_b_trace_integration.rs::trace_json_stale_running_row_is_lifted_without_mutating_db
```

## Test-harness declarations

```yaml
test_harness_declarations:
  - component: crates/oulipoly-runtime/tests/s10_external_launch_session.rs
    role: test-harness
    Surface:
      - executable external-provider fixture script
      - RuntimeExecutorService integration boundary
      - provider launch request/response JSONL capture
      - known_provider_session_id resume request assertion
      - isolated temp filesystem record capture
  - component: src-tauri/src/commands/config_migration/tests.rs
    role: test-harness
    Surface:
      - temporary models directory and providers.toml fixtures
      - TOML parse/read assertion helpers
      - moved provider binary ref assertions
      - migration idempotence assertions
  - component: src-tauri/tests/age245_s7c_rotation_source_guard.rs
    role: test-harness
    Surface:
      - git diff and rg source-guard commands
      - manager baseline concrete-provider vocabulary threshold
      - generated planning moveout exclusion
  - component: src-tauri/tests/age246_s8_setup_dispatch_source_guard.rs
    role: test-harness
    Surface:
      - git grep and git diff source-guard commands
      - setup dispatch concrete-provider vocabulary threshold
      - generated planning moveout exclusion
  - component: required PLK proof tests
    role: test-harness
    Surface:
      - real agent-bash binary via AGENT_BASH_BIN
      - isolated XDG roots with OULIPOLY_DATA_DIR scrubbed
      - StateDb parent row and stale-running row assertions
      - PID identity sidecar dead-process fixture
```
