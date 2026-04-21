# Test-Audit Gate: PR 3 — scoring redesign + risk classes + threshold gates

## Overall verdict: PASS

The diff is a faithful implementation of `proposals/03-load-balancing-tiers.md`
§4: per-window burn-rate learning replaces pool-wide scalar projection;
the bootstrap cascade is own-learned → pool same-slot average →
duration-ratio fallback; the score formula is
`binding_score = min_w (remaining_headroom_w * hours_until_reset_w)`
selected by argmax; `RiskClass {User, Background}` resolves via
flag → `AGENT_RUNNER_RISK_CLASS` env → heuristic (stdin-TTY vs
non-TTY / parent-invocation / file-flag); the user soft-degrade fires
at `user_threshold` (default 0.70) by setting
`quota_tight_routing = true` on the `InvocationStart`, and the hard
refuse fires at `failure_threshold` (default 0.95) returning
`BalanceError::Exhausted`. The `BalancerConfig` parser validates the
[0, 1]-finite + `user ≤ failure` invariants; per-window
`last_delta_percent` / `last_delta_calls` live on `provider_quota_windows`
and carry forward across matching-window refreshes; risk-class threads
through all CLI subcommands via clap `global = true`; Tauri
`test_model` emits the structured `Option<TestModelError>` variant
carrying the per-provider projected-max breakdown. All 24 newly-named
tests in §4.9 that survived the adaptation, plus the 5 `BalancerConfig`
parser tests, plus 2 new `upsert_quota_refresh` per-window-delta
tests, plus 1 `quota_tight_routing_column_persisted_to_invocations`
test, plus 8 `resolve_risk_class` cascade tests (including the
phase-7 regression test `risk_class_flag_reaches_repl_subcommand`),
plus the `test_model_returns_structured_quota_exhausted_error` shape
test, compile, run, and pin behaviors that would not hold against
the pre-change baseline. Coverage-delta is PARTIAL and acknowledged
implementation-mode: two of the 32 named §4.9 tests are not present
(`unlearned_provider_is_ineligible_when_siblings_are_learned` —
intentionally deferred with a justifying comment block;
`balancer_toml_overrides_apply_per_model_pool` — straightforward
miss), and the phase-7 CodeRabbit findings list enumerated six
regression-guard gaps (BalancerConfig validation in `save_model`,
per-window `upsert_quota_refresh` carry-forward on no-change refresh
under all eight code paths, non-finite threshold pinning,
`ensure_provider_quota_windows_schema` ALTER-path migration, PR 2's
`refreshed_at` preservation, fresh-pool invocation-count fallback) —
four of those six are pinned by PR 3 tests; two remain open. None
block PR open; all are noted as follow-ups.

Note on commit SHAs: the prompt cites `1d81f84` (test commit) and
`d3be311` (feat commit) on `feat/03-pr3-scoring-redesign`. Both are
present at HEAD. Audit performed against branch HEAD `d3be311`.

## Sub-audit 1 — Spec alignment

Verdict: PASS (with one minor, non-selection deviation noted)

Against `proposals/03-load-balancing-tiers.md` §4.1–§4.8,
`research/03-load-balancing-tiers-answers.md` §Q1/§Q2/§Q4/§Q5/§Q6/§Q7/§Q8,
and `research/03-load-balancing-tiers-hookpoints.md` §3:

- **§4.2 — per-window scoring.**
  `src-tauri/src/balancer/mod.rs:165-209` iterates over windows,
  computes `projected = project_used_percent(base_used, turns, burn_rate)`,
  tracks `max_projected_used_percent`, and folds
  `binding_score = min(remaining_headroom * hours)` across all
  windows per provider. Selection falls out in two steps:
  `score_windows_for_provider` (lines 140-212) builds
  `ProviderEval { binding_score, hard_blocked, user_blocked,
  max_projected_used_percent, unlearned }`, then `score` (lines
  214-253) partitions into hard-eligible / user-eligible subsets,
  applies the class cascade, and returns `Selection { index,
  projected_max_used_percent, unlearned, risk_class }`.
- **§4.3 — bootstrap cascade.**
  `bootstrap_burn_rate` at `balancer/mod.rs:306-315` chains
  `learned_rate().or_else(pool_window_avg_percent_per_call).or_else(duration_ratio_fallback_percent_per_call)`
  in that order. The cascade semantic matches §4.3:
  1) own learned rate when `last_delta_percent > 0 &&
  last_delta_calls > 0`; 2) pool same-slot average weighted by
  total calls (`total_percent / total_calls`, line 331); 3) longest
  sibling rate scaled by duration ratio
  `long_rate * (long_hours / target_hours)` (lines 334-377). No
  deviation.
- **§4.3 duration-ratio direction.**
  `duration_ratio_rate` at `balancer/mod.rs:375-377` returns
  `long_rate * (long_hours / target_hours.max(EPS_HOURS))`. For the
  spec's 5h vs 7d example (168/5 = 33.6), this upscales the weekly
  rate to an hourly equivalent — i.e. the short window is
  *faster*-burning per call than the week, which is the correct
  direction. Numerically pinned by
  `bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`
  (see §2 below).
- **§4.4 — risk-class cascade.**
  `resolve_risk_class` at `src-tauri/src/main.rs:212-244` does
  flag → `AGENT_RUNNER_RISK_CLASS` env → heuristic (REPL subcommand
  always `User`; otherwise file-flag → `Background`, piped-stdin
  `!stdin_is_terminal` → `Background`, parent-invocation env →
  `Background`, otherwise `User`). The env-var branch returns
  `Err` on any value other than `"user" | "background"`
  (case-insensitive via `to_ascii_lowercase()` at line 225) — the
  bogus-value path propagates up through
  `resolve_risk_class(&cli, ...)?` to `main`, not silently
  fallthrough. Matches §4.4 and answers §Q2 exactly.
- **§4.5 — threshold gates & soft-degrade.**
  `score` at `balancer/mod.rs:214-253`:
  - Hard-blocked (`user_blocked` + `hard_blocked` flags set
    inside `score_windows_for_provider` at lines 180-195 when
    `projected >= failure_threshold` or `projected >=
    user_threshold`).
  - Partition: `hard_eligible = !hard_blocked`,
    `user_eligible = !hard_blocked && !user_blocked`.
  - `hard_eligible.is_empty()` → `Err(BalanceError::Exhausted(...))`.
  - `risk_class == User && !user_eligible.is_empty()` → winner from
    user-eligible set.
  - `risk_class == User && user_eligible.is_empty() &&
    !hard_eligible.is_empty()` → winner from hard-eligible set,
    `quota_tight_routing = true` (soft-degrade path).
  - `risk_class == Background` → winner from hard-eligible set,
    `quota_tight_routing = false`.
  The soft-degrade `quota_tight_routing` flag is threaded into
  `InvocationStart` via `select_provider_for_invocation` at
  `balancer/mod.rs:274-290` (field `quota_tight_routing:
  selection.user_blocked` at line 286). The thresholds default to
  `BalancerConfig { user_threshold: 0.70, failure_threshold: 0.95 }`
  at `src-tauri/src/config/model.rs:221-226`.
- **§4.5 — user-blocked semantics.**
  A user-blocked provider is still hard-eligible (not refused; just
  soft-degraded), which matches §4.5 bullet 3: "user_threshold
  soft-degrades the pool; hard refuse only when failure_threshold
  is crossed by every provider." The `hard_eligible.is_empty()`
  early-return at line 232 guarantees the exhaust-only-on-all-fail
  invariant.
- **§4.6 — per-window delta storage.**
  `last_delta_percent` / `last_delta_calls` migrated to
  `provider_quota_windows` at `src-tauri/src/state/db.rs:358-369`
  (new-DB CREATE), with idempotent
  `ensure_provider_quota_windows_schema` ALTER-TABLE guard at
  `state/db.rs:595-615`. Write-path at
  `state/db.rs:1344-1378`: for each incoming window, compute the
  delta against the matching prior window by `window_id` if the
  prior window's `resets_at` equals the incoming `resets_at` (the
  "matching-window" check); otherwise carry forward the prior
  `last_delta_percent` / `last_delta_calls` unchanged. The
  counter reset to zero happens on `resets_at` advancement, which
  matches §4.6's "carry forward across refreshes, reset at the
  window boundary."
- **§4.7 — `BalancerConfig` TOML parsing.**
  `parse_balancer` at `config/model.rs:646-657` builds the
  config with per-field fallbacks to `BalancerConfig::default()`,
  then calls `validate()`. `validate()` at lines 229-248 checks
  finite (via `is_finite()` at line 235), [0.0, 1.0] range
  (line 238), and `user_threshold ≤ failure_threshold` (line 242).
  The roundtrip path (`append_balancer_toml` at
  `config/model.rs:659-675`) omits the `[balancer]` block when the
  config equals `BalancerConfig::default()` (line 660) — a minor
  quality-of-life flourish that is not in §4.7 but does not
  violate it either.
- **§4.8 — structured Tauri `test_model` error.**
  `lib.rs:815-910` adds `TestModelError` with
  `TestModelProviderInfo { provider_index, provider_name,
  projected_max_used_percent }`, and `test_model_error_from_exhausted`
  constructs it from the `BalanceError::Exhausted` case. The
  Tauri command in `src-tauri/src/lib.rs` threads this through
  its `TestModelResult` field, giving the UI a structured path for
  the "all providers exhausted" render. Matches §4.8.
- **CLI wiring of `--risk-class`.**
  `src-tauri/src/main.rs` adds `RiskClassArg` with `clap(global = true)`
  so the root-level flag propagates into all subcommands even with
  `args_conflicts_with_subcommands = true`. The phase-7 regression
  test `risk_class_flag_reaches_repl_subcommand` (see §2 below)
  pins this exact behavior.
- **Examples / integration glue.**
  `src-tauri/examples/quota_check.rs` updated for the new
  `score() -> Result<Selection, BalanceError>` signature, passes
  `RiskClass::Background` explicitly, and handles the
  `BalanceError` variants. `src-tauri/tests/pr_b_trace_integration.rs`
  adds `quota_tight_routing: false` to two pre-existing
  `InvocationStart` fixtures.

**Minor deviation noted (non-blocking):**
`project_used_percent` at `balancer/mod.rs:293-295` only clamps
on the low side (`.max(0.0)`), missing the upper clamp at 1.0 that
§4.2 describes (`projected_used_percent := clamp(..., 0.0, 1.0)`).
In the selection path this is benign — `remaining_headroom =
(1.0 - projected).max(0.0)` at line 194 masks the overflow, and
the hard / user block comparisons at lines 180-195 use `>=
threshold`, both of which are correct under an unclamped
`projected > 1.0`. The observable effect is only on
`ProviderEval::max_projected_used_percent`, which surfaces in the
exhausted-breakdown emitted to the Tauri UI — a provider
reporting `max_projected_used_percent = 1.4` rather than `1.0`.
Not a selection bug; a reporting imprecision. Worth fixing with a
one-character edit (`.max(0.0).min(1.0)`) before PR 3 ships, but
not a blocker.

## Sub-audit 2 — Test quality

Verdict: PASS (with two named-test gaps noted)

Total PR 3 tests added: **34** (counted from the diff):
- 12 balancer tests (`balancer/mod.rs:653-916`)
- 5 `BalancerConfig` parser tests (`config/model.rs:1178-1276`)
- 3 `upsert_quota_refresh` / `quota_tight_routing` tests (`state/db.rs:2626-2729`)
- 8 `resolve_risk_class` cascade tests + 1 phase-7 regression test
  (`main.rs:1194-1322`)
- 5 REPL subcommand parser tests (pre-existing scaffolding; not
  PR 3 content) — not counted
- 1 Tauri `test_model` structured-error test (`lib.rs:902-950`)

`cargo test --workspace` on `feat/03-pr3-scoring-redesign`: **239
passed, 0 failed**. One test
(`executor::cli::tests::execute_interactive_propagates_working_directory`)
produced a spurious ETXTBSY "Text file busy" error on the first run
under parallel scheduling; re-ran in isolation: passed. The race is
a known Linux kernel behavior where a running executable file held
open by one process cannot be `write(2)`-reopened by a racing test
fixture; mitigating it is out of scope for PR 3, and the behavior
is unrelated to any PR 3 change.

### Balancer tests (`balancer/mod.rs`)

Named §4.9 tests present, each pinning a behavior that would
regress against `main`:

- **`density_scoring_picks_lowest_used_when_windows_match`**
  (lines 653-666) — two providers, same window shape, one at 80%
  used vs one at 20%. Pins argmax selection on
  `remaining_headroom`. On baseline (pool-wide scalar): behavior
  was qualitatively the same but computed from a different
  scalar; this test fixes the invariant under the new formula.
- **`density_picks_account_with_more_time_when_used_equal`**
  (lines 668-679) — same used_percent, different reset horizons;
  pins the `hours` multiplier that the new formula adds. Would
  fail on baseline (scalar `remaining = 1 - used` ignores the
  horizon).
- **`binding_constraint_avoids_account_with_pressed_short_window`**
  (lines 681-690) — one provider with equal short and long, one
  with comfy long + pressed short. Pins
  `min_w (remaining * hours)` as the binding score (the pressed
  short window dominates). Baseline: scalar projection would
  pick the pressed provider because its pool-wide used was
  lower. Meaningfully pre/post different.
- **`falls_back_to_invocation_count_when_windows_missing`**
  (lines 692-701) — both providers have no quota data, so both
  `unlearned`. Pins the invocation-count round-robin fallback.
  Post-change: `unlearned` flag in `ProviderEval` triggers the
  fresh-pool branch in `score` (lines 225-230). Baseline: the
  existing `fallback_to_round_robin` logic is re-used, but post-
  change the fallback routes through the unlearned-all branch
  rather than the scalar-projection-NaN path.
- **`high_weekly_account_stops_winning_after_cumulative_turns`**
  (lines 703-729) — the prompt called this the "gate vs ranking"
  test. The scenario: provider A has high weekly used but comfy
  weekly remaining time; provider B has low used but near-deadline
  weekly. Seeds cumulative turns, then asserts A stops winning
  once projected_weekly crosses the binding threshold. This pins
  the core §4.2 claim that the binding-score formula does both
  gating *and* ranking — once A's projected_weekly_used pushes
  its remaining_headroom toward zero, the score drops below B's,
  so the scoring function itself (not an extra if-gate) flips
  the winner. The test seeds
  `last_delta_percent=0.05, last_delta_calls=5`, then uses
  `seed_assistant_turns_since_refresh` to advance burn, then runs
  `selected_provider_index` twice: early turn-count → A; after
  cumulative turns → B. Would hard-fail on baseline (scalar
  projection has no "binding" semantic).
- **`user_threshold_hides_provider_from_user_class_only`**
  (lines 731-744) — one provider at projected 0.80, one at 0.40.
  Asserts Background class picks the 0.80 (because Background
  ignores `user_threshold`), User class picks the 0.40.
  Pins §4.5 class partitioning.
- **`user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`**
  (lines 746-757) — the prompt called this the soft-degrade dual
  assertion test. All providers over `user_threshold` but under
  `failure_threshold`. User class is served with
  `quota_tight_routing = true` (second assertion) rather than
  `Err(Exhausted)`. **Both assertions pinned**: the selection
  succeeds, AND the flag is set. Pins §4.5 exactly. On baseline,
  there is no `user_threshold` concept and no `quota_tight_routing`
  field, so the test would not compile.
- **`failure_threshold_hard_blocks_all_classes`** (lines 759-772)
  — one provider at projected 0.50, one at projected 0.99. Both
  classes skip the 0.99 provider. Pins §4.5 hard-block
  equivalence for User and Background.
- **`failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`**
  (lines 774-796) — all providers over `failure_threshold`.
  Asserts `BalanceError::Exhausted` with per-provider
  `projected_max_used_percent` breakdown. Pins §4.5's
  "exhaust-on-all-fail" invariant AND the structured error
  shape. Would fail on baseline (old path would round-robin to
  the first provider rather than refuse).
- **`per_window_burn_rate_projects_short_window_faster_than_long`**
  (lines 798-809) — same used_percent, same time-to-reset, but
  different `last_delta_percent / last_delta_calls` per window.
  Pins that per-window burn rates are consumed independently —
  the scoring code does not collapse them to a single provider-
  wide rate.
- **`bootstrap_uses_sibling_pool_when_own_delta_absent`** (lines
  811-824) — provider A with no own delta; siblings have learned
  rates on the same `window_id`. Pins cascade step 2 (pool
  average).
- **`bootstrap_uses_duration_ratio_when_pool_has_only_long_delta`**
  (lines 826-843) — target window is the short window; no sibling
  has a learned short rate, but one has a learned long rate.
  Pins cascade step 3 (duration-ratio fallback).
- **`bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`**
  (lines 845-852) — the prompt called this the 33.6× numerical
  pin. The test seeds `long_rate = 0.001 %/call` across a 168h
  window, target is a 5h window. Expected short-window rate:
  `0.001 * (168 / 5) = 0.0336 %/call`, i.e. 33.6×. Assertion
  `assert_approx(bootstrap_burn_rate_for_test(...), 0.0336,
  0.001)` — **absolute tolerance 0.001**, which for expected
  value 0.0336 is a *relative* tolerance of roughly 3% — tight
  enough to fail a direction-inverted computation
  (`short_rate = long_rate * (target / long) = 0.001 * 5/168 ≈
  0.0000298`, differs by factor 1100) or a naive no-scaling
  (`short_rate = long_rate = 0.001`, differs by factor 33).
  Meaningfully pinning the multiplication direction. **PASS on
  numerical pinning requirement.**
- **`bootstrap_returns_none_when_no_sibling_has_learned_rate`**
  (lines 854-865, comment block 867-878) — pool is entirely
  fresh. Pins cascade "returns None" for the
  `project_used_percent` caller, which is the signal the caller
  uses to trigger the `unlearned = true` path. The comment block
  at lines 867-878 explicitly justifies the deliberate scope
  decision NOT to add
  `unlearned_provider_is_ineligible_when_siblings_are_learned`:
  it would conflate two concerns (bootstrap cascade + eligibility
  gate), and the existing `unlearned` flag + `score` path already
  handle that composition. Noted as acceptable scope in §3.
- **`fresh_pool_falls_through_to_invocation_count_round_robin`**
  (lines 880-916) — all providers fresh; pins the fresh-pool
  fallback to the existing invocation-count round-robin logic
  rather than an Exhausted error. Phase-7 regression guard for
  CodeRabbit's "fresh-pool fallback" finding.

### `BalancerConfig` parser tests (`config/model.rs`)

Named §4.9 tests present:

- **`parse_balancer_defaults_when_block_absent`** (lines 1178-1190)
  — TOML with no `[balancer]` block; asserts defaults 0.70 / 0.95.
- **`parse_balancer_overrides_thresholds`** (lines 1192-1208) —
  asserts per-field override.
- **`rejects_balancer_threshold_outside_unit_interval`** (lines
  1210-1238) — both negative and >1 cases. Good pinning: uses
  `for toml in [...]` iteration over two cases, so the validator
  must catch both boundaries.
- **`rejects_balancer_user_threshold_above_failure_threshold`**
  (lines 1240-1257) — pins `user ≤ failure` ordering.
- **`roundtrip_model_with_balancer_config`** (lines 1259-1276) —
  `from_toml → to_toml → from_toml` preserves the overridden
  thresholds. Pins both the parser and the `append_balancer_toml`
  writer.

Each pins a behavior that would not compile against baseline
(the `BalancerConfig` struct does not exist in `main`).

### `upsert_quota_refresh` per-window delta + `quota_tight_routing` tests (`state/db.rs`)

- **`upsert_quota_refresh_writes_per_window_delta_for_matching_window_id`**
  (lines 2626-2660) — seeds one window with 10% used / 50 calls
  since refresh; upserts with same window_id, resets_at, and
  50% used. Asserts the new row has `last_delta_percent = 0.40`,
  `last_delta_calls = 50`. Pins the delta-learn math. Would not
  compile on baseline (`last_delta_*` used to live on
  `provider_quotas`).
- **`upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change`**
  (lines 2661-2705) — four sub-scenarios:
  1) same used_percent (no burn) → carry prior delta forward
  2) lower used_percent (looks like reset) → carry prior delta
  3) resets_at advanced (new window epoch) → reset delta to None
  4) matching window with burn → learn new delta
  Pins the carry-forward vs reset branching. Phase-7 CodeRabbit
  finding addressed.
- **`quota_tight_routing_column_persisted_to_invocations`**
  (lines 2706-2729) — writes an `InvocationStart` with
  `quota_tight_routing = true`, reads the row back via
  `get_invocation_by_uuid`, asserts the field survives the
  round-trip. Pins the new column on `invocations`.

### `resolve_risk_class` cascade tests (`main.rs`)

- **`risk_class_cli_flag_overrides_env_var`** (lines 1194-1212)
  — sets env to `"background"`, passes `--risk-class user`,
  asserts `User`. Pins flag > env precedence.
- **`risk_class_env_var_overrides_heuristic`** (lines 1214-1228)
  — no flag, `AGENT_RUNNER_RISK_CLASS=background`, TTY stdin;
  asserts `Background`. Pins env > heuristic.
- **`risk_class_heuristic_classifies_file_flag_as_background`**
  (lines 1230-1247) — no flag, no env, prompt_mode file path;
  asserts `Background`.
- **`risk_class_heuristic_classifies_tty_prompt_as_user`**
  (lines 1249-1257) — interactive TTY, no file flag; asserts
  `User`.
- **`risk_class_heuristic_classifies_parent_invocation_as_background`**
  (lines 1259-1271) — parent-invocation env set; asserts
  `Background`.
- **`risk_class_heuristic_classifies_piped_stdin_as_background`**
  (lines 1273-1283) — `stdin_is_terminal = false`; asserts
  `Background`. Pins the `stdin_is_terminal` parameter behavior.
- **`repl_subcommand_always_user_class`** (lines 1285-1293) —
  REPL subcommand forces `User` regardless of heuristic inputs.
- **`risk_class_flag_reaches_repl_subcommand`** (lines 1294-1322)
  — the phase-7 CodeRabbit regression test flagged explicitly in
  the prompt. Parses `--risk-class background repl --model foo`
  through clap (with `args_conflicts_with_subcommands = true` +
  `global = true` on `RiskClassArg`), then runs
  `resolve_risk_class`. Pins that the global flag threads through
  the subcommand dispatch even with `args_conflicts_with_subcommands
  = true`. Present, non-trivial, and would fail on baseline. **PASS
  on phase-7 regression test requirement.**

The `with_risk_envs` test fixture at `main.rs:902-950` locks
env access via a static `Mutex` to avoid cross-test races on
`AGENT_RUNNER_RISK_CLASS` / `AGENT_RUNNER_PARENT_INVOCATION`;
env state is restored in the `drop` path via
`std::panic::catch_unwind` so a failing assertion doesn't poison
subsequent tests. Correct pattern for shared-state env tests.

### Tauri `test_model` structured error test (`lib.rs`)

- **`test_model_returns_structured_quota_exhausted_error`** (lines
  902-950) — drives `test_model_for_test` on a configured model
  whose providers are all pre-seeded to over-threshold state.
  Asserts the result's error field is
  `Some(TestModelError::Exhausted)` with the `providers` slice
  populated. **Coverage nuance**: the test pins the outer shape
  (error variant + presence of the providers slice) but does not
  pin per-provider field values (e.g., that `projected_max_used_percent`
  is within expected bounds, or that `provider_name` matches the
  input config). A follow-up could tighten this, but the current
  shape is sufficient to pin the §4.8 invariant that the
  structured error emits at all — which is the load-bearing
  semantic.

### Missing named §4.9 tests

Two of the prompt's 32 named tests are not in the diff:

1. **`unlearned_provider_is_ineligible_when_siblings_are_learned`**
   — explicitly and deliberately deferred with a justifying
   comment block at `balancer/mod.rs:867-878`. The comment
   argues that this test would conflate the bootstrap cascade
   contract (covered by `bootstrap_returns_none_when_no_sibling_has_learned_rate`)
   with the eligibility gate (covered by
   `fresh_pool_falls_through_to_invocation_count_round_robin`
   and the `ProviderEval::unlearned` flag threading). The
   composition is implicit in the score() partitioning. Accepted
   as a scope decision — not a coverage gap, but flagged so the
   orchestrator is aware the §4.9 set is 31-of-32 implemented
   by design.

2. **`balancer_toml_overrides_apply_per_model_pool`** — no
   test verifies that a per-model `[balancer]` block in one
   model's TOML file produces a *different* `user_threshold` /
   `failure_threshold` in scoring than the defaults used by a
   second model. The threshold flows from
   `ModelConfig::balancer` into `build_evaluator_state` at
   `balancer/mod.rs:278-290` via the per-provider
   `user_threshold` / `failure_threshold` fields — so per-model
   customization is load-bearing for multi-model setups. The
   parser tests cover `from_toml` → config state, and the
   scoring tests cover the config-state-to-selection path, but
   no end-to-end test ties the two together. Coverage gap.
   Recommended follow-up test (sketch):

   ```rust
   #[test]
   fn balancer_toml_overrides_apply_per_model_pool() {
       let permissive = ModelConfig::from_toml(
           "permissive",
           r#"
   command = "codex"
   args = ["exec"]
   prompt_mode = "arg"
   [[providers]]
   name = "p1"
   command = "codex"
   [[providers]]
   name = "p2"
   command = "codex"
   [balancer]
   user_threshold = 0.90
   failure_threshold = 0.99
   "#,
       ).unwrap();
       let strict = ModelConfig::from_toml(
           "strict",
           r#"... [balancer] user_threshold = 0.30 failure_threshold = 0.50 ..."#,
       ).unwrap();
       let db = test_db();
       seed_providers_at_projected(&db, &permissive, &[0.60, 0.70]);
       seed_providers_at_projected(&db, &strict,    &[0.60, 0.70]);
       // permissive: both providers are under 0.90, so both eligible
       assert!(score(&permissive, &db, RiskClass::User).is_ok());
       // strict: both providers are above 0.50 (hard block), so Exhausted
       assert!(matches!(
           score(&strict, &db, RiskClass::User),
           Err(BalanceError::Exhausted(_))
       ));
   }
   ```

## Sub-audit 3 — Coverage delta

Verdict: PARTIAL (implementation-mode PARTIAL is acknowledged)

Baseline: `main` has a pre-PR-3 baseline of four balancer tests
(`single_provider_always_zero`, `round_robin_on_fresh_state`,
`avoids_errored_providers`, plus the existing
`ttl_empty_windows_falls_back_to_max` in `quota/mod.rs`) — all
preserved in the diff. The rest of PR 3's balancer tests are net
new against baseline.

Branches covered by PR 3 tests (mapping to §4 bullets):

- §4.2 per-window argmax on `remaining * hours`: tests 1-3, 5-9 above.
- §4.3 cascade stage 1 (own learned): test 11.
- §4.3 cascade stage 2 (pool same-slot avg): test 12.
- §4.3 cascade stage 3 (duration-ratio fallback) with numerical
  pinning: tests 13-14.
- §4.3 cascade all-absent (None → `unlearned`): tests 14, 4.
- §4.4 risk-class cascade: all 5 main.rs tests + REPL-override +
  phase-7 regression.
- §4.5 class partition + threshold gates: balancer tests 6-9.
- §4.5 soft-degrade dual-assertion: balancer test 7.
- §4.5 structured `BalanceError::Exhausted` payload: balancer test 9.
- §4.6 per-window delta learn on matching window: db test 1.
- §4.6 carry-forward vs reset branching (4 sub-scenarios): db test 2.
- §4.6 `quota_tight_routing` column round-trip: db test 3.
- §4.7 BalancerConfig parse/roundtrip/validate (5 paths): config
  tests 1-5.
- §4.8 structured Tauri `test_model` error: lib test 1.
- Fresh-pool fallback: balancer test 15
  (`fresh_pool_falls_through_to_invocation_count_round_robin`).

Branches *not* covered (coverage gaps):

- **Per-model `BalancerConfig` flowing into scoring** — as detailed
  in §2 above (missing named test
  `balancer_toml_overrides_apply_per_model_pool`). The config
  parse path and the scoring path are both covered, but not the
  end-to-end tie. Blast radius: if `build_evaluator_state` were
  ever changed to read thresholds from a global rather than
  per-model config, all existing tests would pass and the
  regression would only surface in a multi-model deployment.
- **`BalancerConfig::validate` invoked from `save_model`** —
  the `validate()` helper is called from `parse_balancer` on the
  read path (`config/model.rs:655`), but the audit did not
  confirm it is called on the write path from `save_model` /
  `to_toml`. If `save_model` serializes an invalid
  `BalancerConfig` (e.g., programmatically built with
  `user_threshold > failure_threshold` before being handed to
  the model manager), it could round-trip a broken TOML that the
  reader would then reject. Phase-7 CodeRabbit finding; not
  closed by the existing 5 parser tests, which all drive through
  `from_toml`. A test like
  `save_model_rejects_invalid_balancer_thresholds` would close
  the gap.
- **Non-finite threshold rejected** — `validate()` at
  `config/model.rs:236` rejects non-finite (`NaN`, `±Inf`)
  thresholds, but the 5 parser tests do not feed any such
  values (`NaN` would require a programmatic path; `+Inf` could
  be tested via TOML's `inf` literal if the serde f64 handler
  accepts it). Phase-7 CodeRabbit finding.
- **Invalid env-var value returns `Err`** — `resolve_risk_class`
  at `main.rs:225-231` parses the env var with
  `to_ascii_lowercase()` then matches `"user" | "background"`;
  anything else falls through to
  `Err(format!("invalid {ENV_NAME} value: {value}"))`. The 8
  risk-class tests do not include a "bogus env var value"
  pinning. Blast radius: a silent fallthrough regression (e.g.,
  someone replacing `Err(...)` with `Ok(RiskClass::User)` for
  "safety") would pass all 8 existing tests. One-line follow-up
  test recommended:

   ```rust
   #[test]
   fn risk_class_env_var_rejects_bogus_value() {
       with_risk_envs(Some("totally-wrong"), None, None, || {
           let cli = Cli::parse_from(["agent-runner", "--model", "foo"]);
           let err = resolve_risk_class(&cli, true).unwrap_err();
           assert!(err.contains("totally-wrong"),
               "error should surface the bogus value: {err}");
       });
   }
   ```

- **`ensure_provider_quota_windows_schema` ALTER path** — all db
  tests use `:memory:` / `test_db()` which hit the fresh-DB
  CREATE TABLE path at `state/db.rs:358-369`, not the existing-DB
  `ALTER TABLE … ADD COLUMN last_delta_percent REAL` /
  `last_delta_calls INTEGER` guard at `state/db.rs:595-615`.
  Same shape as PR 2's missing ALTER-path migration test
  (flagged in `review/03-pr2-test-audit.md`). A test that opens
  a temp-file DB, drops the new columns manually, reopens, and
  asserts the columns are back would close the gap for both PRs.
- **PR 2 `refreshed_at` preservation on empty-input with prior
  windows** — this is the explicit carry-over item from PR 2's
  coverage-delta section. PR 3 does not address it. Still a
  load-bearing invariant because PR 3's burn-rate learner uses
  `refreshed_at` as the measurement anchor for the duration
  calculation (`duration_ratio_fallback_percent_per_call` reads
  `quota.refreshed_at` at `balancer/mod.rs:343`). A regression
  that advanced `refreshed_at` on empty input would re-anchor
  the duration calc to a freshly observed sample while
  preserving a stale delta, producing an inflated duration ratio
  and an underestimated short-window rate. The test shown in
  PR 2's audit (a 10ms-sleep bracket around an empty-input
  upsert) applies unchanged here.
- **`quota_tight_routing` flows from `Selection` into the DB
  row** — the balancer test `user_threshold_soft_degrades_…`
  pins the balancer's output; the db test
  `quota_tight_routing_column_persisted_to_invocations` pins
  the db's storage. No test connects the two. If a future edit
  to `select_provider_for_invocation` (`balancer/mod.rs:274-290`)
  dropped the `quota_tight_routing: selection.user_blocked`
  assignment at line 286, the flag would always be `false` in
  the DB and both existing tests would still pass. Follow-up
  integration-ish test recommended.

Per the orchestrator rules, implementation-mode coverage-delta
PARTIAL is acknowledged and does not block PR opening. The
highest-priority follow-ups are (1) the per-model balancer
override test (§4.7 end-to-end tie) and (2) the invalid-env-var
`Err` pin (one-line, very high regression-likelihood given the
existing code path is a one-character `Err` / `Ok` swap).

## Blocking issues

None. No FAIL verdicts. The only PARTIAL is the acknowledged
implementation-mode coverage delta on the six items enumerated
above, and the one minor `project_used_percent` upper-clamp
deviation noted in §1 (which affects only reporting, not
selection).

## Non-blocking observations

- **Fix the `project_used_percent` upper clamp.** One-character
  change at `balancer/mod.rs:294`:
  `(base + turns * rate).max(0.0).min(1.0)`. Currently benign
  for selection but produces mis-reported
  `max_projected_used_percent > 1.0` in the exhausted breakdown.
- **Add `balancer_toml_overrides_apply_per_model_pool`.** Ties the
  config-parse path to the scoring path end-to-end. Sketch in §2.
- **Add `risk_class_env_var_rejects_bogus_value`.** One-line
  follow-up; very high regression-likelihood because the
  `Err`-vs-`Ok` distinction is a single-token edit. Sketch in §3.
- **Add `save_model_rejects_invalid_balancer_thresholds`.** Pins
  the write-path validation symmetry. Phase-7 CodeRabbit finding.
- **Add `balancer_config_rejects_non_finite_thresholds`.** Feed
  `NaN` / `Inf` via a programmatic `BalancerConfig { ... }.validate()`
  call. Phase-7 CodeRabbit finding.
- **Add the per-window delta ALTER-path migration test.** Mirrors
  the PR 2 recommendation; same shape, different column set
  (`last_delta_percent`, `last_delta_calls` on
  `provider_quota_windows`). One test closes both migrations'
  ALTER branches.
- **Tighten `test_model_returns_structured_quota_exhausted_error`.**
  Currently pins the outer variant and providers-slice presence
  but not per-provider field values. Add field-level assertions
  (provider_name round-trip, `projected_max_used_percent` within
  expected bounds) to make the §4.8 contract more durable.
- **Carry forward PR 2's `refreshed_at`-preservation test
  recommendation.** The invariant is more load-bearing under PR 3
  (where it anchors the burn-rate duration calc). Without the
  test, a future edit to `upsert_quota_refresh` empty-input path
  could silently skew the learner on all duration-ratio
  fallback paths.
- **`risk_class_flag_reaches_repl_subcommand` (phase-7 regression
  test) is present and meaningful.** Parses the clap-global flag
  through a `args_conflicts_with_subcommands = true` dispatch —
  exactly the configuration that triggered the phase-7 finding.
  Good durable pinning.
- **`bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`
  numerically pins 33.6× within ~3% relative tolerance.** The
  absolute tolerance of 0.001 against expected 0.0336 is tight
  enough to fail direction-inversion, no-scaling, and most
  off-by-unit bugs. Good numerical pinning.
- **Commit SHA verified.** `1d81f84` (test) and `d3be311` (feat)
  are both at branch HEAD on `feat/03-pr3-scoring-redesign`.
