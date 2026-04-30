# Initiative 04 — Reactive Routing

**Status:** landed (PRs #12 reactive flag + #13 missing-window penalty; follow-up `58aa68d` gates missing-window penalty on visible-usage near cap)
**Depends on:** Initiative 03 (load balancing tiers — per-window scoring, bootstrap cascade)
**Blocks:** Initiative 05 (session migration depends on `provider_quotas.exhausted_at` + `score_by_density`)

## Problem (reconstructed; original user framing not preserved verbatim)

Initiative 03 added threshold and risk-class gating on top of per-window
projection: providers above `user_threshold` (default 0.70) were hidden
from "user" class invocations, providers above `failure_threshold`
(default 0.95) were hard-blocked, and the load balancer returned a
structured `BalanceError::Exhausted` when no eligible provider remained.
The CLI exposed `--risk-class`, the env var `OULIPOLY_RISK_CLASS`, and a
heuristic cascade in `resolve_risk_class` to classify each invocation.

The gating fired pre-emptively on projection alone — a provider could be
healthy but appear above threshold due to noisy bootstrap rates and get
hidden from traffic that it could have served. False positives reduced
effective pool capacity. The replacement is to drop predictive gating
entirely and gate only on **observed** quota failure: a provider gets
marked exhausted when stderr classifies as `quota_exhausted`, and the
flag clears on the next successful non-empty quota refresh.

Reference research: `research/04-reactive-routing-problem.md` (delete
inventory across 9 categories, ~430-520 LOC removed).

## Scope

**In scope:**

- Delete `RiskClass`, `Selection`, `BalanceError`, `ExhaustedError`,
  `ExhaustedProviderInfo`, `BalancerConfig.user_threshold`,
  `BalancerConfig.failure_threshold`, `--risk-class`,
  `OULIPOLY_RISK_CLASS`, `resolve_risk_class`, `quota_tight_routing`,
  `TestModelResult.error`, and the eligibility/soft-degrade branches.
- Add `provider_quotas.exhausted_at TEXT NULL`.
- Add `mark_exhausted` write path triggered by quota-classified stderr in
  `run_with_balancing` and Tauri `test_model_with_db_path`.
- Add exhausted-flag clear in the non-empty branch of
  `upsert_quota_refresh` (preserve on empty refresh).
- Add balancer filter that excludes providers with `exhausted_at IS NOT NULL`
  from candidate sets in both density and invocation-count fallback;
  fall through to unfiltered list when filtered list is empty.
- Revert `select_provider` to `usize` return; remove all `risk_class`
  parameters from balancer, main, lib, and `quota_check`.

**Out of scope:**

- Projection math (`score_by_density`), bootstrap cascade, per-window
  delta learning, `round_robin_fallback`, recent-error avoidance — all
  preserved unchanged.
- Quota script execution, JSON parsing, `windows` shape — unchanged.
- REPL stderr capture for quota classification (deferred per answers
  §D6 — accept one extra failed invocation after a REPL quota-exit
  before the flag is set on the next balancer-routed call).
- Heuristic broadening beyond `quota` / `billing` / `usage limit`.

## Reference framework

`~/ai/initiatives/01-risk-and-value-axes.md` — risk axes for
implementation vs integration risk; this initiative reduces integration
risk (false-positive blocking) at the cost of one observable failure
per provider per quota cycle (acceptable; the failure is informative).

## Artifacts

| Phase | Files |
|-------|-------|
| Research | `research/04-reactive-routing-problem.md`, `research/04-reactive-routing-answers.md`, `research/04-reactive-routing-hookpoints.md` |
| Proposal | `proposals/04-reactive-routing.md` |
| Risk | `risk/04-audit.md`, `risk/04-scope.md`, `risk/04-shortcut.md` |
| Review | `review/04-justification.md`, `review/04-multi-concern.md`, `review/04-test-audit.md` |
| Implementation | PR #12 (`6ef03f9`), PR #13 (`ec692b2`), follow-up `58aa68d` |

## Decision gate

Decided in proposal §10 (cross-cutting): single PR vs decomposed. Locked
to single PR — the answers (D1–D5) make the pieces mutually dependent.

## Log

- **2026-04-?** — Research phase: hookpoints + delete inventory + answers locked.
- **2026-04-?** — Proposal v1 written; risk + review gates run.
- **2026-04-?** — PR #12 (`6ef03f9`) merged: reactive routing via per-account exhausted flag.
- **2026-04-?** — PR #13 (`ec692b2`) merged: penalize providers missing windows that siblings have.
- **2026-04-?** — `58aa68d` follow-up: gate missing-window penalty on visible-usage near cap.
- Status: landed.

## Backfill note

This initiative file was reconstructed after the fact from the existing
`research/04-*` and `proposals/04-*` artifacts and from git history. The
original user framing is not preserved verbatim. Future initiatives
(starting with 05) capture the user prompt at initiative open.
