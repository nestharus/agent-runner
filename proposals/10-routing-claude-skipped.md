# Proposal — Fix routing-claude-skipped (identity-keyed fallback aggregate)

## Problem

Phase 0 RCA (`research/10-routing-claude-skipped-rca.md`) reproduced RC-1: fallback routing scores current provider indexes against stale aggregate rows keyed only by `(model_name, provider_index)`, so history from an earlier occupant of index 0 can make the current `claude` account look used and route traffic elsewhere. The Phase 2.5 problem map (`research/10-routing-claude-skipped-problem-map.md`) found the parallel identity-drift path in `recent_error_count`, which also filters by provider index even though `invocations` already records `provider_name` and quota tables are already keyed by provider name.

## Design

### Chosen option: A

Change the fallback aggregate identity from `(model_name, provider_index)` to `(model_name, provider_name)`. Rebuild the aggregate from `invocations`, because `invocations` is the durable source of truth for both provider identity and historical outcomes. Update aggregate writers/readers and `recent_error_count` to use provider identity for scoring and suppression.

### Rationale vs. alternatives

Option A matches the semantic identity used by quotas, session-turn reads, `start_invocation`, CLI dispatch, and pool grouping: the provider account name (`claude`, `claude2`, `claude3`) is the stable account identity, while index is only a current model-order coordinate. It directly fixes both stale fallback counts and stale recent-error suppression.

Option B keeps index in the aggregate key as `(model_name, provider_name, provider_index)`. That preserves order as part of usage identity and would split the same provider's history across rows when a user reorders providers. It prevents some stale-row collisions but does not fully express the desired behavior: a `claude` account should carry its own history independent of where it sits in model order.

Option C drops `providers` and computes fallback counts from `invocations` on demand. That would be the cleanest source-of-truth model but increases every fallback scoring read to grouped scans over invocation history. The app is single-user, so the read cost may be acceptable, but the existing aggregate table already gives a small, bounded scoring surface. Option A keeps that architecture while correcting its key.

No quota table changes are included. `provider_quotas` and `provider_quota_windows` are already keyed by `provider_name` and are the model for the aggregate fix, not a target of it.

### Schema change

Post-fix `providers` schema:

```sql
CREATE TABLE IF NOT EXISTS providers (
    model_name TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    invocation_count INTEGER NOT NULL DEFAULT 0,
    error_count INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_error_at TEXT,
    last_invoked_at TEXT,
    PRIMARY KEY (model_name, provider_name)
);
```

`provider_index` is removed from the aggregate schema. It remains on `invocations` for observability and historical reconstruction, but routing history is not keyed by it.

`ProviderRecord` changes from `provider_index: usize` to `provider_name: String`. No aggregate record type retains `provider_index`, because the post-fix aggregate cannot answer index identity without reintroducing the drift risk.

### Migration

Use a column-existence migration in `StateDb::open`, matching the local `ensure_provider_quotas_schema` style and avoiding any dependency on `PRAGMA user_version`.

Migration steps:

1. Create the post-fix `providers` table for new DBs with `provider_name` and primary key `(model_name, provider_name)`.
2. Add an `ensure_providers_schema(conn)` helper after `ensure_invocations_schema(conn)` and before quota/session helpers.
3. Inspect `PRAGMA table_info(providers)`.
4. If the table is missing, create the post-fix schema and return.
5. If `provider_name` exists and `provider_index` is absent, ensure any indexes needed by the new schema and return.
6. If the table is old-shaped, run one transaction:
   - `ALTER TABLE providers RENAME TO providers_legacy_index_keyed`.
   - Create the post-fix `providers` table.
   - Rebuild aggregate rows from terminal invocation rows that have a non-null provider name:

```sql
INSERT INTO providers (
    model_name,
    provider_name,
    invocation_count,
    error_count,
    last_error,
    last_error_at,
    last_invoked_at
)
SELECT
    model_name,
    provider_name,
    COUNT(*) AS invocation_count,
    SUM(CASE WHEN success = 0 THEN 1 ELSE 0 END) AS error_count,
    NULL AS last_error,
    NULL AS last_error_at,
    MAX(finished_at) AS last_invoked_at
FROM invocations
WHERE provider_name IS NOT NULL
  AND status IN ('succeeded', 'failed')
  AND success IS NOT NULL
GROUP BY model_name, provider_name;
```

   - Backfill `last_error` and `last_error_at` from the most recent **failed** invocation per `(model_name, provider_name)`. The query restricts to `success = 0` rows, takes `MAX(finished_at)` over those rows for `last_error_at`, and reads `error_category` (which is non-null for classified failures) for `last_error`. Successful-invocation timestamps are deliberately not used for `last_error_at`. If a `(model_name, provider_name)` group has no failed rows, both `last_error` and `last_error_at` remain `NULL`. Current `invocations` stores `error_category` but not the stderr snippet that old `providers.last_error` held, so the deterministic rebuild uses `error_category` as the preserved observable signal.
   - Drop `providers_legacy_index_keyed`.
1. The migration is idempotent by shape: after the first run, `providers` has `provider_name` and no `provider_index`, so the rebuild path does not run again.

This reconciles existing aggregate data instead of silently re-keying index rows. The old aggregate is discarded because it cannot prove provider identity. Rebuilt counts come from `invocations`, which already stores both `provider_name` and `provider_index`.

#### Migration error contract

Only the exact pre-fix shape (`provider_index` present, `provider_name` absent, primary key `(model_name, provider_index)`) and the exact post-fix shape (`provider_name` present, `provider_index` absent, primary key `(model_name, provider_name)`) are accepted. Any other observed shape — including a partially-migrated table that has both `provider_index` and `provider_name`, a foreign primary key, or unexpected extra columns — causes `StateDb::open` to return an error rather than attempt a heuristic recovery, so the user (or `agents migrate-db`) sees an explicit failure instead of silent re-keying.

The rename + create + rebuild + drop sequence runs inside one SQLite transaction. If any step fails, the transaction rolls back, leaving the original `providers` table intact (the `RENAME` is reverted, the temporary post-fix table is discarded, no source rows are mutated). A subsequent writable open of the same DB observes the unchanged pre-fix shape and retries the same shape-based migration; the `invocations` source of truth is never modified by the migration, so retries are deterministic.

If the pre-fix `providers` row count is zero, the migration still runs the rebuild query (which returns zero rows) and proceeds to drop the legacy table; the post-fix table is left empty and the supported normal-write path repopulates it on the next finalization.

##### Recovery procedure on unexpected `providers` shape

When `StateDb::open` (or `agents migrate-db`, which calls the same path) returns an unexpected-shape error, the user-facing recovery is:

1. Restore the SQLite DB from the most recent file-system backup of `state.db`. Re-running `agents` then exercises the migration on a known-good pre-fix shape.
2. If no backup is available, the user may manually drop the malformed `providers` table (and any leftover `providers_legacy_index_keyed`) using an external `sqlite3` shell. The next `StateDb::open` then takes the "table is missing → create post-fix shape" branch; the `agents migrate-db` path has the same behavior because it calls `StateDb::open`. This recreates an empty post-fix `providers` table and does not replay existing `invocations` to rebuild pre-drop aggregate counts. Aggregate counts older than the manual drop are permanently lost and only new finalizations repopulate `providers`.

This procedure is operator-level by design: a hybrid or foreign shape can only arise from external mutation (the binary itself never produces one because the migration is transactional), so it is correctly an operator-level event rather than a binary-recoverable event.

##### Residual: rollback during mid-rebuild failure is verified by code review, not runtime test

The transactional-rollback property — "if any step fails, the transaction rolls back, leaving the original `providers` table intact" — is a property of the explicit `BEGIN`/`COMMIT` envelope in `ensure_providers_schema`. Verifying it through a runtime test requires injecting a failure inside the rebuild step (after the `RENAME` succeeds and the post-fix `CREATE TABLE` runs, but before the rebuild `INSERT … FROM invocations` completes), and the only viable injections (test-only `CHECK` constraints, mid-transaction temp triggers attached to the post-fix table, sibling-connection lock contention, OS-level fault injection) either require test-only product-source changes (which violates `~/ai/conventions/no-deferred-stubs.md`) or are out of scope for unit/particular-integration tests. The rollback property is therefore verified at implementation time by code-review of `ensure_providers_schema`'s explicit transaction wrapper, with the unexpected-shape rejection test (below) covering the early-exit DDL collision path. This residual is recorded so that a future round considering crash-recovery or fault-injection coverage knows the gap is intentional, not forgotten.

### Writer changes

`finalize_invocation` must load `model_name`, `provider_name`, and `status` for the invocation being finalized. It should continue to run the invocation terminal update plus aggregate update in one transaction.

The aggregate upsert changes to:

```sql
INSERT INTO providers (model_name, provider_name, invocation_count, error_count, last_invoked_at)
VALUES (?1, ?2, 1, ?3, ?4)
ON CONFLICT (model_name, provider_name)
DO UPDATE SET
    invocation_count = invocation_count + 1,
    error_count = error_count + ?3,
    last_invoked_at = ?4;
```

The failure metadata update filters by `(model_name, provider_name)`. If an invocation has no `provider_name`, finalization should not write aggregate state; current `start_invocation` writes names for supported runtime paths, and legacy/null-name rows are migration-only history.

### Reader changes

Change `StateDb::get_provider` to read by `(model_name, provider_name)` instead of `(model_name, provider_index)`. Keep the method name only with the new signature; do not keep an index-keyed alias. All production call sites must be updated in the same diff:

- `score_by_invocation_count` uses `model.providers[i].name`.
- `round_robin_fallback` uses `model.providers[i].name`.
- State DB tests update their expected lookup key.

Fallback routing still returns provider indexes because the balancer's public behavior is choosing a current provider slot; only the historical score lookup changes to current provider identity.

### recent_error_count change

Change `recent_error_count` to filter by `(model_name, provider_name)`:

```sql
SELECT COUNT(*) FROM invocations
WHERE model_name = ?1
  AND provider_name = ?2
  AND success = 0
  AND created_at > ?3;
```

Update both balancer call sites to pass `&model.providers[i].name`:

- projection scoring in `compute_projections_from_records`
- fallback scoring in `score_by_invocation_count`

This ensures failures from an earlier occupant of an index do not suppress or penalize the current provider account.

## Scope and anti-scope

### In scope

- `providers` aggregate schema keyed by `(model_name, provider_name)`.
- Shape-based migration that rebuilds aggregate counts from `invocations`, with the migration error contract specified above.
- `finalize_invocation` aggregate write path.
- `get_provider` aggregate read path and its balancer callers.
- `recent_error_count` signature, query, and balancer callers.
- `src-tauri/examples/quota_check.rs` call-site update so the workspace continues to build after `get_provider`'s signature changes (developer tool only; not a supported user-facing surface but in-scope for the implementation diff).
- Unit/integration coverage for fallback selection, recent-error identity, migration (including the migration error contract), and aggregate round-trip after reorder/rename.

### Out of scope

- No quota table changes.
- No changes to `provider_quotas` or `provider_quota_windows`.
- No changes to `provider_name(command)` parsing.
- No changes to pool grouping in `derive_pools`.
- No UI changes.
- No new IPC commands.
- No changes to `migrate-config`.
- No changes to `migrate-db` behavior beyond opening `StateDb` and therefore applying the schema migration.
- No backwards-compatibility shim or index-keyed fallback reader kept after migration.

## Supported-surface track

### Deployment mode

Single shipped Tauri v2 desktop binary with embedded SQLite state opened through `StateDb::open`. The migration runs when the app or CLI opens writable state, including normal CLI runs and `agents migrate-db`.

### Cohort

Single-user local desktop users with local SQLite state. The affected cohort is users with multi-provider model pools where provider order, insertion, removal, replacement, or account renaming has made old index-keyed aggregate rows no longer match current provider identities.

### Adjacent user-reachable paths

Normal `agents -m <model>` execution uses `select_provider`, writes invocations, executes the CLI, finalizes, and updates aggregate counts. Interactive resume and non-interactive `agents resume` also write invocation rows and may call projection logic using `recent_error_count`. The UI test-model path can exercise selection but does not write aggregate usage. Pools UI and quota refresh UI remain name-keyed and do not display aggregate counts.

### Blast radius

The intentional behavior change is limited to fallback usage scoring and recent-error suppression. Density scoring still uses name-keyed quotas and windows; only its recent-error input changes from index-keyed to name-keyed. Quota exhaustion filters, quota refreshes, session-turn counts, command parsing, pool grouping, IPC, and UI rendering are unchanged.

The migration touches only the `providers` aggregate. It reads `invocations` but does not modify invocation rows. If a hidden external consumer reads the SQLite `providers` table directly, its schema changes; no such supported consumer is visible in the worktree.

### Migration path

Writable DB open detects old aggregate shape and rebuilds `providers` from terminal `invocations` rows with non-null `provider_name`. New DBs are created directly with the post-fix schema. The path does not require `PRAGMA user_version`; it follows existing shape-inspection migration patterns in `state/db.rs`.

### Rollback path

Rollback is a binary rollback plus restoring the pre-migration database from backup. Once a DB has been opened by the fixed binary, the old binary will not understand the post-fix `providers` primary key shape. This is acceptable under the project's no-backwards-compatibility convention, but it raises the importance of making the forward migration deterministic and covered by tests.

### Observability

`invocations` continues to record both `provider_name` and `provider_index`, so operators can inspect whether current selection matches provider identity and can reconstruct aggregate counts. Quota rows continue to expose provider-name state. The post-fix `providers` table exposes aggregate usage by `(model_name, provider_name)`, which makes the routing basis directly auditable.

## Net-value statement

Yes. This proposal clearly reduces a concrete current-state risk on the supported surface: normal local CLI routing can currently charge stale index history and stale index failures to the wrong provider account, causing an unused `claude` account to be skipped. The reduction outweighs the added blast radius and migration/rollback burden because the change aligns the only inconsistent routing history surfaces with the already-supported provider-name identity, touches no UI or quota schema, and rebuilds the aggregate from an existing source of truth instead of guessing from stale index rows.

## Assumption register

| Assumption | Evidence | Invalidates if |
|---|---|---|
| A1. `provider_name` is the stable supported provider account identity for routing history. | `start_invocation` persists provider name and index; quota tables and session-turn reads are keyed by provider name; `ProvidersConfig::effective_provider` preserves model provider keys. | A supported user path is found where two distinct provider accounts intentionally share the same `provider_name` and require separate fallback history. |
| A2. `provider_index` is not a stable identity and should remain only selection/observability metadata. | RCA red harness proves index-keyed history can be assigned to the wrong current provider after order drift; problem map identifies reorder/insertion/removal/replacement as brittle today. | Product requirements state that fallback usage must reset on reorder even when the same provider name moves. |
| A3. `invocations` is sufficient to rebuild aggregate counts. | `invocations` stores `model_name`, `provider_name`, `provider_index`, status, success, timestamps; current finalization writes one terminal row per invocation. | A supported DB shape has aggregate history that is not represented in `invocations` and is required for routing correctness. |
| A4. Losing exact old `providers.last_error` snippets during migration is acceptable. | The old aggregate has no reliable provider identity, and `invocations` preserves failure/category/timing but not stderr snippets; no UI path displays aggregate `last_error`. | A supported path or customer-facing diagnostic depends on exact aggregate stderr snippets surviving migration. |
| A5. Shape-based migration is the right local schema mechanism. | There is no active `PRAGMA user_version` write path; adjacent quota/schema helpers use column-existence checks. | A new schema-versioning mechanism is introduced before implementation and becomes the required migration path. |
| A6. Hidden direct reads of the `providers` SQLite table are unsupported. | Problem map found no IPC/UI/schema-probe consumer exposing provider aggregate counts; production callers are in `balancer/mod.rs`. | A documented supported integration or command reads `providers` directly by old `(model_name, provider_index)` shape. |

## Test-intent track

| Test or group | Risk | Level | Fixture source | Assumption link | Observable signal | Residual |
|---|---|---|---|---|---|---|
| Existing RCA harness: `fallback_count_routing_uses_current_provider_identity_not_stale_index_history` in `src-tauri/tests/rca_routing_claude_skipped.rs` | RC-1 fallback scoring charges stale index aggregate history to current provider; intended behavior is selecting history-free `claude` by provider name. | unit | Existing Phase 0 red harness with in-memory `StateDb` and three-provider `ModelConfig`. | A1, A2, A3 | `select_provider` returns index whose provider name is `claude`. | Does not verify persistent on-disk migration; only exercises runtime aggregate write/read behavior. |
| `recent_error_count` identity-drift test | Recent failures from prior occupant of an index suppress current provider; intended behavior counts only failures matching `(model_name, provider_name)`. | unit | State DB test fixture using `start_invocation`/`finalize_invocation` to write failed rows for old provider name at reused index and query current provider name. | A1, A2 | Count for current provider name at reused index is `0`; count for failed provider name is nonzero. | Does not verify projection tie-breaking; only verifies DB query contract. |
| Balancer recent-error call-site test | Balancer projection/fallback must pass provider names, not indexes; intended behavior is that stale index failures do not mark current provider over `ERROR_THRESHOLD`. | unit | Balancer test model with current providers reordered and failed invocation rows seeded for old provider names. | A1, A2 | Provider with no failures by name remains selectable/scored; stale index failures do not force `f64::MAX`/suppression. | Does not cover every density window combination. |
| Providers migration from pre-fix aggregate shape | Existing aggregate data must be reconciled, not silently re-keyed; intended behavior rebuilds counts from `invocations` by provider name. | particular-integration | Temporary on-disk SQLite DB manually initialized with old `providers(model_name, provider_index, ...)` and current `invocations`, then opened through `StateDb::open`. | A3, A4, A5 | Post-open `providers` has `provider_name`, no `provider_index`, primary key by name, and counts match grouped terminal invocation rows. | Does not prove every historical live DB variant; bounded to the documented pre-fix shape. |
| Aggregate writer/reader round-trip after provider reorder | Finalization and fallback reader must preserve provider identity across order changes; intended behavior is history follows provider name, not index. | unit | In-memory `StateDb`; write invocation for provider `claude2` at index 0, then query/score a model where `claude2` is at a different index and `claude` occupies index 0. | A1, A2 | `get_provider(model, "claude2")` reports the count; `get_provider(model, "claude")` is none/zero; fallback scoring treats `claude` as unused. | Does not verify migration from old aggregate rows. |
| Aggregate writer/reader round-trip after provider rename | A renamed provider name should not inherit the old name's aggregate history; intended behavior is new name has zero history unless invocation rows use that name. | unit | In-memory `StateDb`; write invocations for `claude-old`, then query/score current provider `claude`. | A1, A2 | `claude` aggregate is none/zero while `claude-old` retains old count by name. | Does not decide whether product should offer a separate rename migration; proposal intentionally does not. |
| Quota path unchanged regression | Fix must not alter quota identity or quota table behavior. | unit | Existing quota tests plus one targeted assertion that quota rows remain keyed only by provider name after opening a migrated DB. | A1 | Quota read/write tests pass; `provider_quotas` and `provider_quota_windows` schemas do not gain aggregate columns or index semantics. | Does not test upstream quota CLI behavior. |
| Migration error contract — unexpected shape rejected | Migration must refuse heuristic recovery on unexpected `providers` shapes (e.g., a partially-migrated table with both `provider_index` and `provider_name`); intended behavior is that `StateDb::open` returns an error rather than silently re-keying or dropping data, and the failed open does not mutate `providers` or `invocations`. | particular-integration | Temporary on-disk SQLite DB manually initialized with a hand-crafted `providers` table that has both `provider_index` and `provider_name` columns, then opened through `StateDb::open`; the same DB is then mutated to drop the malformed `providers` and reopened to confirm recovery. | A5 | First open returns an error whose message names the unexpected shape; `providers` and `invocations` are byte-identical to their pre-open state. After the operator-level cleanup (drop malformed `providers`), the second open completes and creates the post-fix table per the migration's "table is missing" branch. | Does not enumerate every malformed shape; one canonical "both columns present" case proves the rejection contract. Does not exercise the rebuild-step rollback (see "Residual: rollback during mid-rebuild failure" — that property is verified by code review of `ensure_providers_schema` at implementation time). |
| Migration `ensure_providers_schema` is idempotent across reopens | Once the post-fix shape is in place, subsequent `StateDb::open` calls must not re-run the rebuild branch and must not perturb the aggregate; intended behavior is that the helper observes the post-fix shape and returns immediately. | unit | In-memory or on-disk `StateDb` opened once on a pre-fix shape (forcing migration) and then reopened on the same DB file; the second open inspects the same `providers` rows. | A5 | First open migrates and produces a known set of `providers` rows; second open leaves the row count, `invocation_count`, `error_count`, and `last_invoked_at` columns identical (no rebuild re-fires). | Does not directly exercise transactional rollback; complements the unexpected-shape test by proving the "post-fix shape → no-op" branch. |
| Migration `last_error_at` reflects most recent failed invocation | `last_error_at` for a migrated row must be `MAX(finished_at)` over `success = 0` rows for that `(model_name, provider_name)`, not the most recent successful invocation; intended behavior preserves failure-vs-success semantics across migration. | particular-integration | Temporary on-disk SQLite DB with `invocations` containing both successful and failed terminal rows for the same `(model_name, provider_name)`, where the most recent row is successful. | A3, A4 | After `StateDb::open`, `providers.last_error_at` equals the most recent failed `finished_at` and `last_error` equals that failure's `error_category` (or `NULL` if no category); neither field references the later successful invocation. | Does not test multi-error_category preference order beyond "most recent". |

## Open questions

None requiring user input before Phase 4. The proposal fixes the aggregate identity to provider name, removes aggregate `provider_index`, and uses `error_category` as the deterministic migration source for rebuilt `last_error`. The migration error contract is now explicit and covered by tests.
