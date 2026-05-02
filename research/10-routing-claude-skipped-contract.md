# Implementation Contract — routing-claude-skipped

This is the Phase 6a contract for `proposals/10-routing-claude-skipped.md`.
It must be clear enough for the Phase 6b test-writer to author tests
without reading product source. It must preserve every change risk,
selected test level, fixture source, assumption-register link, and
expected observable signal from the approved test-intent track.

## 1. Schema (post-fix `providers`)

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

`provider_index` is **absent**. No additional columns. No additional
indexes are required by this proposal.

## 2. Migration helper

### Signature

```rust
fn ensure_providers_schema(conn: &mut Connection) -> Result<(), String>;
```

Located in `src-tauri/src/state/db.rs`, alongside other
`ensure_*_schema` helpers (next to
`ensure_provider_quotas_schema`, `ensure_provider_quota_windows_schema`,
`ensure_session_turns_schema`).

### Call ordering inside `StateDb::open`

First run a non-mutating `providers` shape preflight before
`Self::ensure_invocations_schema(&conn)?`, so an unexpected
`providers` shape is rejected before any legacy `invocations` migration
can mutate source rows. Then call `Self::ensure_invocations_schema(&conn)?`
and `Self::ensure_providers_schema(&mut conn)?` **before** the inline
`execute_batch` that creates downstream tables. The old inline
`CREATE TABLE IF NOT EXISTS providers (...) PRIMARY KEY (model_name,
provider_index)` block is **deleted**; `ensure_providers_schema` is the
sole creator of the `providers` table.

### Behavior — by observed shape

Use a `providers_columns` helper (mirroring
`provider_quotas_columns`) to inspect `PRAGMA table_info(providers)`.

1. **Table missing.** Create the post-fix table. Return `Ok(())`.
2. **Post-fix shape** (`provider_name` present, `provider_index`
   absent, primary key `(model_name, provider_name)`). Return `Ok(())`
   without modification — idempotent.
3. **Pre-fix shape** (`provider_index` present, `provider_name`
   absent, primary key `(model_name, provider_index)`). Inside one
   `Connection::transaction()`:
   1. `ALTER TABLE providers RENAME TO providers_legacy_index_keyed;`
   2. `CREATE TABLE providers (...post-fix DDL...);`
   3. Rebuild from `invocations` for `success IS NOT NULL` rows whose
      `provider_name IS NOT NULL`:

      ```sql
      INSERT INTO providers (
          model_name, provider_name,
          invocation_count, error_count,
          last_error, last_error_at, last_invoked_at
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

   4. Backfill `last_error_at` and `last_error` for each
      `(model_name, provider_name)` group from the most recent
      `success = 0` row only:

      ```sql
      UPDATE providers
         SET last_error_at = (
                 SELECT i.finished_at
                   FROM invocations i
                  WHERE i.model_name = providers.model_name
                    AND i.provider_name = providers.provider_name
                    AND i.success = 0
                  ORDER BY i.finished_at DESC, i.id DESC
                  LIMIT 1
             ),
             last_error = (
                 SELECT i.error_category
                   FROM invocations i
                  WHERE i.model_name = providers.model_name
                    AND i.provider_name = providers.provider_name
                    AND i.success = 0
                  ORDER BY i.finished_at DESC, i.id DESC
                  LIMIT 1
             )
       WHERE EXISTS (
                 SELECT 1
                   FROM invocations i
                  WHERE i.model_name = providers.model_name
                    AND i.provider_name = providers.provider_name
                    AND i.success = 0
             );
      ```

      Groups with no `success = 0` rows leave both `last_error_at`
      and `last_error` as `NULL`. When failed rows tie on
      `finished_at`, the row with the highest `invocations.id` is the
      deterministic winner.
   5. `DROP TABLE providers_legacy_index_keyed;`
   6. `tx.commit()` — single transaction; any error rolls back the
      entire migration leaving the original `providers` table intact
      (the rename is reverted, the temporary post-fix table is
      discarded, no `invocations` rows are mutated).
4. **Any other shape** — including a partially-migrated table that
   has both `provider_index` and `provider_name`, a foreign primary
   key, or unexpected extra columns. Return an `Err(...)` whose
   message names the unexpected shape. Do not attempt heuristic
   recovery. Do not rename, drop, or modify any table. Do not mutate
   `invocations`.

### Idempotency

After a successful run on case 3, the table is in case 2; subsequent
opens return `Ok(())` without modification.

### Atomicity / no source mutation

The rebuild is wrapped in `Connection::transaction()`. The migration
never modifies `invocations`. Callers may rely on this for retries
after partial failure.

## 3. `StateDb::get_provider` — new signature

### Signature

```rust
fn get_provider(
    &self,
    model_name: &str,
    provider_name: &str,
) -> Result<Option<ProviderRecord>, String>;
```

### `ProviderRecord`

The `provider_index: usize` field is **renamed and retyped** to
`provider_name: String` on `ProviderRecord`. Other routing/invocation
types still retain current-slot index metadata: `InvocationRecord`,
`InvocationStart`, and section 5's invocation-row load continue to use
`provider_index` for observability and supported invocation handling.

```rust
pub struct ProviderRecord {
    pub model_name: String,
    pub provider_name: String,
    pub invocation_count: i64,
    pub error_count: i64,
    pub last_error: Option<String>,
    pub last_error_at: Option<String>,
    pub last_invoked_at: Option<String>,
}
```

Field order, types, and visibility otherwise match the current
`ProviderRecord` definition in `state::db`.

### Query

```sql
SELECT model_name, provider_name, invocation_count, error_count,
       last_error, last_error_at, last_invoked_at
  FROM providers
 WHERE model_name = ?1 AND provider_name = ?2;
```

Returns `Ok(None)` for missing rows. No legacy index-keyed alias.

### Production callers (must all be updated in the same diff)

- `score_by_invocation_count` in `src-tauri/src/balancer/mod.rs` —
  pass `&model.providers[i].name`.
- `round_robin_fallback` in `src-tauri/src/balancer/mod.rs` — pass
  `&model.providers[i].name`.
- The provider diagnostics loop in `src-tauri/examples/quota_check.rs`
  — pass `&p.name`.

### Tests that must be updated in the same diff

- `finalize_invocation_updates_provider_aggregate_stats`.
- `missing_provider_returns_none`.

## 4. `StateDb::recent_error_count` — new signature

### Signature

```rust
fn recent_error_count(
    &self,
    model_name: &str,
    provider_name: &str,
    window_minutes: i64,
) -> Result<i64, String>;
```

### Query

```sql
SELECT COUNT(*) FROM invocations
 WHERE model_name = ?1
   AND provider_name = ?2
   AND success = 0
   AND created_at > ?3;
```

The third argument to `StateDb::recent_error_count` is
`window_minutes`; the method computes the lower bound for `created_at`
internally as `now - window_minutes`.

### Production callers (must all be updated in the same diff)

- Projection scoring inside `compute_projections_from_records`; pass
  `&model.providers[i].name`.
- Fallback scoring inside `score_by_invocation_count`; pass
  `&model.providers[i].name`.

### Tests that must be updated in the same diff

- `recent_errors`.

## 5. `finalize_invocation` — writer changes

### Invocation row load (`StateDb::finalize_invocation`)

Read `model_name`, `provider_name`, `provider_index`, and `status`
from `invocations` in the same SELECT. The current SELECT reads only
`(model_name, provider_index, status)`; extend it to include
`provider_name`. Tuple binding becomes
`(model_name, provider_name, provider_index, status)`.

### Aggregate upsert (`StateDb::finalize_invocation`)

Replace with:

```sql
INSERT INTO providers (
    model_name, provider_name,
    invocation_count, error_count, last_invoked_at
) VALUES (?1, ?2, 1, ?3, ?4)
ON CONFLICT (model_name, provider_name)
DO UPDATE SET
    invocation_count = invocation_count + 1,
    error_count = error_count + ?3,
    last_invoked_at = ?4;
```

`?3` is `1` if the invocation finalized as a failure (`success = 0`)
or `0` otherwise. `?4` is the finalization timestamp string.

### Failure metadata update (`StateDb::finalize_invocation`)

Currently filters `WHERE model_name = ?3 AND provider_index = ?4`.
After this fix, filter by `(model_name, provider_name)`:

```sql
UPDATE providers
   SET last_error = ?1, last_error_at = ?2
 WHERE model_name = ?3 AND provider_name = ?4;
```

`?1` is the stderr snippet (or `None`); `?2` is the finalization
timestamp; `?3`/`?4` come from the invocation row load above.

### Skip-write rule

If the invocation row's `provider_name IS NULL` (only possible for
legacy/migration rows; current `start_invocation` writes the name),
skip both the aggregate upsert and the failure metadata update.
The terminal invocation update still happens.

### Transaction boundary

Both the upsert and the failure metadata update must remain inside
the `StateDb::finalize_invocation` transaction.

## 6. `examples/quota_check.rs`

Replace the call site to use the new signature. Surrounding code
already has `&p.name` available in the loop; only the lookup
argument changes.

## 7. Test-intent handoff (Phase 6b inputs)

Test-writer (gpt-high, separate invocation from Phase 6c) must
emit tests that hold these contracts and that map to the named risks
in `proposals/10-routing-claude-skipped.md` §Test-intent track.

| Risk | Selected level | Fixture source | Test target |
|---|---|---|---|
| RC-1 fallback identity | unit | Existing harness | Existing `src-tauri/tests/rca_routing_claude_skipped.rs` (already RED at pre-fix HEAD; will be GREEN at post-fix HEAD). Keep as-is — Phase 6b output index must list it as the test for this risk. |
| `recent_error_count` identity drift | unit | New `StateDb` test using `start_invocation`/`finalize_invocation` to seed failed rows for old name at reused index, then query current name. | New unit test in `src-tauri/src/state/db.rs` tests module. |
| Balancer recent-error call site | unit | Balancer test using existing `record_invocation_for_test` helper. | New unit test in `src-tauri/src/balancer/mod.rs` tests module. |
| Aggregate round-trip after reorder | unit | Existing `test_db()` helper for in-memory `StateDb`. | New unit test in `src-tauri/src/state/db.rs` tests module. |
| Aggregate round-trip after rename | unit | Existing `test_db()`. | New unit test alongside the reorder test. |
| Quota path unchanged regression | unit | Existing quota tests + one targeted assertion that quota schemas are unchanged after migration. | New assertion in `src-tauri/src/state/db.rs` tests module, near migration tests. |
| Migration error contract — unexpected shape rejected | particular-integration | Temporary on-disk SQLite DB; mirror the `legacy_invocations_db(...)` style fixture in the `state::db` tests. Hand-craft `providers` with both `provider_index` and `provider_name` columns. After first open returns `Err`, drop the malformed table externally and reopen to confirm post-fix branch fires. | New integration test in `src-tauri/tests/` (own file) or in the `state::db` tests module if the on-disk fixture is feasible there. |
| Migration `ensure_providers_schema` is idempotent across reopens | unit | In-memory or on-disk `StateDb`. | New unit test alongside the migration tests. |
| Migration `last_error_at` reflects most recent failed invocation | particular-integration | Temporary on-disk SQLite DB; populate `invocations` with mixed success/failure rows where the most recent row is successful. | New integration test in `src-tauri/tests/` (own file) or in the state tests module. |

### Risk annotations on each test

Every test or test group must carry an annotation naming:

- The risk it reduces (one of the rows above).
- The selected level (`unit` / `particular-integration`).
- The proposal source: `proposals/10-routing-claude-skipped.md
  §Test-intent track`.

## 8. Behavioral assumptions (carried from proposal §Assumption register)

- A1 — `provider_name` is unique within a model. The new aggregate
  primary key relies on this. Quota tables already do.
- A2 — `provider_index` is selection/observability metadata only.
  After this contract lands, no aggregate or recent-error reader
  carries identity by index.
- A3 — `invocations` is sufficient to rebuild aggregate counts.
  Confirmed in Phase 5 hookpoints: schema includes `model_name`,
  nullable `provider_name`, `provider_index`, `status`, `success`,
  `error_category`, `created_at`, `finished_at`.
- A4 — Migrating `last_error` from `error_category` (with stderr
  snippet loss for migrated rows only) is acceptable. Post-migration
  writes restore stderr snippets via `finalize_invocation`'s failure
  metadata update.
- A5 — Shape-based migration is the right local mechanism. No
  `PRAGMA user_version` write path is introduced.
- A6 — No hidden direct readers of the `providers` table exist.
  Confirmed in Phase 5 hookpoints.

## 9. Watch signals (carried from `risk/10-history.md`)

- WS-1: `ensure_providers_schema` remains transactional and rejects
  unexpected shapes; no heuristic recovery affordances.
- WS-2: no index-keyed reader alias is kept on the routing-history
  surface (`providers` / `get_provider` / `recent_error_count`).
- WS-3: rollback during mid-rebuild failure is a code-review residual
  only. The Phase 6b test agent must NOT silently re-introduce a
  runtime-test claim on this property.

## 10. Out of scope (for the test agent and code agent)

- No quota table changes.
- No changes to `provider_name(command)` parsing.
- No changes to pool grouping in `derive_pools`.
- No UI changes.
- No new IPC commands.
- No changes to `migrate-config`.
- No changes to `migrate-db` behavior beyond opening `StateDb`.
- No backwards-compatibility shim or index-keyed fallback reader
  retained after migration.
- No additional schema columns or indexes on `providers` beyond
  what this contract specifies.
- No PRAGMA user_version write path.
- No runtime test of mid-rebuild rollback (it remains a code-review
  residual per WS-3).
