DB snapshots in this file were read from `file:/home/nes/.local/share/oulipoly-agent-runner/state.db?mode=ro` via Python's SQLite client. Snapshot times are stated inline.

## 1. Q1 — Current binding score unit
What unit does the existing `score_by_density` binding score use?

What units could it express without schema changes vs with schema changes? Enumerate — don't pick.

What units are present in the DB today vs what would have to be derived? (e.g., is there any absolute turn-capacity field anywhere?)

Evidence:

`score_by_density()` projects `projected = used_percent + turns * avg`, then computes `remaining = 1 - projected`, computes `hours = (resets_at - now) / 3600`, and scores each window as `remaining / hours`; the doc comment labels this quantity as "remaining headroom per unit time." That makes the current binding-score unit "fraction of one window budget remaining per hour." (`src-tauri/src/balancer/mod.rs:72-82`, `src-tauri/src/balancer/mod.rs:120-131`)

The stored units that feed that expression are:

| Stored field | Stored unit | Evidence |
| --- | --- | --- |
| `provider_quota_windows.used_percent` | Fraction of one window budget, `0..1` | `src-tauri/src/state/db.rs:42-52` |
| `provider_quota_windows.resets_at` | Absolute RFC3339 timestamp | `src-tauri/src/state/db.rs:42-52` |
| `provider_quotas.last_delta_percent` | Fractional change in the longest window's `used_percent` | `src-tauri/src/state/db.rs:29-40`, `src-tauri/src/state/db.rs:1148-1182` |
| `provider_quotas.last_delta_calls` | Assistant-turn count paired with that fractional change | `src-tauri/src/state/db.rs:29-40`, `src-tauri/src/state/db.rs:1148-1182` |
| `session_turns` counted through `count_assistant_turns_since()` | Assistant turns | `src-tauri/src/state/db.rs:61-75`, `src-tauri/src/state/db.rs:1809-1837` |

The quota tables do not store any absolute turn-capacity field today. Schema declarations and live PRAGMA output both show only `used_percent`, `resets_at`, `calls_since_refresh`, `refreshed_at`, `last_delta_percent`, and `last_delta_calls` on the quota side. (`src-tauri/src/state/db.rs:352-367`; DB query at 2026-04-21T11:13Z: `PRAGMA table_info(provider_quotas); PRAGMA table_info(provider_quota_windows);` -> `provider_quotas(provider_name, used_percent, resets_at, calls_since_refresh, refreshed_at, last_delta_percent, last_delta_calls)`, `provider_quota_windows(provider_name, window_id, used_percent, resets_at)`)

Alternatives observed:

- Without schema changes, the current fields can express:
  - Fraction-per-hour: current `remaining / hours` scoring path. (`src-tauri/src/balancer/mod.rs:120-131`)
  - Fraction-per-turn: `last_delta_percent / last_delta_calls`, but only at provider level because the stored delta pair lives on `provider_quotas`, not per window. (`src-tauri/src/balancer/mod.rs:144-164`, `src-tauri/src/state/db.rs:352-360`)
  - A derived turns-remaining quantity, if a burn rate is supplied, from `(1 - used_percent) / (percent-per-turn)`; the ingredients exist, but the turns value is not stored. This is an inference from the current fields above.
- With schema changes, the quota tables could additionally hold:
  - Absolute turn-capacity per window.
  - Absolute remaining turns per window.
  - Per-window burn-rate columns rather than the current provider-level pair.
  - These are not present in the current schema. (`src-tauri/src/state/db.rs:352-367`; DB PRAGMA query above)

## 2. Q2 — Per-window burn rate: storage mechanisms available
Where could a per-window delta be stored without breaking existing migrations? List every table + feasible column addition, with current schema evidence.

Is there any existing column on `provider_quota_windows` that already holds or could plausibly hold a delta (based on current usage)?

What existing write paths would need to change to populate a per-window delta? List the exact function names and file paths.

Is there historical evidence in the DB that would allow backfilling per-window deltas from past refreshes? (Check: does `provider_quota_windows` or any audit/log table retain prior `used_percent` snapshots? Query it.)

Evidence:

The only existing table that already has both provider identity and per-window identity is `provider_quota_windows(provider_name, window_id, ...)`, so it can hold one learned delta per window by adding columns to that row shape. (`src-tauri/src/state/db.rs:362-367`)

`provider_quotas` has one row per `provider_name` and no `window_id`, so it can only hold per-window deltas by multiplexing multiple windows into added columns or an added serialized payload column. That is an inference from its current primary key and column set. (`src-tauri/src/state/db.rs:352-360`)

No existing `provider_quota_windows` column currently holds a delta. The live schema and the Rust struct show only `provider_name`, `window_id`, `used_percent`, and `resets_at`; `used_percent` is the absolute latest utilization ratio, and `resets_at` is the absolute reset timestamp. (`src-tauri/src/state/db.rs:42-52`, `src-tauri/src/state/db.rs:362-367`; DB query at 2026-04-21T11:13Z: `PRAGMA table_info(provider_quota_windows);` -> `(provider_name, window_id, used_percent, resets_at)`)

The current production write path into quota tables is:

1. `refresh_provider()` runs the script, receives `Vec<QuotaWindowInput>`, and calls `state.upsert_quota_refresh(provider_name, &windows)`. (`src-tauri/src/quota/mod.rs:100-127`)
2. `StateDb::upsert_quota_refresh()` computes one provider-level delta from the longest window, writes `provider_quotas`, deletes all existing `provider_quota_windows` rows for that provider, and inserts the fresh rows. (`src-tauri/src/state/db.rs:1148-1242`)

If per-window deltas were derived locally from the script's existing output, those two functions are the write path that would change. If the script output shape itself were extended, `QuotaScriptOutput`, `QuotaScriptWindow`, and `parse_output()` are the parse path that would change before the write. (`src-tauri/src/quota/mod.rs:65-84`, `src-tauri/src/quota/mod.rs:222-265`)

There is no retained refresh history table for quota snapshots in the live DB. A read-only schema query at 2026-04-21T11:14Z returned only `provider_quotas` and `provider_quota_windows` as tables containing `used_percent`, and a query for history/audit/log/snapshot-style table names returned no rows:

```sql
SELECT name
FROM sqlite_master
WHERE type='table'
  AND (name LIKE '%history%' OR name LIKE '%audit%' OR name LIKE '%log%' OR name LIKE '%snapshot%')
ORDER BY name;

SELECT name
FROM sqlite_master
WHERE type='table' AND sql LIKE '%used_percent%'
ORDER BY name;
```

Result:

- History/audit/log/snapshot query: no rows.
- `used_percent` query: `provider_quota_windows`, `provider_quotas`.

`provider_quota_windows` itself keeps only one row per `(provider_name, window_id)`, not prior snapshots. (`src-tauri/src/state/db.rs:362-367`; DB query at 2026-04-21T11:13Z: `SELECT provider_name, window_id, used_percent, resets_at FROM provider_quota_windows ORDER BY provider_name, window_id;` -> 6 current rows, one per current provider/window pair)

Alternatives observed:

- `provider_quota_windows`: add per-window delta columns on the existing `(provider_name, window_id)` row shape. (`src-tauri/src/state/db.rs:362-367`)
- `provider_quotas`: add provider-level storage that encodes multiple window deltas in one row because there is no `window_id` column. This is an inference from `provider_name TEXT PRIMARY KEY` on that table. (`src-tauri/src/state/db.rs:352-360`)

## 3. Q3 — Bootstrap data for new windows
When a window has never seen a refresh-to-refresh delta, what data about it exists locally? (`resets_at` gives duration — how is that computed and is it stable?)

Do any two windows of the same provider appear to have a mathematically consistent duration ratio (e.g., 5h vs 7d consistently ~33.6×)? Measure it from live DB rows.

For sibling providers in the same model pool, how do their learned `last_delta_percent / last_delta_calls` compare? Produce the table and compute per-turn drift for each.

Is there any sibling-provider aggregation function today that could be generalized? (e.g., `global_avg_percent_per_call` already sums across siblings — document exactly what it does.)

Evidence:

When a window has no refresh-to-refresh delta of its own, the locally stored data for that window is only `provider_name`, `window_id`, `used_percent`, and `resets_at`; the provider row separately holds `calls_since_refresh`, `refreshed_at`, and one provider-level `last_delta_percent` / `last_delta_calls` pair. There is no stored window-length field. (`src-tauri/src/state/db.rs:29-58`, `src-tauri/src/state/db.rs:352-367`)

The code does not store a stable duration. It recomputes time-to-reset as `w.resets_at - now` inside both the scorer and the TTL logic, so the duration-like quantity shrinks continuously as `now` advances. (`src-tauri/src/balancer/mod.rs:123-129`, `src-tauri/src/quota/mod.rs:145-159`)

Live DB observation at 2026-04-21T11:15:52Z:

```sql
SELECT w.provider_name, w.window_id, w.used_percent, w.resets_at, q.refreshed_at
FROM provider_quota_windows w
LEFT JOIN provider_quotas q USING (provider_name)
ORDER BY w.provider_name, w.window_id;
```

Computed from those rows:

| Provider | Window | `hours_until_reset` at snapshot | `resets_at - refreshed_at` ("effective hours from refresh") |
| --- | ---: | ---: | ---: |
| `claude` | 0 | `55.735265` | `56.095424` |
| `claude` | 1 | `3.735542` | `4.095702` |
| `claude3` | 0 | `55.735472` | `55.887757` |
| `claude3` | 1 | `4.735472` | `4.887757` |
| `codex` | 0 | `156.171787` | `156.871109` |
| `codex2` | 0 | `60.950676` | `73.104979` |

Measured same-provider long/short ratios from those live rows:

| Provider | Shortest hours-until-reset | Longest hours-until-reset | Ratio |
| --- | ---: | ---: | ---: |
| `claude` | `3.735542` | `55.735265` | `14.920260` |
| `claude3` | `4.735472` | `55.735472` | `11.769781` |

Those observed ratios are not a stable `168 / 5 = 33.6`; they are the current remaining times on the clock, not the full window lengths. The code that computes these values uses `resets_at - now`, not a stored lifetime. (`src-tauri/src/balancer/mod.rs:123-129`, `src-tauri/src/quota/mod.rs:145-159`)

Sibling-provider learned deltas in the current Claude and Codex pools, from a read-only DB query at 2026-04-21T11:15:52Z:

```sql
SELECT provider_name, last_delta_percent, last_delta_calls
FROM provider_quotas
ORDER BY provider_name;
```

Per-turn drift (`last_delta_percent / last_delta_calls`):

| Pool | Provider | `last_delta_percent` | `last_delta_calls` | Percent-per-turn |
| --- | --- | ---: | ---: | ---: |
| Claude | `claude` | `0.01` | `119` | `0.0000840336` |
| Claude | `claude2` | `0.06` | `2194` | `0.0000273473` |
| Claude | `claude3` | `0.01` | `80` | `0.0001250000` |
| Codex | `codex` | `0.02` | `579` | `0.0000345423` |
| Codex | `codex2` | `0.02` | `305` | `0.0000655738` |

There is one sibling-aggregation function today: `global_avg_percent_per_call()`. It iterates the pool's `QuotaRecord`s, keeps only rows where `last_delta_percent > 0` and `last_delta_calls > 0`, sums all `last_delta_percent` values into `total_percent`, sums all `last_delta_calls` into `total_calls`, and returns `total_percent / total_calls` or `0.0` when `total_calls == 0`. (`src-tauri/src/balancer/mod.rs:144-164`)

The same formula applied to the live sibling groups above gives:

| Group | Summed `total_percent` | Summed `total_calls` | `total_percent / total_calls` |
| --- | ---: | ---: | ---: |
| Claude | `0.08` | `2393` | `0.0000334308` |
| Codex | `0.04` | `884` | `0.0000452489` |

Alternatives observed:

- Bootstrap inputs already present per window: `used_percent`, `resets_at`, `window_id`. (`src-tauri/src/state/db.rs:42-58`)
- Bootstrap inputs already present per provider: `refreshed_at`, `calls_since_refresh`, and one provider-level `last_delta_percent` / `last_delta_calls`. (`src-tauri/src/state/db.rs:29-40`)
- Sibling aggregation already present: pool-wide sum of provider-level deltas through `global_avg_percent_per_call()`. (`src-tauri/src/balancer/mod.rs:144-164`)

## 4. Q4 — Sibling variance measurement
Using live DB data, compute and tabulate:

- per-provider `last_delta_percent / last_delta_calls` → percent-per-turn
- per-provider session turn rate over the last 24h, 7d (count `session_turns` by `provider_name` binned by day)
- per-provider `invocation_count` and recent error counts

Report the spread (ratio of max to min) across Claude siblings and separately across Codex siblings. Don't interpret the spread, just report it.

Is there a per-provider hint anywhere (config, TOML, DB) that would let a future algorithm know "these three providers share a plan class"? Check `providers.toml`, model TOMLs, and DB schema.

Evidence:

The live per-provider variance snapshot below was computed at 2026-04-21T11:15:52Z from these read-only queries:

```sql
SELECT provider_name, last_delta_percent, last_delta_calls FROM provider_quotas;

SELECT provider_name, COUNT(*)
FROM session_turns
WHERE role='assistant' AND timestamp > :cutoff
GROUP BY provider_name;

SELECT provider_name, substr(timestamp,1,10) AS day, COUNT(*)
FROM session_turns
WHERE role='assistant' AND timestamp > :cutoff_7d
GROUP BY provider_name, substr(timestamp,1,10)
ORDER BY provider_name, day;

SELECT provider_name, COUNT(*)
FROM invocations
GROUP BY provider_name;

SELECT provider_name, COUNT(*)
FROM invocations
WHERE success = 0 AND created_at > :cutoff_30m
GROUP BY provider_name;
```

The DB's persisted `invocation_count` lives on `providers(model_name, provider_index)`, not on `provider_quotas` or on a provider-name keyed table, because `finalize_invocation()` increments `providers.invocation_count` by `(model_name, provider_index)`. (`src-tauri/src/state/db.rs:799-889`, `src-tauri/src/state/db.rs:1015-1051`) The table below therefore uses `invocations` grouped by `provider_name` to match the prompt's per-provider form.

| Provider | Percent-per-turn | Assistant turns last 24h | Assistant turns last 7d | Invocations by `provider_name` | Recent errors last 30m |
| --- | ---: | ---: | ---: | ---: | ---: |
| `claude` | `0.0000840336` | `3661` | `32863` | `186` | `0` |
| `claude2` | `0.0000273473` | `2094` | `29885` | `137` | `0` |
| `claude3` | `0.0001250000` | `5260` | `16308` | `174` | `0` |
| `codex` | `0.0000345423` | `1100` | `4640` | `251` | `0` |
| `codex2` | `0.0000655738` | `437` | `3814` | `111` | `0` |

Day-binned assistant turns over the last 7 days from the same snapshot:

| Provider | Daily bins |
| --- | --- |
| `claude` | `2026-04-14=1137`, `2026-04-15=4329`, `2026-04-16=10068`, `2026-04-17=5605`, `2026-04-18=2604`, `2026-04-19=4553`, `2026-04-20=2728`, `2026-04-21=1839` |
| `claude2` | `2026-04-14=2596`, `2026-04-15=9250`, `2026-04-16=4655`, `2026-04-17=5572`, `2026-04-18=1003`, `2026-04-19=2664`, `2026-04-20=2906`, `2026-04-21=1239` |
| `claude3` | `2026-04-16=1774`, `2026-04-17=3934`, `2026-04-18=2085`, `2026-04-19=2154`, `2026-04-20=2894`, `2026-04-21=3467` |
| `codex` | `2026-04-14=261`, `2026-04-15=659`, `2026-04-16=680`, `2026-04-17=615`, `2026-04-18=1016`, `2026-04-19=32`, `2026-04-20=799`, `2026-04-21=578` |
| `codex2` | `2026-04-14=279`, `2026-04-15=669`, `2026-04-16=765`, `2026-04-17=452`, `2026-04-18=721`, `2026-04-19=334`, `2026-04-20=461`, `2026-04-21=133` |

Spread ratios from that same snapshot:

| Group | Metric | Min | Max | Max / Min |
| --- | --- | ---: | ---: | ---: |
| Claude | Percent-per-turn | `0.0000273473` | `0.0001250000` | `4.5708333` |
| Claude | Assistant turns last 24h | `2094` | `5260` | `2.5119389` |
| Claude | Assistant turns last 7d | `16308` | `32863` | `2.0151459` |
| Claude | Invocations by `provider_name` | `137` | `186` | `1.3576642` |
| Claude | Recent errors last 30m | `0` | `0` | undefined because min is `0` |
| Codex | Percent-per-turn | `0.0000345423` | `0.0000655738` | `1.8983607` |
| Codex | Assistant turns last 24h | `437` | `1100` | `2.5171625` |
| Codex | Assistant turns last 7d | `3814` | `4640` | `1.2165705` |
| Codex | Invocations by `provider_name` | `111` | `251` | `2.2612613` |
| Codex | Recent errors last 30m | `0` | `0` | undefined because min is `0` |

Explicit "plan class" metadata was not observed in the checked config or schema surfaces:

- `providers.toml` keys each entry by provider name and stores only `quota_script`. (`/home/nes/.config/oulipoly-agent-runner/providers.toml:1-27`, `src-tauri/src/config/providers.rs:6-23`)
- `ProviderConfig` stores `name`, `command`, `args`, `interactive_args`, `resume`, and `session_capture`; there is no plan-class field. (`src-tauri/src/config/model.rs:6-20`)
- The quota tables store provider/window state only; there is no plan-class column. (`src-tauri/src/state/db.rs:352-367`)

The only implicit sibling hints observed are co-membership in the same model TOMLs and shared quota-script family:

- `claude-opus.toml` co-lists `claude`, `claude2`, `claude3`. (`/home/nes/.config/oulipoly-agent-runner/models/claude-opus.toml:1-21`)
- `gpt-high.toml` co-lists `codex`, `codex2` by provider command. (`/home/nes/.config/oulipoly-agent-runner/models/gpt-high.toml:1-16`, `src-tauri/src/config/model.rs:22-35`, `src-tauri/src/config/model.rs:154-180`)
- `providers.toml` maps the Claude siblings to `anthropic-usage` and the Codex siblings to `chatgpt-usage`. (`/home/nes/.config/oulipoly-agent-runner/providers.toml:11-24`)

Alternatives observed:

- Explicit plan-class hint: none observed in the checked TOMLs or DB schema. (`/home/nes/.config/oulipoly-agent-runner/providers.toml:1-27`, `src-tauri/src/config/model.rs:6-20`, `src-tauri/src/state/db.rs:352-367`)
- Implicit grouping hints: same model pools and same quota-script family. (`/home/nes/.config/oulipoly-agent-runner/models/claude-opus.toml:1-21`, `/home/nes/.config/oulipoly-agent-runner/models/gpt-high.toml:1-16`, `/home/nes/.config/oulipoly-agent-runner/providers.toml:11-24`)

## 5. Q5 — In-flight projection change cost
Exactly which lines of `score_by_density` implement the current projection? (You already have this from phase 1; restate with line numbers.)

What signature change is required for the projection loop to use a per-window rate instead of a provider-level scalar? Describe the minimal diff shape (not the code — just "this function needs a `burn_rate_w` parameter per window" style description).

What tests in `src-tauri/src/balancer/mod.rs` would break under a signature change, and what would it take to adapt them? Count tests, don't rewrite them.

Evidence:

The current projection is implemented on these exact lines:

- `let avg = global_avg_percent_per_call(quotas);` loads one provider-pool scalar. (`src-tauri/src/balancer/mod.rs:94`)
- `count_assistant_turns_since(...)` computes the provider's post-refresh assistant-turn count. (`src-tauri/src/balancer/mod.rs:109-116`)
- `let projected = (w.used_percent + (turns as f64) * avg).clamp(0.0, 1.0);` applies the same scalar to every window of that provider. (`src-tauri/src/balancer/mod.rs:123`)
- `let remaining = (1.0 - projected).max(0.0);` and `let hours = ((w.resets_at - now).num_seconds() as f64) / 3600.0;` convert projected usage into a per-window density. (`src-tauri/src/balancer/mod.rs:124-129`)
- `.fold(f64::INFINITY, f64::min)` makes the provider's binding score the minimum window density. (`src-tauri/src/balancer/mod.rs:120-131`)
- The descending sort and the all-`-∞` fallback are on `scores.sort_by(...)`, `if scores.iter().all(...)`, and `scores[0].0`. (`src-tauri/src/balancer/mod.rs:135-141`)

Minimal diff shape, from the current signature and loop structure:

- `score_by_density()` currently receives `quotas` and `windows`, computes one `avg`, and applies it to every `w`. (`src-tauri/src/balancer/mod.rs:88-95`, `src-tauri/src/balancer/mod.rs:120-123`)
- To use per-window burn rates, that loop needs a rate lookup keyed per window instead of the single `avg` scalar. The additional data could arrive either as an extra argument parallel to `windows`, or by enriching the `QuotaWindow` data that `windows` already contains. This is an inference from the current function signature and loop.

Test surface in `src-tauri/src/balancer/mod.rs`:

- No unit test calls `score_by_density()` directly; every test calls `select_provider()`. (`src-tauri/src/balancer/mod.rs:273-434`)
- A private signature change confined inside `balancer/mod.rs` therefore has `0` direct test call sites in that file.
- If the redesign also changes how test fixtures seed scorer inputs, the `4` window-seeding tests are the ones that would need adaptation because they currently only seed `used_percent`/`resets_at` through `upsert_quota_refresh()`:
  - `density_scoring_picks_lowest_used_when_windows_match` (`src-tauri/src/balancer/mod.rs:319-335`)
  - `density_picks_account_with_more_time_when_used_equal` (`src-tauri/src/balancer/mod.rs:337-350`)
  - `binding_constraint_avoids_account_with_pressed_short_window` (`src-tauri/src/balancer/mod.rs:352-414`)
  - `falls_back_to_invocation_count_when_windows_missing` (`src-tauri/src/balancer/mod.rs:416-433`)

Alternatives observed:

- Keep `score_by_density()`'s argument list stable and make per-window rate part of the `windows` data it already receives.
- Add a new per-window-rate argument parallel to `windows` and index it inside the projection loop.
- Both alternatives are consistent with the current call structure; neither is implemented today. (`src-tauri/src/balancer/mod.rs:50-66`, `src-tauri/src/balancer/mod.rs:88-95`)

## 6. Q7 — Threshold volatility and sensitivity
For each provider with >= 2 refreshes in the last 7 days (reconstruct from `refreshed_at` if possible; otherwise state that history is not available), how fast does `used_percent` typically change on the long window vs short window? Produce a table.

What is the observed distribution of `resets_at - now` across all `provider_quota_windows` rows today? (i.e., at any given moment, what fraction of a window's lifetime remains?)

Do any providers expose window-lengths materially different from 5h / 168h? If the quota script emits one, how is its duration currently inferred? (It isn't stored — so the duration is always `resets_at - time_of_refresh`, which is not the same as the window size. Confirm this and quantify what "effective window size" would look like from observation.)

Evidence:

Refresh history sufficient to reconstruct ">= 2 refreshes in the last 7 days" is not available in the current DB. `provider_quotas` retains one `refreshed_at` timestamp and one provider-level delta pair per provider, and there is no quota history table. (`src-tauri/src/state/db.rs:29-40`, `src-tauri/src/state/db.rs:352-360`; DB query at 2026-04-21T11:14Z: history/audit/log/snapshot table-name query returned no rows)

Current retained refresh state:

```sql
SELECT provider_name, refreshed_at, last_delta_percent, last_delta_calls
FROM provider_quotas
ORDER BY provider_name;
```

Observed at 2026-04-21T11:15:52Z:

| Provider | Retained `refreshed_at` | Retained delta fields |
| --- | --- | --- |
| `claude` | `2026-04-21T10:54:15.992539878+00:00` | `last_delta_percent=0.01`, `last_delta_calls=119` |
| `claude2` | `2026-04-20T16:43:16.959984518+00:00` | `last_delta_percent=0.06`, `last_delta_calls=2194` |
| `claude3` | `2026-04-21T11:06:44.340597459+00:00` | `last_delta_percent=0.01`, `last_delta_calls=80` |
| `codex` | `2026-04-21T10:33:55.007183727+00:00` | `last_delta_percent=0.02`, `last_delta_calls=579` |
| `codex2` | `2026-04-20T23:06:37.075005061+00:00` | `last_delta_percent=0.02`, `last_delta_calls=305` |

That table is a current snapshot, not a refresh history, so there is no retained "typical change" series for either long or short windows in the DB today.

Observed distribution of `resets_at - now` across all current `provider_quota_windows` rows, computed at 2026-04-21T11:16:04Z:

```sql
SELECT w.provider_name, w.window_id, w.resets_at, q.refreshed_at
FROM provider_quota_windows w
LEFT JOIN provider_quotas q USING(provider_name)
ORDER BY w.provider_name, w.window_id;
```

Per row:

| Provider | Window | Hours until reset | `hours_until_reset / (resets_at - refreshed_at)` |
| --- | ---: | ---: | ---: |
| `claude` | 0 | `55.731893` | `0.993519` |
| `claude` | 1 | `3.732171` | `0.911241` |
| `claude3` | 0 | `55.732101` | `0.997215` |
| `claude3` | 1 | `4.732101` | `0.968154` |
| `codex` | 0 | `156.168415` | `0.995521` |
| `codex2` | 0 | `60.947304` | `0.833696` |

Bucketed distribution from the same snapshot:

| Bucket | Rows | Fraction of all rows |
| --- | ---: | ---: |
| `<= 6h` | `2` | `2/6` |
| `> 6h and <= 24h` | `0` | `0/6` |
| `> 24h and <= 72h` | `3` | `3/6` |
| `> 72h and <= 168h` | `1` | `1/6` |
| `> 168h` | `0` | `0/6` |

The code does not store a window length anywhere. The closest locally derivable quantity is `resets_at - refreshed_at`, because `upsert_quota_refresh()` stores `refreshed_at = now` when the script output is written, and the runtime computations later use `resets_at - now`. (`src-tauri/src/state/db.rs:1160-1217`, `src-tauri/src/balancer/mod.rs:123-129`, `src-tauri/src/quota/mod.rs:145-159`)

Observed "effective window sizes" from `resets_at - refreshed_at` at 2026-04-21T11:15:52Z:

- `claude`: long `56.095424h`, short `4.095702h`
- `claude3`: long `55.887757h`, short `4.887757h`
- `codex`: only one emitted window, `156.871109h`
- `codex2`: only one emitted window, `73.104979h`

The Codex-family script currently emits only one legacy flat window, not two windows. (`/home/nes/.local/bin/chatgpt-usage:36-46`) The two-window Codex CLI display recorded in phase 2 was:

```text
5h limit:     96% left (resets 07:27)
Weekly limit: 95% left (resets 16:26 on 27 Apr)
```

(`research/03-load-balancing-tiers-needs.md:287-304`)

Alternatives observed:

- Retained history for long/short-window change rates: not present in the DB today. (`src-tauri/src/state/db.rs:352-367`; history-table query above)
- Runtime volatility measurements available today: current `resets_at - now`, and the derivable `resets_at - refreshed_at` from current rows only. (`src-tauri/src/balancer/mod.rs:123-129`, `src-tauri/src/state/db.rs:1160-1217`)

## 7. Q8 — Exhaustion behavior today
When every provider in a pool is scored at `-∞` in `score_by_density`, what does the balancer return? Trace through `src-tauri/src/balancer/mod.rs:135-141` and the fallback.

How does the executor currently surface a quota-exhausted error to the caller? Walk the call chain from `select_provider` return → executor → user-visible output.

Are there any error categories in the `invocations.error_category` field that correspond to quota exhaustion specifically? Query the DB for distinct error categories.

Does the UI (Tauri) have any existing affordance for surfacing "pool is quota-tight"? Find it if it exists.

Evidence:

When every score is `-∞`, `score_by_density()` returns `round_robin_fallback(model, state)`. (`src-tauri/src/balancer/mod.rs:135-141`) `round_robin_fallback()` scans provider indices in ascending order, keeps the smallest `invocation_count`, and updates `best` only on strict `<`, so equal counts leave the earlier provider index in place. (`src-tauri/src/balancer/mod.rs:200-218`)

CLI call chain for a quota-exhausted subprocess today:

1. `run_with_balancing()` calls `select_provider()` and receives a provider index. (`src-tauri/src/main.rs:589-623`)
2. It passes that index into `executor::execute_with_inputs_and_env()`. (`src-tauri/src/main.rs:639-646`, `src-tauri/src/executor/mod.rs:78-95`)
3. The executor returns `ExecutionResult { stdout, stderr, exit_code, ... }` without classifying the error itself. (`src-tauri/src/executor/mod.rs:7-14`, `src-tauri/src/executor/cli.rs:29-66`, `src-tauri/src/executor/cli.rs:270-341`)
4. On non-zero exit, `run_with_balancing()` calls `run_diagnostics(stderr, exit_code, ...)`. (`src-tauri/src/main.rs:670-685`, `src-tauri/src/main.rs:717-735`)
5. `diagnostics::diagnose_error()` can classify the stderr as `quota_exhausted`, using either the LLM category list or the heuristic branch matching `"quota"`, `"billing"`, or `"usage limit"`. (`src-tauri/src/diagnostics/mod.rs:14-33`, `src-tauri/src/diagnostics/mod.rs:47-65`, `src-tauri/src/diagnostics/mod.rs:84-91`, `src-tauri/src/diagnostics/mod.rs:102-132`)
6. The category string is stored in `invocations.error_category`, stderr is printed to the terminal, and the CLI also prints `[diagnostics: <category>]`. (`src-tauri/src/main.rs:678-700`, `src-tauri/src/state/db.rs:833-852`)

The Tauri `test_model` path does not currently run diagnostics. It calls `select_provider(&model, &db, None)`, then `executor::execute(...)`, and returns `stdout`, `stderr`, and `exit_code` to the frontend. (`src-tauri/src/lib.rs:471-504`)

Live DB query at 2026-04-21T11:13Z:

```sql
SELECT COALESCE(error_category,'<NULL>') AS category, COUNT(*)
FROM invocations
GROUP BY COALESCE(error_category,'<NULL>')
ORDER BY COUNT(*) DESC, category ASC;
```

Result:

| Category | Count |
| --- | ---: |
| `<NULL>` | `845` |
| `cli_version_mismatch` | `21` |
| `unknown` | `9` |

No live `quota_exhausted` row was present in that snapshot, even though the category exists in code. (`src-tauri/src/diagnostics/mod.rs:24-33`)

No existing Tauri/Frontend affordance for "pool is quota-tight" was observed in the checked UI tree. A repo search over `src`, `src/lib`, `src/components`, and `src/views` for `refresh_quotas`, `quota_exhausted`, `QuotaRefreshEntry`, `used_percent`, and `resets_at` returned no matches at 2026-04-21T11:18Z. The only quota-related Tauri command found is `refresh_quotas`, which returns per-provider statuses `fresh`, `updated`, `no_script`, `in_flight`, or `failed`, plus raw windows. (`src-tauri/src/lib.rs:304-390`)

Alternatives observed:

- CLI surfacing path: stderr plus `[diagnostics: quota_exhausted]` when diagnostics classifies the failure that way. (`src-tauri/src/main.rs:694-700`, `src-tauri/src/diagnostics/mod.rs:24-33`)
- Tauri `test_model` path: raw `stderr` / `exit_code` only, no quota-specific diagnostic tag. (`src-tauri/src/lib.rs:471-504`)

## 8. Q9 — Fallback-mode tie-break under the redesign
In the current code, when one provider has zero windows and the pool drops to invocation-count fallback, how is the tie broken when counts are equal? (Cite the sort order and any tiebreaker.)

The §5.1 fix will eliminate the "zero windows due to staleness" case. Are there any other current code paths that would still leave a provider window-less? Enumerate them from the schema/write paths.

Evidence:

When the pool drops to invocation-count fallback, `score_by_invocation_count()` builds scores in provider-index order, sorts ascending by the numeric score only, and has no explicit secondary compare key. (`src-tauri/src/balancer/mod.rs:167-197`) The unit test covering "one provider has windows, one does not" documents the equal-count case and expects index `0` when both invocation counts are `0`. (`src-tauri/src/balancer/mod.rs:416-433`)

Current code paths that can still leave a provider with zero windows, independent of the `is_stale` empty-window TTL issue:

1. Provider never refreshed, so `get_windows()` returns an empty vec. (`src-tauri/src/state/db.rs:1109-1145`)
2. `increment_calls_since_refresh()` inserts a `provider_quotas` row with only `calls_since_refresh`, creating provider metadata without any `provider_quota_windows` row. (`src-tauri/src/state/db.rs:1262-1275`)
3. A quota script returns `{"windows":[]}`. `parse_output()` accepts `Some(ws)` even when `ws` is empty, and `refresh_provider()` passes that empty vec to `upsert_quota_refresh()`. (`src-tauri/src/quota/mod.rs:118-127`, `src-tauri/src/quota/mod.rs:222-265`)
4. `upsert_quota_refresh()` then deletes all prior `provider_quota_windows` rows for that provider and inserts none because the `for (i, w) in windows.iter().enumerate()` loop is empty. (`src-tauri/src/state/db.rs:1219-1238`)
5. `refresh_provider()` returns `NoScript`, `AlreadyInFlight`, or `Failed`, which leaves existing window state untouched; if the provider already had zero windows, it stays window-less. (`src-tauri/src/quota/mod.rs:86-98`, `src-tauri/src/quota/mod.rs:107-126`)

Alternatives observed:

- Tie handling in invocation-count mode: no explicit tiebreaker beyond the current order implied by score construction and sort behavior; the test expects index `0` in the equal-count missing-window case. (`src-tauri/src/balancer/mod.rs:167-197`, `src-tauri/src/balancer/mod.rs:416-433`)
- Window-less states today: never refreshed, quota row created by `increment_calls_since_refresh()`, empty-window refresh write, or refresh paths that do not write windows at all. (`src-tauri/src/state/db.rs:1109-1145`, `src-tauri/src/state/db.rs:1219-1238`, `src-tauri/src/state/db.rs:1262-1275`, `src-tauri/src/quota/mod.rs:86-127`, `src-tauri/src/quota/mod.rs:222-265`)

## 9. Q10 — Prerequisite coupling
Files touched by the `is_stale` empty-windows fix (§5.1). List them.

Files touched by the scoring redesign (axes A + B + C). List them based on where the current scoring / projection / call-site code lives.

Files touched by a `chatgpt-usage` second-window fix. This script is external (likely in `~/.local/bin/` or similar); locate it and cite its path. Do it fix it, just find it and describe its current output shape vs the two-window shape Codex reports via `codex` CLI.

Which files are shared across the three fixes? Compute the intersection explicitly.

Evidence:

Files that contain the current empty-window staleness logic are in `src-tauri/src/quota/mod.rs`, specifically `is_stale()` and `dynamic_ttl_secs()`. The unit tests for that TTL logic are in the same file. (`src-tauri/src/quota/mod.rs:129-159`, `src-tauri/src/quota/mod.rs:275-349`)

Files that contain the current scoring/projection/call-site surfaces for axes A/B/C are:

- `src-tauri/src/balancer/mod.rs` for `select_provider()`, `score_by_density()`, `global_avg_percent_per_call()`, and fallback behavior. (`src-tauri/src/balancer/mod.rs:22-218`)
- `src-tauri/src/state/db.rs` for quota schema, per-provider delta storage, window reads/writes, and assistant-turn counting. (`src-tauri/src/state/db.rs:29-58`, `src-tauri/src/state/db.rs:352-367`, `src-tauri/src/state/db.rs:1077-1242`, `src-tauri/src/state/db.rs:1809-1837`)
- `src-tauri/src/quota/mod.rs` for the quota refresh parse/write path that currently learns only one provider-level delta. (`src-tauri/src/quota/mod.rs:65-84`, `src-tauri/src/quota/mod.rs:100-127`, `src-tauri/src/quota/mod.rs:222-265`)
- `src-tauri/src/main.rs` for the CLI one-shot call site that invokes balancing and would be the current place to label caller class. (`src-tauri/src/main.rs:589-703`)
- `src-tauri/src/lib.rs` for the Tauri `test_model` path and quota-refresh command surface. (`src-tauri/src/lib.rs:304-390`, `src-tauri/src/lib.rs:471-504`)

The installed external `chatgpt-usage` script is at `/home/nes/.local/bin/chatgpt-usage`. A file search at 2026-04-21T11:18Z found only that path. (`rg --files -g 'chatgpt-usage' /home/nes/projects/agent-runner /home/nes/.local/bin` -> `/home/nes/.local/bin/chatgpt-usage`)

Its current output shape is a legacy flat single window:

```json
{"used_percent": ..., "resets_at": ...}
```

(`/home/nes/.local/bin/chatgpt-usage:10-12`, `/home/nes/.local/bin/chatgpt-usage:40-46`)

The two-window Codex CLI shape recorded in phase 2 was:

```text
5h limit:     96% left (resets 07:27)
Weekly limit: 95% left (resets 16:26 on 27 Apr)
```

(`research/03-load-balancing-tiers-needs.md:287-304`)

Set intersection from the file lists above:

- `is_stale` fix set: `{src-tauri/src/quota/mod.rs}`
- Scoring redesign set: `{src-tauri/src/balancer/mod.rs, src-tauri/src/state/db.rs, src-tauri/src/quota/mod.rs, src-tauri/src/main.rs, src-tauri/src/lib.rs}`
- `chatgpt-usage` second-window fix set: `{/home/nes/.local/bin/chatgpt-usage}`

Triple intersection: `∅` (empty set).

Alternatives observed:

- Pairwise overlap exists between the first two sets through `src-tauri/src/quota/mod.rs`. (`src-tauri/src/quota/mod.rs:100-159`, `src-tauri/src/quota/mod.rs:222-265`)
- No file was observed in all three sets at once. The external script path is disjoint from the Rust quota/scoring modules. (file list above)

## 10. Q11 — Empty-write failure modes
Under what conditions can `upsert_quota_refresh` be called with an empty windows array today? Trace all call sites and the `parse_output` code path.

Does `parse_output` reject or accept `{"windows": []}`? Test by inspection.

Does `anthropic-usage` or `chatgpt-usage` ever emit an empty windows array under any documented / observable failure mode? If you can run them with invalid credentials or unreachable endpoints safely (without modifying real credentials), document the output shape on failure. Do NOT modify any `.credentials.json` or `auth.json` file.

What does the SQL write path do when the input array is empty? Cite the relevant `DELETE` / `INSERT` statements in `upsert_quota_refresh`.

Evidence:

The only production call site for `upsert_quota_refresh()` is `refresh_provider()` after a successful `run_script()` / `parse_output()` result. (`src-tauri/src/quota/mod.rs:118-127`; repo search at 2026-04-21T11:19Z: `rg -n "upsert_quota_refresh\\(" /home/nes/projects/agent-runner -g '!target'` -> production hit in `src-tauri/src/quota/mod.rs:120`, plus unit-test seeding calls in `src-tauri/src/balancer/mod.rs:327-423`)

`parse_output()` accepts `{"windows":[]}` by inspection. When `parsed.windows` is `Some(ws)`, it returns that vec directly into `raw_windows`, and the normalization loop over `raw_windows` simply runs zero times and returns `Ok(out)` with `out.len() == 0`. There is no explicit empty-array rejection. (`src-tauri/src/quota/mod.rs:222-265`)

Observable failure modes for the installed scripts, run without touching real credential files:

1. Missing file path:

```text
$ /home/nes/.local/bin/anthropic-usage /tmp/does-not-exist
exit 2
stdout: <empty>
stderr: credentials file not readable: /tmp/does-not-exist

$ /home/nes/.local/bin/chatgpt-usage /tmp/does-not-exist
exit 2
stdout: <empty>
stderr: auth file not readable: /tmp/does-not-exist
```

2. Readable JSON file missing the expected token fields:

```text
$ /home/nes/.local/bin/anthropic-usage <tempfile containing {}>
exit 3
stdout: <empty>
stderr: no claudeAiOauth.accessToken in <tempfile>

$ /home/nes/.local/bin/chatgpt-usage <tempfile containing {}>
exit 3
stdout: <empty>
stderr: missing tokens.access_token or tokens.account_id in <tempfile>
```

Those runs were captured at 2026-04-21T11:18Z with temporary files under `/tmp`. In both observed failure modes, neither script emitted JSON at all.

Script code paths:

- `anthropic-usage` exits `2` on unreadable file and `3` on missing token before any JSON is emitted. (`/home/nes/.local/bin/anthropic-usage:23-34`)
- `chatgpt-usage` exits `2` on unreadable file and `3` on missing token/account id before any JSON is emitted. (`/home/nes/.local/bin/chatgpt-usage:17-29`)

On success-shape construction:

- `anthropic-usage` emits `{"windows":[... ]}` and uses `if ... else empty end` for each of the two entries, so an API response with neither `seven_day.resets_at` nor `five_hour.resets_at` would produce `{"windows":[]}`. That is observable from the script body; it was not one of the failure runs above. (`/home/nes/.local/bin/anthropic-usage:41-54`)
- `chatgpt-usage` never emits a `windows` array. It emits legacy flat `{used_percent, resets_at}` JSON from `secondary_window`, and if `reset_at` is absent it would emit `resets_at: null`, which the legacy branch in `parse_output()` rejects because it requires `resets_at` to be present. (`/home/nes/.local/bin/chatgpt-usage:36-46`, `src-tauri/src/quota/mod.rs:229-245`)

When `upsert_quota_refresh()` receives an empty input array:

- `longest_new` is `None`, so the provider-level legacy mirror becomes `(legacy_used = 0.0, legacy_resets = None)`. (`src-tauri/src/state/db.rs:1162-1189`)
- It still upserts `provider_quotas` with `calls_since_refresh = 0` and `refreshed_at = now`. (`src-tauri/src/state/db.rs:1196-1217`)
- It deletes all existing window rows for that provider via `DELETE FROM provider_quota_windows WHERE provider_name = ?1`. (`src-tauri/src/state/db.rs:1219-1223`)
- The insert loop runs zero times, so no new `provider_quota_windows` rows are written. (`src-tauri/src/state/db.rs:1225-1238`)

Alternatives observed:

- Empty-array production path today: only through `refresh_provider()` if `parse_output()` returns `Ok(vec![])`. (`src-tauri/src/quota/mod.rs:118-127`, `src-tauri/src/quota/mod.rs:222-265`)
- Observed documented script failures: non-zero exit plus stderr, with empty stdout, not an empty windows array. (`/home/nes/.local/bin/anthropic-usage:23-34`, `/home/nes/.local/bin/chatgpt-usage:17-29`; `/tmp` probe outputs above)
- Script body that can synthesize `{"windows":[]}`: `anthropic-usage` success-shape jq program when both entries are missing. (`/home/nes/.local/bin/anthropic-usage:45-54`)
