Termination signal: none
Verdict: LOW

## Termination signal

The approved Phase 2.5 problem framing is intact. The proposal's "Problem"
section faithfully reproduces both root causes from the problem map: RC-1
(provider-local `is_stale` with no sibling/peak topology comparison;
missing-window penalty gated at `0.85` so a low-used single-window provider
escapes it) and RC-2 (`best_binding_score` is a plain `max_by` argmax with
no fanout term). Citations align with the problem map at
`research/11-routing-fanout-problem-map.md:67-76` and with current code at
`src-tauri/src/balancer/mod.rs:12-20`, `:288-295`, `:438-454` and
`src-tauri/src/quota/mod.rs:183-217`. No assumption is invalidated.

Net value is clearly positive on the supported surface (single-user Tauri
desktop app + CLI runner with local SQLite, multi-provider pools). The
user-observed risk — 100%-to-one-provider routing reproduced by both RED
harnesses (`tests/routing_fanout_rca/rc1_*`, `rc2_*`) — is reduced by two
narrowly scoped fixes (`proposals/11-routing-fanout.md:11-46`). Added burden
is one additive SQLite migration with two columns (`:60-87`) and two
deterministic `tracing::info!` events. That cost is far below the cost of
leaving concentration in place. Not non_positive_value.

## A. Risk reduction on the supported surface

The two fixes land exactly on the supported routing paths the problem map
identifies (`research/11-routing-fanout-problem-map.md:92-102`). Fix 1
inserts a topology-aware probe into `select_provider` only after the
existing stale-refresh/session-scan loop and only when a `BalanceContext` is
present (`proposals/11-routing-fanout.md:17-21`); this attacks RC-1
directly. Fix 2 replaces the final `best_binding_score(...).index` line in
`score_by_density` with a deterministic score-band selector (`:29-37`); this
attacks RC-2 directly. Both correspond to the AC-1/AC-2 GREEN-flip
requirement in the ticket.

## B. Blast radius vs. value

Blast radius is bounded to the problem map's "Adjacent surfaces inside the
blast radius" set. The proposal does not change `is_stale` semantics — it
adds an additive `is_topology_probe_due` helper while `refresh_quotas`
continues to call the provider-local `is_stale` contract
(`proposals/11-routing-fanout.md:19,22,114`). The `refresh_quotas` IPC
response shape and `QuotaWindow.used_percent` field name are explicitly
held unchanged in anti-scope (`:93-94`), and `lib.rs::test_model_with_db_path`
keeps its `BalanceContext = None` call which gates out the new probe path
(`:117`). The #25 harness is anti-scope and the supported-surface track
names it explicitly as remaining green (`:97`, `:156`). No callers of
`select_provider` see a return-type change (`:116`). Schema-keying remains
provider-name-identity per Assumption A1 (`:129`); the new column names
preserve that contract.

## C. Migration path safety

The migration is forward-only and additive: two new columns on
`provider_quotas`, slotted into `StateDb::open` after the existing
`ensure_provider_quotas_schema` call via a new `ensure_provider_quotas_topology_schema`
helper that follows the same `ensure_*_schema` pattern already used by the
DB module (`proposals/11-routing-fanout.md:67-87`). Legacy rows are
backfilled from `provider_quota_windows` row counts — a stable in-DB
source — and `last_topology_probe_at` is left NULL so the first observed
topology mismatch can probe immediately. `provider_quota_windows` keeps
its current shape, so adjacent learning paths in `upsert_quota_refresh`
are not invalidated (`:65,89`).

## D. Rollback path

Named: `git revert` of the WU PR, plus restoring a pre-migration SQLite
backup if state rollback is required (`proposals/11-routing-fanout.md:123`).
Because the schema change is additive and the existing code uses explicit
column names, legacy rows survive the downgrade as data — no manual DB
surgery is required for normal rollback.

## E. Observability

Two deterministic `tracing::info!` events are specified
(`proposals/11-routing-fanout.md:24,36,125`):
topology-probe-fired (`provider_name`, `live_window_count`,
`pool_expected_live_window_count`, `topology_peak_live_window_count`) and
fanout-selected (`selected_provider_name`, `band_member_names`,
`selected_invocation_count`, `selected_binding_score`). Use of `info`
rather than `debug` is justified: the user must verify in production
traffic that the RC-1/RC-2 repair branches actually fired. Events carry
no wall-clock, UUID, secret, prompt, or path fields, and provider names
are already user-visible through `refresh_quotas` IPC. Persistent in-DB
signals (`topology_peak_live_window_count`, `last_topology_probe_at`,
`provider_quota_windows` counts, per-provider invocation aggregate) round
out the observability story; `quota_check` remains the diagnostic CLI
view.

## F. Adjacent paths covered

Each adjacent surface in the problem map's blast-radius section appears
in the supported-surface track:

- `run_with_balancing` and the interactive CLI path —
  `proposals/11-routing-fanout.md:112-113`.
- `refresh_quotas` IPC — `:114` (response shape unchanged; not affected
  by the routing-time probe).
- `lib.rs::test_model_with_db_path` — `:117` (probe gated by
  `BalanceContext` presence).
- `state/db.rs` learning loop / `upsert_quota_refresh` — `:118` (only
  new write is non-empty refreshes raising `topology_peak_live_window_count`;
  empty refreshes do not lower it).
- Session-turn ingestion — `:119` (no read-path change).
- #25 harness `tests/rca_routing_claude_skipped.rs` — `:97` and test-intent
  row `:156` (must remain green; full `cargo test --no-fail-fast`
  required).
- Schema-keying tests — Assumption A1 (`:129`) plus the explicit migration
  test `provider_quotas_topology_columns_created_and_backfilled` (`:150`).

## Verdict rationale

The proposal reduces a real, reproduced supported-surface risk; bounds
blast radius via two narrow code changes plus an additive schema column;
specifies a forward-only migration with a stable backfill source; names
a clean rollback path; adds deterministic, secret-free observability for
both new branches; and addresses every adjacent surface enumerated in
the problem map. Verdict is LOW.
