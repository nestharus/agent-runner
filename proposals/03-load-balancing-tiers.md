# 1. Scope statement

This initiative closes three linked balancer defects: tier quantities are currently compared as percent-per-hour rather than expected turns-per-hour, between-refresh projection applies one pool-wide scalar to every window, and callers cannot distinguish user-visible prompts from retryable background work (`research/03-load-balancing-tiers-needs.md:15-29`). It ships as the three PRs fixed by the orchestrator: PR 1 updates `chatgpt-usage` to emit both Codex windows, PR 2 fixes empty-window staleness plus empty refresh writes, and PR 3 moves scoring to per-window burn rates with risk-class gates (`research/03-load-balancing-tiers-answers.md:305-323`).

# 2. PR 1 - `chatgpt-usage` emits 5h + weekly windows

## 2.1 Current shape

Current `chatgpt-usage` documents and emits the legacy flat single-window contract:

```json
{"used_percent": 0.01, "resets_at": "2026-04-24T00:00:37Z"}
```

The installed script reads OAuth fields, calls the ChatGPT usage endpoint, and emits only `.rate_limit.secondary_window` as `{used_percent, resets_at}` (`/home/nes/.local/bin/chatgpt-usage:10-13`, `/home/nes/.local/bin/chatgpt-usage:24-34`, `/home/nes/.local/bin/chatgpt-usage:36-46`). Data probe A captured the same current shape and the observed Codex CLI two-tier display: current script output is `{"used_percent": 5, "resets_at": "2026-04-27T23:26:11Z"}`, while Codex reports both `5h limit` and `Weekly limit` (`research/03-load-balancing-tiers-data-a.md:366-373`, `research/03-load-balancing-tiers-data-a.md:473-490`).

## 2.2 Target shape

The target is the two-window `windows` array already accepted by the Rust parser: `QuotaScriptOutput` prefers `windows` and falls back to the flat legacy shape (`src-tauri/src/quota/mod.rs:65-84`, `src-tauri/src/quota/mod.rs:222-265`). Match the `anthropic-usage` convention of longest window first, short window second, because `window_id` is positional (`/home/nes/.local/bin/anthropic-usage:41-54`; `research/03-load-balancing-tiers-answers.md:115-118`).

Example:

```json
{
  "windows": [
    {"used_percent": 5, "resets_at": "2026-04-27T23:26:11Z"},
    {"used_percent": 4, "resets_at": "2026-04-21T14:27:00Z"}
  ]
}
```

Implementation maps `secondary_window` to weekly/window 0 and `primary_window` to 5h/window 1. If upstream omits one of those entries, emit the present one rather than failing, matching the `if ... else empty end` approach in `anthropic-usage` (`/home/nes/.local/bin/anthropic-usage:45-54`).

## 2.3 Implementation notes

PR 1 adds a tracked `scripts/chatgpt-usage` file modeled on `scripts/anthropic-usage:1-54` (shebang/strict-mode at line 1/21, credential-validation exits before JSON emission at lines 23-34, and the conditional `jq` multi-window emit block at lines 41-54). The script:

- preserves the existing credential validation path (`tokens.access_token` + `tokens.account_id` check, matching current `/home/nes/.local/bin/chatgpt-usage:17-29`);
- preserves the existing HTTP call to `https://chatgpt.com/backend-api/wham/usage` (matching `/home/nes/.local/bin/chatgpt-usage:31-34`);
- replaces the current flat `{used_percent, resets_at}` emit at `/home/nes/.local/bin/chatgpt-usage:36-46` with a `windows` array, mapping `secondary_window` → weekly (window_id 0) and `primary_window` → 5h (window_id 1), and using the same `if ... else empty end` pattern `anthropic-usage` uses so an upstream omission produces a shorter array rather than failing;
- updates the doc comment to say `primary_window` is the 5h window and `secondary_window` is weekly.

PR 1 also folds in the docs update that resolves prior scope-risk finding G1 (human-gate decision C, locked 2026-04-21):

- Add `chatgpt-usage` to the reference-adapter list in `scripts/README.md:207-209` alongside `anthropic-usage` and `zai-usage`.
- Add `chatgpt-usage` to the install example in `README.md:254-258` so the documented manual-install step (`install -m 755 scripts/anthropic-usage scripts/zai-usage ~/.local/bin/`) covers the new script.

Deploy is manual per the existing convention (`README.md:247-258` documents `install -m 755 scripts/... ~/.local/bin/`); no build step exists. After PR 1 merges, the installed `/home/nes/.local/bin/chatgpt-usage` is refreshed manually from the tracked source, same as the existing `anthropic-usage` flow.

## 2.4 Test plan

- `test_chatgpt_usage_emits_two_windows_on_normal_response`: mocked usage response with `primary_window` and `secondary_window` emits `windows` with weekly first and 5h second.
- `test_chatgpt_usage_emits_one_window_when_only_weekly_present`: mocked response with only `secondary_window` emits one weekly window.
- `test_chatgpt_usage_emits_one_window_when_only_five_hour_present`: mocked response with only `primary_window` emits one 5h window.
- `test_chatgpt_usage_credential_failure_exits_nonzero`: unreadable or token-missing auth file exits non-zero and emits no JSON, preserving documented behavior (`research/03-load-balancing-tiers-data-a.md:520-555`).
- `scripts_readme_references_chatgpt_usage_adapter`: grep-style assertion that `scripts/README.md` and `README.md` list `chatgpt-usage` in the tracked reference-adapter inventory (resolves prior scope-risk G1).

## 2.5 Risk surface for phase 4

Phase 4 should audit the field mapping from Codex's upstream response to `primary_window`/`secondary_window`, the decision to preserve positional stability by emitting weekly before 5h, and the packaging path from tracked `scripts/chatgpt-usage` to `/home/nes/.local/bin/chatgpt-usage`. Scope risk is low if PR 1 touches only that script and its script-level tests. Shortcut risk is mainly avoiding hardcoded example timestamps or assuming both windows are always present.

# 3. PR 2 - `is_stale` empty-windows fix + `upsert_quota_refresh` reject-empty

## 3.1 Defect recap

`claude2` has had a `provider_quotas` row with zero `provider_quota_windows`, and the current TTL path treats that state as fresh for up to 24h because `dynamic_ttl_secs([])` returns `MAX_TTL_SECS` (`research/03-load-balancing-tiers-needs.md:210-274`, `src-tauri/src/quota/mod.rs:129-159`). The orchestrator answer requires closing the zero-window paths by forcing stale empty rows and rejecting empty-window writes (`research/03-load-balancing-tiers-answers.md:279-303`, `research/03-load-balancing-tiers-answers.md:325-358`).

## 3.2 Fix in `is_stale`

Change `is_stale`, not `dynamic_ttl_secs`: after `let windows = state.get_windows(provider_name).unwrap_or_default();`, return `true` when `windows.is_empty()` before calling `dynamic_ttl_secs` (`src-tauri/src/quota/mod.rs:132-142`). Leave `dynamic_ttl_secs` as a pure TTL helper for non-empty lists except for its stale test expectation; this keeps the semantic distinction clear: a provider quota row with zero windows is forced-stale, while TTL math still operates on actual windows (`src-tauri/src/quota/mod.rs:145-159`, `src-tauri/src/quota/mod.rs:323-358`).

Minimal diff shape: insert an empty-window guard in `is_stale`, revise the doc comments that currently describe empty windows as a max-TTL first-fetch fallback, and replace `ttl_empty_windows_falls_back_to_max` with a test for `is_stale` forced refresh (`src-tauri/src/quota/mod.rs:129-159`, `src-tauri/src/quota/mod.rs:355-358`).

## 3.3 Fix in `upsert_quota_refresh`

`StateDb::upsert_quota_refresh` currently computes a longest-window provider-level delta, upserts `provider_quotas`, deletes all existing windows, then inserts the incoming set (`src-tauri/src/state/db.rs:1148-1242`). On `windows.len() == 0`, it writes `legacy_used = 0.0`, clears `resets_at`, deletes all window rows, and inserts none (`src-tauri/src/state/db.rs:1162-1189`, `src-tauri/src/state/db.rs:1196-1238`).

New semantics:

- Query prior window count before the transaction's `DELETE`.
- If `windows.is_empty()` and prior count > 0, do not delete or insert window rows and do not update `used_percent`, `resets_at`, `calls_since_refresh`, or delta fields. Update only `provider_quotas.refreshed_at` and `provider_quotas.last_empty_refresh_at`.
- If `windows.is_empty()` and prior count == 0, upsert a `provider_quotas` row with `refreshed_at` and `last_empty_refresh_at` so `is_stale` sees the provider row plus empty windows and forces another refresh on the next selection, but still do not create windows.
- If `windows` is non-empty, retain the current wholesale replacement behavior so scripts can legitimately add/remove windows (`src-tauri/src/state/db.rs:1219-1238`; `research/03-load-balancing-tiers-answers.md:338-351`).

## 3.4 Schema migration

Current schema changes are implemented as idempotent schema ensure/migration helpers inside `StateDb::open`, not a numbered `PRAGMA user_version` list (`src-tauri/src/state/db.rs:338-470`, `src-tauri/src/state/db.rs:482-546`, `src-tauri/src/state/db.rs:616-735`). Name this proposal-level migration `M_03_01_provider_quotas_last_empty_refresh_at` and implement it in the same schema-ensure style.

Exact SQL:

```sql
ALTER TABLE provider_quotas
ADD COLUMN last_empty_refresh_at TEXT NULL;
```

Also update the `CREATE TABLE IF NOT EXISTS provider_quotas` declaration for new databases (`src-tauri/src/state/db.rs:352-360`).

## 3.5 Test plan

- `is_stale_forces_refresh_when_windows_empty`: provider quota row with `refreshed_at` but zero window rows returns stale.
- `is_stale_honors_ttl_when_windows_present`: non-empty windows still use dynamic TTL and do not refresh before the computed TTL.
- `is_stale_treats_missing_quota_row_as_stale`: existing missing-row behavior remains unchanged (`src-tauri/src/quota/mod.rs:132-138`).
- `upsert_quota_refresh_preserves_windows_on_empty_input`: prior windows remain byte-for-byte after empty input.
- `upsert_quota_refresh_wipes_on_nonempty_input_with_all_replaced`: non-empty replacement still deletes windows not re-reported and inserts the new set.
- `upsert_quota_refresh_records_last_empty_refresh_at_on_empty_input`: empty input writes the audit timestamp.
- `upsert_quota_refresh_empty_input_with_no_prior_windows_creates_forced_stale_quota_row`: empty first refresh creates provider metadata without windows, so the next `is_stale` call is true.
- `upsert_quota_refresh_empty_input_does_not_reset_calls_since_refresh_when_prior_windows_exist`: empty transient output does not hide accumulated calls by resetting the counter.

## 3.6 Rollout

The currently window-less `claude2` row will force-refresh on the next `select_provider` call because `select_provider` checks `is_stale` before loading windows (`src-tauri/src/balancer/mod.rs:32-47`). A successful `anthropic-usage` refresh re-populates its two window rows through `refresh_provider -> upsert_quota_refresh` (`src-tauri/src/quota/mod.rs:100-127`, `/home/nes/.local/bin/anthropic-usage:41-54`). Once all providers in the pool again have windows, `select_provider` re-enters density scoring rather than invocation-count fallback (`src-tauri/src/balancer/mod.rs:50-69`).

## 3.7 Risk surface for phase 4

Audit risk is concentrated in the empty-write branch: it must update only the two audit timestamps when prior windows exist, and it must not reset calls or delete rows. Scope risk is keeping PR 2 limited to `quota/mod.rs`, `state/db.rs`, and tests, with no scoring redesign. Shortcut risk is masking empty scraper output with logs only; the DB audit column is required because CLI and Tauri have different log sinks (`research/03-load-balancing-tiers-answers.md:353-358`).

# 4. PR 3 - Scoring redesign

## 4.1 Decision summary

PR 3 implements the orchestrator's Q1-Q8 decisions exactly: score providers by binding expected turns-per-hour, store learned burn rates per quota window, bootstrap by provider then model-pool slot then duration ratio, use the model pool as the sibling group, replace the single scalar projection, add explicit/heuristic `User` vs `Background`, default thresholds to `0.70`/`0.95` with model TOML overrides, hard-refuse at 95%, and soft-degrade user calls at 70% with `quota_tight_routing` (`research/03-load-balancing-tiers-answers.md:21-277`).

## 4.2 Schema migrations

As in PR 2, these are proposal-level migration names implemented through `StateDb::open` schema ensure code; the current repo has no standalone numeric migration runner (`src-tauri/src/state/db.rs:338-470`, `src-tauri/src/state/db.rs:482-546`). The repo links `rusqlite` with the `bundled` feature (`src-tauri/Cargo.toml:10-17`), lockfile uses `libsqlite3-sys 0.36.0` (`src-tauri/Cargo.lock:1783-1792`) and `rusqlite 0.38.0` (`src-tauri/Cargo.lock:2825-2838`), and the bundled SQLite header is 3.51.1 (`/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libsqlite3-sys-0.36.0/sqlite3/sqlite3.h:149-151`). SQLite 3.51.1 supports `ALTER TABLE DROP COLUMN` (added in 3.35.0), so the provider-level delta columns can be dropped directly; no table rebuild is required for this specific drop.

Migration `M_03_02_provider_quota_windows_deltas`:

```sql
ALTER TABLE provider_quota_windows
ADD COLUMN last_delta_percent REAL NULL;

ALTER TABLE provider_quota_windows
ADD COLUMN last_delta_calls INTEGER NULL;
```

Migration `M_03_03_drop_provider_quota_provider_level_deltas`:

```sql
ALTER TABLE provider_quotas
DROP COLUMN last_delta_percent;

ALTER TABLE provider_quotas
DROP COLUMN last_delta_calls;
```

Migration `M_03_04_invocations_quota_tight_routing`:

```sql
ALTER TABLE invocations
ADD COLUMN quota_tight_routing BOOLEAN NOT NULL DEFAULT 0;
```

For new databases, update `provider_quotas`, `provider_quota_windows`, and `invocations_schema_sql` declarations (`src-tauri/src/state/db.rs:352-367`, `src-tauri/src/state/db.rs:564-590`). For legacy invocation rebuilds, add `quota_tight_routing` to `invocations_new` with default `0` and insert `0` for migrated rows (`src-tauri/src/state/db.rs:658-727`).

## 4.3 Model TOML - `[balancer]` block

Add a `BalancerConfig` field to `ModelConfig` in `src-tauri/src/config/model.rs`, where `ModelConfig` currently contains `name`, `prompt_mode`, `providers`, and `inputs` (`src-tauri/src/config/model.rs:203-210`). `RawModelToml` currently accepts command/provider/input fields and should add `balancer: Option<BalancerConfig>` (`src-tauri/src/config/model.rs:273-285`). `ModelConfig::from_toml` builds the final config and is the parser hookpoint (`src-tauri/src/config/model.rs:510-591`); `to_toml` is the serializer hookpoint (`src-tauri/src/config/model.rs:390-508`).

Shape:

```rust
pub struct BalancerConfig {
    pub user_threshold: f64,
    pub failure_threshold: f64,
}
```

TOML:

```toml
[balancer]
user_threshold = 0.70
failure_threshold = 0.95
```

Fields are optional in TOML and default to `0.70` and `0.95` (`research/03-load-balancing-tiers-answers.md:216-237`). Validation rejects non-finite values and values outside `0.0..=1.0`, and rejects `user_threshold > failure_threshold` because the user gate must not be stricter than the hard failure gate.

Parser tests:

- `parse_balancer_defaults_when_block_absent`: absent `[balancer]` yields `0.70` and `0.95`.
- `parse_balancer_overrides_thresholds`: TOML overrides populate both fields.
- `rejects_balancer_threshold_outside_unit_interval`: values below 0 or above 1 fail load.
- `rejects_balancer_user_threshold_above_failure_threshold`: invalid ordering fails load.
- `roundtrip_model_with_balancer_config`: `to_toml` preserves non-default balancer thresholds.

## 4.4 Risk class plumbing

Add `RiskClass { User, Background }` to `src-tauri/src/balancer/mod.rs`, next to the selection API it affects (`src-tauri/src/balancer/mod.rs:22-70`). The main CLI parser currently has flags through `inputs` and no risk-class field (`src-tauri/src/main.rs:17-61`); add `#[arg(long = "risk-class", value_parser = ["user", "background"])] risk_class: Option<RiskClassArg>` to `Cli`. Current explicit env-var reads only include `OULIPOLY_PARENT_INVOCATION`, `OPENAI_API_KEY`, and test-only `XDG_CONFIG_HOME`, so `OULIPOLY_RISK_CLASS` is a new runtime read (`research/03-load-balancing-tiers-data-b.md:137-147`).

Heuristic cascade:

1. If `--risk-class user|background` is present, use it.
2. Else if running `repl`, use `User`. The repl override sits above the
   env-var check because an interactive human session cannot tolerate a
   background-class routing inherited from a shell export; the
   `repl_subcommand_always_user_class` test pins this. A workflow that
   genuinely wants background-class repl sets `--risk-class background`
   explicitly, and the `--risk-class` flag is marked `global = true` so
   it reaches the `repl` subcommand under the root parser's
   `args_conflicts_with_subcommands = true` setting.
3. Else if `OULIPOLY_RISK_CLASS=user|background`, use it (validated;
   bogus values error out).
4. Else for one-shot, use `Background` when `-f/--file` is provided.
5. Else use `Background` when `OULIPOLY_PARENT_INVOCATION` is set.
6. Else use `Background` when stdin is not a TTY (pipe or redirect, regardless of whether a positional prompt was also provided). The runner cannot distinguish human-typed pipes (`cat spec.md | agents`, cluster H, 3 of 92 invocations) from scripted pipes, and the majority of observed pipe-stdin cases are workflows. Explicit classification via `--risk-class` or the env var overrides.
7. Else use `User` (positional prompt at a TTY — clusters C, D, and G, 24 of 92 invocations).

This is the authoritative Q6 cascade as revised (`research/03-load-balancing-tiers-answers.md:166-214`; revision reconciles the earlier rule-5 `cat | agents` example that conflicted with rule 4's scripted-pipe treatment — audit-risk finding 2 on the prior revision, plus the repl/env-var precedence reordering from the phase-7 CodeRabbit loop). Hookpoints: `resolve_prompt` already checks stdin TTY and prompt/file state (`src-tauri/src/main.rs:165-188`); `run` has the direct-model and agent execution branches that call `run_with_balancing` (`src-tauri/src/main.rs:204-289`); `resolve_parent_invocation_id` reads `OULIPOLY_PARENT_INVOCATION` (`src-tauri/src/main.rs:706-714`).

Change `select_provider` from:

```rust
pub fn select_provider(model: &ModelConfig, state: &StateDb, ctx: Option<&BalanceContext<'_>>) -> usize
```

to:

```rust
pub fn select_provider(
    model: &ModelConfig,
    state: &StateDb,
    ctx: Option<&BalanceContext<'_>>,
    risk_class: RiskClass,
) -> Result<Selection, BalanceError>
```

`Selection` contains `provider_index: usize` and `quota_tight_routing: bool`. `BalanceError::Exhausted(ExhaustedError)` carries category `quota_exhausted`, the model name, risk class, and per-provider projected max usages. Caller updates:

- `run_with_balancing` threads the parsed class into selection and persists `quota_tight_routing` when starting/finalizing the invocation (`src-tauri/src/main.rs:589-703`, `src-tauri/src/state/db.rs:768-889`).
- `run_repl` hardcodes `RiskClass::User` for non-resume provider selection (`src-tauri/src/main.rs:429-526`).
- Tauri `test_model` hardcodes `RiskClass::User` (`src-tauri/src/lib.rs:471-504`).

The new invocation column is `invocations.quota_tight_routing BOOLEAN NOT NULL DEFAULT 0`. `start_invocation` currently inserts the invocation metadata without that column (`src-tauri/src/state/db.rs:768-797`); add the column to `InvocationStart` or provide an update method before finalization. `InvocationRecord` currently has no field for it and should be extended (`src-tauri/src/state/db.rs:103-120`).

## 4.5 Per-window burn rate learning

Extend `QuotaWindow` with `last_delta_percent: Option<f64>` and `last_delta_calls: Option<u64>`, because `get_windows` is the balancer's window read path (`src-tauri/src/state/db.rs:42-52`, `src-tauri/src/state/db.rs:1109-1146`). Change `StateDb::upsert_quota_refresh` to build a prior-window map keyed by `window_id` before the wholesale delete (`src-tauri/src/state/db.rs:1148-1242`).

For each incoming `windows[i]`:

- Match prior row by `(provider_name, window_id = i)`.
- If prior exists and prior provider `refreshed_at` exists, compute `dp = max(0, new.used_percent - prior.used_percent)`.
- Pair `dp` with `count_assistant_turns_since(provider_name, prior.refreshed_at)`; this function already counts assistant turns after a timestamp (`src-tauri/src/state/db.rs:1809-1837`).
- If `dp > 0` and calls > 0, write those delta columns on the new window row.
- Otherwise carry forward that same prior window's previous delta, if any, so a reset or unchanged percent does not erase the last useful learned rate.

When window counts differ between refreshes, positional matching handles it exactly: a new `window_id` has no prior row and writes no delta until the next refresh; a missing old `window_id` is deleted by the existing replacement semantics; a reordered scraper output is treated as a different window and is therefore a correctness risk for phase 4.

Carry-forward scope: the prior `(dp, dc)` pair is carried forward whenever the new pair is not strictly positive — i.e., the new refresh produces `dp == 0` (window reset or flat observation) OR `dc == 0` (no ingested assistant turns credited to the inter-refresh gap, possible under session-ingestion timing skew). On `dp > 0 && dc > 0`, the new pair overwrites. No staleness bound is enforced in this PR — the carried-forward rate persists until the next positive-delta refresh overwrites it. Pathological drift is bounded in practice by the next refresh after workload resumes; when preserved windows cross their `resets_at`, `dynamic_ttl_secs` floors to `MIN_TTL_SECS` (`src-tauri/src/quota/mod.rs:148-158`), which accelerates re-refresh cadence but does not by itself clear the carried-forward values — only a subsequent positive `(dp, dc)` pair does that. A max-refreshes-without-update decay counter is deferred to a future iteration if observed workloads show drift (audit-risk findings 2, 3, 7 on the prior revision).

Delete provider-level delta computation and writes from `provider_quotas`: remove the longest-window delta block and stop selecting `last_delta_percent`/`last_delta_calls` in `get_quota` (`src-tauri/src/state/db.rs:1077-1099`, `src-tauri/src/state/db.rs:1162-1182`, `src-tauri/src/state/db.rs:1196-1217`). `QuotaRecord` should no longer expose provider-level deltas (`src-tauri/src/state/db.rs:29-40`). Delete `get_quotas` entirely (`src-tauri/src/state/db.rs:1277-1317`) — it has no current `src-tauri/src` callers and would otherwise become a stale compatibility surface after the column drop (human-gate decision B, locked 2026-04-21).

Update the diagnostic example `src-tauri/examples/quota_check.rs:117-119` to match the new `select_provider` signature. Pass `RiskClass::Background` (diagnostic tooling is non-interactive) and handle the `Result<Selection, BalanceError>` return. Drop any provider-level delta reads in that file — the example must compile against the post-PR-3 schema (human-gate decision A, locked 2026-04-21).

## 4.6 Bootstrap cascade

Add `pool_window_avg_percent_per_call` in `src-tauri/src/balancer/mod.rs`, replacing `global_avg_percent_per_call` (`src-tauri/src/balancer/mod.rs:144-164`).

Signature:

```rust
fn pool_window_avg_percent_per_call(
    window_id: u32,
    windows: &[Vec<QuotaWindow>],
) -> Option<f64>
```

Add `duration_ratio_fallback_percent_per_call` in the same module:

```rust
fn duration_ratio_fallback_percent_per_call(
    target_window: &QuotaWindow,
    quotas: &[Option<QuotaRecord>],
    windows: &[Vec<QuotaWindow>],
) -> Option<f64>
```

Return type is `Option<f64>`, not `f64`. A `None` means "this window has no bootstrap available; mark the provider ineligible for density scoring at §4.7." This supersedes the earlier floor-at-zero-plus-`EPS_BURN_RATE` plan, which would have made an unlearned provider outrank learned siblings by orders of magnitude (audit-risk finding 6 on the prior proposal revision).

Pseudocode:

```text
burn_rate(provider_index, window) -> Option<f64>:
  if window.last_delta_percent > 0 and window.last_delta_calls > 0:
    return Some(window.last_delta_percent / window.last_delta_calls)

  if pool_window_avg_percent_per_call(window.window_id, windows) exists:
    return that value

  if any sibling has learned rate on a longer effective window:
    target_hours = max(window.resets_at - provider.refreshed_at, epsilon_hours)
    long_hours = max(long_window.resets_at - sibling.refreshed_at, epsilon_hours)
    return Some(long_rate * (long_hours / target_hours))

  return None
```

The ratio is `effective_long_window_hours / effective_window_hours_w`, corrected from the earlier `(target / long)` direction flagged by audit-risk finding 1 on the prior revision. Physical intuition: a shorter tier has proportionally less capacity per unit workload, so per-turn burn is *larger* on the shorter tier. For a 5h tier vs a 7d tier, the multiplier is ≈ 168 / 5 ≈ 33.6. Verified against live deltas in `research/03-load-balancing-tiers-data-a.md:143-149`: claude's learned long-window rate ≈ 8.4e-5/turn implies a correct 5h bootstrap ≈ 2.82e-3/turn, matching the empirical ratio. Answers §Q3 now carries the corrected formula and physical-intuition note (`research/03-load-balancing-tiers-answers.md:99-115`).

The window-id stability assumption is positional: `QuotaWindow.window_id` is documented as a stable per-provider position index (`src-tauri/src/state/db.rs:42-52`), `get_windows` orders by `window_id` (`src-tauri/src/state/db.rs:1109-1119`), and `anthropic-usage` emits seven-day first then five-hour (`/home/nes/.local/bin/anthropic-usage:45-54`). The installed `anthropic-usage` comment currently says "Window order doesn't matter" because the old learner used the longest window (`/home/nes/.local/bin/anthropic-usage:16-18`); PR 3 should update that comment in any tracked script copy if it becomes misleading, but not bundle script behavior changes into PR 3.

## 4.7 Scoring function

Delete `global_avg_percent_per_call`; it currently sums provider-level deltas and returns one scalar for the entire pool (`src-tauri/src/balancer/mod.rs:144-164`). `score_by_density` currently takes quotas and windows, counts turns per provider, projects every window with the same scalar, computes `remaining / hours`, and falls back to round-robin when every score is `-inf` (`src-tauri/src/balancer/mod.rs:88-141`).

Per-provider evaluation record (explicit shape, resolves audit-risk finding 3 on the prior revision):

```rust
struct ProviderEval {
    index: usize,
    binding_score: Option<f64>,        // None when any window's burn_rate is None, or when hard/operational blocked
    hard_blocked: bool,                // projected_used >= failure_threshold on any window, OR operational error block
    user_blocked: bool,                // projected_used >= user_threshold on any window
    max_projected_used_percent: f64,   // for error reporting in §4.8
    unlearned: bool,                   // set when bootstrap_burn_rate returned None for any window
}
```

New contract:

```text
score_by_density(model, state, quotas, windows, risk_class) -> Result<Selection, BalanceError>
  let evals: Vec<ProviderEval>
  for provider p:
    if recent errors >= threshold:
      evals.push(ProviderEval { hard_blocked: true, binding_score: None, ... })
      continue
    turns = count_assistant_turns_since(p, q.refreshed_at)
    let mut window_rates: Vec<f64>
    let mut max_proj = 0.0
    let mut any_unlearned = false
    let mut any_hard_block = false
    let mut any_user_block = false
    for window w:
      let br = bootstrap_burn_rate(p, w)  // Option<f64> per §4.6
      if br is None:
        any_unlearned = true
        continue                          // this window contributes no rate to the min
      projected_used_w = clamp(w.used_percent + turns * br, 0, 1)
      max_proj = max(max_proj, projected_used_w)
      if projected_used_w >= failure_threshold: any_hard_block = true
      if risk_class == User and projected_used_w >= user_threshold: any_user_block = true
      hours_until_reset_w = max((w.resets_at - now).seconds / 3600, EPS_HOURS)
      remaining_headroom_w = max(0, 1 - projected_used_w)
      # Score shape: (remaining fraction) * (hours until reset). A larger
      # product means more fractional headroom spread over more hours and
      # is the preferred tier to burn against. Per-window burn rates still
      # matter for correctness — they drive `projected_used_w`, which is
      # what the threshold gates read — but they do NOT enter the below-
      # threshold ranking directly. The `density_picks_account_with_more_time_when_used_equal`
      # test pins this formula (more time = higher score = wins).
      window_rates.push(remaining_headroom_w * hours_until_reset_w)
    binding_score = if any_unlearned or any_hard_block or window_rates.is_empty()
                      { None }
                    else
                      { Some(window_rates.min()) }
    evals.push(ProviderEval { index, binding_score, hard_blocked: any_hard_block,
                              user_blocked: any_user_block, max_projected_used_percent: max_proj,
                              unlearned: any_unlearned })

  # Selection policy (explicit filters over evals):
  let hard_eligible = evals.filter(|e| !e.hard_blocked && !e.unlearned)
  if hard_eligible.is_empty():
    # Audit-finding 6 on prior revision: when no provider has a learned rate at all,
    # do not score with EPS-floored bootstrap. Fall through to round_robin_fallback
    # (invocation-count), which is the pre-PR-3 behavior for first-run pools.
    if evals.all(|e| e.unlearned) and evals.none(|e| e.hard_blocked):
      return Ok(Selection { provider_index: round_robin_fallback(model, state),
                            quota_tight_routing: false })
    # Otherwise every still-eligible provider is above failure_threshold.
    return Err(BalanceError::Exhausted(ExhaustedError { ... max_projected per provider ... }))

  if risk_class == User:
    let user_eligible = hard_eligible.filter(|e| !e.user_blocked)
    if user_eligible.is_empty():
      # Soft-degrade: rank by binding_score (all are Some because !unlearned && !hard_blocked)
      let best = hard_eligible.max_by_key(|e| e.binding_score.unwrap())
      return Ok(Selection { provider_index: best.index, quota_tight_routing: true })
    let best = user_eligible.max_by_key(|e| e.binding_score.unwrap())
    return Ok(Selection { provider_index: best.index, quota_tight_routing: false })

  # risk_class == Background: hard-eligible set is final.
  let best = hard_eligible.max_by_key(|e| e.binding_score.unwrap())
  return Ok(Selection { provider_index: best.index, quota_tight_routing: false })
```

Use `EPS_HOURS = 1.0 / 60.0`, the existing one-minute floor carried over unchanged (`src-tauri/src/balancer/mod.rs:125-129`). No `EPS_BURN_RATE`; the `br is None` branch handles the unlearned case without division. An unlearned provider is ineligible for density scoring, which keeps a fresh-join provider from dominating learned siblings; once the pool gathers any learnable rate at any `window_id`, the bootstrap cascade (§4.6) rescues the unlearned provider via the pool-average or duration-ratio paths.

Round-robin fallback only fires when every provider in the pool is unlearned and none is hard-blocked — i.e., a first-run pool with no observations yet. This matches the pre-PR-3 `all_have_windows`-fail behavior at `src-tauri/src/balancer/mod.rs:62-69`.

Selection-policy cross-references:

- Hard refuse at 95% returns `BalanceError::Exhausted`, not `round_robin_fallback` (`research/03-load-balancing-tiers-answers.md:239-277`).
- `User` soft-degrade at 70% picks highest `binding_score` and sets `quota_tight_routing = true` with stderr warning `[warn: no provider below user_threshold; routing via quota-tight path]` on the CLI path (`research/03-load-balancing-tiers-answers.md:250-263`).
- Fresh-pool round-robin is the only remaining round-robin path (`src-tauri/src/balancer/mod.rs:167-218`, `research/03-load-balancing-tiers-answers.md:275-277`).

## 4.8 Error surfacing

CLI one-shot path (`run_with_balancing`) currently selects a provider, starts an invocation, executes, then runs diagnostics only after a subprocess failure (`src-tauri/src/main.rs:589-703`, `src-tauri/src/main.rs:717-740`). Add a pre-flight `BalanceError::Exhausted` branch immediately after selection. It should create an invocation row when a concrete provider can be named only for the soft-degrade path; for hard all-provider exhaustion with no provider chosen, print a quota-exhausted stderr message, print `[diagnostics: quota_exhausted]`, and return exit code `1`. If an invocation row is started for a selected quota-tight provider, finalization stores `error_category = 'quota_exhausted'` only on actual refusal, not on soft routing (`src-tauri/src/state/db.rs:799-889`).

CLI interactive path (`run_repl`) currently calls `balancer::select_provider(model, &state, Some(&ctx))` and immediately indexes `model.providers[provider_index]` (`src-tauri/src/main.rs:429-526`, confirmed at the `select_provider` call near line 525 in the current code). After the signature change to `Result<Selection, BalanceError>`, `run_repl` follows the same exhaustion surface as `run_with_balancing`: on `Err(Exhausted)` it prints a quota-exhausted stderr message plus `[diagnostics: quota_exhausted]` and returns exit code `1` **without** creating an invocation row (no provider was selected, and `run_repl` does not support mid-conversation downgrade). On `Ok(Selection { quota_tight_routing: true, .. })` it proceeds with the selected provider; the `quota_tight_routing` column is persisted when the invocation row is started, and the stderr warning from §4.7 is emitted before the interactive subprocess inherits the terminal.

Tauri `test_model`: current command returns `TestModelResult { success, stdout, stderr, exit_code }` and does not run diagnostics (`src-tauri/src/lib.rs:25-31`, `src-tauri/src/lib.rs:471-504`). Extend it to:

```json
{
  "success": false,
  "stdout": "",
  "stderr": "All providers are projected above failure_threshold",
  "exit_code": 1,
  "error": {
    "category": "quota_exhausted",
    "message": "All providers are projected above failure_threshold",
    "model_name": "claude-opus",
    "risk_class": "user",
    "providers": [
      {
        "provider_name": "claude",
        "projected_max_used_percent": 0.97,
        "failure_threshold": 0.95,
        "user_threshold": 0.70
      }
    ]
  }
}
```

`refresh_quotas` needs no change: it already returns per-provider status, message, and windows (`src-tauri/src/lib.rs:290-390`; `research/03-load-balancing-tiers-data-a.md:421-428`).

## 4.9 Test plan

Rewrite existing scoring tests to seed per-window delta columns instead of relying on provider-level deltas: `density_scoring_picks_lowest_used_when_windows_match`, `density_picks_account_with_more_time_when_used_equal`, `binding_constraint_avoids_account_with_pressed_short_window`, and `falls_back_to_invocation_count_when_windows_missing` (`src-tauri/src/balancer/mod.rs:319-433`; `research/03-load-balancing-tiers-data-a.md:280-288`).

- `high_weekly_account_stops_winning_after_cumulative_turns`: projected weekly runway reduces binding score enough that a lower-weekly sibling wins.
- `user_threshold_hides_provider_from_user_class_only`: a provider above 70% but below 95% is skipped for `User` while still eligible for `Background`.
- `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`: all providers above 70% but below 95% selects best provider and marks quota-tight.
- `failure_threshold_hard_blocks_all_classes`: a provider projected above 95% is unavailable for both risk classes.
- `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`: all providers above 95% returns `BalanceError::Exhausted`.
- `per_window_burn_rate_projects_short_window_faster_than_long`: short window uses larger learned percent-per-turn than long window.
- `bootstrap_uses_sibling_pool_when_own_delta_absent`: missing provider delta reads same-window learned average from siblings.
- `bootstrap_uses_duration_ratio_when_pool_has_only_long_delta`: missing short-window delta scales from a longer learned window by duration ratio.
- `bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`: given long_rate = 8.4e-5/turn on a 168h window, the derived short-window rate for a 5h tier equals approximately `8.4e-5 * 33.6 ≈ 2.82e-3` (direction check for audit-risk finding 1 on the prior revision).
- `bootstrap_returns_none_when_no_sibling_has_learned_rate`: completely unlearned pool returns `None` per window, marking all providers `unlearned`.
- `unlearned_provider_is_ineligible_when_siblings_are_learned`: a newly joined provider without its own delta but with siblings that have learned rates is still scored via the pool-average bootstrap (not ineligible); but an unlearned provider in a pool where siblings have no same-slot or longer-window learning is ineligible.
- `fresh_pool_falls_through_to_invocation_count_round_robin`: all providers unlearned and none hard-blocked falls through to `round_robin_fallback` rather than returning `ExhaustedError`.
- `risk_class_cli_flag_overrides_env_var`: explicit flag wins over `OULIPOLY_RISK_CLASS`.
- `risk_class_env_var_overrides_heuristic`: env var wins when flag is absent.
- `risk_class_heuristic_classifies_file_flag_as_background`: `-f/--file` defaults to `Background`.
- `risk_class_heuristic_classifies_tty_prompt_as_user`: positional prompt at TTY defaults to `User`.
- `risk_class_heuristic_classifies_parent_invocation_as_background`: inherited parent invocation defaults to `Background`.
- `risk_class_heuristic_classifies_piped_stdin_as_background`: `cat spec.md | agents --model glm` defaults to `Background` because the runner cannot distinguish human-typed pipes from scripted pipes and the majority of observed pipe-stdin invocations are workflows. Users who want `User` class for piped stdin set `OULIPOLY_RISK_CLASS=user` or `--risk-class user` explicitly (answers §Q6 rule 4 as revised).
- `repl_subcommand_always_user_class`: repl selection passes `RiskClass::User`.
- `balancer_toml_overrides_apply_per_model_pool`: model-specific thresholds affect only that `ModelConfig`.
- `quota_tight_routing_column_persisted_to_invocations`: soft-degrade routing writes the boolean column.
- `test_model_returns_structured_quota_exhausted_error`: Tauri command returns the structured error shape without spawning a CLI.
- `upsert_quota_refresh_writes_per_window_delta_for_matching_window_id`: refresh-to-refresh deltas land on `provider_quota_windows`.
- `upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change`: reset does not erase the last useful burn rate.

## 4.10 Risk surface for phase 4

Audit risk: correctness of per-window delta math under refresh-to-refresh window renumbering edge cases; correctness of duration-ratio direction; correctness of the 95% hard block returning `ExhaustedError` without livelock or round-robin in pool-wide exhaustion.

Scope risk: every hunk belongs to PR 3; no incidental script packaging, no UI affordance beyond `test_model` response shape, and migration order remains PR 2 before PR 3.

Shortcut risk: no dual-write of deltas on both old and new tables, no provider-level delta compatibility aliases, no TODO-gated rollout, no feature flags, and no hidden fallback to old scalar scoring.

# 5. Cross-PR considerations

## 5.1 Dependency graph

PR 1 and PR 2 are independent and can ship in parallel. PR 3 depends on PR 2 so empty-window rows self-heal before scoring tests validate real pools (`research/03-load-balancing-tiers-answers.md:305-323`). PR 1 is operationally important for Codex tier validation, but it is structurally disjoint from PR 3's Rust code (`research/03-load-balancing-tiers-data-a.md:492-503`).

## 5.2 Migration order

Order the schema changes as:

1. `M_03_01_provider_quotas_last_empty_refresh_at` in PR 2.
2. `M_03_02_provider_quota_windows_deltas` in PR 3.
3. `M_03_03_drop_provider_quota_provider_level_deltas` in PR 3, after code reads window-level deltas.
4. `M_03_04_invocations_quota_tight_routing` in PR 3.

The PR 3 add-before-drop sequence avoids needing any dual-write: code in that PR migrates, reads, and writes the new window-level columns, then removes provider-level columns in the same clean schema version.

## 5.3 State DB file split

The CLI default opens `dirs::data_dir()/oulipoly-agent-runner/state.db` through `StateDb::open_default`, while Tauri commands open `state.db` adjacent to the app's models directory (`src-tauri/src/state/db.rs:475-480`, `src-tauri/src/lib.rs:333-340`, `src-tauri/src/lib.rs:484-492`). Needs synthesis explicitly keeps this two-DB split out of scope (`research/03-load-balancing-tiers-needs.md:31-40`). This proposal does not rely on unifying the files: both paths call `StateDb::open`, so both receive the same schema ensures and both enforce the same scoring semantics against whichever DB they open (`src-tauri/src/state/db.rs:326-340`).

## 5.4 Rollback plan

If PR 3 code is reverted after migrations have run, extra columns on `provider_quota_windows` and `invocations` are tolerated as unused by old code only until `provider_quotas.last_delta_percent` and `last_delta_calls` have been dropped. After `M_03_03`, old code that selects those provider-level delta columns in `get_quota` will fail (`src-tauri/src/state/db.rs:1077-1099`). Recovery path is to roll forward with the PR 3 code or run a deliberate repair migration that re-adds the provider-level columns; do not add compatibility shims or dual-write paths to make rollback transparent.

# 6. Unresolved

None.

# 7. Phase 4 inputs

Use this proposal at `proposals/03-load-balancing-tiers.md` as input to the three phase 4 risk assessments: `risk/03-audit.md`, `risk/03-scope.md`, and `risk/03-shortcut.md`.
