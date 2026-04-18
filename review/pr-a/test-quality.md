# Test Quality: feat/pr-a-invocation-lifecycle

## Verdict: PASS

The new tests are deterministic, mostly well-scoped, and they cover every contract requirement listed for PR-A, with only minor focus/style nits that do not undermine the test gate.

## Per-dimension grade

| Dimension | Grade | Notes |
|---|---|---|
| Determinism | A | No sleeps, no wall-clock waits, no reads from real user state, and the DB tests use `:memory:` or tempdirs. The few tests that mutate `XDG_CONFIG_HOME` serialize access with a mutex. |
| Focus | B | Most tests map cleanly to one behavior, but a few bundle adjacent checks into one scenario, notably `get_invocation_by_uuid_returns_matching_and_missing_rows` and the malformed-env integration loop. The bundling is still reasonable and readable. |
| Contract coverage | A | Every required contract item in the prompt has at least one corresponding test. Coverage is stronger than the old `record_invocation` era because the split lifecycle API is asserted directly. |
| Negative-test discipline | A | The suite exercises the main failure paths deliberately: duplicate UUIDs, missing finalize target, double-finalize, malformed env JSON, invalid UUIDs, extra/missing JSON fields, unresolved parent env, and migration rollback. |
| Test-as-spec discipline | A | Assertions are contract-facing rather than implementation-mirroring overall, there are no `#[ignore]` tests, and no intentionally weakened assertions. The schema test checking DDL text is acceptable here because the contract specifies exact DDL. |
| Removed/modified tests | A | Deleting `record_and_query` is justified because `record_invocation` is deleted by contract; its behavioral intent is preserved across the new start/finalize tests. The `recent_errors` rewrite keeps the same observable intent while moving to the new lifecycle API. |

## Contract requirements coverage

- Schema: **covered** by `schema_creation` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1710). It asserts the rebuilt table DDL contains the new columns and checks the expected invocation indexes exist.
- Migration: **covered** by `migration_backfills_resolved_and_legacy_rows` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1743). It seeds an old-schema DB and verifies both a resolved row and an unresolved row migrate with the expected status/provider outcomes.
- Migration with config failure: **covered** by `migration_succeeds_with_corrupt_models_config_and_marks_rows_legacy` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1852). It forces model-config load failure and verifies open succeeds and rows degrade to legacy.
- `start_invocation`: **covered** by `start_invocation_inserts_running_row_with_null_terminal_fields` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1934). It checks returned rowid, `status='running'`, and NULL terminal fields.
- `start_invocation` UUID collision: **covered** by `start_invocation_rejects_duplicate_uuid` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1960).
- `start_invocation` parent `None` and `Some(rowid)`: **covered** by `start_invocation_inserts_running_row_with_null_terminal_fields` (`None`) and `start_invocation_accepts_parent_rowid` (`Some`) in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1934) and [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:1976).
- `finalize_invocation`: **covered** by `finalize_invocation_sets_terminal_fields` and `finalize_invocation_updates_provider_aggregate_stats` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2004) and [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2029).
- `finalize_invocation` errors: **covered** by `finalize_invocation_errors_for_missing_row` and `finalize_invocation_errors_when_called_twice` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2070) and [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2079).
- `CompositeInvocationId::stderr_line`: **covered** by `composite_invocation_id_formats_and_round_trips` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2124). It checks the exact serialized line.
- `CompositeInvocationId::parse_env_value`: **covered** by `composite_invocation_id_rejects_malformed_env_values` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2143). It covers malformed JSON, missing field, invalid UUID, and extra field rejection.
- `InvocationStatus`: **covered** by `invocation_status_round_trips_through_strings` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2158). All four variants round-trip.
- `get_invocation_by_uuid`: **covered** by `get_invocation_by_uuid_returns_matching_and_missing_rows` in [db.rs](/home/nes/projects/agent-runner/src-tauri/src/state/db.rs:2178). It checks both a hit and a miss.
- Stderr emission: **covered** by `emits_single_invocation_line_and_finalizes_succeeded_row` plus the shared `parse_invocation` helper in [pr_a_invocation_integration.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_a_invocation_integration.rs:91) and [pr_a_invocation_integration.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_a_invocation_integration.rs:105). The helper asserts exactly one matching stderr line and parses the contract format.
- Env propagation: **covered** by `emits_single_invocation_line_and_finalizes_succeeded_row` and `resolves_parent_env_and_overwrites_child_subprocess_env` in [pr_a_invocation_integration.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_a_invocation_integration.rs:105) and [pr_a_invocation_integration.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_a_invocation_integration.rs:127). They verify the child process receives the propagated composite ID and that a child invocation overwrites the inherited parent env with its own ID.
- Env malformed handling: **covered** by `ignores_malformed_and_unresolved_parent_env_values` in [pr_a_invocation_integration.rs](/home/nes/projects/agent-runner/src-tauri/tests/pr_a_invocation_integration.rs:158). It verifies malformed or stale parent env input does not panic and results in a root invocation.

## Findings (severity ≥ medium)

No severity ≥ medium findings.

## Removed/modified test review

`record_and_query` was deleted appropriately. In `main`, the old `record_invocation` API is removed by contract, so retaining a test centered on that API would have been a backwards-compat shim, which would cut against V14. Its old behavioral intent was not dropped; it was redistributed into `start_invocation_inserts_running_row_with_null_terminal_fields`, `finalize_invocation_sets_terminal_fields`, and `finalize_invocation_updates_provider_aggregate_stats`. That is the right rewrite because the contract itself split the lifecycle into insert-on-spawn and update-on-finish.

`recent_errors` was rewritten honestly. The old test asserted that one failed call and one successful call produced a recent-error count of one. The new version preserves that same user-visible behavior while using `start_invocation` + `finalize_invocation` instead of the deleted helper. The rewrite does not weaken the assertion and does not silently narrow the scenario.

One extra positive note: the suite adds a migration rollback test even though it was not in the explicit required list. That is a good test-quality sign. It checks transactional migration behavior without depending on timing or external state, and it strengthens confidence that the schema rebuild is isolated from partial failure.

The only meaningful nits are about scope, not substance. A few tests could be split further if the team wants finer failure localization. `get_invocation_by_uuid_returns_matching_and_missing_rows` currently checks three related things in one body: lookup of a running row, lookup of a migrated legacy row, and the missing-UUID case. Likewise, `ignores_malformed_and_unresolved_parent_env_values` iterates three malformed-parent cases in one test. Those are acceptable tradeoffs here because the loops are explicit, deterministic, and still read as one behavioral class rather than a grab-bag.
