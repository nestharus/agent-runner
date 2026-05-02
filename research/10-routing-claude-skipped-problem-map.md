# Problem Map — routing-claude-skipped

## Touched surface

- `src-tauri/src/state/db.rs:508-518` — Creates the `providers` aggregate table with `PRIMARY KEY (model_name, provider_index)` and no `provider_name` column.
- `src-tauri/src/state/db.rs:931-959` — Creates `invocations` with both `provider_name` and `provider_index`; indexes are provider-name oriented for invocation lookup, but the aggregate table is not.
- `src-tauri/src/state/db.rs:985-1139` — Legacy `invocations` migration rebuilds rows and resolves `provider_name` from current `(model_name, provider_index)` config; unresolved rows become `status='legacy'`.
- `src-tauri/src/state/db.rs:1141-1169` — `start_invocation` persists both runtime `provider_name` and selected `provider_index` on the invocation row.
- `src-tauri/src/state/db.rs:1172-1261` — `finalize_invocation` runs in one transaction, loads only `model_name`, `provider_index`, and `status`, updates the invocation, then upserts the aggregate by `(model_name, provider_index)`.
- `src-tauri/src/state/db.rs:1495-1531` — `get_provider` reads aggregate counts by `(model_name, provider_index)`.
- `src-tauri/src/state/db.rs:1533-1553` — `recent_error_count` reads recent failures from `invocations` by `(model_name, provider_index)`, not provider identity.
- `src-tauri/src/state/db.rs:1557-1810` and `src-tauri/src/state/db.rs:1891-1901` — Quota reads/writes use `provider_name` as the storage key for quota metadata, windows, exhaustion, refreshes, and call ticks.
- `src-tauri/src/balancer/mod.rs:88-167` — `select_provider` filters exhausted providers by name-keyed quota rows, uses density scoring when all candidates have quota windows, and otherwise falls back to invocation-count scoring.
- `src-tauri/src/balancer/mod.rs:240-333` — Density projections use `recent_error_count(&model.name, index, ...)` plus name-keyed quota/session-turn reads.
- `src-tauri/src/balancer/mod.rs:586-639` — Fallback scoring and round-robin fallback both call `get_provider(&model.name, index)` and compare aggregate `invocation_count` by current index.
- `src-tauri/src/main.rs:1967-2112` — Normal `agents` CLI execution opens state, calls `select_provider`, resolves the effective runtime provider, starts an invocation with provider name/index, executes, records quota exhaustion by provider name, finalizes, and increments quota calls by provider name.
- `src-tauri/src/main.rs:1511-1755` and `src-tauri/src/main.rs:1758-1965` — Interactive and non-interactive resume paths start/finalize invocation rows for the resolved provider and call resume migration scoring before execution.
- `src-tauri/src/main.rs:1223-1270` — Resume execution resolves provider index from active provider name: model resumes use current model order; provider-default resumes use sorted `providers.toml` keys.
- `src-tauri/src/config/providers.rs:116-127` and `src-tauri/src/config/providers.rs:157-190` — Runtime provider resolution preserves the model provider key as `ProviderConfig.name` even when command/args are merged from `providers.toml`.
- `src-tauri/src/lib.rs:53-77` — `derive_pools` groups pools by sorted/deduped current provider names.

## Adjacent surfaces in blast radius

- Aggregate readers in `balancer/mod.rs` — `score_by_invocation_count` and `round_robin_fallback` are the only production callers of `get_provider`; any aggregate key shift changes fallback selection semantics at `src-tauri/src/balancer/mod.rs:599-607` and `src-tauri/src/balancer/mod.rs:626-639`.
- Recent-error penalty — `recent_error_count` is not an aggregate-table reader, but it has the same index-keyed identity risk and can mark a current provider unhealthy from older occupant rows at `src-tauri/src/state/db.rs:1541-1548`.
- Quota filter path — `select_provider` excludes candidates with `provider_quotas.exhausted_at` by current provider name at `src-tauri/src/balancer/mod.rs:116-138`; this is already identity-keyed and asymmetric with fallback aggregates.
- Density scoring inputs — projections combine name-keyed quotas/session turns with index-keyed recent errors at `src-tauri/src/balancer/mod.rs:260-282`, so behavior can shift if recent-error identity semantics change.
- Resume migration scoring — `decide_migration` checks active exhaustion by provider name, then uses projections that include index-keyed recent errors at `src-tauri/src/balancer/mod.rs:335-397`.
- UI test-model command — `test_model_with_db_path` calls `select_provider` with no quota-refresh context and can therefore exercise aggregate fallback selection, but it does not call `start_invocation`/`finalize_invocation` or update the aggregate at `src-tauri/src/lib.rs:495-505`.
- Pools IPC/UI — `list_pools` exposes provider-name groupings only at `src-tauri/src/lib.rs:285-288`; the TypeScript `PoolSummary` has `commands`, `model_count`, and `model_names` only at `src/lib/types.ts:181-185`, and `PoolCard` renders model counts, not usage counts, at `src/components/PoolCard.tsx:166-180`.
- Quota refresh IPC/UI — `refresh_quotas` refreshes distinct provider names from multi-provider models at `src-tauri/src/lib.rs:305-355`; aggregate key changes should not alter its storage key.
- Schema probe — compatibility reporting inspects session/invocation structures but does not list the `providers` aggregate table as required at `src-tauri/src/schema_probe/mod.rs:208-280`.
- State migration bootstrap — `StateDb::open` owns schema creation/migration for `providers`, `provider_quotas`, and `provider_quota_windows`; there are no other files under `src-tauri/src/state/`.

## Supported / user-reachable paths today

- Normal CLI run: user invokes `agents -m <model>`; `run_with_balancing` calls `select_provider`, resolves the selected runtime provider, writes an invocation row via `start_invocation`, executes the wrapped CLI, optionally marks quota exhaustion by provider name, finalizes the invocation, and upserts aggregate counts by index at `src-tauri/src/main.rs:1999-2075`.
- Fallback selection inside that run: when at least one non-exhausted provider lacks quota windows, `select_provider` enters `score_by_invocation_count` at `src-tauri/src/balancer/mod.rs:160-167`, then reads aggregate counts by current provider index at `src-tauri/src/balancer/mod.rs:599-607`.
- Interactive resume: `run_repl` resolves the active provider, may call `decide_migration`, then starts and finalizes an invocation row for the resolved provider at `src-tauri/src/main.rs:1591-1715`.
- Non-interactive `agents resume`: `run_resume` resolves/migrates the active provider, starts an invocation, executes resume, and finalizes the invocation at `src-tauri/src/main.rs:1818-1937`.
- `agents migrate-config`: this is user-reachable but does not touch `StateDb` or aggregates; it rewrites model/provider TOML files and derives provider names from config at `src-tauri/src/main.rs:2174-2298` and `src-tauri/src/main.rs:2322-2478`.
- Pools UI: `PoolsView` calls `listPools` through `src/lib/tauri.ts:51-52` and renders provider-name pool groupings from `src-tauri/src/lib.rs:53-77`; no current UI path displays provider aggregate invocation/error counts.
- UI model test: `test_model` can select a provider through the same balancer path at `src-tauri/src/lib.rs:495-505`; it surfaces command success/stderr and can mark quota exhaustion, but it does not persist invocation aggregate usage.

## Known risky / brittle behavior present today

- Aggregate identity is order-dependent: the `providers` table is keyed only by `(model_name, provider_index)` at `src-tauri/src/state/db.rs:508-518`, so reorder, insertion, removal, or replacement can silently assign old history to the current occupant of an index.
- Aggregate writes ignore persisted provider identity: `finalize_invocation` reads only `model_name`, `provider_index`, and `status` before the upsert at `src-tauri/src/state/db.rs:1186-1244`, even though the invocation row stores `provider_name`.
- Fallback readers inherit stale index history: both invocation-count fallback and round-robin fallback read aggregate counts with `get_provider(&model.name, index)` at `src-tauri/src/balancer/mod.rs:599-607` and `src-tauri/src/balancer/mod.rs:626-639`.
- Recent-error suppression can drift the same way: `recent_error_count` filters `invocations` by `(model_name, provider_index)` at `src-tauri/src/state/db.rs:1541-1548`, so failures from a prior provider at that index can penalize a current provider.
- The system already stores stronger identity on invocation rows: `start_invocation` writes `provider_name` and `provider_index` together at `src-tauri/src/state/db.rs:1145-1167`, while `get_invocation_by_uuid` reads both back at `src-tauri/src/state/db.rs:1393-1412`.
- Quota behavior is identity-keyed today: exhausted filtering, quota windows, quota refreshes, and call ticks use provider names at `src-tauri/src/balancer/mod.rs:116-138`, `src-tauri/src/state/db.rs:1557-1810`, and `src-tauri/src/state/db.rs:1891-1901`; this makes aggregate fallback behavior inconsistent with quota behavior.
- Transactionality exists for finalization plus aggregate update: `finalize_invocation` wraps invocation terminal update, aggregate upsert, last-error update, and commit in one transaction at `src-tauri/src/state/db.rs:1180-1261`.
- Aggregate drift can still exist outside transaction boundaries: legacy rows can be migrated to invocations with resolved or null `provider_name`, but the existing aggregate table has no provider-name field to audit or reconcile against at `src-tauri/src/state/db.rs:985-1139`.

## Migration / observability story today

- Schema creation for `providers` is inline in `StateDb::open` after invocation schema setup at `src-tauri/src/state/db.rs:496-518`; there is no helper equivalent to `ensure_provider_quotas_schema` for evolving the aggregate table.
- Existing ALTER-style helpers cover `invocations`, `provider_quotas`, `provider_quota_windows`, and `session_turns` at `src-tauri/src/state/db.rs:723-930`; none alters the `providers` aggregate table.
- `provider_quotas` and `provider_quota_windows` have current ALTER handling for added/dropped columns at `src-tauri/src/state/db.rs:844-897`, showing the local migration style available adjacent to the aggregate table.
- `migrate_legacy_invocations` rebuilds only `invocations` and backfills provider names from current model config; it does not rebuild or validate `providers` aggregates at `src-tauri/src/state/db.rs:985-1139`.
- `agents migrate-db` opens `StateDb`, then runs session-chain and compaction backfills at `src-tauri/src/main.rs:2152-2164` and `src-tauri/src/main.rs:2611-2645`; no aggregate-specific migration or report is present.
- `session schema-probe` reads the DB read-only and reports required tables/columns/indexes at `src-tauri/src/schema_probe/mod.rs:61-75` and `src-tauri/src/schema_probe/mod.rs:95-151`; `providers` is not in its required table or column set at `src-tauri/src/schema_probe/mod.rs:208-280`.
- `CURRENT_SCHEMA_VERSION` and `MINIMUM_SUPPORTED_SCHEMA_VERSION` are both `3` in `src-tauri/src/schema_probe/mod.rs:7-8`; `rg` found no `PRAGMA user_version` write path in `src-tauri/src/state/db.rs`.
- Current observability for this issue is indirect: invocation rows expose `provider_name`/`provider_index`, quota rows expose provider-name state, and aggregate rows expose only `model_name`/`provider_index` counts.

## Open questions

- No current code path shows provider aggregate counts in the SolidJS UI; if a hidden or external consumer reads the SQLite `providers` table directly, it is not discoverable from this worktree.
- The live user DB shape is not available here, so the exact existing mismatch rows and aggregate contents cannot be confirmed from code alone.
