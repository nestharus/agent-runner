# Quota-Tier Load Balancing: Design-Question Answers

Orchestrator (`claude-opus`, this session) synthesis of the 11 design
questions in `research/03-load-balancing-tiers-needs.md` §6 using
evidence from:

- `research/03-load-balancing-tiers-problem.md` (phase 1 problem
  research)
- `research/03-load-balancing-tiers-needs.md` (phase 2 needs)
- `research/03-load-balancing-tiers-data-a.md` (code + schema + DB
  probe)
- `research/03-load-balancing-tiers-data-b.md` (caller environment
  survey)

Each answer cites the data section that backs it. Answers are
decisions, not recommendations — phase 3 (proposal, `gpt-high`)
implements these, not re-evaluates them.

**Trade-offs requiring user input: none identified.** See §12.

## Q1 — Binding score unit

**Answer: turns-per-hour (expected time-budget remaining rate).**

Per window *w* of provider *p*:

```
remaining_turns_{p,w} = (1 − projected_used_{p,w}) / burn_rate_{p,w}
rate_{p,w}            = remaining_turns_{p,w} / max(hours_until_reset_{p,w}, ε)
```

Provider's binding score = `min_w rate_{p,w}`. Pick the provider with
the highest binding score.

Evidence basis: the current formula uses "fraction of window per
hour" which is incommensurable across tiers of different sizes (data
probe A §1). Turns is the one absolute unit the system already
ingests (`count_assistant_turns_since`, data-a §1 evidence table), so
normalizing to turns-per-hour produces a unit that compares
meaningfully between the 5h slice and the 7d parent. Phase 1 §4.2
worked example and user's own framing ("the smaller tiers are just
slices of the large tiers") both require this unit change.

No new stored field is strictly required to express the score —
`remaining_turns` is derived, not stored — but `burn_rate_{p,w}`
must be available per window, which is Q2.

## Q2 — Per-window burn rate: storage

**Answer: add `last_delta_percent` and `last_delta_calls` columns to
`provider_quota_windows`. Delete the two matching columns from
`provider_quotas`.**

Evidence basis: data-a §2 shows `provider_quota_windows` already has
a `(provider_name, window_id)` composite key, which is exactly the
granularity per-window deltas need. `provider_quotas` can only hold
per-window data by serialization hacks (data-a §2 enumeration), so
keeping deltas there would require blob columns. The natural
normalization is to move the pair onto the per-window table.

This repo's AGENTS.md and the operator conventions in `~/work` and
`~/projects/server-manager` forbid backward-compatibility shims, so
the columns on `provider_quotas` are deleted rather than dual-written.

Write path changes (cited from data-a §2):

1. `QuotaScriptOutput` / `QuotaScriptWindow` — no change (scraper
   contract stays the same).
2. `refresh_provider` — no direct change.
3. `StateDb::upsert_quota_refresh` — must read prior window rows
   before the `DELETE FROM provider_quota_windows`, compute
   per-window delta as `new.used_percent − prior.used_percent`
   when both exist and `new ≥ prior`, pair with
   `count_assistant_turns_since(refreshed_at)`, then write the
   deltas back alongside the new window rows. Drop the longest-
   window delta write on `provider_quotas`.
4. Schema migration adds the two columns to
   `provider_quota_windows` and drops them from `provider_quotas`.

Historical backfill is not possible — the DB retains no prior
`used_percent` snapshots (data-a §2). All per-window deltas are
learned forward from the first post-migration refresh.

## Q3 — Bootstrap before first per-window observation

**Answer: three-step cascade.**

For burn rate `burn_rate_{p,w}` when projection runs:

1. **Provider-level learned per-window value.** If this provider's
   `provider_quota_windows` row for `window_id = w` has
   `last_delta_percent > 0` and `last_delta_calls > 0`, use
   `last_delta_percent / last_delta_calls`.
2. **Pool-level sibling average per window slot.** Else, for every
   sibling provider `p'` in this model pool that has a learned
   value for `window_id = w`, compute
   `Σ last_delta_percent / Σ last_delta_calls` (same form as today's
   `global_avg_percent_per_call`, but per window slot).
3. **Duration-ratio fallback from pool's longest window.** Else, if
   any sibling has a learned longer-window rate, scale it by
   `(effective_long_window_hours / effective_window_hours_w)` where
   effective hours is `resets_at − refreshed_at` for the best
   available row. **Physical intuition**: a shorter tier has
   proportionally less capacity per unit workload, so per-turn burn
   is *larger* on the shorter tier. For a 5h tier vs a 7d tier, the
   multiplier is ≈ 168 / 5 ≈ 33.6. Verified against live DB deltas
   (data-a §3): claude's long-window rate ≈ 8.4e-5/turn implies a
   correct 5h bootstrap ≈ 2.8e-3/turn — 33× larger, not smaller.
   If no data at all, return `None` (not 0). The caller treats a
   `None` rate as "provider is ineligible for density scoring" and
   either picks a learned sibling or falls through to
   `round_robin_fallback`. This replaces the earlier plan of floor-
   at-0 + `EPS_BURN_RATE = 1e-9` division guard, which would have
   made an unlearned provider outrank learned siblings by ~1e6×.

Evidence basis: data-a §3 enumerates what bootstrap inputs exist
locally. Sibling aggregation (`global_avg_percent_per_call`) already
exists as a pool-wide sum pattern — this answer generalizes that
pattern to be per-window-slot. Data-a §3 confirms the pool's
existing Claude sibling variance is already low on the long window
(0.01/119, 0.06/2194, 0.01/80 — all in the same order of magnitude
once normalized), which means sibling averaging is a reasonable
stabilizer.

Window-slot identity is `window_id` as stored today — the scraper
emits windows in a stable order per provider family (anthropic-usage
emits longest first, then short; data-a §7 observation), and siblings
on the same `quota_script` share that ordering.

## Q4 — Sibling variance and plan-class grouping

**Answer: use the existing model pool as the sibling group. No new
plan-class metadata. Per-provider learned rates always override pool
average when present.**

Evidence basis: data-a §4 shows the Claude pool spreads 4.57× on
percent-per-turn and 2.01–2.51× on turn-rate over 24h/7d, driven
primarily by sample-size effects (`claude` has 119 calls worth of
learning, `claude2` has 2194, `claude3` has 80). Probe A confirmed no
explicit plan-class hint exists in config or schema — implicit
grouping only via co-membership in the same model TOML and same
quota-script family.

Consequences:

- The grouping key is `model.providers` — the `ModelConfig` already
  carries it (data-a §4 evidence citations).
- Each sibling still learns its own per-window rate. The pool
  average is a bootstrap stabilizer, not a source of truth for
  picked providers with their own learning.
- No schema change for grouping; the implicit pool membership
  already suffices.

## Q5 — Projection change cost

**Answer: low. 4 tests need re-seeding, 1 function signature change
confined to `score_by_density`.**

Evidence basis: data-a §5. No test calls `score_by_density` directly
— all 4 scoring tests call `select_provider` and seed state via
`upsert_quota_refresh`. Once the migration lands and the refresh
pair moves to `provider_quota_windows`, those tests seed the new
columns (or add explicit per-window delta setters on the test
helpers).

Shape of the change:

- `score_by_density` now reads per-window `burn_rate_w` instead of
  one `avg` scalar. Source of `burn_rate_w` is the cascade in Q3.
- `global_avg_percent_per_call` is deleted (it was the scalar that
  conflated tiers); the replacement is a per-window-slot aggregator
  used only as a bootstrap fallback.
- Projection line changes from `projected = used_percent + turns *
  avg` to `projected_w = used_percent_w + turns * burn_rate_w`.

## Q6 — User vs Background declaration in CLI one-shot

**Answer: two-signal classification.**

Precedence order (first match wins):

1. CLI flag `--risk-class user|background` on the main
   `agents` command (new flag added to the parser in `main.rs`
   alongside the existing `-m/-a/-f/-p/-i`). This is the caller's
   explicit per-invocation override.
2. `repl` subcommand → always `User`. The repl override lands
   above the env-var check because an interactive human session
   cannot tolerate a background-class routing decision inherited
   from a shell export (data-b §6.3 shows `stderr().is_terminal()`
   is already used to gate interactive-mode stderr decorations).
   A workflow that genuinely wants a background-class repl sets
   `--risk-class background` explicitly.
3. Env var `OULIPOLY_RISK_CLASS=user|background` (new variable).
   Applies to one-shot invocations and to the heuristic default
   paths below.
**Heuristic defaults** (applied only when steps 1–3 did not resolve):

4. Main one-shot subcommand → `Background` if any of these hold:
   - `-f/--file` is provided (workflow/automation pattern —
     data-b §6.5 clusters A, B, E, F, I, J, K, L — 60 of 92
     observed invocations).
   - `OULIPOLY_PARENT_INVOCATION` is set (caller is another
     runner, not a human).
   - stdin is not a TTY (pipe or redirect — includes the
     `cat spec.md | agents` cluster H, 3 of 92 invocations). The
     runner cannot distinguish a human typing `cat | agents` from a
     scripted pipe usage, and cluster H appears only 3 times in the
     92-invocation survey vs 60+ clearly-workflow invocations.
     Default to the majority case; users who want `User` class for
     piped stdin set the env var or flag explicitly.
5. Main one-shot subcommand otherwise → `User` (positional prompt
   at a TTY — clusters C, D, and G, 24 of 92 invocations).

Evidence basis: data-b §6.1 enumerates 92 real invocations across 17
files; §6.5 clusters them structurally. The explicit flag + env var
is additive (no existing workflow breaks if it does not set them),
and the heuristic defaults correctly classify every cluster I could
trace intent for.

Workflow documents (data-b §6.6) do not currently tag tolerance;
this initiative does not require them to. Workflows that want
deterministic classification set `OULIPOLY_RISK_CLASS=background` in
their shell wrapper (data-b §6.1 cluster A/B/E/F/K patterns all use
shell wrappers that already set env vars).

The `OULIPOLY_PARENT_INVOCATION` variable is used as a Background
hint because data-b §6.2 confirms it is set by this runner when one
runner invokes another — but it is not a veto of an explicit `User`
classification (the cascade above terminates at the first explicit
signal).

`session_capture_method` is not used for risk classification — it
indicates *how* a session id was captured, not *why* the call was
made.

## Q7 — Threshold numbers and configurability

**Answer: defaults `0.70` and `0.95`. Per-model-pool override in
model TOML. No per-pool-runtime override.**

Evidence basis: user named 70/95 as conceptual anchors. Data-a §7
shows no retained refresh history to tune these empirically; they
are the starting point and can be adjusted per pool if observation
reveals asymmetric pain tolerance between providers.

Schema of the TOML block (addition to model configs like
`claude-opus.toml`, `gpt-high.toml`):

```toml
[balancer]
user_threshold = 0.70       # optional; default 0.70
failure_threshold = 0.95    # optional; default 0.95
```

Evaluation always on **projected** `used_percent` (after applying
the Q3 burn-rate projection for in-flight turns since refresh),
never on raw `used_percent`.

## Q8 — Behavior when all providers fail a gate

**Answer: hard refuse at 95%. Soft degrade at 70% with warning.**

Evidence basis: user's explicit priority — "we'd still want to hit
the weekly in that case because we are at the edge of having real
failures" (needs §4.2). Data-a §8 shows the CLI already has a
`quota_exhausted` diagnostic category but no `quota_tight` pre-flight
category, and zero live rows currently — the error surface exists
but is underused.

Policy:

1. **All providers' projected max tier ≥ 0.95** → refuse the call
   with error_category `quota_exhausted` (pre-flight). Do NOT run
   round-robin; the current fallback to `round_robin_fallback` at
   `balancer/mod.rs:135-141` (data-a §8) stops being the all-`-∞`
   fallback for this case.
2. **User class and all providers' projected max tier ≥ 0.70, but
   some < 0.95** → pick the lowest-risk provider (highest binding
   rate) and emit a warning to stderr:
   `[warn: no provider below user_threshold; routing via quota-tight
   path]`. Persist `quota_tight_routing = true` on the `invocations`
   row (new boolean column).
3. **Background class** → 70% gate never applies; only 95% applies.

Exhaustion state is visible to the caller:

- CLI: existing stderr `[diagnostics: quota_exhausted]` flow used,
  with pre-flight classification reaching the same error surface
  (data-a §8 step 5–6).
- Tauri `test_model`: add a dedicated error path so the UI receives
  a structured error, not raw stderr (data-a §8 notes
  `test_model` does not run diagnostics; adding a quota-tight
  return shape is in scope).

The `round_robin_fallback` path stays as the last resort when no
quota data exists at all (never-refreshed pool on first run), which
is rare after the §5.1 fix.

## Q9 — Fallback tie-break and other zero-window paths

**Answer: five zero-window paths close via the §5.1 + Q11 fixes.
Invocation-count tiebreak stays as today.**

Evidence basis: data-a §9 enumerates five paths:

1. Provider never refreshed → handled by first-refresh self-heal
   after the §5.1 fix (empty-window quotas row is forced-stale).
2. `increment_calls_since_refresh` creates provider row without
   windows → same self-heal.
3. Quota script returns `{"windows":[]}` → closed by Q11 empty-write
   rejection.
4. `upsert_quota_refresh` with empty vec deletes prior rows → closed
   by Q11.
5. Refresh fails (NoScript/AlreadyInFlight/Failed) → existing state
   untouched. If provider never had windows, stays empty → self-heals
   via §5.1 on next call. If provider had windows, nothing changes
   (correct behavior).

With those closed, invocation-count fallback only fires on first-run
fresh pools. Tie-break is unchanged: ascending sort on invocation
count, strict `<` update, lowest index wins ties. This is the only
tiebreaker (data-a §9), and the test at
`balancer/mod.rs:416-433` locks the current behavior — keep it.

## Q10 — Prerequisite sequencing

**Answer: three PRs in dependency order.**

Evidence basis: data-a §10 file sets show triple intersection is
empty. Pairwise overlap is only `quota/mod.rs` between `is_stale`
fix and the scoring redesign. The `chatgpt-usage` fix is in an
external script under `~/.local/bin/`, structurally disjoint from
Rust code.

PR sequence:

| # | Title | Files | Depends on |
|---|---|---|---|
| 1 | `chatgpt-usage` emits 5h + weekly windows | `/home/nes/.local/bin/chatgpt-usage` (and any packaging location of that script if tracked in the repo — verify during proposal) | none |
| 2 | `is_stale` empty-windows fix + `upsert_quota_refresh` reject-empty (Q11) | `src-tauri/src/quota/mod.rs`, `src-tauri/src/state/db.rs` + tests | none (but ship with or before PR 3) |
| 3 | Scoring redesign: per-window burn rate + risk classes + thresholds + `--risk-class` flag | `src-tauri/src/balancer/mod.rs`, `src-tauri/src/state/db.rs` (schema + query), `src-tauri/src/quota/mod.rs` (minor refresh-write change), `src-tauri/src/main.rs` (flag + class plumbing), `src-tauri/src/lib.rs` (Tauri command surfaces + `test_model` error shape), model TOML parser + `[balancer]` block | PR 2 (so `claude2` has windows and scoring redesign tests can validate on it) |

PR 1 and PR 2 can ship in parallel. PR 3 waits on PR 2.

## Q11 — Empty-write failure modes

**Answer: reject empty-windows writes in `upsert_quota_refresh` when
prior windows existed for that provider. Log the transient failure.**

Evidence basis: data-a §11 shows `parse_output` accepts
`{"windows":[]}`, `anthropic-usage` script can synthesize this shape
when both timer entries are absent from the API response, and
`upsert_quota_refresh` with empty input wipes all prior windows.
Scripts' non-zero-exit failures emit empty stdout (not empty
windows), so empty-windows arriving via the happy path is strictly
a "scraper had nothing useful to say" case.

Fix shape (in `StateDb::upsert_quota_refresh`):

1. Query `SELECT COUNT(*) FROM provider_quota_windows WHERE
   provider_name = ?1` before the DELETE.
2. If incoming `windows.len() == 0` and prior count > 0 → do NOT
   delete or modify the window rows; update only `refreshed_at`
   and `last_empty_refresh_at` (new audit column, see below) so
   the next caller can see that a refresh happened and produced
   no windows. Return an Ok-shaped result so callers don't
   spuriously hit error paths, but the audit column captures the
   soft-failure for later diagnosis.
3. If incoming `windows.len() == 0` and prior count == 0 → still
   upsert `provider_quotas` row (so subsequent `is_stale` gets the
   forced-stale signal from §5.1) but don't delete or insert window
   rows.

New column: `provider_quotas.last_empty_refresh_at TEXT NULL`. A
simple audit trail so the `chatgpt-usage` / `anthropic-usage`
degenerate-output case is diagnosable from the DB. Alternative of
logging to stderr alone was considered — rejected because CLI and
Tauri paths have different log sinks (data-a §8) and the DB is the
one sink both see.

## 12. Trade-offs requiring user input

None identified. Every question above resolved from code/DB/workflow
evidence or from the user's prior stated priorities (70/95, hard
block at 95%, per-configuration flexibility). Phase 3 proposal can
begin without further input.

If phase 3 surfaces a genuine trade-off during design (e.g., a cost
asymmetry that isn't visible from current code), the proposer will
escalate it through this orchestrator, not answer it unilaterally.

## 13. Phase 3 input package

Phase 3 proposal agent (`gpt-high`) receives as input:

- `research/03-load-balancing-tiers-problem.md` (phase 1)
- `research/03-load-balancing-tiers-needs.md` (phase 2)
- `research/03-load-balancing-tiers-data-a.md` (code evidence)
- `research/03-load-balancing-tiers-data-b.md` (caller environment)
- this document (orchestrator answers)

Proposal output location: `proposals/03-load-balancing-tiers.md`.

Proposal MUST cover all three PRs from Q10 with separate design
sections, concrete migration SQL, test plan, and risk surface for
the three parallel `claude-opus` risk assessments that follow
(audit risk + scope risk + shortcut risk per `~/projects/server-manager/AGENTS.md` phase 4).
