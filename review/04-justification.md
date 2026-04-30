# Justification: Initiative 04 — reactive routing

## Verdict: JUSTIFIED

Every hunk in `feat/04-reactive-routing` (commits `ba20ced` test +
`69486a0` feat) maps to one of four traceable sources:

1. **Problem research §1** delete inventory (`research/04-reactive-routing-problem.md:1-119`)
2. **Answers §D1–§D8** (`research/04-reactive-routing-answers.md:114-228`)
3. **Proposal §2–§10** (`proposals/04-reactive-routing.md`)
4. **Phase-7 CodeRabbit amendments** — past-reset window skip,
   all-exhausted oldest-pick short-circuit, and `mark_exhausted`
   upsert. These are documented in inline code comments
   (`balancer/mod.rs:81-88`, `:150-158`, `state/db.rs:1244-1252`)
   but NOT in the commit-message bodies (both bodies are empty).
   The prompt's claim that the amendments are "documented in the
   commit message" is not currently accurate against the branch;
   the inline-comment trail is what carries the attribution.

No hunk falls outside these sources. Three minor observations on
the trace:

- The `examples/quota_check.rs` `None → Some(&ctx)` argument
  swap is inside the §10 cleanup but is a behavioral expansion
  (the example now performs `refresh_provider` I/O on every run).
  Justified-but-loose against §10's "drop `Result` handling, and
  print only the selected provider index/name" framing.
- The phase-7 past-reset window skip is just outside proposal
  §4's "Do not change projection math" framing (the skip
  materially alters which windows enter the binding-score fold).
  Justified under (4) as a CodeRabbit amendment but readers
  expecting strict §4 fidelity should know that scope expanded
  during convergence.
- Three §3 / §8 named tests are not present in the diff but no
  unjustified test surface appears either; gaps documented in
  `review/04-test-audit.md` §3 (coverage delta).

## Hunks kept

### `src-tauri/src/balancer/mod.rs`

- **Delete `RiskClass` enum + serde derive**
  (pre-PR `:11-17`). Source: §3.1, hookpoints §2.
- **Delete `Selection` struct** (pre-PR `:19-23`). Source: §3.2,
  `D3`.
- **Delete `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo`**
  (pre-PR `:25-55`) and the `exhausted_error` helper (pre-PR
  `:272-289`). Source: §3.3, `Q4`.
- **`ProviderEval` field reduction** to `{ index, binding_score,
  unlearned }` (`:11-16`). Source: §3.8 — drops
  `hard_blocked`, `user_blocked`, `max_projected_used_percent`.
- **`select_provider` signature revert** to
  `(model, state, ctx) -> usize` (`:30-34`). Source: §3.2,
  §3.3, `D3`.
- **Single-provider early return** changed from
  `Selection { provider_index: 0, quota_tight_routing: false }`
  to bare `0` (`:35-38`). Source: §3.2.
- **Quota/window vector population unchanged** (`:59-68`). Source:
  §4 keep list — the `get_quota` / `get_windows` reads remain
  the per-call source-of-truth for the new filter.
- **Candidates filter** at `:69-80` (`filtered_indices` excluding
  providers whose `quota.exhausted_at.is_some()`). Source: §7,
  `D4`.
- **All-exhausted oldest-pick short-circuit** at `:81-99`.
  **Source: phase-7 CodeRabbit pass-1 amendment** (inline
  comment block at `:81-88`). This deviates from proposal §7 +
  `Q4` (which directed fall-through-to-round-robin); the
  amendment is justified by the no-spam invariant from
  `tmp/init04-scope-anchor.md:9-13` — round-robin would re-route
  into known-exhausted accounts on every invocation.
- **Density vs invocation-count branch** at `:101-109`. Both paths
  now take the `candidates` slice. Source: §7 — "apply the
  candidate list to both scoring paths."
- **Past-reset window skip** at `:149-161` inside
  `score_by_density`'s window-fold loop. **Source: phase-7
  amendment, inline comment block**. Live-caught 2026-04-22 on
  claude3 when anthropic-usage returned `{"windows":[]}` and the
  preserve-on-empty path kept a past-reset row alive. Justified
  by the same projection-as-ranking invariant the rest of
  initiative 04 sits on top of, but materially alters
  `score_by_density`'s window-fold semantics — a reader expecting
  proposal §4 strict-fidelity would not predict this hunk.
- **Threshold gating removal in `score_by_density`** — deleted
  comparison to `model.balancer.failure_threshold` and
  `user_threshold`, removed `hard_blocked`/`user_blocked`
  assignment (`:162-183` becomes the simpler binding-score-only
  path). Source: §3.4, §3.8.
- **Eligible filter simplification** at `:185-188`
  (`!eval.unlearned && eval.binding_score.is_some()`). Source:
  §3.8 — replaces `hard_eligible` / `user_eligible` partition
  with a single eligibility check.
- **Empty-eligible round-robin fallback** at `:190-192`. Source:
  §3.8 — collapses the prior unlearned-fallback / hard-eligible-
  empty branches into one path.
- **User soft-degrade and quota-tight return paths deleted**
  (pre-PR `:230-251`). Source: §3.4, §3.6, §3.8.
- **`score_by_invocation_count` and `round_robin_fallback`
  refactored** to take `candidates: &[usize]` (`:345-399`).
  Source: §7 — both paths must respect the exhausted filter.
  Adds a `debug_assert!(!candidates.is_empty())` to
  `round_robin_fallback` (`:378-381`) because the all-exhausted
  short-circuit now intercepts the empty case earlier; the
  assert documents the new invariant.
- **`record_invocation_for_test` literal cleanup** (`:421`) —
  drops `quota_tight_routing: false` from the test helper's
  `InvocationStart`. Source: §3.6.
- **`two_provider_model` and `three_provider_model` fixtures**
  (`:428-451`) drop `balancer: Default::default()`. Source:
  §3.4, `D3`.
- **`single_provider_always_zero`** (`:453-463`) — `ModelConfig`
  literal drops `balancer`; assertion adapted to `usize` return.
  Source: §3.2, §3.4.
- **`round_robin_on_fresh_state`, `avoids_errored_providers`**
  (`:465-489`) — same mechanical adapt to `usize`. Source: §3.2.
- **`selected_provider_index` helper reduction** (`:544-546`) —
  one-liner dropping `RiskClass` arg and `Selection.provider_index`
  read. Source: §3.1, §3.2.
- **`selected_provider` helper deletion** (pre-PR `:642-645`) —
  no `Selection` to return any auxiliary fields from. Source:
  §3.2.
- **NEW: `select_provider_filters_exhausted_accounts`**
  (`:555-565`). Source: §8 named test +
  `Q1` / `D4`.
- **NEW: `all_providers_exhausted_picks_oldest_exhausted`**
  (`:567-592`). Source: phase-7 amendment + reframed §8 named
  test (renamed from `_falls_through_to_round_robin`).
- **NEW: `score_by_density_skips_past_reset_windows`**
  (`:594-631`). Source: phase-7 amendment.
- **NEW: `exhausted_filter_does_not_prevent_refresh_loop_from_clearing`**
  (`:633-649`). Source: §8 named test + `D5`.
- **`density_scoring_picks_lowest_used_when_windows_match`,
  `density_picks_account_with_more_time_when_used_equal`,
  `binding_constraint_avoids_account_with_pressed_short_window`,
  `falls_back_to_invocation_count_when_windows_missing`,
  `high_weekly_account_stops_winning_after_cumulative_turns`,
  `bootstrap_uses_sibling_pool_when_own_delta_absent`,
  `fresh_pool_falls_through_to_invocation_count_round_robin`**
  — all retained with assertions intact, mechanically adapted to
  `selected_provider_index(model, db) -> usize`. Source: §3.9 —
  "Existing behavioral tests that still cover kept behavior
  should be mechanically updated to the reverted
  `select_provider -> usize` API instead of deleted." The
  fresh-pool test loses the `quota_tight_routing` assertion per
  §3.9.
- **DELETE: `user_threshold_hides_provider_from_user_class_only`,
  `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`,
  `failure_threshold_hard_blocks_all_classes`,
  `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`**.
  Source: §3.9.

### `src-tauri/src/state/db.rs`

- **`MAX_LEARNABLE_BURN_RATE`, `MIN_LEARN_SAMPLE_CALLS`,
  `NEAR_EXHAUSTED_USED_PERCENT` doc-comment edits** at `:13-44` —
  prose tweaks replacing `failure_threshold` references with
  "near the ceiling" / "nearly exhausted" framing now that
  thresholds are gone. Source: §3.4 (cascading prose update for
  the threshold deletion).
- **`QuotaRecord.exhausted_at` field add** at `:72`. Source: §5,
  hookpoints §3.
- **`InvocationRecord.quota_tight_routing` and
  `InvocationStart.quota_tight_routing` field deletes** at
  `:138-166` (pre-PR). Source: §3.6.
- **Fresh `provider_quotas` schema** at `:393` adds
  `exhausted_at TEXT NULL`. Source: §2.
- **`ensure_invocations_schema` add → drop branch swap** at
  `:544-549`. Source: §2 — the existing-DB ALTER guard now drops
  the column instead of adding it.
- **`ensure_provider_quotas_schema` ADD branch** at `:622-628`.
  Source: §2 — mirrors the `last_empty_refresh_at` precedent at
  the same scope.
- **`invocations_schema_sql` column delete** at `:702-712`.
  Source: §2.
- **`migrate_legacy_invocations` column delete** at `:797-825`
  (drops the column from `invocations_new`'s `CREATE TABLE`,
  the insert column list, and the literal `0` value). Source:
  §2.
- **`start_invocation` insert SQL update** at `:907-923` — drops
  `quota_tight_routing` from column list and drops
  `start.quota_tight_routing as i64` from the `params!` macro.
  Source: §3.6.
- **`get_invocation_by_uuid` and `get_child_invocations` SELECT
  list updates** at `:1051-1097` (pre-PR) — drop the column
  from the SELECT list. Source: §3.6.
- **`map_invocation_row` column-index renumbering** at `:1097-1145`
  — created_at / finished_at slide from columns 13/14 to 12/13.
  Source: §3.6 (forced follow-through of the SELECT list edit).
- **`get_quota` SELECT list update** at `:1213-1233` — adds
  `exhausted_at` to the SELECT and the row mapping. Source: §5.
- **NEW: `mark_exhausted`** at `:1243-1262`. **Source: §5 +
  phase-7 CodeRabbit pass-1 amendment** (the upsert semantics
  rather than proposal §5's plain UPDATE; inline comment block
  at `:1244-1252`). Justified by the first-call quota failure
  scenario.
- **`upsert_quota_refresh` non-empty branch addition** at
  `:1393-1405` — adds `exhausted_at = NULL` to the
  `ON CONFLICT DO UPDATE SET` clause. Source: §6 + the
  audit-noted optimization (`risk/04-audit.md` §1 minor shape
  note: "the cleaner implementation is to extend the existing
  `INSERT ... ON CONFLICT DO UPDATE SET` ... rather than a
  separate UPDATE").
- **Empty-branch unchanged** at `:1326-1370`. Source: §6 — "must
  not clear exhausted state."
- **NEW test helpers `exhausted_at_raw` and `exhausted_at`** at
  `:2179-2197`. Source: §8 — needed for the four
  `mark_exhausted_*` and `upsert_quota_refresh_*` tests.
- **`insert_invocation_fixture` literal cleanup** at `:2211`.
  Source: §3.6.
- **NEW: `mark_exhausted_writes_timestamp_on_existing_quota_row`**
  at `:2324-2342`. Source: §8.
- **NEW: `mark_exhausted_creates_row_when_missing`** at
  `:2344-2374`. Source: phase-7 amendment + §8 (reframed from
  `_is_noop_when_no_quota_row`).
- **NEW: `upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh`**
  at `:2376-2389`. Source: §8.
- **NEW: `upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`**
  at `:2391-2406`. Source: §8.
- **NEW: `quota_tight_routing_column_dropped_after_migration`**
  at `:2409-2448`. Source: §8.
- **DELETE: `quota_tight_routing_column_persisted_to_invocations`**
  (pre-PR `:2929-2951`). Source: §3.6, §3.9.
- **18+ existing-test `InvocationStart` literal cleanups** at
  `:3105`, `:3131`, `:3147`, `:3156`, `:3175`, `:3200-3207`,
  `:3250`, `:3267`, `:3306`, `:3341`, `:3371`, `:3401-3408`,
  `:3681` — drop `quota_tight_routing: false`. Source: §3.6
  cascading mechanical follow-through.
- **`upsert_quota_refresh` doc-comment prose tweaks** at `:2942`
  and `:3012` — replace `failure_threshold` references with
  "near the ceiling" framing. Source: §3.4 follow-through.

### `src-tauri/src/diagnostics/mod.rs`

- **NEW: `classify_exhaustion`** at `:37-40`. Source: §5,
  hookpoints §3, `D7` (extract heuristic unchanged).
- **`heuristic_diagnosis` delegation** at `:115` — the in-line
  `lower.contains(...)` chain is replaced with a call to the new
  helper. Source: §5 — keeps `heuristic_diagnosis` behavior
  identical (the inline-vs-delegated check is the same
  computation), preserves `D7`'s "use the existing classifier
  unchanged" framing.
- **NEW: `classify_exhaustion_matches_quota_billing_usage_limit_stderr`**
  at `:144-156`. Source: §8.
- **NEW: `classify_exhaustion_ignores_non_quota_errors`** at
  `:158-172`. Source: §8.

### `src-tauri/src/lib.rs`

- **DELETE: `TestModelResult.error` field** at `:31-33` (pre-PR).
  Source: §3.7, `D2`.
- **DELETE: `TestModelError`, `TestModelProviderInfo`** at
  `:35-50` (pre-PR). Source: §3.7, `D2`.
- **DELETE: `model.balancer.validate()?` from `save_model`** at
  `:266-280` (pre-PR). Source: §3.4.
- **`test_model_with_db_path` rewrite** at `:498-516` — drops
  `Result<Selection, BalanceError>` match + preflight error
  mapping; calls `select_provider` for `usize` directly; adds
  `mark_exhausted` write site after `executor::execute`. Source:
  §3.7, §5, `D2`.
- **DELETE: `test_model_error_from_exhausted` helper** at
  `:548-568` (pre-PR). Source: §3.7.
- **DELETE: `use balancer::RiskClass` test-module import** at
  `:812` (pre-PR). Source: `D8`.
- **`make_model` fixture** at `:766` drops `balancer:
  Default::default()`. Source: §3.4.
- **Two more in-line `ModelConfig` literals** at `:809-825`
  (pre-PR) drop `balancer`. Source: §3.4.
- **DELETE: `test_model_returns_structured_quota_exhausted_error`**
  (pre-PR `:902-942`). Source: §3.7, §3.9.
- **NEW: `test_model_marks_provider_exhausted_on_quota_stderr`**
  at `:843-877`. Source: §8.

### `src-tauri/src/main.rs`

- **Imports cleanup** at `:1-10` (pre-PR) — drop `BalanceError`,
  `RiskClass`, `ValueEnum`. Source: §3.1, §3.3, §3.5.
- **DELETE: `cli.risk_class` arg + `RiskClassArg` enum +
  `From<RiskClassArg>` impl** at `:62-82` (pre-PR). Source:
  §3.1, §3.5.
- **DELETE: `resolve_risk_class`** at `:212-244` (pre-PR).
  Source: §3.5.
- **`run` dispatch update** at `:227-233` — drops
  `cli.risk_class.map(Into::into)` from the `run_repl` call;
  drops `risk_class` from `resolve_risk_class` + propagation.
  Source: §3.5.
- **`run_with_balancing` and `run_repl` signature updates** at
  `:425-433` and `:592-598` — drop the `risk_class` /
  `risk_class_override` parameters. Source: `D3`, §10.
- **`run_repl` selection branch** at `:519-523` — drops the
  `Result<Selection, _>` match + the quota-tight warning;
  reduces to bare `select_provider` call. Source: §3.2, §3.3,
  §3.6.
- **`run_repl` `InvocationStart` literal** at `:537-542` drops
  `quota_tight_routing`. Source: §3.6.
- **`run_with_balancing` selection branch** at `:602` — same
  reduction. Source: §3.2, §3.3, §3.6.
- **`run_with_balancing` `InvocationStart` literal** at `:626-632`
  drops `quota_tight_routing`. Source: §3.6.
- **NEW: `mark_exhausted` write site** at `:677-682`. Source:
  §5.
- **DELETE: `emit_balance_error`** at `:810-816` (pre-PR).
  Source: §3.3.
- **DELETE: `use agent_runner_lib::balancer::RiskClass` test-module
  import** at `:865` (pre-PR). Source: `D8`.
- **DELETE: `with_risk_envs` test helper** at `:906-953` (pre-PR).
  Source: §3.5.
- **DELETE: 8 risk cascade tests** at `:1198-1325` (pre-PR).
  Source: §3.5, §3.9.
- **`InvocationStart` literal cleanups** at `:1084`, `:1171`,
  `:1199`, `:1227` — drop `quota_tight_routing`. Source: §3.6.

### `src-tauri/src/config/model.rs`

- **DELETE: `ModelConfig.balancer` field** at `:208` (pre-PR).
  Source: §3.4.
- **DELETE: `BalancerConfig` struct + `Default` + `validate`**
  at `:214-251` (pre-PR). Source: §3.4.
- **DELETE: `RawBalancerBlock` + `RawModelToml.balancer` field**
  at `:316-333` (pre-PR). Source: §3.4.
- **DELETE: `parse_balancer` and `append_balancer_toml`** at
  `:648-673` (pre-PR). Source: §3.4.
- **`ModelConfig::from_toml` and `to_toml` cleanup** — drop
  `balancer` from the constructed model and from the serialized
  output. Source: §3.4.
- **DELETE: 5 balancer config tests** at `:1180-1278` (pre-PR).
  Source: §3.4, §3.9.

### `src-tauri/src/executor/cli.rs`

- **9 `ModelConfig` test fixture cleanups** — each drops
  `balancer: Default::default()`. Source: §3.4 mechanical
  follow-through (cited in §10 "Removing `ModelConfig.balancer`
  has broad compile fallout in test fixtures").

### `src-tauri/tests/pr_b_trace_integration.rs`

- **Two `InvocationStart` literal cleanups** at `:73-99` — drop
  `quota_tight_routing: false`. Source: §3.6, §10.

### `src-tauri/examples/quota_check.rs`

- **Import cleanup** at `:10` — drop `RiskClass` from the
  import list. Source: §3.1.
- **`select_provider` call rewrite** at `:115-122` — drops
  `Result<Selection, _>` match, drops `RiskClass::Background`
  argument, drops `quota_tight=` printout. Source: §3.1, §3.2,
  §3.6, §10.
- **`None → Some(&ctx)` argument swap** at `:116`. Source:
  §10 (loose). Justified-but-loose: §10 specifies "drop
  `RiskClass::Background`, drop `Result` handling, and print
  only the selected provider index/name" but does not direct a
  `BalanceContext` change. The swap enables opportunistic
  refresh on every example run, which is a behavioral expansion
  for the example binary. Not a scope blocker (an example
  binary's network-call behavior has no production impact), but
  worth flagging as the only hunk in the diff that does not
  trace cleanly to a §3-§10 directive. Recommend either reverting
  to `None` for proposal-fidelity or naming the change in the
  commit-message body.

### `README.md`

- **DELETE: `--risk-class` CLI option row** at `:117-130`
  (pre-PR). Source: §9.
- **DELETE: `### Risk classes` Load Balancing subsection** at
  `:217-234` (pre-PR), including the `[balancer]` TOML block.
  Source: §9.
- **NEW prose** for per-window scoring + reactive exhausted-flag
  behavior. Source: §9 — "change the eligibility prose:
  projection ranks providers, recent-error avoidance still
  deprioritizes noisy providers, and a provider that actually
  fails with quota/billing/usage-limit stderr is marked
  exhausted at the account level until the next successful
  non-empty quota refresh clears it."

## Hunks that don't trace cleanly

One. The `examples/quota_check.rs` `None → Some(&ctx)` swap is
described above. Not a scope creep — the diff is small and the
behavior is harmless for an example binary — but it is the only
hunk that requires either loose interpretation of §10 or a
phase-7-amendment-style explicit "this is intentional polish"
note that the commit message would carry. Currently it carries
neither.

## Hunks that did NOT happen but should have

Per `review/04-test-audit.md` §3, two named §8 tests are absent:

- `run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`
  — production write site is wired (`main.rs:677-682`) but no
  test drives it end-to-end.
- `exhausted_at_column_added_after_migration` — companion to the
  existing DROP migration test.

Neither is a justification gap (no extra hunks shipped that
shouldn't have); both are coverage-delta gaps where a hunk that
should exist is missing. Documented in the test-audit gate.

## Cross-cutting cleanups

The diff contains no cross-cutting refactors outside the
reactive-routing concern. Specifically checked for:

- **`scripts/` changes** — none. Initiative-04 is a Rust-only
  change.
- **CI / GitHub Actions changes** — none.
- **`agents/` config changes** — none.
- **Frontend (`src/`) changes** — none. `D2` confirmed the
  frontend type already matched the reverted shape.
- **Unrelated dependency bumps** — none. `Cargo.toml` is
  untouched.
- **Drive-by formatting / clippy fixes** — none. The diff does
  not run `cargo fmt --all` or clippy auto-fix; only the
  reactive-routing surfaces are touched.

The doc-comment prose tweaks at `state/db.rs:13-44`, `:2942`,
`:3012` (replacing `failure_threshold` references with "near the
ceiling" / "nearly exhausted" framing) are inside the §3.4
threshold-deletion concern — they would otherwise leave broken
prose pointing at a deleted concept.

## Summary

JUSTIFIED. Every hunk traces to problem research §1, answers
§D1-§D8, proposal §2-§10, or a phase-7 CodeRabbit amendment
documented in inline code comments. One hunk
(`examples/quota_check.rs:116` `None → Some(&ctx)`) is loosely
justified under §10 and would benefit from explicit commit-message
attribution. Two named §8 tests are absent (coverage gap, not a
justification gap). The phase-7 amendments are the only
substantive proposal deviations: the `mark_exhausted` upsert
narrows scope (handles a corner case the proposal said was
intentional no-op), the all-exhausted oldest-pick widens scope
(replaces fall-through-to-round-robin with a new ranking
heuristic), and the past-reset window skip widens scope (changes
`score_by_density`'s window-fold semantics). All three are
defensible as CodeRabbit-driven refinements of the same
reactive-routing concern; the multi-concern gate
(`review/04-multi-concern.md`) confirms none of them is
genuinely separable.
