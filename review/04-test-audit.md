# Test-Audit Gate: Initiative 04 — reactive routing via per-account exhausted flag

## Overall verdict: PARTIAL

The diff is a faithful implementation of `proposals/04-reactive-routing.md`
with three phase-7 amendments rolled in (past-reset window skip,
all-exhausted oldest-pick short-circuit, `mark_exhausted` upsert).
Spec alignment is PASS: the schema migration adds
`provider_quotas.exhausted_at` and drops `invocations.quota_tight_routing`
exactly as §2 specifies; the §3 delete inventory is fully removed
(all 9 categories, every named symbol scrubbed cleanly — only
`setup/actions.rs::CliSelection`, an unrelated struct, survives,
matching `risk/04-audit.md` §7); the §5 write path adds
`classify_exhaustion` and `mark_exhausted`, wires them into
`run_with_balancing` and `test_model_with_db_path`, and explicitly
defers `run_repl` per §D6; the §6 clear path piggybacks
`exhausted_at = NULL` onto the existing non-empty branch's
`ON CONFLICT DO UPDATE SET` in the same transaction; and the §7
filter excludes exhausted candidates with all-exhausted short-circuit.
Test quality is PASS for the three phase-7 amendments — each has a
purpose-built test that names the regression it is guarding. The
PARTIAL verdict is **coverage-delta, implementation-mode, not
blocking**: the §8 test plan named
`run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`
and the diff lands the production write site at `main.rs:677-682`
but ships no integration test that drives the failure-then-mark path
end-to-end. The Tauri-side parallel
(`test_model_marks_provider_exhausted_on_quota_stderr`) does cover
the write-site shape, so the regression risk is bounded to a
single-line edit on the CLI one-shot path.

Branch HEAD verified at `69486a0` (feat) on top of `ba20ced` (test);
no other commits on the branch.

## Sub-audit 1 — Spec alignment

Verdict: PASS

Walked proposal §2-§7 against the diff item-by-item.

### §2 Schema migration

- **`provider_quotas.exhausted_at` ADD branch** — fresh `CREATE TABLE`
  at `src-tauri/src/state/db.rs:393` includes `exhausted_at TEXT NULL`
  in the canonical list; `ensure_provider_quotas_schema` ALTER guard
  at `src-tauri/src/state/db.rs:622-628` mirrors the
  `last_empty_refresh_at` precedent. Idempotent (the existing-column
  check guards re-runs).
- **`invocations.quota_tight_routing` DROP branch** — the existing
  ADD branch at the pre-PR `state/db.rs:545-550` is replaced with a
  symmetric DROP at `src-tauri/src/state/db.rs:544-549` (`columns
  contains "quota_tight_routing" → ALTER TABLE invocations DROP
  COLUMN`). Fresh schema at `src-tauri/src/state/db.rs:702-712`
  drops the column from the canonical `CREATE TABLE`; the legacy
  rebuild (`migrate_legacy_invocations`) at `state/db.rs:797-825`
  drops the column from `invocations_new`, the insert column list,
  and the literal value. Migration test
  `quota_tight_routing_column_dropped_after_migration` at
  `src-tauri/src/state/db.rs:2409-2448` opens a temp DB seeded with
  the pre-04 schema, runs `StateDb::open`, and asserts
  `PRAGMA table_info(invocations)` no longer contains the column.
- **`map_invocation_row` column-index renumbering** — `state/db.rs:1097-1145`
  shifts created_at/finished_at from columns 13/14 to 12/13 to match
  the dropped column. Rusqlite `FromSqlConversionFailure` indices
  updated at lines 1100 and 1115.
- **Schema-ensure test coverage gap (non-blocking)** — there is no
  symmetric `exhausted_at_column_added_after_migration` test.
  Flagged in `risk/04-scope.md` Observation 2 as a non-blocker
  during phase 4 and remains non-blocking here: the column is
  exercised by every `mark_exhausted_*` and
  `upsert_quota_refresh_*_exhausted_at_*` test under fresh-DB
  initialization, so any regression in the ALTER path would still
  surface as a refresh-cycle column-not-found failure. Recommend
  the companion test as cheap defensive coverage.

### §3 Delete list

Walked all 9 sub-categories; every named symbol/file/line cited in
proposal §3.1-§3.9 is gone from the diff.

- **§3.1 `RiskClass`** — enum at
  `src-tauri/src/balancer/mod.rs:11-17` (pre-PR) deleted; CLI
  `RiskClassArg` + `From<RiskClassArg>` at `main.rs:62-82` (pre-PR)
  deleted; `lib.rs:35-50` (pre-PR) `TestModelError.risk_class` +
  `TestModelProviderInfo` deleted; `examples/quota_check.rs:10`
  import removed. Compile-fallout `use balancer::RiskClass`
  imports per `D8` at `main.rs:865` and `lib.rs:812` are gone.
- **§3.2 `Selection`** — struct deleted; `select_provider` returns
  `usize` (`balancer/mod.rs:30-34`); REPL/one-shot/Tauri callers
  now use the index directly (`main.rs:519`, `:602`,
  `lib.rs:506`); `examples/quota_check.rs:122` reads the bare
  `usize`.
- **§3.3 `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo`**
  — all three structs and `exhausted_error` helper deleted from
  `balancer/mod.rs`; `emit_balance_error` deleted from
  `main.rs:810-816` (pre-PR); REPL and one-shot caller branches
  for `Err(BalanceError::Exhausted(_))` deleted; Tauri preflight
  exhausted mapping deleted from `lib.rs:519-568` (pre-PR).
- **§3.4 `BalancerConfig`, thresholds, `[balancer]`** —
  `BalancerConfig` struct, `RawBalancerBlock`, `parse_balancer`,
  `append_balancer_toml`, and `validate()` all deleted from
  `src-tauri/src/config/model.rs`; `model.balancer.validate()`
  removed from `save_model` (`lib.rs:260`); threshold reads in
  `score_by_density` deleted; README `[balancer]` block deleted.
- **§3.5 `--risk-class`, `OULIPOLY_RISK_CLASS`, `resolve_risk_class`,
  `with_risk_envs`** — all deleted from `main.rs` (the `clap`
  `--risk-class` arg, the `RiskClassArg` enum, the resolver, the
  test helper). `cli.risk_class.map(Into::into)` removed from the
  REPL dispatch (`main.rs:227-233`). README `--risk-class` row
  removed (`README.md:120-130` pre-PR). Eight named cascade tests
  deleted from `main.rs:1198-1325` (pre-PR).
- **§3.6 `quota_tight_routing`** — column dropped (above);
  `InvocationRecord.quota_tight_routing` and
  `InvocationStart.quota_tight_routing` fields removed from
  `state/db.rs:138-166` (pre-PR); the SELECT lists, `start_invocation`
  insert, and `map_invocation_row` updated; warning emission in
  `run_repl` and `run_with_balancing` removed
  (`main.rs:598-600`, `:711-713` pre-PR); 18+
  `quota_tight_routing: false` literals scrubbed from
  `balancer/mod.rs`, `state/db.rs`, `main.rs`,
  `executor/cli.rs`, and `tests/pr_b_trace_integration.rs`. The
  pinned test `quota_tight_routing_column_persisted_to_invocations`
  is deleted (`state/db.rs:2929-2951` pre-PR), replaced by the
  migration-shape test described above.
- **§3.7 `TestModelResult.error` / `TestModelError` /
  `TestModelProviderInfo`** — all three deleted from `lib.rs`;
  `test_model_with_db_path` returns the plain
  `{success, stdout, stderr, exit_code}` shape; the structured
  preflight error test deleted from `lib.rs:902-942` (pre-PR).
- **§3.8 `ProviderEval.hard_blocked` / `user_blocked` /
  `max_projected_used_percent` / soft-degrade** — `ProviderEval` at
  `balancer/mod.rs:11-16` is now `{ index, binding_score, unlearned }`;
  threshold gating in `score_by_density` deleted; `hard_eligible`
  / `user_eligible` partition deleted, replaced by single
  `eligible` filter (`!unlearned && binding_score.is_some()`);
  user soft-degrade branch deleted; recent-error avoidance now
  signals "skip" by emitting `ProviderEval { binding_score: None,
  unlearned: false }` rather than `hard_blocked: true,
  user_blocked: true` (resolves the implementation-detail gap
  flagged in `risk/04-scope.md` Observation 1; the `binding_score:
  None` route into the `eligible.is_empty()` round-robin fallback
  is identical in observable behavior to the pre-PR
  hard-blocked-everywhere path).
- **§3.9 Tests pinning delete behavior** — all 19 enumerated tests
  are gone:
  - 4 balancer threshold/error tests
    (`user_threshold_hides_provider_from_user_class_only`,
    `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`,
    `failure_threshold_hard_blocks_all_classes`,
    `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`).
  - 8 main risk cascade tests at `main.rs:1198-1325` (pre-PR).
  - 5 config balancer tests at `config/model.rs:1180-1278` (pre-PR).
  - 1 state quota-tight persistence test
    (`quota_tight_routing_column_persisted_to_invocations`).
  - 1 Tauri structured preflight error test
    (`test_model_returns_structured_quota_exhausted_error`).
  - The other 10 balancer tests named in §3.9 / §8 (those whose
    behavior remains but the API shape changed) are mechanically
    edited to the `usize` signature, not deleted, exactly as the
    proposal directs.

### §5 New write path

- **`classify_exhaustion(stderr: &str) -> bool`** at
  `src-tauri/src/diagnostics/mod.rs:37-40` extracts the existing
  three-substring heuristic (`"quota"`, `"billing"`, `"usage limit"`,
  lowercase-matched) into a pure helper. `heuristic_diagnosis` at
  `diagnostics/mod.rs:115` now delegates to the helper, so the
  shared classifier behavior is preserved verbatim per `D7`.
- **`QuotaRecord.exhausted_at: Option<DateTime<Utc>>`** added at
  `src-tauri/src/state/db.rs:72`; `get_quota` SELECT list updated
  at `state/db.rs:1213-1233` to read column 2 as
  `Option<DateTime<Utc>>`.
- **`mark_exhausted`** at `src-tauri/src/state/db.rs:1243-1262`
  — **deviates from proposal §5** (which spelled out a plain
  `UPDATE` with no insert): the diff implements an `INSERT ... ON
  CONFLICT (provider_name) DO UPDATE SET exhausted_at = excluded.exhausted_at`.
  This is the documented phase-7 CodeRabbit pass-1 amendment
  (comment block at `state/db.rs:1244-1252`) — under the
  proposal's UPDATE-only shape, a first-call quota-exhausted
  failure for a provider with no prior `provider_quotas` row
  silently dropped the write, leaving the account eligible for
  immediate re-routing on the next call (a guaranteed re-failure
  the reactive model exists to prevent). The amendment narrows
  rather than widens scope: same single-statement, same
  `provider_name` key, same `exhausted_at = now()` semantics; only
  the corner case "row didn't exist" now lands the flag instead
  of being a no-op.
- **`run_with_balancing` write site** at
  `src-tauri/src/main.rs:677-682` — after `error_category` is
  computed from `run_diagnostics` and before
  `finalize_invocation`, calls `state.mark_exhausted(provider_name)`
  when `error_category == Some("quota_exhausted")`. Outside the
  `finalize_invocation` transaction per proposal §5; failure
  `unwrap_or_else(|e| eprintln!(...))` matches the existing
  best-effort logging of provider-state writes elsewhere in
  `run_with_balancing`.
- **Tauri `test_model_with_db_path` write site** at
  `src-tauri/src/lib.rs:506-508` — after `executor::execute`
  returns, when `result.exit_code != 0` and
  `classify_exhaustion(&result.stderr)` is true, calls
  `db.mark_exhausted(&model.providers[provider_index].name)?`
  before constructing the plain `TestModelResult`. Note: this
  path uses `?` (returns Err) rather than `unwrap_or_else`,
  matching the rest of the Tauri command's error-propagation
  style.
- **`run_repl`** — confirmed NOT-implemented per `D6`. The
  `select_provider` call at `main.rs:523` returns `usize` and the
  diff has no `mark_exhausted` call in the REPL path. Consequence
  documented in answers §D6 / proposal §11.

### §6 New clear path

`upsert_quota_refresh` at `src-tauri/src/state/db.rs:1311-1466`
preserves the existing transactional structure unchanged. The
non-empty branch's `INSERT ... ON CONFLICT DO UPDATE SET` at
`state/db.rs:1393-1405` adds `exhausted_at = NULL` to the
`UPDATE SET` clause. This is in the same transaction as the
window delete/replace per proposal §6 — concurrent readers see
either old quota+old flag or new quota+cleared flag, never a
mix. The empty branch at `state/db.rs:1326-1370` writes only
`refreshed_at` and `last_empty_refresh_at`; `exhausted_at` is
preserved (verified by both code reading and the
`upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`
test).

The diff implements the audit-noted optimization
(`risk/04-audit.md` §1 minor shape note): a single
`ON CONFLICT DO UPDATE SET` rather than an additional UPDATE.
Both forms are correct; this saves one SQL roundtrip per refresh.

### §7 New filter

`select_provider` at `src-tauri/src/balancer/mod.rs:30-110`:

- Quota and window vectors are populated unchanged from the
  pre-PR per-provider `get_quota`/`get_windows` reads.
- `filtered_indices` (lines 70-80) excludes any provider whose
  `quotas[i].exhausted_at.is_some()`. Providers with no quota
  row (the `quota.is_none()` case) are NOT excluded — matches
  proposal §7's "Providers with no quota row are not excluded."
- All-exhausted short-circuit (lines 81-99) — **deviates from
  proposal §7** (which directed "use the unfiltered provider list
  so the balancer always returns a provider, matching answers
  Q4") and from answers Q4 itself ("Fall through to
  `round_robin_fallback` (invocation-count)"). The diff instead
  picks the provider with the oldest `exhausted_at` (ties break
  on index), short-circuiting before density/invocation-count
  scoring. This is the documented phase-7 CodeRabbit pass-2
  amendment (comment block at `balancer/mod.rs:81-88`) — under
  the proposal's "fall through to round-robin" semantics, a pool
  where every account is exhausted would route to the
  lowest-invocation-count provider on every subsequent call
  (because invocation counts continue to advance after marking),
  which the comment frames as "spamming already-exhausted
  accounts on every invocation" and "contradicts the user-locked
  'wait until refresh' invariant." The oldest-`exhausted_at`
  pick is the best-guess "most likely to have recovered on its
  next refresh" heuristic.
- The filter applies to both density (`score_by_density(..., candidates)`,
  line 105) and invocation-count fallback
  (`score_by_invocation_count(..., candidates)`, line 109), each
  receiving the candidate slice. `round_robin_fallback` also
  takes `candidates` (line 377) with a `debug_assert!` on
  non-empty (since the empty case is now intercepted by the
  short-circuit, callers downstream of the filter always see a
  non-empty slice).
- No caching introduced. `state.get_quota(...)` is called per
  provider per `select_provider` call exactly as before; the
  filter reads the field on the freshly-loaded `QuotaRecord`.
  Test `exhausted_filter_does_not_prevent_refresh_loop_from_clearing`
  pins the cleared-by-refresh-then-eligible path.

### Phase-7 amendments — also explicitly checked

1. **Past-reset window skip** at `balancer/mod.rs:149-161` —
   `score_by_density` skips windows whose `resets_at <= now` so the
   stale `used_percent` and the EPS_HOURS-clamped hours-until-reset
   no longer torpedo an otherwise-healthy provider's binding
   score. Pinned by `score_by_density_skips_past_reset_windows`
   (`balancer/mod.rs:594-631`) which seeds a healthy 7d window +
   a past-reset 5h window on provider `a` and asserts `a` wins
   over a heavily-used 7d-only `b`. The test reproduces the
   2026-04-22 live-caught regression scenario described in the
   inline comment.
2. **All-exhausted oldest-pick** at `balancer/mod.rs:81-99` —
   covered above. Pinned by
   `all_providers_exhausted_picks_oldest_exhausted`
   (`balancer/mod.rs:567-592`) which marks `b` exhausted FIRST
   (older timestamp), `a` exhausted SECOND, and asserts `b`
   (index 1) wins despite invocation count being equal — the
   oldest-flag heuristic, not invocation count, is the
   tie-breaker.
3. **`mark_exhausted` upsert** at `state/db.rs:1243-1262` —
   covered above. Pinned by `mark_exhausted_creates_row_when_missing`
   (`state/db.rs:2344-2374`) which calls `mark_exhausted` for a
   provider with no prior quota row and asserts both
   `COUNT(*) == 1` and `exhausted_at IS NOT NULL` after the
   call.

## Sub-audit 2 — Test quality

Verdict: PASS

Twelve net-new tests on this branch, organized by surface:

### `state/db.rs` (5 net new)

- **`mark_exhausted_writes_timestamp_on_existing_quota_row`**
  (`state/db.rs:2324-2342`) — happy path. Seeds a row via
  `upsert_quota_refresh`, calls `mark_exhausted`, brackets the call
  with `Utc::now()` before/after and asserts the parsed
  `exhausted_at` falls within ±1s of the bracket. Fails on
  baseline (`mark_exhausted` does not exist, `exhausted_at` does
  not exist).
- **`mark_exhausted_creates_row_when_missing`**
  (`state/db.rs:2344-2374`) — phase-7 amendment pin. Asserts both
  the row-existence (`COUNT(*) == 1`) and the timestamp
  presence after marking a never-refreshed provider. Comment
  block calls out the regression class explicitly. Would fail on
  the proposal's UPDATE-only shape.
- **`upsert_quota_refresh_clears_exhausted_at_on_nonempty_refresh`**
  (`state/db.rs:2376-2389`) — clear path happy. Seeds a row,
  marks exhausted, refreshes with one window, asserts
  `exhausted_at` is NULL.
- **`upsert_quota_refresh_preserves_exhausted_at_on_empty_refresh`**
  (`state/db.rs:2391-2406`) — clear path negative. Seeds a row,
  marks exhausted, refreshes with `&[]`, asserts the timestamp is
  bit-identical to the pre-refresh value (string compare).
  Pins the §6 "do not clear on empty refresh" guarantee.
- **`quota_tight_routing_column_dropped_after_migration`**
  (`state/db.rs:2409-2448`) — migration shape. Hand-creates the
  pre-04 `invocations` schema with `quota_tight_routing` BOOLEAN,
  reopens via `StateDb::open`, asserts the column is gone via
  `PRAGMA table_info`. The hand-rolled `CREATE TABLE` matches
  every other column of the pre-PR schema, so the test would
  catch a regression where the DROP branch silently no-ops or is
  bypassed.

### `diagnostics/mod.rs` (2 net new)

- **`classify_exhaustion_matches_quota_billing_usage_limit_stderr`**
  (`diagnostics/mod.rs:144-156`) — three-case happy with mixed
  case ("QUOTA", "Billing", "USAGE LIMIT") to pin the
  `to_lowercase()` step.
- **`classify_exhaustion_ignores_non_quota_errors`**
  (`diagnostics/mod.rs:158-172`) — five-case negative covering
  auth, network, compile, unknown-flag, generic-failure stderr.
  Tight enough to fail a regression that broadens the heuristic
  (e.g., adding "limit" alone as a substring).

### `balancer/mod.rs` (4 net new)

- **`select_provider_filters_exhausted_accounts`**
  (`balancer/mod.rs:555-565`) — happy path. Seeds two providers
  where density would normally pick `a` (lower used_percent),
  marks `a` exhausted, asserts `b` wins.
- **`all_providers_exhausted_picks_oldest_exhausted`**
  (`balancer/mod.rs:567-592`) — phase-7 amendment pin. Detailed
  in §1 above. The 10ms `thread::sleep` between the two
  `mark_exhausted` calls intentionally creates a deterministic
  timestamp ordering so the oldest-pick assertion is stable.
- **`score_by_density_skips_past_reset_windows`**
  (`balancer/mod.rs:594-631`) — phase-7 amendment pin. Detailed
  in §1 above. Seeds the exact 5h-past-reset + 7d-healthy shape
  from the live regression.
- **`exhausted_filter_does_not_prevent_refresh_loop_from_clearing`**
  (`balancer/mod.rs:633-649`) — `D5` pin. Marks both providers
  exhausted, then directly calls `upsert_quota_refresh("b", ...)`
  with one window to simulate the refresh-loop clear, asserts
  `b` (the cleared provider) wins. The comment correctly notes
  this is a state-transition simulation rather than an end-to-end
  refresh-loop drive — the production refresh loop's own
  test surface lives in `quota/mod.rs`.

### `lib.rs` (1 net new)

- **`test_model_marks_provider_exhausted_on_quota_stderr`**
  (`lib.rs:843-877`) — Tauri write-site happy. Configures a
  one-provider model whose command is `sh -c "echo quota
  exhausted >&2; exit 7"`, drives `test_model_for_test`,
  asserts the provider's `quota.exhausted_at` is set after the
  call. `#[cfg(unix)]` correctly gates the `sh` dependency.
  Closes the named §8 test
  `test_model_marks_provider_exhausted_on_quota_stderr`.

### Mechanical edits (not net new tests)

`single_provider_always_zero`, `round_robin_on_fresh_state`,
`avoids_errored_providers`, `density_scoring_picks_lowest_used_when_windows_match`,
`density_picks_account_with_more_time_when_used_equal`,
`binding_constraint_avoids_account_with_pressed_short_window`,
`falls_back_to_invocation_count_when_windows_missing`,
`high_weekly_account_stops_winning_after_cumulative_turns`,
`bootstrap_uses_sibling_pool_when_own_delta_absent`, and
`fresh_pool_falls_through_to_invocation_count_round_robin` all keep
their behavioral assertions but are mechanically updated to the
`usize` return shape (helper `selected_provider_index` reduces to a
one-liner; `selected_provider` helper deleted because there is no
`Selection` to read auxiliary fields from). The
`fresh_pool_falls_through_to_invocation_count_round_robin` test
loses the `assert!(!selection.quota_tight_routing)` line but keeps
the index assertion — matches §3.9 directive "delete only the
`quota_tight_routing` assertion inside the fresh-pool test."

## Sub-audit 3 — Coverage delta

Verdict: PARTIAL (implementation-mode, not blocking)

Two named §8 tests not present:

1. **`run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`**
   — the production write site is wired at `main.rs:677-682`,
   gated on `error_category.as_deref() == Some("quota_exhausted")`,
   but no test drives a failing-subprocess-then-mark integration
   path through `run_with_balancing`. The Tauri test
   (`test_model_marks_provider_exhausted_on_quota_stderr`)
   covers the parallel write site at `lib.rs:506-508`, but the
   one-shot CLI path traverses additional plumbing:
   `executor::execute` (different flow from
   Tauri `executor::execute`), `run_diagnostics`, the
   `error_category.as_deref()` comparison, and the
   `unwrap_or_else(|e| eprintln!(...))` error path. A
   regression that, for example, swapped the comparison to
   `error_category.is_some()` (firing for every error category
   not just quota), or that dropped the `mark_exhausted` call
   itself from the conditional, would not be caught by any
   existing test. Blast radius: bounded to a single-line edit
   on the CLI one-shot path; the parallel Tauri site catches
   the analogous Tauri-side regression. Recommended follow-up
   sketch:

   ```rust
   #[cfg(unix)]
   #[test]
   fn run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics() {
       // Set up a one-provider model whose subprocess prints
       // "quota exceeded" to stderr and exits non-zero.
       // Call run_with_balancing through the CLI dispatch.
       // Assert provider_quotas.exhausted_at is set after the call
       // returns.
   }
   ```

2. **Symmetric migration ADD test for `exhausted_at`** —
   `quota_tight_routing_column_dropped_after_migration` covers
   the DROP, but no companion
   `exhausted_at_column_added_after_migration` test seeds a
   pre-04 `provider_quotas` schema and asserts the column is
   added. Carried from `risk/04-scope.md` Observation 2; same
   phase-4 status as here (non-blocking). Recommend the
   one-test follow-up.

Branches covered (mapping to proposal §):

- §2 schema migration DROP: `quota_tight_routing_column_dropped_after_migration` ✓
- §2 schema migration ADD (existing-DB ALTER path): not directly tested
  (covered indirectly by every fresh-DB test using `exhausted_at`)
- §5 `classify_exhaustion` heuristic: positive + negative ✓
- §5 `mark_exhausted` happy: ✓
- §5 `mark_exhausted` first-use upsert (phase-7 amendment): ✓
- §5 `run_with_balancing` write-site integration: not tested (gap)
- §5 `test_model_with_db_path` write-site integration: ✓
- §5 `run_repl` deferred per D6: documented absence
- §6 clear on non-empty refresh: ✓
- §6 preserve on empty refresh: ✓
- §7 filter happy: `select_provider_filters_exhausted_accounts` ✓
- §7 all-exhausted oldest-pick (phase-7 amendment): ✓
- §7 refresh-clears-then-filter (D5 invariant): ✓
- §7 past-reset window skip (phase-7 amendment): ✓

Branches NOT covered (other than the named-test gap above):

- **Multi-pool spillover** — an account exhausted via one model
  pool excludes it from every model pool that routes through it
  (`provider_name` keying per `Q3`). No dedicated test seeds
  two `ModelConfig` instances sharing a `provider_name` and
  asserts the cross-pool exclusion. The single-key shape of
  `provider_quotas` makes a regression here unlikely (the
  filter reads `state.get_quota(name)` per call, so any
  cross-pool routing gets the same flag), but a defensive test
  would close the loop on the user-locked per-account framing.
- **Heuristic miss path** — proposal §11 / answers §D7 commit to
  the existing three-substring heuristic and document the
  graceful degradation when real quota stderr uses different
  phrasing. There is no test asserting that a known false
  negative (e.g., `"plan_limit_reached"`) leaves the flag
  unset. Acceptable per §D7's "future broadening as a separate
  PR against the shared diagnostics module" framing.
- **`mark_exhausted` failure path in `run_with_balancing`** —
  the `unwrap_or_else(|e| eprintln!(...))` swallow path at
  `main.rs:679-680` is not exercised. Hard to fault: contriving
  a `mark_exhausted` failure requires a corrupt DB state, and
  the swallow-and-warn pattern matches the rest of
  `run_with_balancing`.

## Blocking issues

None. Proposal §8 also flagged a contradiction between the
`run_repl_marks_provider_exhausted_on_quota_stderr` test bullet
and the §D6 deferral (caught in `risk/04-scope.md` Observation 4
and `risk/04-shortcut.md` "One coherence wart"). The diff resolves
that contradiction the right way: no `run_repl_*` test exists,
matching §5 / §11 / §D6 unanimously. The proposal §8 bullet
should be cleaned up in a followup doc edit, but that is not a
gate.

## Non-blocking observations

- **Add the named §8 test
  `run_with_balancing_marks_provider_exhausted_on_quota_exhausted_diagnostics`.**
  One-line regression risk on the CLI one-shot mark-exhausted
  conditional that no existing test would catch. Sketch in §3
  above. Highest-priority follow-up.
- **Add `exhausted_at_column_added_after_migration`.** Companion
  to the existing DROP migration test; same shape with the
  pre-04 `provider_quotas` schema seeded as `last_empty_refresh_at`-
  only. Cheap defensive coverage.
- **The `mark_exhausted` upsert is a substantive proposal
  deviation, justified.** Proposal §5 explicitly says "No
  insert, no retry, no error on zero affected rows; a missing
  quota row is a no-op by design." The diff replaces this with
  an upsert. The justification (first-call quota-exhausted
  failures must land the flag, otherwise the next call routes
  back to the same broken account) is sound and the inline
  comment block calls out the change clearly. The rationale
  belongs in the commit message body for future archaeology;
  both commit bodies are currently empty. Recommend the
  amendment be documented in commit-trailer or PR-description
  text before merge.
- **The all-exhausted oldest-pick is also a substantive proposal
  deviation, justified.** Proposal §7 / answers `Q4` directed
  fall-through-to-`round_robin_fallback`; the diff instead
  short-circuits to oldest-`exhausted_at`. The justification
  (round-robin would re-route into known-exhausted accounts on
  every invocation, contradicting the no-spam invariant that
  governs reactive routing's whole reason to exist) is sound,
  and the inline comment block calls it out. Same recommendation
  as above re: commit-message documentation. Note that the
  semantic shift means the named §8 test
  `all_providers_exhausted_falls_through_to_round_robin` was
  renamed to `all_providers_exhausted_picks_oldest_exhausted`;
  the proposal text is now divergent from both the code and the
  test name.
- **The past-reset window skip is a behavior change to projection
  math, justified, but slightly outside proposal §4's "do not
  change projection math" framing.** The skip materially changes
  what `score_by_density` ranks (a window with `resets_at <=
  now` is silently dropped from the binding-score fold). The
  inline comment explains the live-caught regression and the
  poisoned-binding-score mechanism that triggered it. The
  semantic is correct (a past-reset row carries a
  prior-window-instance `used_percent`, which has no bearing on
  current headroom), but the proposal's keep-list at §4 says
  "projection remains the ranking signal: `score_by_density`
  still computes projected window usage and binding score from
  remaining headroom times hours to reset" — the past-reset
  skip is outside that wording. Future readers should be told,
  in the commit message body, that this is a separate phase-7
  fix riding alongside the reactive-routing concern.
- **`examples/quota_check.rs` switched from `None` to
  `Some(&ctx)` for the `BalanceContext` argument.** The pre-PR
  call site passes `None`; the new call site passes `Some(&ctx)`
  using a `BalanceContext` constructed earlier in `main()`. This
  enables opportunistic refresh on every example run, which
  changes the example's behavior (it now performs network/script
  I/O via `refresh_provider`). Not justified by proposal §10
  ("revert to the simpler balancer API: drop `RiskClass`,
  drop `Result`, print only the selected provider index/name").
  Behaviorally harmless for an example binary, but worth either
  reverting to `None` for proposal-fidelity or naming this in
  the commit message as an intentional polish.
- **Empty commit-message bodies.** Both commits (`ba20ced` test,
  `69486a0` feat) have only the subject line. The phase-7
  amendments (past-reset skip, all-exhausted oldest-pick,
  upsert) are documented in inline code comments but not in the
  commit messages. Future archaeology — `git log -p
  src-tauri/src/balancer/mod.rs` — would not surface the
  CodeRabbit-pass attribution that the comments carry. Phase-8
  isn't the place to re-author commits, but the merge commit
  (or PR description) should carry the amendment summary so the
  blame trail is searchable.
