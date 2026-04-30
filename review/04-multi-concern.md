# Multi-Concern Check: Initiative 04 — reactive routing

## Verdict: single-concern

Initiative 04 is one coherent unit of work: *replace initiative 03's
threshold/risk-class gating surface with per-account reactive
exhausted-flag routing*. Every file in the diff (10 files, 269 lines
inserted, 985 deleted) traces back to either (a) the deletion of the
threshold-gating surface, (b) the per-account exhausted-flag
mechanism replacing it, or (c) the three phase-7 amendments that
emerged during CodeRabbit convergence on the same concern.
`risk/04-scope.md` rev 2 already validated the single-PR framing
against three split candidates and rejected all three; re-evaluating
the actual diff (with phase-7 amendments included) against the same
seam analysis confirms the verdict — none of the amendments are
genuinely separable from the reactive-routing concern.

## Re-evaluation of the three scope-rev-2 seams against the actual diff

### Seam A — schema migration as a prerequisite PR — rejected (confirms scope rev 2)

Two schema changes land in this PR:

- `provider_quotas` **gains** `exhausted_at TEXT NULL`
  (`src-tauri/src/state/db.rs:393`, `:622-628`).
- `invocations` **drops** `quota_tight_routing`
  (`src-tauri/src/state/db.rs:544-549`, `:702-712`, `:797-825`).

Splitting the migration into a prerequisite PR fails the same way it
did in scope rev 2:

- The `exhausted_at` column has no consumer except the `select_provider`
  filter (`balancer/mod.rs:69-100`), the `mark_exhausted` writer
  (`state/db.rs:1243-1262`), and the `upsert_quota_refresh` clear
  (`state/db.rs:1402`) — all introduced in this same PR. A migration-
  only PR would land a column that nothing reads or writes.
- The `quota_tight_routing` column **cannot** ship a deletion alone:
  the column is written by `start_invocation`'s INSERT through
  `InvocationStart.quota_tight_routing` (pre-PR
  `state/db.rs:138-166`), and every `InvocationStart` literal in
  `run_repl`, `run_with_balancing`, `pr_b_trace_integration.rs`, and
  the test suites carries the field. Dropping the column without
  removing the field is a deliberate schema/code mismatch that fails
  at INSERT time on the next call.

The phase-7 `mark_exhausted` upsert amendment (state/db.rs:1243-1262)
sits inside this same migration concern: it is the writer for the
new `exhausted_at` column, and it relies on the column having been
added in the same PR.

### Seam B — `RiskClass` deletion before exhausted-flag add — rejected (confirms scope rev 2)

Same dead-intermediate-state argument as scope rev 2: removing the
threshold gating leaves the balancer with no provider-exclusion
mechanism between the deletion and the filter add — every
provider always eligible regardless of account state. The
`BalanceError::Exhausted(exhausted_error(..., risk_class, ...))`
construction at the pre-PR `balancer/mod.rs:225-228` and `:272-289`
mechanically forces the `BalanceError`, `RiskClass`, and
`ExhaustedError` deletes into one PR (the `exhausted_error` helper
is the sole producer of the type, and its argument is the
`risk_class`).

### Seam C — Tauri `TestModelResult` revert as its own PR — rejected (confirms scope rev 2)

Same argument as scope rev 2 plus an additional reactive-routing-
specific point: Tauri `test_model_with_db_path` is one of two
production write sites for `mark_exhausted`
(`src-tauri/src/lib.rs:506-508`). The structured-error revert and
the new write-site instrumentation share the function body verbatim —
the revert removes `lib.rs:519-568` (the preflight-error mapping),
and the write-site add lands at `lib.rs:506-508` (after
`executor::execute`). Splitting would either leave the structured
error in place while the new write site fires (semantically
inconsistent: subprocess failure flagged via `mark_exhausted`, but
the preflight mapping still claims to be the authoritative
exhaustion signal), or remove the structured error before the
write site exists (a window where `test_model` cannot surface
quota state at all).

## Evaluation of the three phase-7 amendments specifically

### Amendment 1 — past-reset window skip (balancer/mod.rs:149-161)

**Same concern: balancer reactive behavior.**

The skip is a behavior change in `score_by_density`'s window-fold
loop: windows whose `resets_at <= now` are dropped from the
binding-score computation (the inline comment at lines 150-158
calls out the live-caught 2026-04-22 regression class — a stale
past-reset row poisons the binding score by clamping
hours-until-reset to `EPS_HOURS`).

This sits inside the reactive-routing concern for three reasons:

- **Same function, same hunk neighborhood.** `score_by_density`
  is the function whose threshold gating §3.8 deletes and whose
  candidates-list filter §7 introduces. The past-reset skip is a
  three-line addition inside the same window-fold loop the
  threshold deletion already restructured. A separate PR for this
  fix would re-touch the same function for a third time.
- **Same root cause class.** Both the past-reset skip and the
  exhausted-flag short-circuit address the same broader concern:
  `score_by_density` was producing wrong rankings under
  edge-case state. The exhausted flag handles the
  account-is-exhausted-but-still-being-considered case; the
  past-reset skip handles the window-is-stale-but-still-being-
  scored case. Both are corrective filters at the projection
  layer, both tighten the ranking signal, both ship as part of
  the projection-as-ranking-not-gating reconciliation that
  initiative 04 is built around.
- **No standalone test value.** The fix's test
  (`score_by_density_skips_past_reset_windows`) seeds a window
  shape that only arises from the same `last_empty_refresh_at`
  preserve-on-empty path that the rest of initiative 04
  reasons about. Splitting would land a test that exercises
  initiative-04 plumbing in a non-initiative-04 PR.

The "ranking math fix" framing the prompt asks about is
technically accurate but misleading — a separated "ranking math
fix" PR would have to delete and re-introduce the threshold-gating
code path it touches, because the past-reset skip's surrounding
context (`hard_blocked` / `user_blocked` evaluation, `eligible`
filter shape) only exists in its post-04 form. Same concern.

### Amendment 2 — all-exhausted oldest-pick short-circuit (balancer/mod.rs:81-99)

**Same concern: exhausted flag semantics.**

This is unambiguously inside the reactive-routing concern: it is a
direct refinement of the §7 filter's all-exhausted fallback path.
The proposal/answers `Q4` directed fall-through-to-round-robin; the
phase-7 amendment changes the fallback to "pick the
oldest-`exhausted_at` provider" because round-robin would route
back into known-exhausted accounts on every invocation (the inline
comment at lines 81-88 explicitly grounds this in the
user-locked "wait until refresh" invariant from
`tmp/init04-scope-anchor.md`).

The amendment touches the same function, the same code path, the
same data structure (`quotas[i].exhausted_at`), and the same
contract (`select_provider: usize`) as the rest of initiative 04.
Splitting would require the all-exhausted path to first ship in the
proposal-spec form (round-robin), then immediately ship a
follow-up that retracts that decision — pure churn with no
intermediate value.

### Amendment 3 — `mark_exhausted` upsert (state/db.rs:1243-1262)

**Same concern: reactive write path.**

The amendment is a refinement of the §5 write helper itself.
Proposal §5 specified a plain UPDATE; the amendment makes it an
INSERT-OR-UPDATE upsert because a UPDATE on a never-refreshed
provider silently no-ops, leaving a known-broken account eligible
for re-routing on the next call (a guaranteed re-failure that the
reactive model exists to prevent). Inline comment at
`state/db.rs:1244-1252` documents this as CodeRabbit pass-1.

This is a one-statement change inside the function that is the
write-side of the exhausted flag. It cannot be separated from the
flag's introduction without either (a) shipping the flag in a
broken form first (regression risk on every first-use quota
failure) or (b) shipping the upsert against a column that does not
yet exist (compile failure). The two halves co-exist in the same
function body in this PR; that is the only shape that builds.

## Why the files belong together

Ten files touched. Every file is load-bearing for at least one other:

- **`src-tauri/src/balancer/mod.rs`** — defines the new
  `select_provider: usize` contract, the candidates filter, the
  past-reset skip, and the all-exhausted short-circuit. Consumed
  by `main.rs`, `lib.rs`, and `examples/quota_check.rs`.
- **`src-tauri/src/state/db.rs`** — adds `exhausted_at` column,
  `mark_exhausted` writer (with phase-7 upsert), `get_quota`
  reader, and the `upsert_quota_refresh` clear. Removes the
  `quota_tight_routing` column and field. Consumed by the
  balancer (filter reads `QuotaRecord.exhausted_at`), `main.rs`
  and `lib.rs` (write-site callers), and every test fixture that
  constructs an `InvocationStart`.
- **`src-tauri/src/diagnostics/mod.rs`** — extracts
  `classify_exhaustion` from the existing heuristic. Consumed by
  `lib.rs` (Tauri write site) and indirectly by `main.rs`
  (`run_with_balancing` keys on `error_category == "quota_exhausted"`,
  which the heuristic already produces).
- **`src-tauri/src/main.rs`** — drops `--risk-class` CLI surface,
  `resolve_risk_class`, `emit_balance_error`, the
  `BalanceError::Exhausted` catch in REPL/one-shot, and the
  quota-tight warnings. Adds the `mark_exhausted` write site in
  `run_with_balancing`. Each removal pairs with a balancer-side
  removal (the catches were the consumers of `BalanceError`); the
  add pairs with the new diagnostics helper.
- **`src-tauri/src/lib.rs`** — drops `TestModelError`,
  `TestModelProviderInfo`, `TestModelResult.error`, and the
  preflight exhausted mapping. Adds the `mark_exhausted` write
  site in `test_model_with_db_path`. Drops
  `model.balancer.validate()` from `save_model` (paired with the
  `BalancerConfig` deletion in `config/model.rs`).
- **`src-tauri/src/config/model.rs`** — deletes `BalancerConfig`,
  `RawBalancerBlock`, `parse_balancer`, `append_balancer_toml`,
  and the threshold validation. Forced follow-through of the
  threshold-gate deletion: every consumer (the balancer reads at
  the pre-PR `balancer/mod.rs:184-191`, the `save_model` call,
  the `[balancer]` README block) is gone in this PR.
- **`src-tauri/examples/quota_check.rs`** — drops the
  `RiskClass::Background` import/argument and the
  `Result<Selection, _>` match (forced by the `select_provider`
  signature revert). Drops the `quota_tight_routing` printout.
- **`src-tauri/src/executor/cli.rs`** — mechanical
  `balancer: Default::default()` removals from `ModelConfig`
  test fixtures (forced by the `ModelConfig.balancer` field
  removal in `config/model.rs`).
- **`src-tauri/tests/pr_b_trace_integration.rs`** — mechanical
  `quota_tight_routing: false` removal from two `InvocationStart`
  literals (forced by the `InvocationStart.quota_tight_routing`
  field removal in `state/db.rs`).
- **`README.md`** — deletes the `--risk-class` CLI option row,
  the `### Risk classes` Load Balancing subsection, and the
  `[balancer]` TOML block. Adds prose for the new reactive
  exhausted-flag behavior. Anchored surface; matches every
  delete the code makes.

The cross-file dependencies form a closed graph rooted at
`balancer/mod.rs`. Splitting any seam produces one of three shapes
the AGENTS.md split rules forbid: (a) a half-wired intermediate that
fails to build (seam A's deletion-only schema PR), (b) a dormant
plumbing PR with no producer or consumer (seam B's risk-class
delete with no replacement), or (c) a forced-follow-through PR with
no standalone reviewable content (the example/integration-test
mechanical edits, the phase-7 amendments which are corrective
refinements of code that does not yet exist outside this PR).

## Cross-checks against the AGENTS.md split rules

- **"Large deletion is its own PR."** Not applicable as an
  *isolatable* PR — the threshold-gating deletion (`RiskClass`,
  `Selection`, `BalanceError`, `BalancerConfig`,
  `quota_tight_routing`) is the cleared substrate that the
  exhausted-flag mechanism replaces. Per scope rev 2 §3 split-B
  analysis, dropping the gates without the replacement creates a
  behavioral regression no reviewer would accept.
- **"Additive changes go before behavioral changes."** Satisfied
  *within* the PR: the `test(04)` commit (`ba20ced`) lands the
  test scaffolding first, the `feat(04)` commit (`69486a0`) lands
  the implementation. The phase-7 amendments are folded into the
  feat commit because they are corrections to that same
  implementation, not separate behavioral changes against an
  earlier-shipped baseline.
- **"Dependency order matters."** Initiative 04 depends on
  initiative 03 (PR #7 shipped the threshold gating; PRs #9–#11
  shipped the per-window scoring guards that the past-reset skip
  amendment refines). All three predecessors are on `main`. No
  intra-initiative split signal.

## Summary

Ship as one PR. 10 files, 269 insertions / 985 deletions, two
commits test-then-feat. Every file load-bearing for at least one
other; every phase-7 amendment is a refinement of code that does
not exist outside this PR. Scope rev 2 already rejected three
splits for the right reasons; the phase-7 amendments do not change
that calculus — each amendment is mechanically fused with the
reactive-routing concern by virtue of touching the same function,
the same column, or the same write site that the proposal
introduces. No seam proposed would produce a strictly better
reviewer experience, and the user-locked "single PR because the
locked answers make the pieces mutually dependent" framing in
proposal §1 is correct on inspection.
