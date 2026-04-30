# 1. Scope statement

Initiative 04 deletes the Initiative 03 threshold/risk-class gating surface and replaces it with per-account reactive exhausted routing: keep projection, per-window learning, bootstrap, refresh, and recent-error avoidance; delete `RiskClass`, thresholds, `Selection`, `BalanceError`, `quota_tight_routing`, and the Rust-only structured test-model preflight error; add `provider_quotas.exhausted_at` as the sticky exhausted flag that only successful non-empty refresh clears. This ships as one PR because the locked answers make the pieces mutually dependent: the invocation schema drop and `Selection -> usize` revert touch the same call sites, risk-class deletion cascades through CLI/REPL/Tauri in one API change, and the exhausted flag is only meaningful when the write path, refresh clear path, and `select_provider` filter land together; splitting would create dead intermediate plumbing explicitly rejected by answers D1-D5 (`research/04-reactive-routing-answers.md:91-124`).

# 2. Schema migration

The repo uses schema-ensure migrations in `StateDb::open`, not numbered migration files. `ensure_invocations_schema`, `ensure_provider_quotas_schema`, and the related column-inspection helpers are the migration hookpoints (`src-tauri/src/state/db.rs:522-558`, `src-tauri/src/state/db.rs:611-638`, `src-tauri/src/state/db.rs:659-672`). Match the PR #6 `last_empty_refresh_at` style: inspect `PRAGMA table_info`, then run an `ALTER TABLE` only when the column is absent (`src-tauri/src/state/db.rs:611-622`).

Migration chunk, idempotent by schema-ensure checks:

```sql
ALTER TABLE provider_quotas ADD COLUMN exhausted_at TEXT NULL;

ALTER TABLE invocations DROP COLUMN quota_tight_routing;
```

The `DROP COLUMN` is supported by the bundled SQLite in this repo: proposal 03 records that `rusqlite` uses bundled `libsqlite3-sys 0.36.0`, the bundled header is SQLite 3.51.1, and SQLite 3.51.1 supports `ALTER TABLE DROP COLUMN` (`proposals/03-load-balancing-tiers.md:123-125`). The two statements are dependency-free; current code may execute the invocations ensure before provider quota ensure or vice versa, and the result is safe either way.

Update fresh `provider_quotas` schema at the current declaration (`src-tauri/src/state/db.rs:389-396`):

```sql
CREATE TABLE IF NOT EXISTS provider_quotas (
    provider_name TEXT PRIMARY KEY,
    used_percent REAL NOT NULL DEFAULT 0,
    resets_at TEXT,
    calls_since_refresh INTEGER NOT NULL DEFAULT 0,
    refreshed_at TEXT,
    last_empty_refresh_at TEXT,
    exhausted_at TEXT NULL
);
```

Update fresh `invocations` schema at `invocations_schema_sql` to remove the column (`src-tauri/src/state/db.rs:691-718`):

```sql
CREATE TABLE IF NOT EXISTS invocations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    invocation_uuid TEXT NOT NULL UNIQUE,
    model_name TEXT NOT NULL,
    provider_name TEXT,
    provider_index INTEGER NOT NULL,
    parent_invocation_id INTEGER REFERENCES invocations(id),
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'legacy')),
    success INTEGER,
    exit_code INTEGER,
    error_category TEXT,
    session_id TEXT,
    session_capture_method TEXT,
    created_at TEXT NOT NULL,
    finished_at TEXT
);
```

Update the legacy-invocation rebuild migration to stop creating, inserting, or defaulting `quota_tight_routing`. The rebuild currently creates `invocations_new` with the column and inserts a literal `0` for migrated rows (`src-tauri/src/state/db.rs:786-825`); remove that column from the `CREATE TABLE`, insert column list, and values list.

# 3. Delete list

## 3.1 `RiskClass`

Files touched: `src-tauri/src/balancer/mod.rs`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/examples/quota_check.rs`, `README.md`.

Delete the `RiskClass` enum from the balancer module (`src-tauri/src/balancer/mod.rs:12-17`). Revert `select_provider` from `select_provider(model, state, ctx, risk_class) -> Result<Selection, BalanceError>` to `select_provider(model, state, ctx) -> usize` (`src-tauri/src/balancer/mod.rs:79-84`). Remove `risk_class` from `score_by_density`, delete the user/background branch, and delete exhausted-error construction that carries `risk_class` (`src-tauri/src/balancer/mod.rs:136-142`, `src-tauri/src/balancer/mod.rs:225-246`, `src-tauri/src/balancer/mod.rs:272-289`).

In CLI code, remove the import, `RiskClassArg`, `resolve_risk_class`, the `--risk-class` field, the `OULIPOLY_RISK_CLASS` read, and all call-site arguments (`src-tauri/src/main.rs:1-10`, `src-tauri/src/main.rs:62-82`, `src-tauri/src/main.rs:212-244`, `src-tauri/src/main.rs:282-320`, `src-tauri/src/main.rs:340-347`, `src-tauri/src/main.rs:488-494`, `src-tauri/src/main.rs:670-677`). In Tauri `test_model`, remove the hardcoded `RiskClass::User` selection call and any serialized risk class field (`src-tauri/src/lib.rs:35-42`, `src-tauri/src/lib.rs:519-536`). In `quota_check`, drop the `RiskClass::Background` import/argument (`src-tauri/examples/quota_check.rs:10-12`, `src-tauri/examples/quota_check.rs:116-128`). Remove README CLI and load-balancing risk-class prose (`README.md:117-130`, `README.md:217-234`).

Tests deleted: all main risk cascade tests listed in section 8; any balancer tests that only exist to distinguish `RiskClass::User` from `RiskClass::Background`.

## 3.2 `Selection`

Files touched: `src-tauri/src/balancer/mod.rs`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/examples/quota_check.rs`.

Delete `Selection { provider_index, quota_tight_routing }` (`src-tauri/src/balancer/mod.rs:19-23`). Make all `select_provider` branches return `usize` directly instead of `Result<Selection, BalanceError>` (`src-tauri/src/balancer/mod.rs:79-134`, `src-tauri/src/balancer/mod.rs:218-251`). Update callers to use the returned index: `run_repl` currently reads `selection.provider_index` and `selection.quota_tight_routing` (`src-tauri/src/main.rs:586-605`), `run_with_balancing` does the same (`src-tauri/src/main.rs:704-727`), Tauri `test_model_with_db_path` reads `selection.provider_index` (`src-tauri/src/lib.rs:519-545`), and `quota_check` prints `selection.quota_tight_routing` (`src-tauri/examples/quota_check.rs:116-128`).

Tests deleted: all `Selection.quota_tight_routing` assertions, including `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail` and the quota-tight assertion in `fresh_pool_falls_through_to_invocation_count_round_robin` (`src-tauri/src/balancer/mod.rs:747-758`, `src-tauri/src/balancer/mod.rs:881-890`).

## 3.3 `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo`

Files touched: `src-tauri/src/balancer/mod.rs`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`.

Delete `BalanceError`, `ExhaustedError`, `ExhaustedProviderInfo`, their `Display`/`Error` impls, and `exhausted_error` (`src-tauri/src/balancer/mod.rs:25-55`, `src-tauri/src/balancer/mod.rs:272-289`). Remove the hard-error branch from density scoring; all-exhausted-by-flag falls through to invocation-count selection per answers Q4 rather than returning a balancer error (`src-tauri/src/balancer/mod.rs:213-228`). Remove `emit_balance_error` and both `Err(BalanceError::Exhausted(_))` caller branches (`src-tauri/src/main.rs:586-596`, `src-tauri/src/main.rs:704-710`, `src-tauri/src/main.rs:810-816`). Remove the Tauri preflight error mapping from `test_model_with_db_path` (`src-tauri/src/lib.rs:519-568`).

Tests deleted: `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail` and `test_model_returns_structured_quota_exhausted_error` (`src-tauri/src/balancer/mod.rs:775-797`, `src-tauri/src/lib.rs:902-942`).

## 3.4 `BalancerConfig`, thresholds, and `[balancer]`

Files touched: `src-tauri/src/config/model.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/balancer/mod.rs`, `README.md`.

Remove `ModelConfig.balancer`, `BalancerConfig`, `RawBalancerBlock`, `parse_balancer`, `append_balancer_toml`, and threshold validation (`src-tauri/src/config/model.rs:203-251`, `src-tauri/src/config/model.rs:316-333`, `src-tauri/src/config/model.rs:648-673`). Remove parsing and construction of `balancer` in `ModelConfig::from_toml`, and remove serialization from `to_toml` (`src-tauri/src/config/model.rs:491-491`, `src-tauri/src/config/model.rs:572-572`, `src-tauri/src/config/model.rs:638-644`). Remove `model.balancer.validate()` from `save_model` (`src-tauri/src/lib.rs:266-280`). Remove threshold reads from density scoring and exhausted provider info (`src-tauri/src/balancer/mod.rs:184-191`, `src-tauri/src/balancer/mod.rs:282-287`). Remove README `[balancer]` documentation (`README.md:217-234`).

Fields removed from structs: `ModelConfig::balancer`, `BalancerConfig::{user_threshold, failure_threshold}`, `RawModelToml::balancer`, `RawBalancerBlock::{user_threshold, failure_threshold}`.

Tests deleted: `parse_balancer_defaults_when_block_absent`, `parse_balancer_overrides_thresholds`, `rejects_balancer_threshold_outside_unit_interval`, `rejects_balancer_user_threshold_above_failure_threshold`, and `roundtrip_model_with_balancer_config` (`src-tauri/src/config/model.rs:1180-1278`).

## 3.5 `--risk-class`, `OULIPOLY_RISK_CLASS`, `resolve_risk_class`, cascade branches

Files touched: `src-tauri/src/main.rs`, `README.md`.

Remove the global CLI flag, `RiskClassArg`, conversion impl, `resolve_risk_class`, and `with_risk_envs` test helper (`src-tauri/src/main.rs:62-82`, `src-tauri/src/main.rs:212-244`, `src-tauri/src/main.rs:906-953`). Remove `cli.risk_class.map(Into::into)` from the `repl` call and stop resolving `risk_class` before direct-model/agent execution (`src-tauri/src/main.rs:282-320`, `src-tauri/src/main.rs:340-347`). Remove README references to the flag, env var, REPL override, and heuristic default (`README.md:117-130`, `README.md:226-234`).

Tests deleted: `risk_class_cli_flag_overrides_env_var`, `risk_class_env_var_overrides_heuristic`, `risk_class_heuristic_classifies_file_flag_as_background`, `risk_class_heuristic_classifies_tty_prompt_as_user`, `risk_class_heuristic_classifies_parent_invocation_as_background`, `risk_class_heuristic_classifies_piped_stdin_as_background`, `repl_subcommand_always_user_class`, and `risk_class_flag_reaches_repl_subcommand` (`src-tauri/src/main.rs:1198-1325`).

## 3.6 `quota_tight_routing`

Files touched: `src-tauri/src/state/db.rs`, `src-tauri/src/main.rs`, `src-tauri/src/balancer/mod.rs`, `src-tauri/tests/pr_b_trace_integration.rs`, `src-tauri/examples/quota_check.rs`, `README.md`.

Remove `quota_tight_routing` from `InvocationRecord` and `InvocationStart` (`src-tauri/src/state/db.rs:138-166`). Delete the schema add-column branch, remove the column from fresh `invocations` schema, and remove it from legacy rebuild create/insert SQL (`src-tauri/src/state/db.rs:545-550`, `src-tauri/src/state/db.rs:691-718`, `src-tauri/src/state/db.rs:786-825`). Remove the insert parameter in `start_invocation` and remove query selection/mapping in lookup methods (`src-tauri/src/state/db.rs:898-929`, `src-tauri/src/state/db.rs:1051-1147`). Remove warning emission and `InvocationStart` literals in `run_repl`/`run_with_balancing` (`src-tauri/src/main.rs:522-605`, `src-tauri/src/main.rs:619-626`, `src-tauri/src/main.rs:704-727`). Remove helper/test fixture fields in balancer, state, main, and integration tests (`src-tauri/src/balancer/mod.rs:483-501`, `src-tauri/src/state/db.rs:2157-2172`, `src-tauri/src/main.rs:1348-1364`, `src-tauri/src/main.rs:1441-1525`, `src-tauri/tests/pr_b_trace_integration.rs:73-99`). Remove `quota_tight` output from `quota_check` (`src-tauri/examples/quota_check.rs:123-128`). Remove README persisted-flag/warning prose (`README.md:221-226`).

Fields removed from structs: `InvocationRecord::quota_tight_routing`, `InvocationStart::quota_tight_routing`, and `Selection::quota_tight_routing`.

Tests deleted: `quota_tight_routing_column_persisted_to_invocations` (`src-tauri/src/state/db.rs:2929-2951`) plus all selection tests whose only assertion is the tight-routing flag.

## 3.7 `TestModelResult.error`, `TestModelError`, `TestModelProviderInfo`

Files touched: `src-tauri/src/lib.rs`; frontend type remains already shaped correctly.

Revert Rust `TestModelResult` to `{ success, stdout, stderr, exit_code }` by deleting `error: Option<TestModelError>`, `TestModelError`, and `TestModelProviderInfo` (`src-tauri/src/lib.rs:25-50`). Delete `test_model_error_from_exhausted` and the preflight exhausted branch in `test_model_with_db_path` (`src-tauri/src/lib.rs:519-568`). The frontend already has the reverted shape and `testModel` already uses it (`src/lib/types.ts:109-114`, `src/lib/tauri.ts:1-11`, `src/lib/tauri.ts:73-75`), so no frontend compatibility shim is added.

Tests deleted: `test_model_returns_structured_quota_exhausted_error` (`src-tauri/src/lib.rs:902-942`).

## 3.8 `ProviderEval.hard_blocked` / `user_blocked`, eligibility filters, soft-degrade branch

Files touched: `src-tauri/src/balancer/mod.rs`, `src-tauri/src/main.rs`.

Remove `hard_blocked` and `user_blocked` from `ProviderEval` (`src-tauri/src/balancer/mod.rs:57-65`). Keep projection and scoring, but stop comparing projected usage to `model.balancer.failure_threshold` and `model.balancer.user_threshold` (`src-tauri/src/balancer/mod.rs:162-196`). Remove `hard_eligible`, `user_eligible`, all-threshold-exhausted error construction, and the user soft-degrade branch (`src-tauri/src/balancer/mod.rs:213-246`). Keep recent-error avoidance as an eligibility/deprioritization path (`src-tauri/src/balancer/mod.rs:146-160`, `src-tauri/src/balancer/mod.rs:422-450`). Remove quota-tight warnings in main (`src-tauri/src/main.rs:598-600`, `src-tauri/src/main.rs:711-713`).

Tests deleted: `user_threshold_hides_provider_from_user_class_only`, `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`, `failure_threshold_hard_blocks_all_classes`, and `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail` (`src-tauri/src/balancer/mod.rs:732-797`).

## 3.9 Tests pinning delete behavior

Files touched: `src-tauri/src/balancer/mod.rs`, `src-tauri/src/main.rs`, `src-tauri/src/config/model.rs`, `src-tauri/src/state/db.rs`, `src-tauri/src/lib.rs`.

Delete, not rewrite, tests whose purpose is to pin removed behavior: the 70%/95% threshold tests, all `RiskClass` cascade tests, all `Selection.quota_tight_routing` assertions, the direct quota-tight DB persistence test, and the structured Tauri preflight error test (`src-tauri/src/balancer/mod.rs:732-797`, `src-tauri/src/main.rs:1198-1325`, `src-tauri/src/config/model.rs:1180-1278`, `src-tauri/src/state/db.rs:2929-2951`, `src-tauri/src/lib.rs:902-942`). Existing behavioral tests that still cover kept behavior should be mechanically updated to the reverted `select_provider -> usize` API instead of deleted.

# 4. Keep list - unchanged confirmation

Projection remains the ranking signal: `score_by_density` still computes projected window usage and binding score from remaining headroom times hours to reset (`src-tauri/src/balancer/mod.rs:162-196`). The bootstrap cascade remains learned window rate, sibling pool average by `window_id`, then duration-ratio fallback (`src-tauri/src/balancer/mod.rs:296-377`). Per-window delta learning and its guards remain in `upsert_quota_refresh` (`src-tauri/src/state/db.rs:8-45`, `src-tauri/src/state/db.rs:1282-1437`). Fully unlearned pools still reach round-robin (`src-tauri/src/balancer/mod.rs:213-224`, `src-tauri/src/balancer/mod.rs:455-474`, `src-tauri/src/balancer/mod.rs:881-890`). Recent-error avoidance remains in density scoring and invocation-count fallback, backed by `recent_error_count` (`src-tauri/src/balancer/mod.rs:8-9`, `src-tauri/src/balancer/mod.rs:146-160`, `src-tauri/src/balancer/mod.rs:422-450`, `src-tauri/src/state/db.rs:1188-1208`).

# 5. New: exhausted flag write path

Add `pub fn classify_exhaustion(stderr: &str) -> bool` in `src-tauri/src/diagnostics/mod.rs`. Extract the current quota heuristic exactly: lowercase stderr and match `"quota"`, `"billing"`, or `"usage limit"` (`src-tauri/src/diagnostics/mod.rs:102-132`). Keep `diagnose_error` and `parse_diagnosis` behavior intact; the helper is pure and heuristic-only (`src-tauri/src/diagnostics/mod.rs:37-100`).

Extend `QuotaRecord` with `exhausted_at: Option<DateTime<Utc>>` and update `get_quota` to select/map it from `provider_quotas` (`src-tauri/src/state/db.rs:63-73`, `src-tauri/src/state/db.rs:1212-1239`). Add:

```rust
pub fn mark_exhausted(&self, provider_name: &str) -> Result<(), String>
```

It writes one statement using `Utc::now().to_rfc3339()`:

```sql
UPDATE provider_quotas
SET exhausted_at = ?2
WHERE provider_name = ?1;
```

No insert, no retry, no error on zero affected rows; a missing quota row is a no-op by design.

Call sites:

- `run_with_balancing`: after `run_diagnostics` and before/alongside finalization, if `error_category == Some("quota_exhausted")`, call `state.mark_exhausted(provider_name)`. This path already has stderr, provider name, and the diagnostics category after subprocess failure (`src-tauri/src/main.rs:763-779`, `src-tauri/src/main.rs:781-794`).
- `run_repl`: **not implemented in this PR** per answers §D6. `execute_interactive` inherits stderr for TTY passthrough (`src-tauri/src/executor/cli.rs:344-404`) and returns only `i32` on child exit; capturing stderr without breaking terminal forwarding is non-trivial (tee via os_pipe + forwarding thread, ringbuffer, or ptty wrapping — all medium-cost). Decision: accept that a REPL quota-exit does NOT set the flag. Consequence: the next balancer-routed invocation to the same account runs diagnostics, classifies, flags — one guaranteed extra failure per REPL quota-exit before the flag is set. This is acceptable behavior per the locked "no spam" invariant (which governs post-classification stickiness, not signal acquisition). Future work can add REPL stderr capture if the one-extra-failure becomes painful in practice.
- Tauri `test_model_with_db_path`: after `executor::execute`, if the test subprocess exits nonzero and `classify_exhaustion(&result.stderr)` is true, call `db.mark_exhausted(&model.providers[provider_index].name)` before returning the ordinary `TestModelResult` (`src-tauri/src/lib.rs:519-545`).

Each `mark_exhausted` call is its own single-statement write; it is intentionally outside `finalize_invocation`'s transaction, which updates invocation/provider aggregate state (`src-tauri/src/state/db.rs:931-1021`).

# 6. New: exhausted flag clear path

In `StateDb::upsert_quota_refresh`, add the clear only in the non-empty branch. The empty branch preserves existing windows and writes only `last_empty_refresh_at`, so it must not clear exhausted state (`src-tauri/src/state/db.rs:1303-1348`). In the non-empty branch, within the existing transaction and alongside the quota metadata/window replacement writes, add:

```sql
UPDATE provider_quotas
SET exhausted_at = NULL
WHERE provider_name = ?1;
```

The non-empty branch currently upserts provider quota metadata, deletes old windows, inserts replacement windows, and commits one transaction (`src-tauri/src/state/db.rs:1350-1440`). The clear belongs in that same transaction so a concurrent reader sees either old quota plus old exhausted flag or new quota plus cleared flag.

# 7. New: balancer filter

`select_provider` already refreshes stale providers before loading cached quotas and windows (`src-tauri/src/balancer/mod.rs:93-121`). After those `state.get_quota(...)` reads, build the candidate provider index list by excluding providers whose `quota.exhausted_at.is_some()`. Providers with no quota row are not excluded. If the filtered list is empty, use the unfiltered provider list so the balancer always returns a provider, matching answers Q4.

Apply the candidate list to both scoring paths:

- Density scoring: evaluate only candidate indices, preserve the projection formula and binding-score ranking, and remove threshold gating (`src-tauri/src/balancer/mod.rs:136-252`).
- Invocation-count fallback: change `score_by_invocation_count` to score candidate indices, with the same empty-filter fallback to all indices; keep recent-error penalty and all-error round-robin behavior (`src-tauri/src/balancer/mod.rs:422-453`).

Do not cache exhausted state across calls. The filter reads `provider_quotas.exhausted_at` fresh through `get_quota` every `select_provider` call (`src-tauri/src/state/db.rs:1212-1239`). Because refresh happens before the quota read, a successful refresh in `select_provider`'s own loop can clear `exhausted_at`, and the same call will see the provider as eligible (`src-tauri/src/balancer/mod.rs:93-116`, `src-tauri/src/state/db.rs:1350-1440`).

# 8. Test plan

- `mark_exhausted_writes_timestamp_on_existing_quota_row`: seed a `provider_quotas` row, call `mark_exhausted`, assert `exhausted_at` is non-null and within the call window.
- `mark_exhausted_is_noop_when_no_quota_row`: call `mark_exhausted` for an unknown provider and assert no row is created.
- `upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh`: mark an existing provider exhausted, run `upsert_quota_refresh` with at least one window, assert `exhausted_at IS NULL`.
- `upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`: mark an existing provider exhausted, run `upsert_quota_refresh(provider, &[])`, assert the timestamp remains set.
- `classify_exhaustion_matches_quota_billing_usage_limit_stderr`: assert the helper returns true for representative stderr containing "quota", "billing", and "usage limit".
- `classify_exhaustion_ignores_non_quota_errors`: assert the helper returns false for auth, network, unknown-flag, and generic failures.
- `select_provider_filters_exhausted_accounts`: seed two providers where normal ranking would pick provider 0, mark provider 0 exhausted, assert `select_provider` returns provider 1.
- `all_providers_exhausted_falls_through_to_round_robin`: mark every provider exhausted, seed invocation counts, assert `select_provider` still returns the lowest invocation-count provider.
- `exhausted_filter_does_not_prevent_refresh_loop_from_clearing`: set `exhausted_at`, make the provider stale, run `select_provider` with a refresh context whose refresh returns non-empty windows, assert the provider is eligible after the same call's refresh clears the flag.
- `quota_tight_routing_column_dropped_after_migration`: open a DB with the current `quota_tight_routing` column, run `StateDb::open`, assert `PRAGMA table_info(invocations)` no longer includes the column.
- `run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`: execute a failing provider whose stderr diagnoses as quota exhausted, assert the provider quota row gets `exhausted_at`.
- `test_model_marks_provider_exhausted_on_quota_stderr`: run Tauri test-model against a failing fixture provider with quota/billing/usage-limit stderr, assert the provider quota row gets `exhausted_at` while `TestModelResult` remains the plain shape.
<!-- Removed per §D6 (answers) / §5 (proposal): run_repl does NOT classify exhaustion in this PR. -->


Tests to delete:

- Balancer threshold/error behavior: `user_threshold_hides_provider_from_user_class_only`, `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`, `failure_threshold_hard_blocks_all_classes`, `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail` (`src-tauri/src/balancer/mod.rs:732-797`).
- Balancer API tests should be modified for `usize`, but delete only assertions/helpers that exist for `Selection` or `RiskClass`: `selected_provider`, all `.quota_tight_routing` assertions, and all `RiskClass::User`/`RiskClass::Background` argument distinctions in `single_provider_always_zero`, `round_robin_on_fresh_state`, `avoids_errored_providers`, `density_scoring_picks_lowest_used_when_windows_match`, `density_picks_account_with_more_time_when_used_equal`, `binding_constraint_avoids_account_with_pressed_short_window`, `falls_back_to_invocation_count_when_windows_missing`, `high_weekly_account_stops_winning_after_cumulative_turns`, `bootstrap_uses_sibling_pool_when_own_delta_absent`, and `fresh_pool_falls_through_to_invocation_count_round_robin` (`src-tauri/src/balancer/mod.rs:531-582`, `src-tauri/src/balancer/mod.rs:637-645`, `src-tauri/src/balancer/mod.rs:654-730`, `src-tauri/src/balancer/mod.rs:812-825`, `src-tauri/src/balancer/mod.rs:881-890`).
- Main risk cascade tests: `risk_class_cli_flag_overrides_env_var`, `risk_class_env_var_overrides_heuristic`, `risk_class_heuristic_classifies_file_flag_as_background`, `risk_class_heuristic_classifies_tty_prompt_as_user`, `risk_class_heuristic_classifies_parent_invocation_as_background`, `risk_class_heuristic_classifies_piped_stdin_as_background`, `repl_subcommand_always_user_class`, `risk_class_flag_reaches_repl_subcommand` (`src-tauri/src/main.rs:1198-1325`).
- Config balancer tests: `parse_balancer_defaults_when_block_absent`, `parse_balancer_overrides_thresholds`, `rejects_balancer_threshold_outside_unit_interval`, `rejects_balancer_user_threshold_above_failure_threshold`, `roundtrip_model_with_balancer_config` (`src-tauri/src/config/model.rs:1180-1278`).
- State quota-tight persistence test: `quota_tight_routing_column_persisted_to_invocations` (`src-tauri/src/state/db.rs:2929-2951`).
- Tauri structured preflight error test: `test_model_returns_structured_quota_exhausted_error` (`src-tauri/src/lib.rs:902-942`).

# 9. README update

Revise the Load Balancing section by deleting the risk-class/threshold block and the optional `[balancer]` TOML example (`README.md:217-234`). Keep the per-window scoring, bootstrap cascade, lazy refresh, and shared provider account descriptions, but change the eligibility prose: projection ranks providers, recent-error avoidance still deprioritizes noisy providers, and a provider that actually fails with quota/billing/usage-limit stderr is marked exhausted at the account level until the next successful non-empty quota refresh clears it (`README.md:210-238`). Remove `--risk-class` from CLI usage (`README.md:117-130`).

# 10. Cross-cutting considerations

Signature reverts:

- `select_provider(model, state, ctx) -> usize`; no `Result`, no `RiskClass`, no `Selection` (`src-tauri/src/balancer/mod.rs:79-84`).
- `run_with_balancing` drops the `risk_class` argument, and direct-model/agent callers stop resolving or passing it (`src-tauri/src/main.rs:292-320`, `src-tauri/src/main.rs:340-347`, `src-tauri/src/main.rs:670-677`).
- `run_repl` drops `risk_class_override: Option<RiskClass>`, and the subcommand call stops passing `cli.risk_class.map(Into::into)` (`src-tauri/src/main.rs:277-288`, `src-tauri/src/main.rs:488-494`).
- Tauri `test_model_with_db_path` handles the returned provider index directly and no longer catches preflight exhausted errors (`src-tauri/src/lib.rs:519-545`).

`examples/quota_check.rs` reverts to the simpler balancer API: drop `RiskClass::Background`, drop `Result` handling, and print only the selected provider index/name (`src-tauri/examples/quota_check.rs:10-12`, `src-tauri/examples/quota_check.rs:116-128`).

`src-tauri/tests/pr_b_trace_integration.rs` cleanup is mechanical: remove `quota_tight_routing: false` from the two `InvocationStart` literals (`src-tauri/tests/pr_b_trace_integration.rs:73-99`). The same mechanical literal cleanup applies across state and main tests after `InvocationStart` loses the field (`src-tauri/src/state/db.rs:2157-2172`, `src-tauri/src/state/db.rs:2953-3290`, `src-tauri/src/state/db.rs:3533-3554`, `src-tauri/src/main.rs:1348-1364`, `src-tauri/src/main.rs:1441-1525`).

Removing `ModelConfig.balancer` has broad compile fallout in test fixtures that currently populate `balancer: Default::default()`; those fixtures should be mechanically updated, not replaced with a compatibility field (`src-tauri/src/balancer/mod.rs:504-529`, `src-tauri/src/lib.rs:815-825`, `src-tauri/src/executor/cli.rs:971-1148`).

# 11. Risk surface for phase 4

Audit risk: correctness of exhausted-on-refresh clear ordering. The clear must be in the same transaction as the non-empty quota update, so concurrent readers see old quota plus old flag or new quota plus cleared flag, never new quota plus stale exhausted flag (`src-tauri/src/state/db.rs:1350-1440`).

Scope risk: no incidental deletion of projection math, bootstrap, per-window delta learning, fully-unlearned round-robin, refresh behavior, session scanning, or recent-error avoidance (`src-tauri/src/balancer/mod.rs:162-196`, `src-tauri/src/balancer/mod.rs:296-377`, `src-tauri/src/state/db.rs:1282-1437`, `src-tauri/src/balancer/mod.rs:422-474`, `src-tauri/src/balancer/mod.rs:93-116`).

Shortcut risk: the exhausted filter must read `provider_quotas.exhausted_at` fresh for every `select_provider` call through `get_quota`; no caching, no per-invocation memoization, and no background re-probe loop (`src-tauri/src/balancer/mod.rs:111-121`, `src-tauri/src/state/db.rs:1212-1239`).

REPL-stderr-capture is explicitly deferred per answers §D6. `executor::cli::execute_interactive` inherits stderr for TTY passthrough and returns only `i32`; capturing stderr without breaking terminal forwarding is non-trivial. The implementation does NOT add REPL-side classification. Consequence documented in §5: one guaranteed extra quota-failed invocation after a REPL quota-exit before the flag is set on the next balancer-routed call. This is consistent with the locked "no spam" invariant, which governs post-classification stickiness (flag sticky until refresh clears), not signal acquisition (the first classification event itself).

Heuristic-coverage scope per answers §D7: the proposal uses `diagnostics::diagnose_error`'s existing quota heuristic unchanged (`"quota"` / `"billing"` / `"usage limit"`, lowercase-matched). If real CLI stderr uses non-matching phrasing, the flag is not set; the refresh TTL clock self-corrects. Broadening the heuristic is future work against `src-tauri/src/diagnostics/mod.rs` in a separate PR that benefits both one-shot diagnostics and reactive routing.

# 12. Unresolved

