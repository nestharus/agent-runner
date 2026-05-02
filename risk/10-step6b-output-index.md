# Phase 6b output index — routing-claude-skipped

## Inputs read

- approved proposal: `proposals/10-routing-claude-skipped.md`
- contract: `research/10-routing-claude-skipped-contract.md`
- problem map: `research/10-routing-claude-skipped-problem-map.md`
- supported-surface risk: `risk/10-supported-surface.md`
- hookpoint research: `research/10-routing-claude-skipped-hookpoints.md`
- Phase 0 RCA: `research/10-routing-claude-skipped-rca.md`
- audit history: `risk/10-history.md`
- Phase 6b prompt: `risk/10-step6b-prompt.md`
- Phase 6b log: `risk/10-step6b-log.md`

## Mapping

| Proposal test-intent item | Risk name | Level | Source | Emitted test file | Test-or-group identifier | Fixture source / application point | Residual entry |
|---|---|---|---|---|---|---|---|
| Existing RCA harness: `fallback_count_routing_uses_current_provider_identity_not_stale_index_history` | RC-1 fallback identity | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/tests/rca_routing_claude_skipped.rs` | `fallback_count_routing_uses_current_provider_identity_not_stale_index_history` | Existing Phase 0 in-memory `StateDb` harness; firstness already satisfied pre-fix. | None |
| `recent_error_count` identity-drift test | `recent_error_count` identity drift | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `recent_error_count_uses_provider_name_not_reused_index_history` | `record_provider_invocation(...)` helper seeds failed rows through `start_invocation` / `finalize_invocation`; test queries by provider name. | None |
| Balancer recent-error call-site test | Balancer recent-error call-site | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/balancer/mod.rs` | `fallback_recent_error_scoring_uses_provider_name_not_reused_index` | Existing `record_invocation_for_test(...)` helper seeds stale failed rows for old provider name at reused index. | None |
| Providers migration from pre-fix aggregate shape | Providers migration from pre-fix aggregate shape | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `providers_migration_rebuilds_aggregate_from_invocations_by_provider_name` | On-disk `legacy_providers_db(...)` fixture helper creates old `providers(model_name, provider_index, ...)` and current `invocations`; `StateDb::open` applies migration. | None |
| Aggregate writer/reader round-trip after provider reorder | Aggregate writer/reader round-trip after provider reorder | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `provider_aggregate_round_trip_follows_name_after_reorder` | In-memory `test_db()` plus `record_provider_invocation(...)`; current model is reordered after history is written. | None |
| Aggregate writer/reader round-trip after provider rename | Aggregate writer/reader round-trip after provider rename | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `provider_aggregate_round_trip_does_not_inherit_renamed_provider_history` | In-memory `test_db()` plus `record_provider_invocation(...)`; current lookup uses a different provider name than historical rows. | None |
| Quota path unchanged regression | Quota path unchanged regression | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `quota_schema_remains_name_keyed_after_provider_migration` | On-disk `provider_rebuild_fixture_db()` opens a migrated DB, then asserts quota tables remain provider-name keyed and do not gain aggregate identity columns. Existing quota tests remain unchanged. | None |
| Migration error contract — unexpected shape rejected | Migration error contract — unexpected shape rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `providers_migration_rejects_unexpected_shape_without_mutating_source_tables` | On-disk `malformed_providers_shape_db()` creates a hybrid table with both `provider_index` and `provider_name`; table snapshots are taken outside `StateDb::open`; cleanup drops malformed `providers` before recovery open. | `risk/10-test-residuals.md#ws-3-rollback-during-mid-rebuild-failure` |
| Migration `ensure_providers_schema` is idempotent across reopens | Migration `ensure_providers_schema` is idempotent across reopens | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `providers_migration_is_idempotent_across_reopens` | On-disk `provider_rebuild_fixture_db()` forces migration once; `provider_aggregate_snapshot(...)` compares aggregate rows after a second `StateDb::open`. | None |
| Migration `last_error_at` reflects most recent failed invocation | Migration `last_error_at` reflects most recent failed invocation | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | `src-tauri/src/state/db.rs` | `providers_migration_last_error_at_uses_most_recent_failure_not_later_success` | On-disk `provider_last_error_fixture_db()` includes failed, later successful, and more-recent failed terminal rows for one provider. | None |

## Post-Phase-6b additions (firstness evidence)

The following tests were added **after** the Phase 6b commit `e84adaf`
during the Phase 7 CodeRabbit loop and the Phase-7 close-out for
`R8-F03/F04`. Each was added in the **same commit** as the product
code or contract refinement that motivated it; the test was authored
before the matching product hunk in the diff and would have failed
RED at that pre-hunk point. Phase 6b firstness applies per named
risk and selected level; these are additional risk cells that
emerged from the CodeRabbit / R8 reviews on the Phase-6c diff.

| Test or group | Risk name | Level | Source | Emitted in commit | Origin / firstness route |
|---|---|---|---|---|---|
| `providers_migration_rejects_wrong_affinity_shape` | Migration error contract — wrong column affinity rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | CodeRabbit pass (folded into `5c10702`) | Folded amend during CodeRabbit loop; test added before the corresponding affinity-rejection branch in `validate_providers_schema`. RED at pre-hunk HEAD. |
| `providers_preflight_rejects_malformed_shape_before_invocations_migration` | Migration error contract rejects before source-table mutation | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §2 Migration helper | CodeRabbit pass (folded into `5c10702`) | Folded amend during CodeRabbit loop; test added before the preflight call ordering change in `StateDb::open`. RED at pre-hunk HEAD. |
| `providers_migration_last_error_ties_use_highest_invocation_id` | Migration `last_error_at` deterministic tie-break | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §2 Migration helper | CodeRabbit pass (folded into `5c10702`) | Folded amend during CodeRabbit loop; test added before the deterministic tie-break clause in the `last_error_at` rebuild query. RED at pre-hunk HEAD. |
| `finalize_invocation_skips_provider_aggregate_for_null_provider_name` | Null-provider legacy rows must not synthesize aggregate identity | unit | `proposals/10-routing-claude-skipped.md` §Test-intent track; `research/10-routing-claude-skipped-contract.md` §5 finalize_invocation | CodeRabbit pass (folded into `5c10702`) | Folded amend during CodeRabbit loop; test added before the skip-write clause in `finalize_invocation`. RED at pre-hunk HEAD. |
| `providers_migration_rejects_non_table_object_named_providers` | Migration error contract — providers as non-table object rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | `bb106f7` | R8-F03 close-out; test added before the `providers_object_type()` validation branch. RED at pre-hunk HEAD. |
| `providers_migration_rejects_table_with_foreign_keys` | Migration error contract — providers with foreign keys rejected | particular-integration | `proposals/10-routing-claude-skipped.md` §Test-intent track | `bb106f7` | R8-F04 close-out; test added before the `providers_has_foreign_keys()` validation branch. RED at pre-hunk HEAD. |

The original Phase 6b test-writer/code-writer separation is preserved
for the proposal-track items in the table above; these are
additional risk cells that the Phase 7 / Phase 7 close-out reviewers
required, with their authoring colocated with the product hunk in
the same commit and verified against the pre-hunk diff. None of
these tests relax assertions, regenerate baselines, delete coverage,
narrow input space, or remove a risk annotation in the Phase 6b set.

## Residuals

- `risk/10-test-residuals.md#ws-3-rollback-during-mid-rebuild-failure` records the approved residual for mid-rebuild rollback injection. Phase 6b did not introduce a runtime test claim for WS-3.

## Pre-fix run state

- `cargo test --no-run`: compile-error, expected at pre-fix HEAD because tests encode the post-fix `get_provider(model, provider_name)` and `recent_error_count(model, provider_name, ...)` contracts.
- `cargo test --test rca_routing_claude_skipped -- --nocapture`: red, existing Phase 0 harness still fails by selecting `claude3` instead of `claude`.
- New test filters: compile-error, same API mismatch as `cargo test --no-run`.
- Verbatim tails: `risk/10-step6b-log.md`.

## Notes

- The compile error is firstness evidence for the new signature and record-field contracts: pre-fix product code still exposes index-keyed APIs.
- No product logic was changed. Edits under `src-tauri/src/state/db.rs` and `src-tauri/src/balancer/mod.rs` are confined to `#[cfg(test)]` test modules.
- The existing RCA harness was not modified.
