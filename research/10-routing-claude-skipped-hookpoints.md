# Hookpoints — routing-claude-skipped

Line numbers in this document were verified against commit `b4f2a50`
on 2026-05-02. Treat them as navigational hints; before implementation,
verify each hookpoint by symbol/context search and stop if the expected
surrounding code no longer matches.

## Staleness check

Before applying these hookpoints, run searches for:
`ensure_invocations_schema`, `ensure_providers_schema`,
`execute_batch`, `ensure_provider_quotas_schema`,
`ensure_provider_quota_windows_schema`, and
`ensure_session_turns_schema`. Confirm the writable `StateDb::open`
path still opens the SQLite connection, enables WAL, calls
`Self::ensure_invocations_schema(&conn)?`, then places
`Self::ensure_providers_schema(&mut conn)?` before quota/session
helpers. Fail the task instead of guessing if any symbol is missing or
if the expected context has moved to a different ownership boundary.

## Insertion points

### `ensure_providers_schema` insertion

- `src-tauri/src/state/db.rs:496-506` is the owned writable schema-open path. It opens the SQLite connection, enables WAL, then calls `Self::ensure_invocations_schema(&conn)?`.
- Stable anchor: `pub fn open(path: &Path) -> Result<Self, String>` with
  the adjacent `Connection::open(path)`, `PRAGMA journal_mode=WAL`, and
  `Self::ensure_invocations_schema(&conn)?` operations.
- Slot `Self::ensure_providers_schema(&conn)?` immediately after `src-tauri/src/state/db.rs:506`, before the large `execute_batch` that currently begins with the old `providers` DDL at `src-tauri/src/state/db.rs:508-518`.
- Stable anchor for the old insertion point: the surrounding
  `execute_batch` used to contain inline
  `CREATE TABLE IF NOT EXISTS providers` DDL before the
  `provider_quotas` DDL; after the fix, the helper name
  `Self::ensure_providers_schema(&mut conn)?` is the durable marker.
- Remove the old inline `CREATE TABLE IF NOT EXISTS providers (...) PRIMARY KEY (model_name, provider_index)` block from that batch. If it remains before the helper, new DBs will be created in the pre-fix shape before the migration can inspect shape, which conflicts with the proposal.
- The rest of the batch can continue to create `provider_quotas`, `provider_quota_windows`, memory/setup/account/discovery/session tables at `src-tauri/src/state/db.rs:520-663`.
- Keep the existing helper order at `src-tauri/src/state/db.rs:666-668`: `ensure_provider_quotas_schema`, `ensure_provider_quota_windows_schema`, then `ensure_session_turns_schema`. The proposed providers migration depends on `invocations` already existing because its rebuild query selects from `invocations`; it does not depend on quota/session helpers.

### Aggregate writer hookpoint

- `src-tauri/src/state/db.rs:1172-1261` is the full `finalize_invocation` transaction.
- The invocation row load is at `src-tauri/src/state/db.rs:1186-1200`. It currently reads only `model_name`, `provider_index`, and `status` via:
  - `src-tauri/src/state/db.rs:1186`: tuple binding `(model_name, provider_index, status)`
  - `src-tauri/src/state/db.rs:1188`: `SELECT model_name, provider_index, status FROM invocations WHERE id = ?1`
  - `src-tauri/src/state/db.rs:1191-1195`: row extraction
- `provider_name` is not read from the invocation row today. The hook is to extend this SELECT and tuple to read `provider_name` from `invocations`; `start_invocation` already writes it at `src-tauri/src/state/db.rs:1145-1167`.
- The terminal invocation update is at `src-tauri/src/state/db.rs:1206-1227` and should stay inside the same transaction.
- The aggregate upsert to change is at `src-tauri/src/state/db.rs:1229-1244`. It currently inserts into `providers (model_name, provider_index, ...)` and conflicts on `(model_name, provider_index)`.
- The failure metadata update is also part of the writer hookpoint at `src-tauri/src/state/db.rs:1246-1257`. It currently filters `WHERE model_name = ?3 AND provider_index = ?4`; after the schema change it must filter by `(model_name, provider_name)`. This is adjacent to, but just outside, the narrow cited `1186-1244` upsert range.
- If the extended invocation row load returns `provider_name = NULL`, the proposal says finalization should skip aggregate writes. The invocation terminal update should still happen.

### Aggregate reader hookpoint

- `ProviderRecord` is defined at `src-tauri/src/state/db.rs:117-127`. The aggregate-specific identity field is currently `provider_index: usize` at `src-tauri/src/state/db.rs:121`; replace it with `provider_name: String`.
- `StateDb::get_provider` is at `src-tauri/src/state/db.rs:1495-1531`.
  - Signature today: `get_provider(&self, model_name: &str, provider_index: usize)`.
  - Query today: `src-tauri/src/state/db.rs:1503-1505`, filtering `FROM providers WHERE model_name = ?1 AND provider_index = ?2`.
  - Parameter binding today: `src-tauri/src/state/db.rs:1508`, passing `provider_index as i64`.
  - Record construction today: `src-tauri/src/state/db.rs:1509-1523`, setting `provider_index`.
- Change the signature to `get_provider(&self, model_name: &str, provider_name: &str)`, query by `(model_name, provider_name)`, and construct `ProviderRecord { provider_name, ... }`.
- All current direct call sites found by `rg "get_provider\\("`:
  - `src-tauri/src/balancer/mod.rs:600` in `score_by_invocation_count`; pass `&model.providers[i].name`.
  - `src-tauri/src/balancer/mod.rs:628` in `round_robin_fallback`; pass `&model.providers[i].name`.
  - `src-tauri/examples/quota_check.rs:123`; pass `&p.name`.
  - `src-tauri/src/state/db.rs:4260` state test `finalize_invocation_updates_provider_aggregate_stats`; update expected lookup key.
  - `src-tauri/src/state/db.rs:4770` state test `missing_provider_returns_none`; update expected lookup key.
- Reads of `ProviderRecord.provider_index`: none outside record construction. `rg "ProviderRecord|\\.provider_index|get_provider\\("` shows `provider_index` reads in balancer projections/resume/trace/invocation records, but not reads from `ProviderRecord` itself.

### `recent_error_count` hookpoint

- `StateDb::recent_error_count` is at `src-tauri/src/state/db.rs:1533-1553`.
  - Signature today: `recent_error_count(&self, model_name: &str, provider_index: usize, window_minutes: i64)`.
  - Query today: `src-tauri/src/state/db.rs:1544-1547`, filtering `model_name = ?1 AND provider_index = ?2 AND success = 0 AND created_at > ?3`.
- Change the signature to take `provider_name: &str` and filter `provider_name = ?2`.
- Current call sites:
  - Projection scoring: `src-tauri/src/balancer/mod.rs:260-262` inside `compute_projections_from_records`; pass `&model.providers[i].name`.
  - Fallback scoring: `src-tauri/src/balancer/mod.rs:590-592` inside `score_by_invocation_count`; pass `&model.providers[i].name`.
  - State test: `src-tauri/src/state/db.rs:4454` in `recent_errors`; update expected lookup key.
- No other call sites were found in `src-tauri/src`, `src-tauri/tests`, or `src-tauri/examples`.

### `examples/quota_check.rs` hookpoint

- `src-tauri/examples/quota_check.rs:116-128` prints balancer picks and per-provider diagnostics for providers with no quota windows.
- The old aggregate reader call is at `src-tauri/examples/quota_check.rs:123`: `let rec = db.get_provider(name, i).unwrap();`.
- The surrounding loop already has both the current slot index `i` and provider config `p` at `src-tauri/examples/quota_check.rs:119-120`; change the aggregate lookup to `db.get_provider(name, &p.name)`.
- Neighboring code only reads `rec.map(|r| r.invocation_count)` at `src-tauri/examples/quota_check.rs:124` and prints the current display index at `src-tauri/examples/quota_check.rs:126`. No old-signature dependency beyond the lookup argument.

## Reusable pieces

- `src-tauri/src/state/db.rs:723-781` — `ensure_invocations_schema` shows the local shape-inspection + conditional migration style and centralizes invocation schema readiness before dependent migrations.
- `src-tauri/src/state/db.rs:783-796` — `invocations_columns` is the existing `PRAGMA table_info(...)` column-inspection idiom; the new providers helper should add a `providers_columns` sibling rather than ad hoc inspection inline.
- `src-tauri/src/state/db.rs:829-842`, `src-tauri/src/state/db.rs:899-929` — `session_turns_columns`, `provider_quotas_columns`, and `provider_quota_windows_columns` use the same `PRAGMA table_info` row-map pattern.
- `src-tauri/src/state/db.rs:844-878` — `ensure_provider_quotas_schema` is the nearest ALTER-style helper. Match its error-message style and column-existence checks where possible.
- `src-tauri/src/state/db.rs:880-897` — `ensure_provider_quota_windows_schema` is the second adjacent schema helper with the same shape.
- `src-tauri/src/state/db.rs:985-1108` — `migrate_legacy_invocations` is the existing transactional schema-rebuild pattern: collect source rows, create a replacement table, copy, replace/drop, create indexes, and `commit`. There is no generic rebuild helper to call, but the new `ensure_providers_schema` should reuse this transaction pattern.
- `src-tauri/src/state/db.rs:3101-3103` — state tests already have `test_db()` for in-memory `StateDb`.
- `src-tauri/src/balancer/mod.rs:649-667` — balancer tests already have `record_invocation_for_test(...)`, which seeds invocation rows through the supported `start_invocation`/`finalize_invocation` path.
- `src-tauri/tests/rca_routing_claude_skipped.rs:20-31` — the RCA harness has a small supported-path seeding helper that records provider name and index combinations.
- `src-tauri/src/state/db.rs:3267-3301` — `legacy_invocations_db(...)` manually creates old-shaped invocation fixtures for migration tests; use the same on-disk temp DB style for the old-shaped `providers` migration fixtures.

## Conflicting / parallel systems

- `src-tauri/src/state/db.rs:508-518` — old inline providers DDL conflicts with the proposed helper. Delete/replace it with `ensure_providers_schema`; do not leave both.
- `src-tauri/src/state/db.rs:1229-1244` and `src-tauri/src/state/db.rs:1246-1257` — current aggregate writer is the only writer to `providers`. Merge the fix here; do not create a second name-keyed aggregate table or shadow writer.
- `src-tauri/src/state/db.rs:1495-1531` — current `get_provider` is the only aggregate reader API. Change it in place; do not keep an index-keyed alias, per `WS-2`.
- `src-tauri/src/balancer/mod.rs:599-607` and `src-tauri/src/balancer/mod.rs:626-639` — the only production fallback aggregate readers are in `score_by_invocation_count` and `round_robin_fallback`. Update these call sites; no alternative aggregate query should remain.
- `src-tauri/src/state/db.rs:1533-1553`, with callers at `src-tauri/src/balancer/mod.rs:260-262` and `src-tauri/src/balancer/mod.rs:590-592` — this is the parallel identity-drift system over `invocations`. Merge the identity change here rather than adding a separate recent-error helper.
- `src-tauri/examples/quota_check.rs:123` — developer example has the old reader signature. Update it in place so the workspace keeps building.
- `src-tauri/src/state/db.rs:1111-1138` — `provider_name_lookup` still derives names from `(model_name, provider_index)`, but only for legacy `invocations` migration. This is not a fallback-history reader and should not be deleted as part of this fix.
- `src-tauri/src/balancer/mod.rs:31`, `src-tauri/src/balancer/mod.rs:182`, `src-tauri/src/balancer/mod.rs:379-394`, and related `ProviderProjection.provider_index` uses remain current-slot selection metadata, not historical identity. They are not deletion candidates.
- `rg "#\\[ignore\\]" src-tauri/src src-tauri/tests` found no ignored tests on this routing surface. The only `#[ignore]` hits are older initiative-07 risk/proposal documents outside the product source and test files.

## Deletion candidates

- Old `providers.provider_index` column and primary key `(model_name, provider_index)`.
- Old inline providers DDL block at `src-tauri/src/state/db.rs:508-518`.
- Old `get_provider(&str, usize)` signature and all index-keyed call sites.
- Old `recent_error_count(&str, usize, ...)` signature and all index-keyed call sites.
- `ProviderRecord.provider_index` after the aggregate reader becomes provider-name keyed.
- The one-time `providers_legacy_index_keyed` table after migration commits.
- Any future or accidental index-keyed aggregate alias, shim, or fallback reader. None exists today beyond the current `get_provider`.

## Problem-map / assumption re-validation

- `src-tauri/src/state/db.rs:508-518` — upheld. Current `providers` DDL is still index-keyed and has no `provider_name`.
- `src-tauri/src/state/db.rs:1186-1244` — upheld for the row load and aggregate upsert. Add implementation awareness that last-error metadata at `src-tauri/src/state/db.rs:1246-1257` must also switch to provider-name filtering.
- `src-tauri/src/state/db.rs:1495-1531` — upheld. `get_provider` still reads by `(model_name, provider_index)`.
- `src-tauri/src/state/db.rs:1533-1553` — upheld. `recent_error_count` still filters `invocations` by `(model_name, provider_index)`.
- `src-tauri/src/balancer/mod.rs:586-639` — upheld. Fallback scoring and round-robin fallback still call `recent_error_count`/`get_provider` with indexes.
- `src-tauri/src/balancer/mod.rs:240-282` — upheld. Projection scoring still calls `recent_error_count(&model.name, i, ...)` while adjacent quota/session-turn reads use provider names.
- `src-tauri/src/state/db.rs:723-930` — upheld. Existing ALTER-style helpers and PRAGMA table-info helpers live here.
- `src-tauri/src/state/db.rs:496-518` — upheld. `StateDb::open` owns schema setup and currently creates old-shaped `providers` inline after `ensure_invocations_schema`.
- A1 — upheld. Current runtime writes provider name and index together at `src-tauri/src/state/db.rs:1145-1167`; quotas/session turns remain provider-name keyed.
- A2 — upheld. `provider_index` remains current selection/observability metadata in balancer, executor, trace, and invocation records; the aggregate remains the inconsistent identity surface.
- A3 — upheld. `invocations` has `model_name`, nullable `provider_name`, `provider_index`, `status`, `success`, `error_category`, `created_at`, and `finished_at` at `src-tauri/src/state/db.rs:931-959`, enough for the proposed aggregate rebuild over terminal rows.
- A4 — upheld. The only observed `last_error` aggregate consumer is the state test at `src-tauri/src/state/db.rs:4263-4266`; no UI/IPC/direct product reader of `providers.last_error` was found.
- A5 — upheld. No `PRAGMA user_version` write path was found in `src-tauri/src/state/db.rs`; adjacent helpers use shape inspection.
- A6 — upheld from worktree evidence. `rg` found no IPC/UI/schema-probe consumer or alternative aggregate query exposing old `providers` table shape; direct reads are limited to `get_provider`, balancer call sites, one example, and state tests.
- Phase 4 watch signals — upheld. `WS-1` maps to the new helper's transaction + rejection behavior; `WS-2` requires changing `get_provider` in place with no alias; `WS-3` remains a code-review residual for the migration transaction wrapper, not a runtime-test claim.

## Open questions

- None requiring user input before implementation.
