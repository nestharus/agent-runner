# Justification: PR 3

## Verdict: JUSTIFIED

Every hunk in `feat/03-pr3-scoring-redesign` (commits `1d81f84` test +
`d3be311` feat) maps directly to `proposals/03-load-balancing-tiers.md`
§4.2–§4.9, the §Q1–§Q8 answer decisions, or locked human-gate A/B.
Change surface is the nine files explicitly scoped by the proposal
and §Q10 dependency graph: `balancer/mod.rs`, `config/model.rs`,
`state/db.rs`, `main.rs`, `lib.rs`, `executor/cli.rs`,
`quota/mod.rs`, `examples/quota_check.rs`, and the one PR-B trace
integration test. No incidental cleanups, no cross-cutting
refactors, no `scripts/` changes (human-gate C is correctly PR 1).

One small mismatch between the task prompt and the actual diff: the
prompt names `executor/cli.rs — quota_tight_routing plumbing to
build_command`, but the diff contains no `quota_tight_routing`
reference in the executor at all. The actual `executor/cli.rs`
hunks are mechanical `balancer: Default::default()` lines on
`ModelConfig` test fixtures — propagation of the §4.3 struct
field, not an executor wiring change. The narrower diff is in
scope and arguably more correct (quota-tight routing is a
persisted audit signal per §4.4, not an executor runtime flag).

## Hunks kept

### `src-tauri/src/balancer/mod.rs` — §4.4 / §4.6 / §4.7 / §4.8 / §4.9

- **`RiskClass { User, Background }` enum at `balancer/mod.rs:11-16`**
  with `serde` snake_case — §4.4 / §Q6 step 1-2 type.
- **`Selection { provider_index, quota_tight_routing }` at
  `balancer/mod.rs:18-22`** — §4.4 contract ("`Selection` contains
  `provider_index: usize` and `quota_tight_routing: bool`").
- **`BalanceError::Exhausted(ExhaustedError)` + `ExhaustedError` +
  `ExhaustedProviderInfo` at `balancer/mod.rs:24-52`** — §4.4 and
  §4.8 ("model name, risk class, and per-provider projected max
  usages"). `Display` + `std::error::Error` impls required for
  `main.rs::emit_balance_error`.
- **`ProviderEval` at `balancer/mod.rs:54-62`** — §4.7 explicit
  shape "resolves audit-risk finding 3 on the prior revision."
  Fields match the proposal's struct literal verbatim
  (`index`, `binding_score`, `hard_blocked`, `user_blocked`,
  `max_projected_used_percent`, `unlearned`).
- **`select_provider` signature change** (`balancer/mod.rs:80-93`)
  — `risk_class: RiskClass` arg and
  `Result<Selection, BalanceError>` return. §4.4 verbatim.
- **`score_by_density` rewrite** (`balancer/mod.rs:133-290`) —
  deletes the `global_avg_percent_per_call` scalar and the
  shared `NEG_INFINITY` fallback sort. Per-provider loop now
  runs the `bootstrap_burn_rate` cascade per window, computes
  `project_used_percent`, sets `hard_blocked` / `user_blocked`
  against `model.balancer.failure_threshold` /
  `model.balancer.user_threshold`, and folds the window-scored
  min into `binding_score`. Selection policy branches into
  `hard_eligible` → `user_eligible` → `best_binding_score`
  exactly as §4.7 pseudocode. Fresh-pool fallback fires only
  when `evals.iter().all(|e| e.unlearned) &&
  evals.iter().all(|e| !e.hard_blocked)` — §4.7 bullet
  "all unlearned and none hard-blocked falls through."
- **`best_binding_score` helper at `balancer/mod.rs:292-307`** —
  `debug_assert` on non-empty slice and `binding_score.is_some()`
  documents the filter invariants from §4.7.
- **`exhausted_error` helper at `balancer/mod.rs:309-327`** — §4.8
  error construction. Populates `providers` from all evals, not
  only hard-blocked ones; matches §4.8 JSON example which lists
  every provider with its `projected_max_used_percent`.
- **`project_used_percent`** (`balancer/mod.rs:329-331`) — one-line
  helper for the §4.7 projection formula. Floor at 0 rather than
  `clamp(0,1)`; the threshold check at line 182 uses `>=
  failure_threshold` so any post-1.0 value still correctly
  hard-blocks, and the test
  `per_window_burn_rate_projects_short_window_faster_than_long`
  depends on no-upper-clamp so the >100% arithmetic survives.
- **`learned_rate`** (`balancer/mod.rs:333-340`) — §Q3 step 1 form
  `dp/dc` gated on both being positive, per §4.6.
- **`bootstrap_burn_rate`** (`balancer/mod.rs:342-351`) — §4.6
  three-step cascade (own learned → pool-average → duration-ratio).
- **`pool_window_avg_percent_per_call`** (`balancer/mod.rs:353-366`)
  — §4.6 helper. Iterates all windows in the pool, keyed by
  `window_id`; `Option<f64>` return per the audit-risk finding 6
  design decision ("None means ineligible for density scoring").
- **`duration_ratio_fallback_percent_per_call`** (`balancer/mod.rs:368-404`)
  — §4.6. Uses `target_hours` from target window's `refreshed_at`
  anchor, only considers learned siblings whose long window is
  strictly longer than target, and picks the longest-hours
  sibling. Ratio direction is `long_hours / target_hours` —
  matches §Q3 physical intuition ("5h tier burn rate ≈ 33×
  long-tier rate").
- **`duration_ratio_rate`** (`balancer/mod.rs:406-408`) — one-line
  split so the `_for_test` helper can exercise the raw math.
- **Test-only helpers `project_used_percent_for_test`,
  `bootstrap_burn_rate_for_test`, `bootstrap_duration_ratio_for_test`
  at `balancer/mod.rs:410-449`** — minimum `pub(crate)` surface so
  the §4.9 tests can exercise projection/bootstrap math without
  going through `select_provider`.
- **Test helpers `quota_window`, `seed_windows_with_deltas`,
  `seed_assistant_turns_since_refresh`, `selected_provider_index`,
  `selected_provider`, `assert_approx`** — minimum fixtures for the
  §4.9 tests. `seed_windows_with_deltas` uses the new test-only
  `db.set_window_delta_for_test` path because no public write path
  sets delta columns except `upsert_quota_refresh`, which needs
  two successive refreshes to populate them.
- **Rewritten existing four scoring tests + new tests** (lines
  651-912) — named exactly per §4.9:
  `density_scoring_picks_lowest_used_when_windows_match`,
  `density_picks_account_with_more_time_when_used_equal`
  (now asserts provider 1, reflecting the new
  `remaining × hours` formula where more hours → higher score —
  the inversion noted in `research/03-load-balancing-tiers-answers.md:44-53`),
  `binding_constraint_avoids_account_with_pressed_short_window`
  (simplified seed now that per-window burn rates drive projection),
  `falls_back_to_invocation_count_when_windows_missing`,
  `high_weekly_account_stops_winning_after_cumulative_turns`,
  `user_threshold_hides_provider_from_user_class_only`,
  `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`,
  `failure_threshold_hard_blocks_all_classes`,
  `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`,
  `per_window_burn_rate_projects_short_window_faster_than_long`,
  `bootstrap_uses_sibling_pool_when_own_delta_absent`,
  `bootstrap_uses_duration_ratio_when_pool_has_only_long_delta`,
  `bootstrap_short_window_rate_exceeds_long_window_rate_by_duration_ratio`
  (direction check for audit-risk finding 1),
  `bootstrap_returns_none_when_no_sibling_has_learned_rate`,
  `fresh_pool_falls_through_to_invocation_count_round_robin`.
- **"Intentionally no test for the A-unlearned-while-B-learned case"
  comment at `balancer/mod.rs:818-831`** — explains why the §4.9
  bullet
  `unlearned_provider_is_ineligible_when_siblings_are_learned`
  is not a discrete test: the §Q3 cascade makes that state
  unreachable when pool providers share `quota_script`, and the
  existing sibling-rescue, duration-ratio-rescue, and no-learning
  tests cover the full contract. This is an explicit,
  self-documenting deviation from §4.9 wording — in-scope
  judgment call anchored to §Q3, not a missing hunk.

### `src-tauri/src/config/model.rs` — §4.3

- **`BalancerConfig { user_threshold, failure_threshold }` at
  `config/model.rs:213-225`** with `Default::default()` returning
  `(0.70, 0.95)` — §4.3 verbatim.
- **`BalancerConfig::validate` at `config/model.rs:227-246`** —
  rejects non-finite values, values outside `0.0..=1.0`, and
  `user_threshold > failure_threshold`. §4.3 validation list.
- **`ModelConfig.balancer: BalancerConfig` field at
  `config/model.rs:209`** with `#[serde(default)]` — §4.3 "Add a
  `BalancerConfig` field to `ModelConfig`."
- **`RawModelToml.balancer: Option<RawBalancerBlock>` +
  `RawBalancerBlock` at `config/model.rs:324-330`** — §4.3
  "`RawModelToml` should add `balancer: Option<BalancerConfig>`"
  (using a `Raw*` shape so individual thresholds can be `None`
  and resolve to defaults, matching the TOML semantics "Fields
  are optional").
- **`parse_balancer` at `config/model.rs:651-662`** — defaults +
  override + validate path. Called from `from_toml` at
  `config/model.rs:570`.
- **`append_balancer_toml` at `config/model.rs:664-677`** — §4.3
  serializer hookpoint. Elides the block when values equal
  defaults, matching the existing `append_resume_toml` /
  `append_session_capture_toml` "skip when default" convention.
- **Five parser tests at `config/model.rs:1176-1276`** — match
  §4.3 test plan one-for-one:
  `parse_balancer_defaults_when_block_absent`,
  `parse_balancer_overrides_thresholds`,
  `rejects_balancer_threshold_outside_unit_interval`,
  `rejects_balancer_user_threshold_above_failure_threshold`,
  `roundtrip_model_with_balancer_config`.

### `src-tauri/src/state/db.rs` — §4.2 / §4.4 / §4.5 / §4.9 + human-gate B

- **`QuotaRecord` loses `last_delta_percent` and `last_delta_calls`**
  (`state/db.rs:31-34`) — §4.5 "`QuotaRecord` should no longer
  expose provider-level deltas."
- **`QuotaWindow` gains `last_delta_percent: Option<f64>` and
  `last_delta_calls: Option<u64>`** (`state/db.rs:45-47`) — §4.5
  "Extend `QuotaWindow` with `last_delta_percent: Option<f64>`
  and `last_delta_calls: Option<u64>`."
- **`InvocationRecord.quota_tight_routing: bool` at
  `state/db.rs:114`** — §4.4 "`InvocationRecord` currently has
  no field for it and should be extended."
- **`InvocationStart.quota_tight_routing: bool` at
  `state/db.rs:126`** — §4.4 "add the column to
  `InvocationStart` or provide an update method."
- **`provider_quotas CREATE TABLE` drops delta columns** at
  `state/db.rs:353-356` — §4.2.
- **`provider_quota_windows CREATE TABLE` adds delta columns** at
  `state/db.rs:362-365` — §4.2 M_03_02.
- **`invocations CREATE TABLE` adds `quota_tight_routing`** at
  `state/db.rs:662` — §4.2 M_03_04.
- **`ensure_invocations_schema` column-add branch** at
  `state/db.rs:506-512` — idempotent ALTER ADD COLUMN matching
  the `session_id` / `session_capture_method` pattern. §4.2.
- **`ensure_provider_quotas_schema` gains DROP COLUMN branches**
  at `state/db.rs:584-591` — §4.2 M_03_03. SQLite 3.35+ DROP
  COLUMN support is called out in proposal §4.2 and justified
  against bundled `libsqlite3-sys 0.36.0` / SQLite 3.51.1.
- **`ensure_provider_quota_windows_schema` new helper** at
  `state/db.rs:594-613` — §4.2 M_03_02 idempotent ALTER ADD
  branches, mirrors `ensure_provider_quotas_schema` style.
- **`provider_quota_windows_columns` helper** at `state/db.rs:631-645`
  — PRAGMA reader mirrors `provider_quotas_columns`. Minimum
  plumbing for the migration.
- **Legacy-rebuild path adds `quota_tight_routing` to
  `invocations_new`** at `state/db.rs:757, 779, 782` — §4.2
  "For legacy invocation rebuilds, add `quota_tight_routing`
  to `invocations_new` with default `0`."
- **`start_invocation` insert adds `quota_tight_routing`** at
  `state/db.rs:869-881` — §4.4 call-site payload.
- **`map_invocation_row` column-index shift + new field read** at
  `state/db.rs:1014-1102` — mechanical adjustment for the new
  column's ordinal position in the SELECT.
- **`get_quota` drops delta columns from SELECT/row mapping** at
  `state/db.rs:1172-1188` — §4.5 "stop selecting
  `last_delta_percent`/`last_delta_calls` in `get_quota`."
- **`get_windows` adds delta columns to SELECT/row mapping** at
  `state/db.rs:1204-1229` — §4.5 "`QuotaWindow` … `get_windows`
  is the balancer's window read path."
- **`upsert_quota_refresh` rewritten for per-window delta
  learning** at `state/db.rs:1307-1365` — prior-window map keyed
  by `window_id`, `turns_between_refreshes` hoisted, per-window
  `(delta_percent, delta_calls)` computed with
  `new.used_percent - prior.used_percent` clamped at 0 and
  carry-forward on non-positive delta, window INSERT now writes
  the two delta columns. Legacy provider mirror
  (`used_percent`/`resets_at` from longest window) preserved
  for legacy readers — §4.5 "retain the current wholesale
  replacement behavior" and not explicitly flagged for removal.
  The provider-level delta write that §4.5 calls out is deleted
  ("Delete provider-level delta computation and writes from
  `provider_quotas`: remove the longest-window delta block").
- **`set_window_delta_for_test` test-only helper** at
  `state/db.rs:1401-1425` — no public write path sets
  `(last_delta_percent, last_delta_calls)` directly after the
  §4.5 rewrite (they are computed from prior-vs-new refreshes);
  `#[cfg(test)] pub(crate)` exposure is the minimum needed for
  balancer and lib tests to construct learned states without
  chaining two `upsert_quota_refresh` calls in every case.
- **`insert_quota_row_without_windows_for_test` drops dropped
  columns** at `state/db.rs:1435-1445` — mechanical fix-up after
  §4.2 M_03_03.
- **`get_quotas` deletion** at `state/db.rs:1470-` (old lines
  1277-1317) — human-gate B verbatim.
- **Test helper `insert_assistant_turns_after` at
  `state/db.rs:2042-2061`** — deterministic turn seeding for the
  new refresh-to-refresh delta tests.
- **Three new tests at `state/db.rs:2626-2729`** match §4.9
  bullets:
  `upsert_quota_refresh_writes_per_window_delta_for_matching_window_id`
  (§4.9 "refresh-to-refresh deltas land on
  `provider_quota_windows`"),
  `upsert_quota_refresh_carries_prior_window_delta_on_reset_or_no_change`
  (§4.9 "reset does not erase the last useful burn rate"),
  `quota_tight_routing_column_persisted_to_invocations`
  (§4.9 "soft-degrade routing writes the boolean column").
- **Mechanical `quota_tight_routing: false` on 13 existing
  `InvocationStart` literals in the state/db tests** — propagation
  of the new struct field.

### `src-tauri/src/main.rs` — §4.4 / §4.8 / §4.9

- **Imports `BalanceError`, `RiskClass`, `ValueEnum`** at
  `main.rs:1, 10`.
- **`Cli.risk_class: Option<RiskClassArg>` with `global = true`**
  at `main.rs:61-64` — §4.4 "`#[arg(long = "risk-class", …)]`";
  `global = true` documented in hookpoints §3.3 and in the
  proposal's Q6 rule 2 prose.
- **`RiskClassArg` enum + `From<RiskClassArg> for RiskClass`** at
  `main.rs:67-80` — clap `ValueEnum` shim so `balancer::RiskClass`
  doesn't need to depend on clap.
- **`resolve_risk_class` at `main.rs:212-244`** — implements the
  §Q6 cascade in the exact order: explicit flag → repl → env var
  → `-f/--file` → `OULIPOLY_PARENT_INVOCATION` → stdin-not-TTY →
  TTY default. Repl-above-env ordering matches §4.4 rule 2 and
  `research/03-load-balancing-tiers-answers.md:194-202`.
- **`run` / `run_with_balancing` / `run_repl` signature and
  call-site updates** at `main.rs:287, 295, 319, 346, 493,
  522-600, 672, 700-711, 722` — §4.4 and §4.8 call-site updates.
- **`emit_balance_error` at `main.rs:806-812`** — §4.8 "print a
  quota-exhausted stderr message, print
  `[diagnostics: quota_exhausted]`, and return exit code `1`."
- **Test helper `with_risk_envs` at `main.rs:902-948`** —
  env-locked guard mirroring the existing
  `with_parent_invocation_env` pattern noted in hookpoints §3.9.
- **Eight risk-class tests at `main.rs:1194-1322`** — match §4.9
  bullets one-for-one:
  `risk_class_cli_flag_overrides_env_var`,
  `risk_class_env_var_overrides_heuristic`,
  `risk_class_heuristic_classifies_file_flag_as_background`,
  `risk_class_heuristic_classifies_tty_prompt_as_user`,
  `risk_class_heuristic_classifies_parent_invocation_as_background`,
  `risk_class_heuristic_classifies_piped_stdin_as_background`,
  `repl_subcommand_always_user_class`, plus a
  `risk_class_flag_reaches_repl_subcommand` regression guard for
  `#[arg(global = true)]` — explicit, narrow test of the clap
  behavior documented in §4.4 rule 2 prose. In-scope.
- **Mechanical `quota_tight_routing: false` on four existing
  `InvocationStart` literals in main tests** — propagation of the
  new struct field.

### `src-tauri/src/lib.rs` — §4.3 / §4.4 / §4.8 / §4.9

- **`TestModelResult.error: Option<TestModelError>` at `lib.rs:31-32`**
  — §4.8 "Extend it to … `error: { category, message, model_name,
  risk_class, providers: [...] }`."
- **`TestModelError` + `TestModelProviderInfo` structs at
  `lib.rs:34-48`** — §4.8 JSON shape verbatim.
- **`save_model` calls `model.balancer.validate()` at `lib.rs:279`**
  — §4.3 "Validation rejects non-finite values and values outside
  `0.0..=1.0`, and rejects `user_threshold > failure_threshold`."
  The Tauri `save_model` path is the model-write boundary and
  must apply the same validation `ModelConfig::from_toml`
  applies, otherwise a UI-authored ModelConfig could bypass
  checks.
- **`test_model` now delegates to `test_model_with_db_path` +
  handles `BalanceError::Exhausted`** at `lib.rs:510-524` — §4.4
  "Tauri `test_model` hardcodes `RiskClass::User`." Function
  split is the minimum needed to add a unit-testable helper
  without running the async Tauri command machinery.
- **`test_model_error_from_exhausted` helper at `lib.rs:549-570`**
  — builds §4.8 structured error from `ExhaustedError`.
- **`test_model_for_test` helper at `lib.rs:572-584`** —
  `#[cfg(test)]` entry for the new `test_model_returns_structured_quota_exhausted_error`
  test (§4.9). Hits the real code path minus the Tauri command
  wrapper.
- **`test_model_returns_structured_quota_exhausted_error` test at
  `lib.rs:901-942`** — §4.9 "Tauri command returns the structured
  error shape without spawning a CLI."
- **Mechanical `balancer: Default::default()` on three existing
  `ModelConfig` literals in lib tests** — propagation of the §4.3
  struct field.

### `src-tauri/src/executor/cli.rs` — §4.3 propagation

All 13 hunks are mechanical `balancer: Default::default(),` lines
on existing `ModelConfig` test fixtures. No runtime-path
changes, no `quota_tight_routing` plumbing. Required because
`ModelConfig` gained the `balancer` field in §4.3.

### `src-tauri/src/quota/mod.rs` — §4.5 propagation

Two hunks: each adds `last_delta_percent: None, last_delta_calls:
None` to an existing `QuotaWindow` literal in the `dynamic_ttl_secs`
tests (`dynamic_ttl_secs_uses_shortest_window`,
`ttl_clamps_to_min_when_window_near_reset`). Required because
`QuotaWindow` gained two `Option` fields in §4.5. PR 2's
§3.7 scope gate ("no scoring redesign") is intact — these are
only test-fixture propagation, not a behavior change in
`quota/mod.rs`.

### `src-tauri/examples/quota_check.rs` — human-gate A

- **Imports `RiskClass`** at line 10.
- **`select_provider` call becomes `match … { Ok, Err }`** at
  `quota_check.rs:116-125` — passes `RiskClass::Background` per
  "diagnostic tooling is non-interactive" in hookpoints §7 A.
  Handles the new `Result<Selection, BalanceError>` return.
- **Drops provider-level delta readout** at `quota_check.rs:88-94` —
  hookpoints §7 A: "Drop any provider-level delta reads in that
  file — the example must compile against the post-PR-3 schema."
- **Prints `quota_tight` flag in the pick line** — diagnostic
  surface for the new signal, consistent with the example's role
  as a quota-readout tool.

### `src-tauri/tests/pr_b_trace_integration.rs` — §4.4 propagation

Two `quota_tight_routing: false` lines on existing `InvocationStart`
literals. Mechanical propagation only; the integration test is a
PR-B trace-rendering test, not a scoring test, and its semantics
are unchanged.

## Hunks that should move elsewhere

None.

## Non-blocking observations

- **`project_used_percent` floors at 0 but does not clamp at 1.**
  The §4.7 pseudocode writes `clamp(used_percent + turns *
  burn_rate, 0, 1)`, but the impl at `balancer/mod.rs:329-331`
  uses `.max(0.0)` only. The threshold check `projected >=
  failure_threshold` at `balancer/mod.rs:182` still correctly
  hard-blocks any >1.0 value, so user-visible behavior matches
  §4.7. The test
  `per_window_burn_rate_projects_short_window_faster_than_long`
  pins the un-clamped arithmetic (`short_projected - 0.10 ==
  (long_projected - 0.10) * 30.0`), which would fail under a
  hard upper clamp when the short side exceeds 1.0. The divergence
  from pseudocode is therefore a deliberate fit with the §4.9
  direction-check test, not a drift.

- **`ExhaustedError.providers` lists every eval, not only
  hard-blocked ones.** When a provider is unlearned (its
  projected_max_used_percent stays at 0), it still appears in
  the error surface with a 0 value. The §4.8 JSON example shows
  providers that are all above threshold, so this edge case
  isn't exercised. `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`
  seeds both providers with learned deltas, so the test passes.
  Consumers parsing this error surface should filter on
  `projected_max_used_percent >= failure_threshold` if they need
  only the hard-blocked rows; non-blocking for this PR.

- **Legacy `provider_quotas.used_percent` / `resets_at` mirror
  preserved.** `upsert_quota_refresh` still writes
  `legacy_used` / `legacy_resets` (longest window) to
  `provider_quotas` on every non-empty refresh
  (`state/db.rs:1325-1333`). §4.5 focuses on delta columns and
  does not explicitly flag this mirror for removal, and
  `get_quota` continues to return a `QuotaRecord` whose
  provider-level `used_percent`/`resets_at` fields are read by
  `quota::is_stale` via `dynamic_ttl_secs(get_windows(...))`
  (which uses the window rows, not the mirror). The mirror is
  effectively dead in the production read path after §4.5 but
  kept as "backwards-compat: keep used_percent/resets_at on
  provider_quotas in sync with the longest window so legacy
  readers see something sensible" per the in-file comment. A
  follow-up cleanup PR could remove both the mirror columns and
  the sync logic; out of scope for PR 3.

- **`risk_class_flag_reaches_repl_subcommand` is a test beyond
  the §4.9 list.** It asserts that `--risk-class` before `repl
  <model>` parses and reaches `resolve_risk_class`. §4.4 rule 2
  prose ("the `--risk-class` flag is marked `global = true`")
  implies this guarantee but §4.9 doesn't enumerate it. The test
  is a narrow regression guard, consistent with §4.4's design
  intent, and is in scope.

- **"Intentionally no test" block at `balancer/mod.rs:818-831`.**
  §4.9 bullet
  `unlearned_provider_is_ineligible_when_siblings_are_learned`
  is omitted as a discrete test. The attached comment justifies
  it from the §Q3 cascade (same-slot sibling rescue via step 2,
  longer-window rescue via step 3 — so the "A unlearned & B
  learned" state requires mismatched window_id layouts across
  providers, which is off-pattern). Companion tests
  `bootstrap_uses_sibling_pool_when_own_delta_absent` and
  `bootstrap_uses_duration_ratio_when_pool_has_only_long_delta`
  cover the actual reachable states. An explicit omission with
  justification is appropriate for a scope-constrained PR; the
  test-audit gate can reassess whether to resurrect it.

- **`test_model_for_test` parent-dir lookup** (`lib.rs:583`) uses
  `models_dir.parent().unwrap_or(&models_dir)`. In the single
  test that calls it, `models_dir` is
  `tempdir.path().join("models")`, so `.parent()` is the
  tempdir, where the test also puts `state.db`. The helper is
  `#[cfg(test)]` and its coupling to test-fixture layout is
  narrow; a reviewer who expects the production `test_model`
  path's `dirs::data_dir()` layout should not mistake this for
  production behavior.
