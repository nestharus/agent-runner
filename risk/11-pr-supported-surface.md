Termination signal: none
Verdict: LOW

## Termination signal

The Phase 2.5 problem framing is intact. Both root causes match what the
diff actually attacks: RC-1 (provider-local `is_stale` with no
sibling/peak comparison; missing-window penalty gated at `0.85`) is
addressed by an additive `is_topology_probe_due` helper plus a
topology-probe pass in `select_provider`, while RC-2 (plain
`max_by` argmax in `best_binding_score`) is addressed by replacing the
final selection line in `score_by_density` with
`select_binding_score_with_fanout`. No assumption from
`proposals/11-routing-fanout.md:127-135` is invalidated by the diff:
provider-name remains the keying identity (A1), `is_stale` is unchanged
so the visible-usage threshold A3 is preserved, the `2.0` band ratio is
encoded literally as `FANOUT_SCORE_BAND_RATIO` (A5), and the additive
schema migration matches A6.

Net value remains clearly positive. The two RED reproduction harnesses
exist under `src-tauri/tests/routing_fanout_rca/` and the Phase 7 r3 log
records `cargo test --no-fail-fast` PASS after the CodeRabbit pass 1
fixes (`risk/11-process-tree-audit-phase7.md:38`). Blast radius is
bounded to the routing-fanout problem map's adjacent surfaces, and
migration is forward-only and idempotent. Cost is far below the cost of
leaving 100%-to-one-provider concentration in place. Not
non_positive_value.

## A. Risk reduction on the supported surface

The diff lands on the supported routing path the problem map identifies
(`research/11-routing-fanout-problem-map.md:92-102`).

- RC-1: `select_provider` now computes per-provider `live_window_counts`
  and a `pool_expected_live_windows` value as `max(current, peak)` across
  the model, then probes any quota-script-equipped provider where
  `is_topology_probe_due` returns true (`src-tauri/src/balancer/mod.rs:127-188`).
  The probe records `last_topology_probe_at` and reloads the probed
  provider's quota/window rows before the existing exhausted filter and
  density branch (`:175-186`).
- RC-2: `score_by_density` no longer ends in `best_binding_score(...).index`
  but in `select_binding_score_with_fanout(model, state, &eligible)`
  (`src-tauri/src/balancer/mod.rs:249-260`). The new selector preserves
  hard-pin when `eligible.len() < 2`, when any score is non-finite, or
  when fewer than two providers fall inside `best / FANOUT_SCORE_BAND_RATIO`,
  matching the proposal at `proposals/11-routing-fanout.md:29-37`. Inside
  the band, it picks the lowest-invocation-count provider, then
  tie-breaks by higher binding score, then lower index — exactly the
  ordering the proposal specifies.

Both reproduction harnesses are in-tree
(`src-tauri/tests/routing_fanout_rca/rc1_incomplete_quota_topology.rs`,
`src-tauri/tests/routing_fanout_rca/rc2_argmax_concentration.rs`) and the
phase-7 r3 trace records the full `cargo test --no-fail-fast` PASS after
CodeRabbit pass-1 fixes (`risk/11-process-tree-audit-phase7.md:26,38`).

## B. Adjacent paths unchanged

- `refresh_quotas` IPC: `git diff main..HEAD -- src-tauri/src/lib.rs` is
  empty — `QuotaRefreshEntry`, `QuotaRefreshWindow`, the `is_stale`
  call, and the field set (`provider_name`, `status`, `windows`,
  `message`, `used_percent`, `resets_at`) are all preserved. The new
  topology probe is a routing-time pool decision in
  `balancer::select_provider` and is not invoked from the IPC path.
- `lib.rs::test_model_with_db_path`: not in the diff; it still calls
  `select_provider` with `None`, and the topology-probe block in
  `src-tauri/src/balancer/mod.rs:127-188` is gated by `if let Some(ctx)
  = ctx`, so the cached-only path is unchanged.
- `tests/rca_routing_claude_skipped.rs` (#25 harness): not in the diff;
  Phase 7 r3 confirms full test suite GREEN
  (`risk/11-process-tree-audit-phase7.md:26`).
- `state/repository.rs`, `session_replace/_export/_metadata`, `setup/`,
  frontend `src/`, `e2e/`: combined `git diff` against these paths is
  empty.

## C. Migration path safety

Forward-only via `StateDb::ensure_provider_quotas_topology_schema`,
slotted into `StateDb::open` between `ensure_provider_quotas_schema` and
`ensure_provider_quota_windows_schema` (`src-tauri/src/state/db.rs:670-672`).
For new DBs the columns are added in the `CREATE TABLE` block
(`src-tauri/src/state/db.rs:529-530`); for legacy DBs the helper does
guarded `ALTER TABLE` adds and a backfill `UPDATE` that uses
`MAX(topology_peak_live_window_count, COUNT(...))`
(`src-tauri/src/state/db.rs:1099-1136`). That `MAX` shape is the
CodeRabbit pass-1 R1-F06 self-healing fix: re-running the migration on
a DB that already has the column does not lower a previously learned
peak. The dedicated regression
`provider_quotas_topology_backfill_recovers_when_column_already_exists`
asserts the `already-high` row's `4` is preserved
(`src-tauri/src/state/db.rs:5023-5081`). Legacy rows survive a
downgrade as data; no compatibility shim exists in the diff.

## D. Rollback path

`git revert` of the WU PR plus restoring a pre-migration SQLite backup
if state rollback is required, exactly as named in
`proposals/11-routing-fanout.md:123` and the approved Phase 4
supported-surface report (`risk/11-supported-surface.md:66-72`).
Because the schema change is additive and the existing code paths use
explicit column names, legacy rows survive the downgrade as data.

## E. Observability

Both runtime branches emit deterministic `tracing::info!` events.

- Topology-probe-fired (`src-tauri/src/balancer/mod.rs:171-178`) carries
  `provider_name`, `live_window_count`, `pool_expected_live_window_count`,
  and `topology_peak_live_window_count` — exactly the proposal field
  set.
- Fanout-selected (`src-tauri/src/balancer/mod.rs:584-602`) carries
  `selected_provider_name`, `band_member_names`, `selected_invocation_count`,
  and `selected_binding_score`. `band_member_names` is built after the
  band is sorted by `eval.index` (`:546`), giving stable
  fixture-independent ordering.
- `tracing-subscriber` is wired in `main.rs::main`
  (`src-tauri/src/main.rs:2741-2744`) with `EnvFilter::from_default_env()`
  and `try_init`, so events surface at runtime when `RUST_LOG` requests
  them and the call is idempotent.

Log content carries no wall-clock fields, UUIDs, prompt text, or
secrets; provider names are already user-visible through `refresh_quotas`
output and pool/model configuration.

## F. Symbolic hardening check (algorithm vs. contract §5/§6)

Reading the algorithm against
`tmp/scratch/wu-11-01/contracts/wu-11-01-routing-fanout.md`:

- The probe-eligibility guard requires `live_window_count > 0` AND
  `live_window_count < pool_expected_live_windows` AND no current
  cooldown (`src-tauri/src/quota/mod.rs:204-220`); zero-window
  providers are deliberately excluded — that surface is owned by
  `is_stale`. The probe is only run for providers with a configured
  `quota_script` (`src-tauri/src/balancer/mod.rs:151-159`), matching
  the proposal's eligibility rule at `proposals/11-routing-fanout.md:18`.
- `record_topology_probe` writes the timestamp before
  `refresh_provider` runs (`src-tauri/src/balancer/mod.rs:170,179`),
  so a script that errors still moves the cooldown timer forward — the
  proposal's "including failed attempts" requirement
  (`proposals/11-routing-fanout.md:23`).
- `select_binding_score_with_fanout` matches contract §6: it computes
  `best` from positive scores, returns argmax when `best <= 0.0` or
  `!best.is_finite()`, requires `>= 2` band members, sorts by
  `eval.index` for deterministic input order, then orders by
  `(invocation_count asc, binding_score desc, index asc)`
  (`src-tauri/src/balancer/mod.rs:517-572`). The fanout log fires only
  when the selected index differs from argmax (`:573`), so the supported
  hard-pin branch produces no fanout-event noise.

The diff therefore reduces the user's observed concentration rather
than only matching the supported-surface label.

## G. Residuals impact

`risk/11-test-residuals.md` enumerates nine unencoded residuals: persistent
one-window probes beyond a single call, wide-score hard-pin extreme
ratios, long real-run distribution at hundreds of selections, real
upstream API rate safety, host clock skew, historical peak
reconstruction for never-cached topologies, peak decay after upstream
product changes, concurrent refresh races between IPC `refresh_quotas`
and runtime topology probing, and README/doc semantic automation. Each
residual self-classifies as temporal/concurrency, integration-hidden,
emergent-interaction, or bounded-model, and each row's
"whether the residual changes the net-value case" field is `no`. Read
together with the proposal's value statement
(`proposals/11-routing-fanout.md:162-166`), the WU's primary value —
fanout fix plus topology repair — is verified by the GREEN RC-1/RC-2
harnesses and the inline component/unit coverage at
`src-tauri/src/balancer/mod.rs:1208-1351` and
`src-tauri/src/state/db.rs:4884-5161`. Residuals are real follow-ups
but do not collapse the approved net-value case.

## Verdict rationale

The diff reduces the named, reproduced supported-surface risk via two
narrowly scoped code changes plus an additive, idempotent schema
column; preserves every adjacent surface enumerated in the problem
map (`refresh_quotas` IPC, `test_model_with_db_path`, the #25 harness,
session-turn ingestion, schema-keying); names and observably preserves
a forward-only migration with self-healing backfill; carries a clean
revert + backup rollback path; and adds deterministic, secret-free
`tracing::info!` events on both new branches with `tracing-subscriber`
wired in `main`. Verdict is LOW.
