# Coverage Delta: feat/pr-a-invocation-lifecycle

## Verdict: PARTIAL

Roughly 80-85% of the added production logic appears exercised by tests: the new invocation lifecycle APIs, the main happy-path wiring, malformed/unresolved parent env handling, and the legacy-schema rebuild are all covered, but a few load-bearing branches still lack direct tests.

## Diff size summary

- `src-tauri/src/state/db.rs`: `+955 / -63`
- `src-tauri/src/main.rs`: `+51 / -8`
- `src-tauri/src/executor/mod.rs`: `+35 / -2`
- `src-tauri/src/executor/cli.rs`: `+9 / -3`
- `src-tauri/src/state/mod.rs`: `+1 / -0`
- `src-tauri/src/balancer/mod.rs`: `+24 / -6` (test-only adjustments under `#[cfg(test)]`; not counted as production surface)

## Per-function coverage

| Function/Method | Lines | Covered | Test name(s) |
|---|---|---|---|
| `StateDb::open` via `ensure_invocations_schema` | `db.rs:440-454` | partial | `schema_creation`, `migration_backfills_resolved_and_legacy_rows`, `migration_rolls_back_when_rebuild_fails` |
| `StateDb::migrate_legacy_invocations` | `db.rs:505-620` | partial | `migration_backfills_resolved_and_legacy_rows`, `migration_rolls_back_when_rebuild_fails` |
| `provider_name_lookup` | `db.rs:629-650` | partial | `migration_backfills_resolved_and_legacy_rows` |
| `StateDb::start_invocation` | `db.rs:653-681` | yes | `start_invocation_inserts_running_row_with_null_terminal_fields`, `start_invocation_rejects_duplicate_uuid`, `start_invocation_accepts_parent_rowid` |
| `StateDb::finalize_invocation` | `db.rs:684-773` | yes | `finalize_invocation_sets_terminal_fields`, `finalize_invocation_updates_provider_aggregate_stats`, `finalize_invocation_errors_for_missing_row`, `finalize_invocation_errors_when_called_twice`, `emits_single_invocation_line_and_finalizes_succeeded_row` |
| `StateDb::get_invocation_by_uuid` | `db.rs:776-793` | yes | `get_invocation_by_uuid_returns_matching_and_missing_rows`, `emits_single_invocation_line_and_finalizes_succeeded_row`, `resolves_parent_env_and_overwrites_child_subprocess_env` |
| `CompositeInvocationId::stderr_line` and `parse_env_value` | `db.rs:141-155` | yes | `composite_invocation_id_formats_and_round_trips`, `composite_invocation_id_rejects_malformed_env_values`, `emits_single_invocation_line_and_finalizes_succeeded_row` |
| `InvocationStatus::as_str` and `FromStr` | `db.rs:109-130` | yes | `invocation_status_round_trips_through_strings` |
| `execute_with_inputs_and_env` and `cli::execute_provider` env propagation | `executor/mod.rs:52-68`, `executor/cli.rs:216-247` | yes | `resolves_parent_env_and_overwrites_child_subprocess_env`, `emits_single_invocation_line_and_finalizes_succeeded_row`, existing executor unit tests (`execute_arg_prompt`, `execute_stdin_prompt`, `execute_resolves_input_flags_or_defaults`) for the `None` path |
| `run_with_balancing` invocation creation/finalization path | `main.rs:256-309` | partial | `emits_single_invocation_line_and_finalizes_succeeded_row`, `resolves_parent_env_and_overwrites_child_subprocess_env` |
| `resolve_parent_invocation_id` | `main.rs:329-337` | partial | `resolves_parent_env_and_overwrites_child_subprocess_env`, `ignores_malformed_and_unresolved_parent_env_values` |

Coverage notes:

- `StateDb::start_invocation` is fully covered for the API contract called out in the prompt, including the UUID collision path.
- `StateDb::finalize_invocation` is fully covered for the required branches: success, failure with `error_count`/`error_category`, missing id, and double-finalize rejection.
- `StateDb::get_invocation_by_uuid` is covered for both `Some` and `None`, and the integration tests also prove it works on rows created through `main.rs`.
- The end-to-end stderr emission requirement is satisfied by `emits_single_invocation_line_and_finalizes_succeeded_row`, which asserts exactly one `OULIPOLY_INVOCATION=` line and then verifies the persisted row it identifies.
- Env propagation to the spawned `Command` is covered by `resolves_parent_env_and_overwrites_child_subprocess_env`, which proves the subprocess sees the newly generated child composite id rather than the caller-supplied parent env payload.

## Per-branch coverage gaps

- `src-tauri/src/state/db.rs:448-451`  
  The `ensure_invocations_schema` branch for an already-upgraded database (`invocation_uuid` present, only indexes ensured) is not directly exercised. Suggested test: create a DB with the new table shape but missing one invocation index, reopen via `StateDb::open`, and assert the missing index is recreated without triggering a rebuild.

- `src-tauri/src/state/db.rs:589-590`  
  `migrate_legacy_invocations` covers mapped-success and unmapped-legacy rows, but not the mapped-failure arm (`status = failed` when provider lookup succeeds and `success = 0`). Suggested test: seed an old-schema row for a mapped model with `success = 0` and assert `provider_name` is populated, `status = failed`, and `finished_at` preserves `created_at`.

- `src-tauri/src/state/db.rs:636-641`  
  `provider_name_lookup` has no test for `load_models` failure. This is the fallback path for corrupt or unreadable model config, and it is explicitly supposed to degrade to `legacy` rows instead of blocking DB open. Suggested test: point `XDG_CONFIG_HOME` at a models dir containing invalid TOML, open an old-schema DB, and assert migration succeeds with `provider_name = NULL` and `status = legacy`.

- `src-tauri/src/main.rs:333-336`  
  `resolve_parent_invocation_id` is tested for missing env, malformed JSON, unresolved UUID, and matching source, but not for an existing UUID paired with the wrong `source`. Suggested test: create a parent invocation, pass its UUID with a different `source`, and assert the child row is created with `parent_invocation_id = NULL`.

## Acceptable exclusions

- `main.rs:283-289` plus `executor/cli.rs:274-276` are not directly exercised for subprocess spawn failure. Per the audit instructions, real subprocess spawn paths are acceptable exclusions here.
- The serialization fallback inside `CompositeInvocationId::stderr_line` and the `serde_json::to_string` call in `run_with_balancing` are effectively infallible for this two-string struct and fit the allowed “unwrap on data the function just produced” exclusion.

## Required followups

- Add a migration test for a mapped failed legacy row. This is the most important uncovered branch because it affects one-time upgrade correctness for historical failed invocations.
- Add a migration test for corrupt model config during `provider_name_lookup`. That branch exists specifically to preserve observability under config failure and should be locked down.
- Add an integration test for parent-source mismatch in `OULIPOLY_PARENT_INVOCATION`. The code intentionally rejects mismatched composites; that behavior should be demonstrated explicitly.
- Add a schema-open test for an already-new invocations table missing indexes. That closes the remaining branch in `StateDb::open` without needing a coverage tool.
