# spec-tauri-client — Top-level Tauri client wiring, CLIs, adapters

## Source files

- `src-tauri/src/mailbox_delivery.rs`
- `src-tauri/src/wake_coordinator/mod.rs`
- `src-tauri/src/wake_coordinator/consumed_completion.rs`
- `src-tauri/src/wake_coordinator/turn_recheck.rs`
- `src-tauri/src/wake_coordinator/wake_start/mod.rs`

- `src-tauri/src/lib.rs`
- `src-tauri/src/app_state.rs`
- `src-tauri/src/app_paths.rs`
- `src-tauri/src/run_tauri.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/dispatch.rs`
- `src-tauri/src/wiring.rs`
- `src-tauri/src/lib_commands.rs`
- `src-tauri/src/commands/models/mod.rs`
- `src-tauri/src/commands/models/accessor.rs`
- `src-tauri/src/commands/models/validator.rs`
- `src-tauri/src/commands/models/formatter.rs`
- `src-tauri/src/commands/models/orchestration.rs`
- `src-tauri/src/commands/models/reload.rs`
- `src-tauri/src/commands/pools/mod.rs`
- `src-tauri/src/commands/pools/derive.rs`
- `src-tauri/src/commands/pools/update.rs`
- `src-tauri/src/commands/pools/accessor.rs`
- `src-tauri/src/commands/pools/validator.rs`
- `src-tauri/src/commands/pools/writer.rs`
- `src-tauri/src/commands/quota_refresh/mod.rs`
- `src-tauri/src/commands/quota_refresh/orchestration.rs`
- `src-tauri/src/commands/quota_refresh/candidates.rs`
- `src-tauri/src/commands/quota_refresh/accessor.rs`
- `src-tauri/src/commands/quota_refresh/mapper.rs`
- `src-tauri/src/commands/accessor.rs`
- `src-tauri/src/commands/setup_flow/mod.rs`
- `src-tauri/src/commands/setup_flow/orchestration.rs`
- `src-tauri/src/commands/setup_flow/provider_probe.rs`
- `src-tauri/src/commands/setup_flow/accessor.rs`
- `src-tauri/src/commands/setup_flow/formatter.rs`
- `src-tauri/src/commands/providers_accounts/mod.rs`
- `src-tauri/src/commands/providers_accounts/orchestration.rs`
- `src-tauri/src/commands/providers_accounts/accessor.rs`
- `src-tauri/src/commands/providers_accounts/validator.rs`
- `src-tauri/src/commands/providers_accounts/mapper.rs`
- `src-tauri/src/commands/providers_accounts/formatter.rs`
- `src-tauri/src/commands/providers_accounts/display_name.rs`
- `src-tauri/src/commands/discovery/mod.rs`
- `src-tauri/src/commands/discovery/orchestration.rs`
- `src-tauri/src/commands/discovery/accessor.rs`
- `src-tauri/src/commands/discovery/predicate.rs`
- `src-tauri/src/commands/discovery/formatter.rs`
- `src-tauri/src/commands/test_model/mod.rs`
- `src-tauri/src/commands/test_model/orchestration.rs`
- `src-tauri/src/commands/test_model/diagnostics_fallback.rs`
- `src-tauri/src/commands/test_model/lookup.rs`
- `src-tauri/src/commands/test_model/dispatch.rs`
- `src-tauri/src/commands/test_model/validator.rs`
- `src-tauri/src/commands/test_model/formatter.rs`
- `src-tauri/src/commands/test_model/mapper.rs`
- `src-tauri/src/balanced_cli.rs`
- `src-tauri/src/cli_inputs.rs`
- `src-tauri/src/config_migration_cli.rs`
- `src-tauri/src/repl_cli.rs`
- `src-tauri/src/resume_acceptance_adapter.rs`
- `src-tauri/src/resume_cli.rs`
- `src-tauri/src/resume_cli/diagnostics.rs`
- `src-tauri/src/resume_cli/target.rs`
- `src-tauri/src/run/repl/mod.rs`
- `src-tauri/src/run/repl/orchestration.rs`
- `src-tauri/src/run/repl/resolution.rs`
- `src-tauri/src/run/repl/execution.rs`
- `src-tauri/src/run/repl/migration.rs`
- `src-tauri/src/run/repl/terminal.rs`
- `src-tauri/src/run/repl/mapper.rs`
- `src-tauri/src/run/repl/formatter.rs`
- `src-tauri/src/run/resume/mod.rs`
- `src-tauri/src/run/resume/orchestration.rs`
- `src-tauri/src/run/resume/execution.rs`
- `src-tauri/src/run/resume/lifecycle.rs`
- `src-tauri/src/run/resume/migration.rs`
- `src-tauri/src/run/resume/terminal.rs`
- `src-tauri/src/run/resume/wake.rs`
- `src-tauri/src/run/resume/mapper.rs`
- `src-tauri/src/run/resume/predicate.rs`
- `src-tauri/src/commands/resume_list/mod.rs`
- `src-tauri/src/commands/resume_list/orchestration.rs`
- `src-tauri/src/commands/resume_list/validator.rs`
- `src-tauri/src/commands/resume_list/formatter.rs`
- `src-tauri/src/commands/resume_list/parser.rs`
- `src-tauri/src/commands/session_list/mod.rs`
- `src-tauri/src/commands/session_list/orchestration.rs`
- `src-tauri/src/commands/session_list/formatter.rs`
- `src-tauri/src/commands/session_import_replace/`
- `src-tauri/src/session_ingest_cli.rs`
- `src-tauri/src/session_metadata_cli.rs`
- `src-tauri/src/terminal_outcome_adapter.rs`
- `src-tauri/src/commands/trace/`
- `src-tauri/src/main/owned_turn_event_ingest.rs`
- `src-tauri/src/setup/flow.rs`
- `src-tauri/src/setup/mod.rs`

## Preconditions

- A built Tauri binary (or `cargo run -p oulipoly-tauri --bin ...`).
- The runtime crate(s) compiled and wired via `wiring.rs`.
- For per-CLI flows: a configured deployment whose `oulipoly-config` and
  `oulipoly-state` surfaces resolve.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| `main.rs` invoked with no args. | Launch the Tauri GUI. |
| `main.rs` invoked with a sub-CLI (`balanced`, `repl`, `resume`, etc.). | Parse argv and delegate to `dispatch.rs`, which routes to the matching command driver. |
| `balanced_cli.rs` invoked with a prompt. | Route via balancer + executor; return result; emit trace/diagnostics. |
| `repl_cli.rs` invoked. | Open an interactive session per `repl_default_provider.rs`'s resolution. |
| `resume_cli.rs` invoked with a session id. | Resolve via session_lifecycle, attach to existing session, continue. |
| `resume --list <UUID>` legacy syntax, or hidden `resume-list <UUID>`, invoked. | `main.rs` normalizes legacy argv via `normalize_resume_list_args`; `dispatch.rs` routes to `run_resume_list`, which validates the UUID, reads `StateDb::open_default().resume_previews(uuid)`, and prints one `chain_id=... last_used_at=... active_provider=... active_session_id=... turn_count=... recent_turns_count=...` line per chain. |
| `resume --list <UUID>` / `resume-list <UUID>` invoked with no matching chains. | Print `No chains found for {uuid}` and exit successfully. |
| `session list [--json]` invoked. | `dispatch.rs` routes to `run_session_list`, which opens `state.db` read-only when present, reads `imported_session_list()`, and prints either a tab-separated session table or the same rows as JSON. |
| `session_ingest_cli.rs` invoked. | Import an externally-produced session transcript into the local state DB. |
| `commands/session_import_replace/` invoked. | Replace an existing local session with an imported payload. |
| `session_metadata_cli.rs` invoked. | Read or update metadata for a known session. |
| `commands/trace/` invoked. | Surface the diagnostics/trace history for an invocation. |
| `config_migration_cli.rs` invoked. | Migrate config schema forward. |
| Setup flow invoked (`setup/flow.rs`). | Drive the wizard from `oulipoly-setup`; persist results via `oulipoly-config`. |
| Tauri setup-flow IPC commands invoked (`commands/setup_flow/`). | Preserve UUID session ids, response channel capacity 16, sender storage/clear behavior, setup response strings, memory DB path beside `models_dir`, memory-open error event text with `recoverable: false`, CLI detection delegation, and the existing `which claude` setup-needed residual probe. |
| Tauri provider/account IPC commands invoked (`commands/providers_accounts/`). | Route reads and mutations through `SetupRepository`, prefer test repository injection before real DB fallback, preserve `AddAccountInput` fields, account validation strings, provider-not-found strings, `AuthStatus::Unknown`, RFC3339 timestamps, delete boolean results, sync detection delegation, and the residual display-name map. |
| Tauri discovery IPC commands invoked (`commands/discovery/`). | Run runtime discovery in a blocking task, open the GUI-derived `state.db`, map join failures to `Discovery task failed: {e}`, preserve empty-result stale-delete guard, delete stale rows before model upserts before parameter upserts, and preserve provider/model read filters through `SetupRepository`. |
| Tauri owned-turn event arrives. | `main/owned_turn_event_ingest.rs` parses and persists per `oulipoly-state` schema. |
| `commands/quota_refresh/` invoked. | Refresh stale quota data only for providers in multi-provider models, preserve sorted provider-name output, map runtime quota outcomes to the stable frontend DTO strings. |
| A terminal `agent-bash` result is polled after its completion row was already enqueued. | Reconcile the durable `consumed` marker against its registered owner before wake startup, acknowledge the exact completion row once as `consumed_in_call`, do not resume for that row, and preserve normal wake delivery for sibling or unrelated unpolled rows. |

AGE-237 owns the adjacent usage-CLI/quota-refresh outcome drift. This spec
records the current quota-refresh command behavior only; it does not normalize
or consolidate usage CLI row-state strings with the quota-refresh DTO strings.

## Edge cases

- CLI arg parse fails — exit non-zero with usage; do not panic.
- Tauri context exists but no GUI display (headless host) — CLI sub-mode
  still works; GUI mode reports a clear error.
- Adapter (e.g. `terminal_outcome_adapter.rs`) receives an envelope
  whose schema does not match — typed error; do not swallow.
- Quota refresh sees a fresh cached provider — return `fresh` without calling
  the runtime quota service.
- Quota refresh sees an in-flight runtime refresh — return `in_flight`
  byte-identically for the DTO status.
- Setup response without an active setup session returns `No active setup session`;
  sending to a closed setup response channel returns `Failed to send response: {e}`.
- Discovery persistence with no discovered models preserves existing rows; a
  non-empty result deletes stale rows before upserting models and parameters.
- A completion consumed before event trigger remains suppressed by the normal
  completion path; a completion consumed after enqueue is reconciled at turn
  end and again immediately before wake startup.
- Resume acceptance adapter sees a session in `mutability: read-only` —
  refuses gracefully (delegates to `oulipoly-runtime/session_metadata/
  mutability.rs`).
- Resume-list UUID validation fails — return `invalid session UUID: ...`
  before opening the state DB.

## Error conditions

- `CliParseFailed` — invalid CLI arguments.
- `WiringFailed` — `wiring.rs` could not construct the runtime service
  graph (typically an internal-config mismatch; programmer error).
- `TauriBootFailed` — GUI surface could not initialize.
- `AdapterError` — a typed `*_adapter.rs` translation failure.
- Resume-list loading failure — return `Failed to list resume chains: {e}`.
- Quota refresh state DB open failure — return `Failed to open state DB: {e}`.
- Setup memory graph open failure emits `Failed to open memory store: {e}` with
  `recoverable: false` on the setup event channel.
- Provider/account validation failures return `Account id cannot be empty`,
  `Account provider cannot be empty`, or `Account profile_name cannot be empty`;
  missing providers return `Provider '{name}' not found`.
- Discovery blocking task join failures return `Discovery task failed: {e}`.

## Boundaries

- Tauri client does NOT implement balancer policy, recognizer logic,
  quota refresh, or session metadata storage — those live in
  `oulipoly-runtime` and `oulipoly-state`. The client is composition +
  CLI + adapter only.
- Tauri client does NOT bypass the documented service ports — every
  runtime call goes through `services/` wired by `wiring.rs`.
- Tauri client does NOT mutate config files directly — it goes through
  `oulipoly-config`.
- Resume-list is read-only over the state DB; it lists chain previews
  without mutating session or chain state.

## Declared test patterns

Per `~/ai/conventions/testing.md`: integration tests on each CLI driver,
wiring smoke tests, adapter contract tests, workspace-layout invariants.

- `src-tauri/tests/age36_wiring.rs`
- `src-tauri/tests/age37_wiring.rs`
- `src-tauri/tests/age38_test_model_services.rs`
- `src-tauri/tests/age38_wiring.rs`
- `src-tauri/tests/age39_main_thinning_source_guard.rs`
- `src-tauri/tests/age236_quota_refresh_extraction.rs`
- `src-tauri/tests/age151_source_guard.rs`
- `src-tauri/tests/age154_test_model_disposition.rs`
- `src-tauri/tests/age8_cli_characterization.rs`
- `src-tauri/src/commands/resume_list/tests.rs`
  (`resume_list_user_syntax_rewrites_to_hidden_subcommand`,
  `resume_list_line_includes_required_chain_fields`)
- `src-tauri/tests/age134_main_session_and_migrate.rs`
  (`age134_resume_list_empty_outputs_no_chains_for_user_and_hidden_syntax`)
- `src-tauri/tests/age_32_state_db_migrations.rs` (`resume-list` /
  `resume --list` against state fixtures)
- `src-tauri/tests/initiative_05_migration.rs` (populated and
  malformed-UUID resume-list paths)
- `src-tauri/tests/initiative_07_canonical_reader_unification.rs`
- `src-tauri/tests/nes_259_returned_artifacts_integration.rs`
- `src-tauri/tests/pr_a_invocation_integration.rs`
- `src-tauri/tests/pr_c_locator_scripts.rs`
- `src-tauri/tests/pr_f_resume_integration.rs`
- `src-tauri/tests/release_yml_contract.rs`
- `src-tauri/tests/workflow_yml_contract.rs`
- `src-tauri/tests/wiring_smoke.rs`
- `src-tauri/tests/workspace_layout.rs`
- `src-tauri/tests/wu_d_proactive_wake_integration/`
  (`polled_completion_after_enqueue_does_not_wake_parent`,
  `consumed_completion_preserves_unpolled_completion_wake`,
  `delayed_agent_bash_completion_wakes_inactive_headless_parent_once`)
- `src-tauri/src/wake_coordinator/turn_recheck.rs`
  (`turn_end_pending_count_reconciles_late_consumption`)
- `src-tauri/src/wake_coordinator/wake_start/mod.rs`
  (`wake_start_reconciles_late_consumption_before_claim`)
- `src-tauri/tests/claude_path_hash_rca/age158_characterization.rs`
- `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs`
- `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`
- `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`
- `src-tauri/tests/routing_fanout_rca/age158_characterization.rs`
- `src-tauri/tests/opencode_resume_storage_migration_rca.rs`
  (nine production-built ownership-selection, fail-closed, exact-chain, and
  single-native compatibility cases using isolated public-export fakes)
- `src-tauri/src/run/repl/source_guard.rs` and
  `src-tauri/src/run/resume/source_guard.rs` (colocated module-split source
  guards)
- `src-tauri/tests/marker_producer_format.rs`,
  `src-tauri/tests/age245_s7c_rotation_source_guard.rs`, and
  `src-tauri/tests/age153_support/mod.rs` (existing source-aggregation tests and
  shared support updated only to follow the approved module split)

## Cross-references

- `planning/coverage/spec-balancer.md`, `spec-quota.md`,
  `spec-recognizer.md`, `spec-executor.md` — runtime surfaces this
  client invokes.
- `planning/coverage/spec-session-lifecycle.md` — session CLIs depend
  on this.
- `planning/coverage/spec-config.md` — config-migration CLI.
- `planning/coverage/spec-setup.md` — setup flow this client drives.
- `AGENTS.md` § Commands.
