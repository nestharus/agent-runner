# Supported-Surface Verification (Phase 8): proposals/10-routing-claude-skipped.md (diff)

## Termination signal: none
## Verdict: LOW

The diff at `bb106f7` implements the approved proposal on the
supported surface (single-user desktop Tauri v2 binary with embedded
SQLite opened through `StateDb::open`, exercised by both normal CLI
runs and `agents migrate-db`). RC-1 is closed: the Phase 0 harness
(`src-tauri/tests/rca_routing_claude_skipped.rs::fallback_count_routing_uses_current_provider_identity_not_stale_index_history`)
is GREEN at HEAD, fallback scoring now reads from a name-keyed
`providers` aggregate, and both `recent_error_count` call sites
(`compute_projections_from_records` and `score_by_invocation_count`)
pass `&model.providers[i].name`. The migration helper
(`ensure_providers_schema`) wraps rename+create+rebuild+drop in a
single `Connection::transaction()` and `validate_providers_schema`
runs as a non-mutating preflight inside `StateDb::open` (and again
inside `migrate_legacy_invocations` after `R6-F03`) so unexpected
shapes are rejected before any source mutation. Object-type, FK, and
column-shape rejections all return `Err(...)` without touching
`providers` or `invocations`, with byte-identity-asserting integration
tests covering each branch. Quota schemas, `provider_name(command)`
parsing, `derive_pools`, IPC, UI, and `migrate-config` are untouched
on the branch. The only diff scope outside the in-scope code surface
is the in-scope `examples/quota_check.rs:123` call-site update and the
in-scope `tests/rca_routing_claude_skipped.rs` harness, both
explicitly listed by the approved proposal. A1-A6 hold against the
actual code. The change is not symbolic hardening: the routing-history
identity surface is materially re-keyed and verified by tests beyond
the RCA harness (round-trip after reorder, round-trip after rename,
recent-error identity drift, null-name skip-write,
last_error_at-vs-success, deterministic tie-break).

## Watch-signal status (from risk/10-history.md)

- WS-1: upheld
- WS-2: upheld
- WS-3: upheld

## Findings

### RC-1 closed end-to-end on the supported routing path

- Severity: low
- Surface concerned: `select_provider` → `score_by_invocation_count`
  / `round_robin_fallback` → `StateDb::get_provider` /
  `StateDb::recent_error_count` on the single-user local desktop
  cohort.
- Net effect: reduces risk.
- Evidence: `src-tauri/src/balancer/mod.rs:261` and
  `src-tauri/src/balancer/mod.rs:591` now pass
  `&model.providers[i].name` to `recent_error_count`;
  `src-tauri/src/balancer/mod.rs:600` and
  `src-tauri/src/balancer/mod.rs:632` pass it to `get_provider`. The
  Phase 0 RCA harness is GREEN at HEAD
  (`fallback_count_routing_uses_current_provider_identity_not_stale_index_history`,
  passes 1/1). The new in-balancer
  `fallback_recent_error_scoring_uses_provider_name_not_reused_index`
  test (`src-tauri/src/balancer/mod.rs:732-749`) covers the
  recent-error suppression call site by name.

### Migration error contract is enforced and tested on actual diff

- Severity: low
- Surface concerned: writable `StateDb::open` on the supported
  cohort.
- Net effect: reduces risk.
- Evidence: `validate_providers_schema`
  (`src-tauri/src/state/db.rs`) runs three ordered layers — object
  type via `sqlite_master`, foreign keys via
  `PRAGMA foreign_key_list(providers)`, and column shape via
  `providers_columns` — before `ensure_invocations_schema`. The
  helper is re-invoked inside `migrate_legacy_invocations` after the
  invocation transaction begins (commit `bb106f7` applies the
  Round 6 strengthening). Tests cover each rejection branch with
  byte-identity assertions:
  `providers_migration_rejects_unexpected_shape_without_mutating_source_tables`,
  `providers_migration_rejects_wrong_affinity_shape`,
  `providers_migration_rejects_non_table_object_named_providers`,
  `providers_migration_rejects_table_with_foreign_keys`, and
  `providers_preflight_rejects_malformed_shape_before_invocations_migration`.
  Idempotency-across-reopens is asserted by
  `providers_migration_is_idempotent_across_reopens`. None of these
  introduces a heuristic-recovery branch inside `StateDb::open`.

### Aggregate identity round-trip after reorder/rename verified

- Severity: low
- Surface concerned: routing-history identity invariant on the
  supported cohort.
- Net effect: reduces risk.
- Evidence:
  `provider_aggregate_round_trip_follows_name_after_reorder` and
  `provider_aggregate_round_trip_does_not_inherit_renamed_provider_history`
  in `src-tauri/src/state/db.rs` exercise the writer/reader contract
  by provider name across reorder and rename, matching the proposal's
  test-intent track entries. The reorder test additionally asserts
  via `select_provider` that fallback scoring treats the current
  `claude` slot as unused, tying the round-trip to the supported
  selection path.

### Out-of-scope surfaces unchanged

- Severity: low
- Surface concerned: surfaces explicitly out of scope for this fix
  (quota tables, IPC, UI, `migrate-config`,
  `provider_name(command)`, pool grouping, `migrate-db` behavior).
- Net effect: neutral.
- Evidence: `git diff main..HEAD --name-only` lists only
  `src-tauri/src/state/db.rs`, `src-tauri/src/balancer/mod.rs`,
  `src-tauri/examples/quota_check.rs`, and
  `src-tauri/tests/rca_routing_claude_skipped.rs` on the code
  surface; remaining changes are proposal/research/risk artifacts.
  `quota_schema_remains_name_keyed_after_provider_migration`
  asserts that `provider_quotas` and `provider_quota_windows`
  retain their existing name-keyed shape and gain no aggregate
  identity columns after the migration runs. (`.github/workflows/release.yml`
  and `DECISIONS.md` appear in `git diff main..HEAD` only because
  they advanced on `main` after the merge-base `9cadc90`; they are
  not branch-side changes.)

### WS-3 residual remains code-review-only — no symbolic re-promotion

- Severity: low
- Surface concerned: pre-merge verification of the migration's
  transactional-rollback property.
- Net effect: neutral (preserves Round 3 posture).
- Evidence: `risk/10-test-residuals.md` records the residual class,
  technique attempted, and reason it is not verified by Phase 6b
  runtime tests. No test in the diff claims runtime coverage of
  mid-rebuild rollback. The `BEGIN`/`COMMIT` envelope inside
  `ensure_providers_schema` is small (RENAME → CREATE → INSERT
  rebuild → UPDATE backfill → DROP → commit) and amenable to
  code-review verification, exactly as `WS-3` requires.

## Assumption review

- A1 (provider_name is the stable supported provider account
  identity for routing history) — uphold. `providers` PK is
  `(model_name, provider_name)`; `get_provider` and
  `recent_error_count` filter by `provider_name` only;
  `finalize_invocation` upserts and updates by name.
- A2 (provider_index is selection/observability metadata only) —
  uphold. `ProviderRecord.provider_index` is replaced by
  `provider_name`. `provider_index` survives only on
  `InvocationRecord`/`InvocationStart` and the balancer's runtime
  selection-slot semantics, matching the contract's narrowing
  (`R4-F04`).
- A3 (`invocations` is sufficient to rebuild aggregate counts) —
  uphold. Rebuild SELECT filters
  `provider_name IS NOT NULL AND status IN ('succeeded', 'failed')
  AND success IS NOT NULL`; the `last_error_at` backfill restricts
  to `success = 0` rows and orders by
  `finished_at DESC, id DESC`. `providers_migration_rebuilds_aggregate_from_invocations_by_provider_name`
  asserts the resulting aggregate matches the expected derivation,
  including the deliberate skip of the null-`provider_name` row.
- A4 (losing exact `providers.last_error` snippets during migration
  is acceptable) — uphold. Migration backfills `last_error` from
  `error_category`; `finalize_invocation_skips_provider_aggregate_for_null_provider_name`
  confirms the supported normal-write path remains responsible for
  populating fresh aggregate state.
- A5 (shape-based migration is the right local schema mechanism) —
  uphold. No `PRAGMA user_version` write path was introduced; the
  helper validates exact pre-fix and post-fix column shapes (name,
  type affinity, NOT NULL, PK position) and rejects everything else.
- A6 (hidden direct reads of the `providers` SQLite table are
  unsupported) — uphold. The branch adds no new external readers of
  `providers`. `examples/quota_check.rs` is the only developer-tool
  surface that reads aggregate state and was updated in the same
  diff.

## Notes

- HEAD `bb106f7` includes the `R8-F03`/`R8-F04` close commit which
  added the object-type and foreign-key preflight branches. Without
  those branches the contract's "unexpected shape rejection" was
  promised but only partially enforceable; the diff at HEAD now
  enforces the contract per the supported-surface gate's
  expectations.
- `WS-2` is upheld because `provider_index` references that remain
  in `balancer/mod.rs` are exclusively about runtime selection slots
  (`select_provider`, projection bookkeeping, migration target
  decisions); none reads aggregate or recent-error state by index.
- The R7-F02 `finalize_invocation_skips_provider_aggregate_for_null_provider_name`
  test directly exercises the proposal's "skip-write rule" for
  null-`provider_name` legacy rows, closing the gap between
  contract and test track.
- Net value on the supported surface remains clearly positive.
