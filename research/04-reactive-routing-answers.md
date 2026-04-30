# Initiative 04 — Reactive Routing: Orchestrator Answers

Phase 2 synthesis. Feeds phase 3 (proposal, `gpt-high`). Grounded in
`research/04-reactive-routing-problem.md` + user-locked scope anchors
in `tmp/init04-scope-anchor.md`.

## Locked scope recap

- Projection continues to drive **ranking**, never **gating**.
- Reactive exhausted flag is **per-account** (`provider_name` keyed).
- Flag is **sticky** — set once, NOT re-probed per invocation. User:
  "don't spam reactive." Only refresh clears it.
- All RiskClass / threshold surface from initiative 03 is deleted,
  not shimmed.

## Answers to phase 1 open questions

### Q1 — Where does the flag live?

**`provider_quotas.exhausted_at TEXT NULL`** — RFC3339 timestamp.
NULL = not exhausted. Non-NULL = exhausted, with timestamp of when
the classification landed (useful for audit).

Why this table: user stated "exhausted is specific to an account."
`provider_quotas` is the only table keyed by `provider_name` alone.
`providers` keys by `(model_name, provider_index)` — wrong
granularity, would flag claude2-in-claude-opus-pool separately
from claude2-in-claude-sonnet-pool even though both hit the same
account cap. `provider_quota_windows` keys per-window; wrong
granularity (an account is exhausted, not a specific tier).

Column type `TEXT NULL` instead of boolean + separate timestamp
because (a) the timestamp is always useful for debugging/audit,
(b) NULL cleanly encodes "not exhausted" without a second column,
(c) matches the `last_empty_refresh_at` precedent from PR #6.

Migration: idempotent ALTER TABLE ADD COLUMN (matches existing
schema-ensure pattern). Also update the fresh-DB CREATE TABLE.

### Q2 — What refresh result clears the flag?

**Any successful `upsert_quota_refresh` call with `windows.len() > 0`
clears `exhausted_at` to NULL.** Rationale:

- A successful refresh with windows means the quota script ran,
  returned a parseable payload, and reported current usage. That's
  ground truth newer than the stored exhausted classification.
- Whether `used_percent` is above/below some threshold doesn't
  matter for clearing — user has explicitly deprecated threshold-
  based gating. If the account is still at cap, the next invocation
  against it will immediately re-classify as exhausted and re-set
  the flag. That's the intended single-bit loop.
- Empty-window refreshes (the PR #6 preserve-on-empty path) do NOT
  clear the flag. An empty refresh is a transient failure signal,
  not positive evidence.

Implementation: in `upsert_quota_refresh`'s non-empty branch, set
`provider_quotas.exhausted_at = NULL` alongside the other quota
updates. Same transaction.

### Q3 — Per-account or per-pool?

**Per-account.** `provider_name` is the account key. An account
exhausted via one model pool is excluded from every model pool
that routes through it, until the next successful refresh. User
explicitly confirmed.

### Q4 — All providers in pool exhausted?

**Fall through to `round_robin_fallback` (invocation-count).** No
hard error from the balancer itself. Rationale:

- Even with all providers flagged, the lowest-invocation-count one
  is the best guess for "where to route next." The subprocess will
  either succeed (account recovered) or fail with quota_exhausted
  (flag stays set, next refresh eventually clears it).
- User's framing: "route to a different model that isn't exhausted."
  If everything is exhausted, user sees the quota-exhausted stderr
  from the actual subprocess — that's visible ground truth, not a
  balancer-side pre-flight error.
- Avoids bringing back anything resembling `BalanceError::Exhausted`.
- Test coverage: a dedicated test pins this
  (`all_providers_exhausted_falls_through_to_round_robin`).

### Q5 — Does reactive classification apply to REPL exits?

**Yes.** The exhausted flag must be maintained consistently across
all three call sites (CLI one-shot, REPL, Tauri `test_model`),
otherwise a REPL session that hits quota leaves the flag unset and
the next balancer call routes back to the exhausted account.

Implementation: extract the heuristic part of diagnostics
(`stderr contains "quota" / "billing" / "usage limit"` → classify
as `QuotaExhausted`) into a shared helper `classify_exhaustion`
that takes stderr and returns `bool`. Call it from:

- `run_with_balancing` subprocess-failure path (already runs full
  diagnostics; no behavior change — heuristic result is separately
  captured as "exhausted" if category is QuotaExhausted).
- `run_repl` subprocess-failure path (new site — heuristic only,
  cheap).
- Tauri `test_model` subprocess-failure path (new site).

Do NOT run the LLM-based diagnostics from REPL / Tauri — that's
existing behavior (only CLI one-shot runs the LLM diagnostics).
Heuristic is enough to flip the flag.

When `classify_exhaustion` returns true, write
`UPDATE provider_quotas SET exhausted_at = now() WHERE provider_name = ?`.
Single write. No retry. No re-classification on subsequent
invocations of the same account — the flag filters before
`select_provider` even considers routing there.

## Additional locked decisions

### D1 — Schema migration order

PR deletes `invocations.quota_tight_routing` AND adds
`provider_quotas.exhausted_at` in the same migration block. Both
are idempotent ALTER TABLE statements. Dependency-free order.

### D2 — Test_model Tauri response reverts

`TestModelResult` reverts to `{ success, stdout, stderr, exit_code }`
— the pre-initiative-03 shape. The frontend type already omitted
the `error` field (`src/lib/types.ts:109-114` per problem research
§7 note 5). `TestModelError` / `TestModelProviderInfo` structs
deleted.

### D3 — Signature reverts

- `select_provider(model, state, ctx) -> usize` — reverts PR 3
  signature change. Drops `risk_class` arg and `Result` return.
- `run_with_balancing(model, prompt, ...)` — drops `risk_class` arg.
- `run_repl(model, ...)` — drops `risk_class_override: Option<RiskClass>`
  arg.

### D4 — Balancer selection filter

In `score_by_density` (and `score_by_invocation_count` fallback),
add a preliminary filter: **exclude any provider whose
`provider_quotas.exhausted_at IS NOT NULL`** from the eval list.
If the filtered list is empty, proceed with the unfiltered list
(all exhausted → still pick by ranking, matching Q4 answer). The
check reads `provider_quotas` (not a per-call cache) so there's no
per-invocation probe.

### D6 — REPL stderr capture: NOT implemented

Phase 4 audit flagged that `executor::cli::execute_interactive` inherits
stderr (`Stdio::inherit()`), so `run_repl` currently has no plumbing to
surface the child's stderr text for heuristic classification. Options
considered:

- A. Tee stderr via `os_pipe`/`Stdio::piped` + forwarding thread to
  preserve TTY passthrough. Non-trivial; color/line-buffering
  changes; TTY-mode awareness. Phase 6 implementation cost medium.
- B. Accept the gap: REPL quota-exit does NOT set the flag. The next
  balancer-routed invocation to the same account runs diagnostics,
  classifies, flags — one extra guaranteed failure per REPL
  quota-exit.
- C. Ringbuffer-last-N-bytes stderr capture.

**Decision: B.** Rationale:

- The user-locked "don't spam reactive" invariant is about flag
  stickiness AFTER classification ("once a provider is known
  exhausted, the balancer skips it without re-checking until the
  next successful refresh"). It does not govern the single
  pre-classification failure. One extra failure to observe quota
  exhaustion is not spam — it is the signal acquisition event.
- Option A's cost does not buy much. A REPL session that hits quota
  is unrecoverable to the user mid-session anyway (terminal handoff
  is already made, the wrapped CLI has already printed its error).
  Flagging on REPL exit vs on next CLI call differs only by whether
  the one-extra-failure shows up immediately or at the next
  balancer invocation.
- Option C has the same design cost as A for a more-partial outcome.
- Graceful degradation fits the broader pattern: the refresh TTL
  clock will also eventually produce a successful refresh that
  either reports headroom (clearing any flag that was set
  elsewhere) or doesn't. The system self-corrects without REPL
  having a classification path.

Consequence documented: **one guaranteed extra quota-failed
invocation after a REPL quota-exit before the flag is set.** This is
acceptable semantic behavior per the no-spam invariant. Future work
can implement Option A if the one-extra-failure becomes painful in
practice.

### D7 — Heuristic coverage: use existing classifier unchanged

Audit §6 observed the heuristic (`"quota"` / `"billing"` / `"usage
limit"`) is unverified against real CLI stderr samples. Decision:

- Use the existing `diagnostics::diagnose_error` heuristic unchanged.
  It is shared code with CLI one-shot classification; broadening the
  match terms for reactive routing affects that path too (out of
  scope).
- If a known-quota stderr doesn't match the heuristic, the flag is
  not set; the next balancer call re-attempts; the refresh TTL
  eventually catches up. Graceful degradation, same mechanism as
  REPL decision D6.
- Future work: collect a corpus of real Claude/Codex/opencode quota
  stderr samples; if the existing heuristic misses recurring
  phrasings, broaden the classifier in a separate, small PR against
  `src-tauri/src/diagnostics/mod.rs`. That PR would benefit both
  the existing one-shot diagnostics surface and reactive routing.
- Phase 6 MAY run a quick grep against any stderr samples found in
  existing test fixtures or `scripts/tests/` to validate the
  heuristic matches expected quota text; not a gate.

### D8 — Test-module `use RiskClass` cleanup

Audit §9 noted two test-module imports (`src-tauri/src/main.rs:865`,
`src-tauri/src/lib.rs:812`) that become dead after the cascade tests
delete. Rust's unused-import warning will flag them at compile time;
the code agent removes them as part of the delete pass. Not a
proposal-level revision — compile fallout the implementer handles
mechanically.

### D5 — `BalanceContext` unchanged

The refresh loop at the top of `select_provider` still runs before
the filter. If a provider's refresh lands during this call, the
flag might get cleared, then the filter sees it not-exhausted. That's
correct behavior — the refresh is exactly the authorized clear
signal.

## Non-goals reconfirmed (from phase 1 §7)

Per-account error-count tracking beyond `providers.error_count`
(already used for recent-error avoidance). No new
exhausted-specific metrics dashboard. No frontend surface for
exhausted state — the existing quota display in the app uses
`used_percent` via the refresh command, which is sufficient.

## Phase 3 input

Phase 3 proposal (`gpt-high`) reads this file plus problem research
and writes `proposals/04-reactive-routing.md`. Single PR because
the keep/delete items are tightly coupled:

- `Selection` revert to `usize` + `quota_tight_routing` column drop
  + TestModelResult revert all touch the same selection API surface.
- Risk-class deletion cascades through 3 call sites simultaneously.
- Exhausted flag add is paired with filter-on-read in the same
  `select_provider` function.

Splitting produces dead-plumbing intermediate states (e.g., add
exhausted column but don't filter; filter but never set) that
CodeRabbit / phase-8 multi-concern review would flag as not-yet-
useful.
