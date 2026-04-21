# Audit Risk Assessment: proposals/03-load-balancing-tiers.md

## Verdict: LOW

The revised proposal correctly addresses every HIGH and MEDIUM finding
from the prior audit gate. The duration-ratio fallback is now written
in the right direction (`long_hours / target_hours`) with a live-data
sanity check; `EPS_BURN_RATE` is gone in favor of an `Option<f64>`
ineligibility signal; `ProviderEval` has a concrete shape; `run_repl`
has an explicit `BalanceError::Exhausted` handling paragraph; the
`cat | agents` / cluster-H cascade rule is now internally consistent
with the answers doc. Causal chains for the three defects match the
actual code paths at the cited sites, and migration ordering is
implementable against the bundled SQLite 3.51.1. Remaining issues are
all minor: several line-range citations are off by 1–4 lines, a
cluster-count arithmetic slip propagates from the answers doc, and a
§4.5 pseudocode vs. prose mismatch about when the carry-forward
branch fires. None of these would steer the implementer wrong.

## Findings

### 1. LOW — Cluster-count arithmetic error in §4.4

**Location:** proposals/03-load-balancing-tiers.md §4.4 step 7,
"clusters C and G, 16 of 92 invocations".

**Evidence:** `research/03-load-balancing-tiers-data-b.md` §6.5 counts
are cluster C = 11 and cluster G = 4 (sum = 15, not 16). Additionally,
cluster D (sample size 9: `agents -m <model> -i <key=value> ...
"prompt" > <binary>`) is a positional-prompt / TTY-stdin pattern that
also terminates at step 7 of the cascade (`User`), but the proposal
doesn't enumerate it. The correct cluster roster for step 7 is C, G,
and D = 24 of 92 invocations.

The same miscount sits in `research/03-load-balancing-tiers-answers.md`
§Q6 rule 5 — the proposal faithfully inherits it. Neither document's
arithmetic changes the cascade's behavior: clusters C, G, and D all
correctly classify as `User` under rule 7 as written.

**Recommendation:** Change "clusters C and G, 16 of 92 invocations"
to "clusters C, D, and G, 24 of 92 invocations" in both the proposal
and the answers doc. No code change.

### 2. LOW — §4.5 carry-forward branch is wider than the prose claims

**Location:** proposals/03-load-balancing-tiers.md §4.5, two
paragraphs in tension.

**Evidence:** The pseudocode shape in §4.5 says "If `dp > 0` and
calls > 0, write those delta columns on the new window row. Otherwise
carry forward that same prior window's previous delta". That
"Otherwise" branch fires for every `dp == 0` case **and** every
`calls == 0` case. But the next paragraph says carry-forward applies
"only on window resets (`new.used_percent < prior.used_percent`) and
flat-observation refreshes (`new.used_percent == prior.used_percent`)".
The `dp > 0 && calls == 0` corner (percent up, but no ingested
assistant turns credited to the gap — e.g. session ingestion skew
against the refresh timing) isn't covered by the prose but is caught
by the pseudocode.

This is a minor spec ambiguity. Both possible behaviors are defensible
(carrying forward is safe; skipping is also safe), but the contract
should say which is canonical so the implementer doesn't have to
guess.

**Recommendation:** Either revise the §4.5 prose to "carry-forward
applies whenever the pair (dp, dc) isn't both positive", or tighten
the pseudocode to also distinguish `dp > 0 && calls == 0`. Same
outcome either way — just make them agree.

### 3. LOW — Carry-forward bounding claim slightly off

**Location:** proposals/03-load-balancing-tiers.md §4.5, "This is
bounded in practice by `resets_at`: preserved windows age out
naturally, and when they cross their reset time, `dynamic_ttl_secs`
floors to `MIN_TTL_SECS` and forces aggressive re-refresh."

**Evidence:** Verified against `src-tauri/src/quota/mod.rs:148-159`:
`dynamic_ttl_secs` computes `(w.resets_at - now).num_seconds().max(0)`
and clamps to `[MIN_TTL_SECS, MAX_TTL_SECS]`. So when a window
passes its reset time, TTL drops to 5 minutes. That does cause more
frequent *refreshes*, but it doesn't clear the carried-forward
`last_delta_percent` / `last_delta_calls`. The carry-forward values
get overwritten only when a subsequent refresh observes a positive
`dp` with positive calls — i.e., when real activity resumes. The
proposal's "preserved windows age out naturally" framing suggests
an automatic cleanup that isn't actually implemented; the bound is
really "next active refresh", not "reset time".

Outcome still holds (no pathological indefinite drift in practice
for an active workload), but the causal chain is imprecise.

**Recommendation:** Reword to "bounded in practice by the next
positive-delta refresh after workload resumes, which frequent
post-reset TTL drops accelerate". No code change.

### 4. LOW — Cargo.lock citation range for rusqlite is truncated

**Location:** proposals/03-load-balancing-tiers.md §4.2,
"`rusqlite 0.38.0` (`src-tauri/Cargo.lock:2825-2834`)".

**Evidence:** Verified lockfile — the `[[package]] rusqlite`
block begins at line 2825 and the terminating `]` of its
`dependencies` array is on line 2838. The cited range ends at
2834, which truncates the block one line before
`"libsqlite3-sys",` — the most audit-relevant dependency line.

**Recommendation:** Update the range to `2825-2838`. No code
change.

### 5. LOW — Minor off-by-N line-range drift on several code citations

**Location:** Multiple sites in proposals/03-load-balancing-tiers.md.

**Evidence:** Spot-checks revealed a handful of citations that are
slightly off:

- `src-tauri/src/state/db.rs:1809-1837` for
  `count_assistant_turns_since` — actual is 1811–1837 (off by 2 at
  start).
- `src-tauri/src/quota/mod.rs:129-142` for `is_stale` — actual is
  132–143 (off by 3).
- `src-tauri/src/config/model.rs:273-285` for `RawModelToml` — actual
  is 275–285 (off by 2).
- `src-tauri/src/balancer/mod.rs:88-141` for `score_by_density` —
  actual is 88–142 (off by 1 at end).

All within a few lines of the real spans; each still lands in the
cited function. Non-blocking but cumulative imprecision.

**Recommendation:** Spot-check the ranges once more during PR 3
authoring. No code change.

### 6. LOW — §2.2 preserves the `else empty end` window-drop-on-absent pattern

**Location:** proposals/03-load-balancing-tiers.md §2.2, "If upstream
omits one of those entries, emit the present one rather than failing,
matching the `if ... else empty end` approach in `anthropic-usage`".

**Evidence:** `/home/nes/.local/bin/anthropic-usage:45-54` uses
`if .seven_day.resets_at then {...} else empty end` for each slot.
Result: if `seven_day` disappears from an upstream response, the
emitted array shrinks from `[weekly, 5h]` to `[{5h}]` — window_id
identity flips (the 5h's `used_percent` now occupies window 0, whose
prior row was the weekly). On the next `upsert_quota_refresh`, the
per-window delta computation at §4.5 pairs that bogus slot against
the prior weekly's `used_percent` and writes a nonsense `dp`. The
proposal explicitly acknowledges this risk in §4.5 ("a reordered
scraper output is treated as a different window and is therefore a
correctness risk for phase 4") and in §4.6 ("the window-id stability
assumption is positional"). So the hazard is disclosed — but the
proposed chatgpt-usage change in §2.2 propagates the same fragility
to Codex.

A more robust convention would be to emit an explicit null-filled
window for any absent tier so window_id positions stay stable. That
is a scope expansion and may not be right for this initiative. Noted
as an observable correctness-risk that survives the proposal as
designed.

**Recommendation:** No change required for this gate; consider
revisiting in a later pass if DB inspection ever shows a jump-dp
pattern that correlates with an upstream missing-tier response.

## Spot-checks verified

1. `src-tauri/src/balancer/mod.rs:22-70` — `select_provider` signature
   matches proposal's "from" form, with `ctx: Option<&BalanceContext>`
   and `-> usize` return, confirming the signature change to
   `Result<Selection, BalanceError>` is a real break, not a
   reinterpretation.
2. `src-tauri/src/balancer/mod.rs:88-142` — `score_by_density`
   currently uses one scalar `avg` and falls back to
   `round_robin_fallback` when every score is `-inf`; the proposal's
   replacement covers both cases (binding rate per window; explicit
   `Err(Exhausted)` vs. `round_robin_fallback` for the all-unlearned
   path).
3. `src-tauri/src/balancer/mod.rs:62-69` — the `all_have_windows`
   gate. Proposal §4.7 replaces both this gate and the `-inf`
   fallback with explicit filter semantics over `ProviderEval`.
4. `src-tauri/src/state/db.rs:1155-1242` — `upsert_quota_refresh`.
   Verified that the function already reads `prior` and
   `prior_windows` at lines 1164-1166 **before** any mutation (upsert
   at 1196, delete at 1219, inserts 1225-1237), so the §4.5
   requirement "build a prior-window map keyed by `window_id` before
   the wholesale delete" lands in the right place in the current
   flow. Concern 6 from the prompt (prior-row read before upsert)
   holds.
5. `src-tauri/src/state/db.rs:1109-1146` — `get_windows` returns rows
   ordered by `window_id`, confirming positional stability assumption
   for same-slot delta matching in §4.5.
6. `src-tauri/src/quota/mod.rs:132-143` — `is_stale` currently goes
   through `get_quota`, then checks `refreshed_at`, then feeds a
   possibly-empty window list into `dynamic_ttl_secs`. Verified that
   a windowless row does return the 24h max TTL today (line 149), so
   §3.2's single-line guard is the minimal fix.
7. `src-tauri/src/quota/mod.rs:65-84` — `QuotaScriptOutput` already
   prefers the `windows` array and falls back to the flat legacy
   shape, matching proposal §2.2's assumption.
8. `src-tauri/src/state/db.rs:352-360` — `CREATE TABLE
   provider_quotas` currently has `last_delta_percent REAL,
   last_delta_calls INTEGER` with no index or constraint referencing
   them, so `ALTER TABLE … DROP COLUMN` in M_03_03 is safe on this
   schema.
9. `/home/nes/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/libsqlite3-sys-0.36.0/sqlite3/sqlite3.h:149`
   — `#define SQLITE_VERSION "3.51.1"`. Confirms the bundled
   SQLite supports `ALTER TABLE DROP COLUMN` (added in 3.35.0).
10. `src-tauri/Cargo.toml:16` — `rusqlite = { version = "0.38",
    features = ["bundled"] }`. Confirms `bundled` feature is set,
    so runtime SQLite *is* the 3.51.1 from the registry source,
    not the system one.
11. `src-tauri/src/main.rs:622` — current `run_with_balancing` calls
    `balancer::select_provider(model, &state, Some(&ctx))` without a
    risk-class argument. Confirms proposal §4.4's call-site update is
    a real new threading, not a redundant rewrite.
12. `src-tauri/src/main.rs:525` — `run_repl` calls the same signature.
    Confirms §4.4 and §4.8 correctly identify `run_repl` as a
    distinct caller needing the new hardcoded `RiskClass::User`
    pass-through.
13. `src-tauri/src/lib.rs:492` — Tauri `test_model` calls
    `balancer::select_provider(&model, &db, None)` with no ctx.
    Confirms proposal §4.4's `test_model` update is correct and
    §4.8's structured error shape has a single integration point.
14. `src-tauri/src/state/db.rs:1811-1837` — `count_assistant_turns_since`
    exists with signature `(provider_name: &str, since:
    Option<&DateTime<Utc>>) -> Result<u64, String>`. Confirms §4.5's
    "Pair `dp` with `count_assistant_turns_since(provider_name,
    prior.refreshed_at)`" uses the real existing API.
15. `/home/nes/.local/bin/chatgpt-usage:36-46` — current jq pipeline
    emits the flat `{used_percent, resets_at}` shape from
    `secondary_window` only; PR 1's two-window rewrite is a real
    behavior change, not a refactor.
16. `/home/nes/.local/bin/anthropic-usage:41-54` — emits
    `[{seven_day}, {five_hour}]` in that order, verifying proposal
    §2.2's claim about the "longest first, short second" convention.

## Synthesis adherence check

The proposal faithfully implements every Q1–Q11 decision from
`research/03-load-balancing-tiers-answers.md`:

- **Q1 (turns-per-hour binding score):** §4.7 pseudocode computes
  `remaining_turns_w / hours_until_reset_w` and takes `min_w` — exact
  match.
- **Q2 (per-window delta storage; drop provider-level):**
  M_03_02 adds `last_delta_percent` / `last_delta_calls` to
  `provider_quota_windows`; M_03_03 drops both from `provider_quotas`
  — exact match.
- **Q3 (three-step bootstrap cascade; `None` when no data; ratio
  direction `long_hours / target_hours`):** §4.6 pseudocode is the
  correct direction (verified numerically: 8.4e-5/turn long-window ×
  168/5 ≈ 2.82e-3/turn short-window, matching the physical-intuition
  note in §Q3 and in §4.6). Ineligibility semantics via `Option<f64>`
  match the answers doc's "caller treats a `None` rate as provider
  ineligible" clause.
- **Q4 (sibling = model pool; no plan-class metadata):** §4.6's
  `pool_window_avg_percent_per_call` takes `windows: &[Vec<QuotaWindow>]`
  — that's the pool — and no schema change introduces a plan-class
  column.
- **Q5 (low cost, confined change):** §4.9 rewrites the existing
  four scoring tests and adds targeted coverage. No new surfaces.
- **Q6 (explicit flag + env var + heuristic cascade):** §4.4 has
  the correct 7-step cascade. Rule 6 (`cat | agents` → Background)
  is now internally consistent between proposal and answers.
- **Q7 (0.70 / 0.95 defaults, per-model TOML, validation):** §4.3
  matches, including the cross-ordering validation (reject
  `user_threshold > failure_threshold`) that the answers doc
  implies via the gate semantics.
- **Q8 (hard-refuse 95; soft-degrade 70 with `quota_tight_routing`
  stderr warning):** §4.7 and §4.8 cover both, with the structured
  Tauri `test_model` response shape in §4.8 matching the answers
  doc's call for a dedicated error path.
- **Q9 (invocation-count tiebreak unchanged; fresh-pool fallback
  preserved):** §4.7 "Fresh-pool round-robin is the only remaining
  round-robin path" is exact.
- **Q10 (three-PR sequence):** §2, §3, §4 match.
- **Q11 (reject empty-window writes; audit column; DB sink for
  both CLI and Tauri):** §3.3 and §3.4 match. Cross-ref:
  `provider_quotas.last_empty_refresh_at TEXT NULL` is the correct
  column shape.

Orchestrator-mandated updates from the addendum all land as claimed:
duration-ratio direction fixed (§4.6 + §4.9's direction-check test),
`EPS_BURN_RATE` removed (confirmed absent from proposal), cluster-H
rule reconciled (both proposal §4.4 rule 6 and answers §Q6 rule 4
now agree), `ProviderEval` struct concrete (§4.7), `run_repl`
exhausted surface explicit (§4.8), Cargo.lock ranges reordered
(though §4.2's rusqlite range is still slightly truncated; see
Finding 4), carry-forward staleness bound discussed (§4.5; its
mechanism description is slightly imprecise — Finding 3), off-by-one
range fixed (verified in §4.9 test list alignment).

Adherence is high; the proposal is a faithful implementation of the
orchestrator's answers, and every decision traces back to a cited
evidence line in the answers doc.
