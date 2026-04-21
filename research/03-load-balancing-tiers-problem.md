# Quota-Tier Load Balancing: Problem Research

This document describes the current load-balancing behavior for
Initiative 03 without selecting a design. It is grounded in the current
repo, local config on disk, and read-only observations from
`~/.local/share/oulipoly-agent-runner/state.db` captured on
2026-04-21T10:40:16Z.

## 1. Current selection flow

`select_provider()` has two phases before it scores anything. When the
caller passes a `BalanceContext`, it first refreshes any stale provider
quota and scans each provider's session logs so direct CLI activity can
affect the next pick (`src-tauri/src/balancer/mod.rs:22-47`). It then
loads one `QuotaRecord` plus zero or more `QuotaWindow` rows per
provider (`src-tauri/src/balancer/mod.rs:50-60`).

The scoring mode is binary at the pool level:

| Pool state | Current scorer |
| --- | --- |
| Every provider in the model has at least one quota window | `score_by_density()` (`src-tauri/src/balancer/mod.rs:62-66`) |
| At least one provider in the model has zero quota windows | Invocation-count fallback (`src-tauri/src/balancer/mod.rs:62-69`, `167-198`) |

That gate matters in the current local state. `claude-opus.toml` defines
three providers: `claude`, `claude2`, and `claude3`
(`~/.config/oulipoly-agent-runner/models/claude-opus.toml:1-14`). The
live DB currently has quota windows for `claude` and `claude3`, but not
for `claude2`:

| Provider | `provider_quotas` row present | `provider_quota_windows` rows present |
| --- | --- | --- |
| `claude` | Yes | 2 |
| `claude2` | Yes | 0 |
| `claude3` | Yes | 2 |

Because `claude2` currently has zero windows, the local `claude-opus`
pool does not currently reach `score_by_density()` on this machine; it
falls back to invocation-count scoring instead
(`src-tauri/src/balancer/mod.rs:62-69`).

## 2. Quota schema and current data flow

### 2.1 Stored quota fields

`provider_quotas` stores provider-level metadata, not the full
multi-window state. The code creates these columns
(`src-tauri/src/state/db.rs:352-360`):

| Column | Type | Current meaning |
| --- | --- | --- |
| `provider_name` | `TEXT PRIMARY KEY` | Provider/account key used across every model routed through that account (`src-tauri/src/state/db.rs:24-40`) |
| `used_percent` | `REAL` | Legacy mirror of the longest window's percent (`src-tauri/src/state/db.rs:1184-1217`) |
| `resets_at` | `TEXT` | Legacy mirror of the longest window's reset timestamp (`src-tauri/src/state/db.rs:1184-1217`) |
| `calls_since_refresh` | `INTEGER` | Count of runner invocations since last refresh (`src-tauri/src/state/db.rs:31-33`, `1262-1275`) |
| `refreshed_at` | `TEXT` | Time the last quota refresh was written (`src-tauri/src/state/db.rs:33`, `1197-1217`) |
| `last_delta_percent` | `REAL` | Most recent positive change in the longest window's `used_percent` (`src-tauri/src/state/db.rs:34-36`, `1148-1182`) |
| `last_delta_calls` | `INTEGER` | Assistant-turn count paired with `last_delta_percent` (`src-tauri/src/state/db.rs:37-39`, `1168-1182`) |

`provider_quota_windows` stores the actual window set
(`src-tauri/src/state/db.rs:362-367`):

| Column | Type | Current meaning |
| --- | --- | --- |
| `provider_name` | `TEXT` | Same provider/account key as `provider_quotas` |
| `window_id` | `INTEGER` | Stable per-provider position index (`src-tauri/src/state/db.rs:42-45`) |
| `used_percent` | `REAL` | `0..1` ratio consumed for this window (`src-tauri/src/state/db.rs:49-51`) |
| `resets_at` | `TEXT` | Absolute RFC3339 reset timestamp for this window |

The schema does not store raw budget size, remaining-call count, or any
per-window learned delta. The stored quota shape is percent plus reset
time per window, plus one provider-level delta pair
(`src-tauri/src/state/db.rs:24-58`, `352-367`).

### 2.2 Refresh flow

Quota refresh has two entry points today:

1. `select_provider()` refreshes stale providers synchronously when it
   is called with a `BalanceContext` (`src-tauri/src/balancer/mod.rs:32-47`).
2. The Tauri `refresh_quotas` command refreshes every provider that
   appears in at least one multi-provider model
   (`src-tauri/src/lib.rs:304-389`).

In the current local config, Claude-family providers use
`anthropic-usage ...` quota scripts and Codex-family providers use
`chatgpt-usage ...` quota scripts
(`~/.config/oulipoly-agent-runner/providers.toml:11-27`).

Staleness is dynamic. `is_stale()` reads the provider's windows, takes
the minimum time until reset, divides by `5`, and clamps the result to
`[5 minutes, 24 hours]` (`src-tauri/src/quota/mod.rs:13-20`,
`129-159`). If the provider has no quota row or no `refreshed_at`, it is
stale immediately (`src-tauri/src/quota/mod.rs:129-143`).

The quota script contract is JSON on stdout. The parser accepts either:

- `{"windows":[{"used_percent":...,"resets_at":"..."}]}` for
  multi-window output, or
- legacy flat `{"used_percent":...,"resets_at":"..."}` output

and normalizes `used_percent` into `0..1`
(`src-tauri/src/quota/mod.rs:65-84`, `222-265`). `refresh_provider()`
passes the parsed windows to `upsert_quota_refresh()`
(`src-tauri/src/quota/mod.rs:100-127`).

`upsert_quota_refresh()` writes all new windows, deletes any old windows
that were not re-reported, resets `calls_since_refresh` to `0`, and
stores one learned delta pair from the longest window only
(`src-tauri/src/state/db.rs:1148-1242`).

### 2.3 Turn ingestion flow

Session scans are separate from quota refresh. `scan_provider()` runs a
provider-specific `turn_script`, parses one JSON object per line, and
bulk-ingests those turns into `session_turns`
(`src-tauri/src/sessions/mod.rs:53-127`). Each stored session turn has
`provider_name`, `session_id`, `turn_id`, `timestamp`, `role`,
`parent_turn_id`, `is_sidechain`, `source_file`, and `ingested_at`
(`src-tauri/src/state/db.rs:61-88`, `454-465`).

In the current local config, Claude-family providers use
`claude-code-turns ...` session adapters and Codex-family providers use
`codex-turns ...` adapters
(`~/.config/oulipoly-agent-runner/sessions.toml:15-28`).

Only assistant turns count toward the balancer's projection.
`count_assistant_turns_since()` counts `role = 'assistant'` rows for one
provider after a given timestamp (`src-tauri/src/state/db.rs:1809-1837`).

### 2.4 Projection path

The current end-to-end data path is:

```text
select_provider
  -> is_stale? -> refresh_provider -> quota_script JSON -> upsert_quota_refresh
  -> scan_provider -> turn_script JSONL -> ingest_session_turns_batch
  -> get_quota + get_windows + count_assistant_turns_since
  -> global_avg_percent_per_call
  -> score_by_density
```

The refresh half is implemented in `balancer/mod.rs`, `quota/mod.rs`,
and `state/db.rs` (`src-tauri/src/balancer/mod.rs:32-65`,
`src-tauri/src/quota/mod.rs:100-159`,
`src-tauri/src/state/db.rs:1148-1242`). The turn-ingestion half is in
`balancer/mod.rs`, `sessions/mod.rs`, and `state/db.rs`
(`src-tauri/src/balancer/mod.rs:36-47`, `109-116`,
`src-tauri/src/sessions/mod.rs:53-127`,
`src-tauri/src/state/db.rs:1691-1741`, `1809-1837`).

## 3. Observed local state

Read-only queries against `~/.local/share/oulipoly-agent-runner/state.db`
at 2026-04-21T10:40:16Z returned:

| Table | Row count |
| --- | --- |
| `provider_quotas` | 7 |
| `provider_quota_windows` | 6 |
| `session_turns` | 688,856 |
| `invocations` | 863 |

The current quota rows were:

| Provider | `calls_since_refresh` | `refreshed_at` | `last_delta_percent` | `last_delta_calls` |
| --- | ---: | --- | ---: | ---: |
| `claude` | 4 | `2026-04-21T10:03:30.437192527+00:00` | `0.01` | `22` |
| `claude2` | 12 | `2026-04-20T16:43:16.959984518+00:00` | `0.06` | `2194` |
| `claude3` | 0 | `2026-04-21T10:32:56.601658919+00:00` | `0.01` | `80` |
| `codex` | 0 | `2026-04-21T10:33:55.007183727+00:00` | `0.02` | `579` |
| `codex2` | 7 | `2026-04-20T23:06:37.075005061+00:00` | `0.02` | `305` |
| `droid` | 11 | `NULL` | `NULL` | `NULL` |
| `gemini` | 5 | `NULL` | `NULL` | `NULL` |

The current window rows were:

| Provider | `window_id` | `used_percent` | `resets_at` |
| --- | ---: | ---: | --- |
| `claude` | 0 | `0.77` | `2026-04-23T18:59:59.908949+00:00` |
| `claude` | 1 | `0.00` | `2026-04-21T15:00:00.908921+00:00` |
| `claude3` | 0 | `0.59` | `2026-04-23T19:00:00.549940+00:00` |
| `claude3` | 1 | `0.45` | `2026-04-21T11:00:00.549925+00:00` |
| `codex` | 0 | `0.05` | `2026-04-27T23:26:11+00:00` |
| `codex2` | 0 | `0.11` | `2026-04-24T00:12:55+00:00` |

Those observations match the code's intended storage shape: Claude
accounts currently expose two windows locally, Codex accounts currently
expose one window locally, and some providers have quota metadata rows
without any windows at all.

## 4. Axis A: tier quantities are not weighted

### 4.1 Current mechanism

`score_by_density()` does not aggregate windows by a weighted sum or by
an explicit parent/child relationship. For each provider it:

1. Computes one pool-wide `avg` scalar with
   `global_avg_percent_per_call()` (`src-tauri/src/balancer/mod.rs:94`,
   `144-164`).
2. Counts assistant turns since the provider's `refreshed_at`
   (`src-tauri/src/balancer/mod.rs:109-116`).
3. For each window, projects
   `projected_used = used_percent + turns * avg`, clamps to `0..1`,
   computes `remaining = 1 - projected_used`, converts
   `resets_at - now` to hours, floors that at `1/60`, and divides
   `remaining / hours` (`src-tauri/src/balancer/mod.rs:120-130`).
4. Takes the provider's binding score as the minimum window density
   (`src-tauri/src/balancer/mod.rs:120-132`).
5. Picks the provider with the highest binding score
   (`src-tauri/src/balancer/mod.rs:135-141`).

The units in that formula are:

| Quantity | Current unit |
| --- | --- |
| `used_percent` | Fraction of one window budget, `0..1` (`src-tauri/src/state/db.rs:49-51`, `src-tauri/src/quota/mod.rs:253-258`) |
| `avg` | Fractional percent drift per assistant turn (`src-tauri/src/balancer/mod.rs:144-164`) |
| `turns` | Count of ingested assistant turns since refresh (`src-tauri/src/balancer/mod.rs:109-116`, `src-tauri/src/state/db.rs:1809-1837`) |
| `hours` | Hours until that specific window resets (`src-tauri/src/balancer/mod.rs:125-128`) |
| Window score | Remaining fraction per hour (`src-tauri/src/balancer/mod.rs:124-130`) |

There is no stored relationship that marks one window as "a slice of"
another. Windows are independent rows identified only by
`(provider_name, window_id)` (`src-tauri/src/state/db.rs:42-52`,
`362-367`), and the scorer iterates those rows independently before
taking `min()` (`src-tauri/src/balancer/mod.rs:120-131`).

### 4.2 Worked example from the reported symptom

Using the current formula exactly, with a 7-day window modeled as
`168h` and a 5-hour window modeled as `5h`:

| Provider | Window | Used | Remaining | Hours | Density |
| --- | --- | ---: | ---: | ---: | ---: |
| A | weekly | `0.80` | `0.20` | `168` | `0.20 / 168 = 0.00119` |
| A | 5h | `0.04` | `0.96` | `5` | `0.96 / 5 = 0.19200` |
| B | weekly | `0.10` | `0.90` | `168` | `0.90 / 168 = 0.00536` |
| B | 5h | `0.85` | `0.15` | `5` | `0.15 / 5 = 0.03000` |

Binding scores are the minimum per provider, so:

- A binds at `min(0.00119, 0.19200) = 0.00119`
- B binds at `min(0.00536, 0.03000) = 0.00536`

Under the current code, B wins because `0.00536 > 0.00119`
(`src-tauri/src/balancer/mod.rs:120-141`).

If B's 5-hour window were only 30 minutes from reset instead of 5 hours,
its short-window density would increase to `0.15 / 0.5 = 0.30`, so the
binding score would still be the weekly window at `0.00536`. That
behavior comes from the existing `remaining / hours` normalization, not
from any explicit weighting between long and short tiers
(`src-tauri/src/balancer/mod.rs:124-130`).

The balancer tests already contain one similar counterintuitive case. The
`binding_constraint_avoids_account_with_pressed_short_window` test shows
that a provider with a `95%`-used short window can still win if its long
window's density remains higher than the competing provider's long-window
density (`src-tauri/src/balancer/mod.rs:352-413`).

### 4.3 Information available to a future phase

The current system has:

- Per-window `used_percent` and `resets_at`
  (`src-tauri/src/state/db.rs:46-58`, `362-367`)
- Provider-level learned drift on the longest window only
  (`src-tauri/src/state/db.rs:34-39`, `1148-1182`)

The current system does not store:

- Absolute quota sizes per window
- Any explicit relationship between windows
- Any flag that one window is a sub-budget of another

Those absences are schema observations, not proposal points
(`src-tauri/src/state/db.rs:24-58`, `352-367`).

## 5. Axis B: turns accumulate equally across tiers

### 5.1 Where `avg_percent_per_turn` comes from today

The learned drift scalar is pool-wide and provider-level:

- `global_avg_percent_per_call()` sums every provider's
  `last_delta_percent` and `last_delta_calls`, then divides the totals
  (`src-tauri/src/balancer/mod.rs:144-164`).
- Those fields live on `provider_quotas`, not on
  `provider_quota_windows` (`src-tauri/src/state/db.rs:29-40`,
  `352-367`).
- `upsert_quota_refresh()` computes the delta against the longest window
  only, then stores that one pair back onto `provider_quotas`
  (`src-tauri/src/state/db.rs:1148-1182`, `1197-1217`).

That means the learned delta is not stored per window. One provider gets
one `last_delta_percent` and one `last_delta_calls`, regardless of how
many quota windows it exposes.

### 5.2 Where turns come from today

The projection term in `score_by_density()` is:

`turns_since_refresh * global_avg_percent_per_call`

implemented as:

- `turns = count_assistant_turns_since(provider_name, refreshed_at)`
  (`src-tauri/src/balancer/mod.rs:109-116`)
- `projected = used_percent + turns * avg`
  (`src-tauri/src/balancer/mod.rs:123`)

Those assistant turns come from `session_turns`, not directly from
`provider_quotas.calls_since_refresh`
(`src-tauri/src/state/db.rs:1809-1837`). `select_provider()` tries to
make that count current by calling `scan_provider()` for every provider
before scoring (`src-tauri/src/balancer/mod.rs:36-47`), and
`scan_provider()` ingests the provider-specific turn script into
`session_turns` (`src-tauri/src/sessions/mod.rs:53-127`).

`calls_since_refresh` is still written today, but in a narrower role:

- one-shot CLI execution increments it after the invocation finishes
  (`src-tauri/src/main.rs:688-692`)
- `increment_calls_since_refresh()` only touches
  `provider_quotas.calls_since_refresh`
  (`src-tauri/src/state/db.rs:1262-1275`)
- `upsert_quota_refresh()` only uses it as a fallback when
  `count_assistant_turns_since()` errors while computing the next learned
  delta (`src-tauri/src/state/db.rs:1168-1178`)

### 5.3 Worked example using current local Claude and Codex rows

Read-only DB rows at 2026-04-21T10:40:16Z showed:

- Claude-family deltas: `0.01/22` for `claude`, `0.06/2194` for
  `claude2`, and `0.01/80` for `claude3`
- Codex-family deltas: `0.02/579` for `codex` and `0.02/305` for
  `codex2`
- `claude3` windows: `0.59` on the long window and `0.45` on the short
  window
- `codex` window: `0.05`

For the Claude pool, the current global average is:

`(0.01 + 0.06 + 0.01) / (22 + 2194 + 80) = 0.08 / 2296 = 0.00003484`

For the Codex pool, the current global average is:

`(0.02 + 0.02) / (579 + 305) = 0.04 / 884 = 0.00004525`

Under the current projection loop, `100` additional assistant turns would
change the stored percentages like this:

| Provider/window | Current `used_percent` | Pool avg | Added by `100 * avg` | Projected |
| --- | ---: | ---: | ---: | ---: |
| `claude3` long window | `0.59` | `0.00003484` | `0.003484` | `0.593484` |
| `claude3` short window | `0.45` | `0.00003484` | `0.003484` | `0.453484` |
| `codex` only window | `0.05` | `0.00004525` | `0.004525` | `0.054525` |

The important current-state fact is that both Claude windows receive the
same added percentage. The schema does not provide any per-window drift
field that would let the short and long windows grow at different rates
between refreshes (`src-tauri/src/state/db.rs:24-58`, `352-367`), and
the projection loop does not branch by window type
(`src-tauri/src/balancer/mod.rs:120-130`).

On this machine, that worked Claude projection describes the
`score_by_density()` function itself, but the live `claude-opus` pool
would currently stay on invocation-count fallback because `claude2` has
no window rows (Sections 1 and 3; `src-tauri/src/balancer/mod.rs:62-69`).

### 5.4 Expected behavior under the current code

When one tier is much smaller than another in raw capacity, the current
code has no place to represent that difference between refreshes. The
projection step only sees:

- current `used_percent`
- current `resets_at`
- one provider-level `avg`
- one assistant-turn count

(`src-tauri/src/balancer/mod.rs:94-130`,
`src-tauri/src/state/db.rs:24-58`, `352-367`). The raw size of each
window is not stored anywhere in the current quota schema, so the code
cannot make one unseen assistant turn count as different fractions of two
different windows.

## 6. Axis C: no risk-class awareness for user vs background traffic

### 6.1 `select_provider()` call sites

The current repo paths that call `select_provider()` are:

| Caller | Path | Refresh/session context passed? | Metadata available before selection | User-vs-background distinction present? |
| --- | --- | --- | --- | --- |
| One-shot CLI execution | `src-tauri/src/main.rs:589-645` | Yes, `Some(&ctx)` | `model`, `prompt`, `working_dir`, `extra_inputs`, optional `parent_invocation_id` resolved from env (`src-tauri/src/main.rs:618-633`) | No explicit field |
| Interactive CLI `repl` without `--resume` | `src-tauri/src/main.rs:430-526` | Yes, `Some(&ctx)` | `model`, `working_dir`, optional `parent_invocation_id` (`src-tauri/src/main.rs:454-460`) | No explicit field |
| Tauri `test_model` command | `src-tauri/src/lib.rs:471-494` | No, `None` | `model` only; fixed prompt added after selection (`src-tauri/src/lib.rs:490-494`) | No explicit field |

There is also one interactive path where the balancer is not called at
all: `repl --resume <session_id>`. In that case the code resolves the
provider by `session_id` lookup in `session_turns`, validates that the
selected model contains that provider, and then launches the interactive
resume path directly (`src-tauri/src/main.rs:462-523`).

### 6.2 Metadata that actually flows today

The executor interfaces do not accept any caller-risk or traffic-class
field. The one-shot path accepts:

- `model`
- `provider_index`
- `prompt`
- `working_dir`
- `extra_inputs`
- optional `parent_invocation_env`

(`src-tauri/src/executor/mod.rs:43-94`, `src-tauri/src/executor/cli.rs:241-341`).

The interactive path accepts:

- `provider`
- `working_dir`
- optional `parent_invocation_env`
- optional `ResumePayload { session_id, strategy }`

(`src-tauri/src/executor/cli.rs:236-239`, `344-404`).

The persisted invocation row likewise has no caller-class column. The
invocation schema is:

- `invocation_uuid`
- `model_name`
- `provider_name`
- `provider_index`
- `parent_invocation_id`
- `status`
- `success`
- `exit_code`
- `error_category`
- `session_id`
- `session_capture_method`
- `created_at`
- `finished_at`

(`src-tauri/src/state/db.rs:564-590`, `768-916`).

The only "source" field in the active invocation flow is
`CompositeInvocationId.source`, and that field contains the selected
provider name, not a request type or risk class
(`src-tauri/src/state/db.rs:176-198`,
`src-tauri/src/main.rs:624-636`, `706-714`).

### 6.3 What current fields can and cannot express

Current fields that are present:

- `parent_invocation_id` for invocation lineage
  (`src-tauri/src/state/db.rs:123-129`, `564-590`, `768-795`)
- `session_id` for resumed or captured sessions
  (`src-tauri/src/state/db.rs:116-118`, `891-916`)
- `session_capture_method` for how a session id was obtained
  (`src-tauri/src/executor/mod.rs:16-38`,
  `src-tauri/src/state/db.rs:116-118`, `891-916`)
- `session_turns.source_file` for where a turn was ingested from
  (`src-tauri/src/state/db.rs:74`, `454-465`)

What is not present in the current balancer input, executor input, or
invocation schema:

- a "user prompt" / "background workflow" tag
- a risk threshold class
- a persisted field that differentiates interactive traffic from
  background traffic before provider selection

That absence is visible both in the `select_provider()` signature, which
accepts only `(model, state, ctx)` (`src-tauri/src/balancer/mod.rs:22-25`),
and in the invocation schema above.

### 6.4 Caller-specific availability differences

The three call sites also differ in what quota/session corpus they can
see:

- CLI one-shot and CLI interactive both open the default DB under
  `dirs::data_dir()/oulipoly-agent-runner/state.db`
  (`src-tauri/src/state/db.rs:475-480`,
  `src-tauri/src/main.rs:435`, `596-622`).
- Tauri commands derive `state.db` from `models_dir.parent()`
  (`src-tauri/src/lib.rs:113-117`, `333-339`, `484-493`, `509-515`).

On this machine, `~/.config/oulipoly-agent-runner/state.db` currently
contains only `memory_*` and `setup_*` tables, while
`~/.local/share/oulipoly-agent-runner/state.db` contains the quota,
provider, invocation, and session tables listed in Section 3. That is a
current data-availability split between the CLI call sites and the Tauri
`test_model` call site.

## 7. Open questions for phase 2

1. Should phase 2 treat the CLI DB path and the Tauri DB path as one
   intended logical store or as two separate current-state stores? The
   current code uses both paths, and the local files differ.
2. For providers that currently expose only one reported window locally
   (for example `codex` and `codex2`), is that because the upstream CLI
   only has one quota window, or because the configured `quota_script`
   emits only one window?
3. Should local DB observations from this machine be treated as
   representative behavior or as illustrative samples only? The code path
   is fixed, but the observed windows and deltas are machine-specific.
4. When the user says "interactive UI prompts" versus "background
   workflow turns," are those the only two traffic categories that phase
   2 needs to reason about, or are there more caller classes in actual
   use?
5. Does phase 2 need to reason about pools where one provider has no
   quota windows and therefore disables density scoring for the whole
   model, or should that fallback path stay out of scope for this
   initiative?

## 8. Non-goals inferred from the current problem statement

- This initiative is not a quota-scraper refactor. The current research
  scope is the balancer's use of the scraper output, not the mechanics of
  replacing `quota_script` execution (`src-tauri/src/quota/mod.rs:100-127`).
- It is not a session-ingestion storage redesign. The current scope uses
  the existing `turn_script` -> `session_turns` path as the source of
  assistant-turn counts (`src-tauri/src/sessions/mod.rs:53-127`,
  `src-tauri/src/state/db.rs:1691-1741`, `1809-1837`).
- It is not an interactive resume redesign. `repl --resume` already
  bypasses the balancer and resolves the provider by session ownership
  (`src-tauri/src/main.rs:462-523`).
- It is not a new invocation-tracing system. The current invocation
  schema already records lineage and session-capture fields, and this
  research only describes what those fields do today
  (`src-tauri/src/state/db.rs:564-590`, `768-916`).
