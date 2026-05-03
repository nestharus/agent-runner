Verdict: LOW_CONCERN

# WU-11-01 Phase 8 — Justification Review

Scope: every change in `git diff main..HEAD` (commit `74f05e5`) is checked against
the proposal's stated purpose (eliminate single-provider concentration in
`claude-opus` and `gpt-high` pools), the ticket Anti-scope, the contract §2/§5/§6/§7
in-scope code surfaces, AC-1..AC-5, and the audit/CodeRabbit history.

## Code changes

### `src-tauri/src/balancer/mod.rs`

- `FANOUT_SCORE_BAND_RATIO = 2.0` (`src-tauri/src/balancer/mod.rs:21`) — encodes AC-2's
  "within ~2x" boundary per contract §4 (`product-strategy/contracts/wu-11-01-routing-fanout.md:74`)
  and proposal Fix 2 (`proposals/11-routing-fanout.md:31`).
- Topology-probe pass in `select_provider` (`src-tauri/src/balancer/mod.rs:128`-`186`) —
  matches contract §5 algorithm steps 2–3 (`product-strategy/contracts/wu-11-01-routing-fanout.md:121`-`134`)
  and proposal RC-1 (`proposals/11-routing-fanout.md:17`-`24`). Gated on
  `BalanceContext` per §5 invariant (preserves `lib.rs::test_model_with_db_path`).
- Conversion of `eligible` from `Vec<&ProviderEval>` to `Vec<ProviderEval>` at
  `src-tauri/src/balancer/mod.rs:249`-`253` — required by the new
  `select_binding_score_with_fanout(&[ProviderEval])` signature (contract §4
  function signature, line 98–103). Smallest type-shape change consistent with
  the new selector.
- `select_binding_score_with_fanout` (`src-tauri/src/balancer/mod.rs:517`-`604`) —
  algorithm matches contract §6 steps 1–5 (`product-strategy/contracts/wu-11-01-routing-fanout.md:139`-`148`)
  and proposal Fix 2 (`proposals/11-routing-fanout.md:29`-`30`). Tiebreak order
  (count → higher score → lower index) is the contract's exact tiebreak.
- Two `tracing::info!` event sites at `src-tauri/src/balancer/mod.rs:160`-`166`
  (topology probe fired) and `src-tauri/src/balancer/mod.rs:594`-`600`
  (fanout selected non-argmax) — match contract §7 field lists
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:159`-`172`) and proposal
  observability (`proposals/11-routing-fanout.md:24`,`36`); no wall-clock /
  UUID / path / secret fields.
- Five new inline tests (`src-tauri/src/balancer/mod.rs:1209`-`1352`):
  `density_fanout_uses_invocation_counts_within_score_band`,
  `density_fanout_tiebreaks_by_score_then_index`,
  `density_hard_pins_when_score_gap_exceeds_band`,
  `topology_probe_refreshes_incomplete_cached_provider_before_density`,
  `topology_probe_respects_cooldown_for_persistent_short_topology`. All five
  appear verbatim in contract §9 expected test set
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:191`-`197`) and
  proposal Test-intent rows 3–7 (`proposals/11-routing-fanout.md:143`-`147`).
- Two new test helpers `provider_eval` and `providers_config_with_scripts`
  (`src-tauri/src/balancer/mod.rs:821`-`844`) — used only by the five new tests;
  not drive-by additions.

### `src-tauri/src/quota/mod.rs`

- `TOPOLOGY_PROBE_COOLDOWN_SECS = 60 * 60` (`src-tauri/src/quota/mod.rs:21`) —
  contract §4 (`product-strategy/contracts/wu-11-01-routing-fanout.md:73`),
  proposal (`proposals/11-routing-fanout.md:20`).
- `is_topology_probe_due` (`src-tauri/src/quota/mod.rs:204`-`221`) — signature and
  invariants match contract §4 / §5 invariants
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:81`-`87`,`129`).
- Two new inline tests
  `topology_probe_due_when_below_expected_and_no_probe_timestamp` and
  `topology_probe_not_due_when_counts_match_or_cooldown_active`
  (`src-tauri/src/quota/mod.rs:540`-`587`) — contract §9 lines 201–202;
  proposal Test-intent rows 8–9 (`proposals/11-routing-fanout.md:148`-`149`).
- `is_stale` is unchanged — preserves contract §11 conflict-resolution
  ordering (`product-strategy/contracts/wu-11-01-routing-fanout.md:235`).

### `src-tauri/src/state/db.rs`

- `QuotaRecord` extension fields (`src-tauri/src/state/db.rs:148`-`149`) — match
  contract §4 `QuotaRecord` extension (`product-strategy/contracts/wu-11-01-routing-fanout.md:107`-`112`).
- `provider_quotas` schema additions in `CREATE TABLE` block
  (`src-tauri/src/state/db.rs:529`-`530`) and `ensure_provider_quotas_topology_schema`
  (`src-tauri/src/state/db.rs:1099`-`1138`) — implement proposal Schema and
  migration (`proposals/11-routing-fanout.md:47`-`87`) and contract §4 migration
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:60`-`69`). Slotted in
  `StateDb::open` between `ensure_provider_quotas_schema` and
  `ensure_provider_quota_windows_schema` (`src-tauri/src/state/db.rs:671`),
  exactly the position the proposal specifies.
- The `MAX(topology_peak_live_window_count, COUNT(...))` self-healing form of
  the backfill at `src-tauri/src/state/db.rs:1126`-`1135` is the CodeRabbit
  Pass 1 R1-F06 fix (`tmp/scratch/wu-11-01/coderabbit/loop-summary.md:18`) —
  applied during Phase 7.
- `get_quota` SELECT extension (`src-tauri/src/state/db.rs:1836`-`1858`) —
  required to surface the new `QuotaRecord` fields.
- `upsert_quota_refresh` peak-monotonic update
  (`src-tauri/src/state/db.rs:2018`-`2042`) — contract §4 directive "modify
  `upsert_quota_refresh` to update `topology_peak_live_window_count` to
  `max(prev, new)` only on non-empty refresh"
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:28`); the empty-input
  path leaves the peak alone, satisfying the proposal's monotonicity claim
  (`proposals/11-routing-fanout.md:23`).
- `record_topology_probe` (`src-tauri/src/state/db.rs:2107`-`2118`) — contract §4
  signature (`product-strategy/contracts/wu-11-01-routing-fanout.md:90`-`94`) and
  proposal data flow (`proposals/11-routing-fanout.md:23`).
- Four new inline tests at `src-tauri/src/state/db.rs:4889`-`5106`:
  `provider_quotas_topology_columns_created_and_backfilled`,
  `provider_quotas_topology_backfill_recovers_when_column_already_exists`,
  `upsert_quota_refresh_updates_topology_peak_without_lowering_on_shrink`,
  `record_topology_probe_sets_timestamp_without_changing_windows`. Three are in
  the contract §9 expected list (`product-strategy/contracts/wu-11-01-routing-fanout.md:204`-`208`);
  the fourth (`provider_quotas_topology_backfill_recovers_when_column_already_exists`)
  is the regression test for the CodeRabbit R1-F06 idempotency fix
  (`tmp/scratch/wu-11-01/coderabbit/loop-summary.md:18`) — Phase 7-justified.
- Two new test helpers `quota_window_detail_rows` and
  `last_topology_probe_at_raw` (`src-tauri/src/state/db.rs:3446`-`3514`) — used
  only by the new tests.

### `src-tauri/src/main.rs`

- Tracing subscriber init at `src-tauri/src/main.rs:2741`-`2744` — contract §7
  default option (a) ("Step 6c picks (a) by default";
  `product-strategy/contracts/wu-11-01-routing-fanout.md:175`). Required for the
  two `tracing::info!` events in §7 to be visible at runtime per hookpoints L136
  ("current operational messages use `eprintln!`"). Single-line addition; no
  unrelated `main.rs` edits.

### `src-tauri/Cargo.toml` and `Cargo.lock`

- New dependencies `tracing = "0.1"` and `tracing-subscriber = "0.3"` features
  `["env-filter"]` (`src-tauri/Cargo.toml:29`-`30`). Justified by contract §7
  default option (a). The contract specifically permits this dependency
  ("`tracing` adds a non-trivial dependency the project does not already have"
  was the only escape hatch to fall back to `eprintln!`, and Step 6c picked
  the default subscriber path). Lockfile entries are the consequence.

### `src-tauri/tests/routing_fanout_rca/` (and its top-level shim)

- `tests/routing_fanout_rca.rs` (runner shim) and
  `tests/routing_fanout_rca/mod.rs` shared module are listed as anti-scope
  preservation in the proposal (`proposals/11-routing-fanout.md:98`). They are
  "new" against `main` because the worktree was branched from
  `rca/routing-fanout` (per `audit-history.md` Artifact lineage); merging the
  WU-11-01 PR is the first time main sees these harnesses. This is the
  expected RED→GREEN regression-test handoff.
- `rc1_incomplete_quota_topology.rs` uses the relative-time fixture
  (`Duration::hours(80)` long, `Duration::hours(5)` short) and
  `used_percent = 80` for `claude`'s short window. Both choices are explicitly
  prescribed in contract §13 Round 2 / Round 3 follow-up
  (`product-strategy/contracts/wu-11-01-routing-fanout.md:259`-`295`); the
  iteration trail is recorded in `audit-history.md` Round 2 and Round 3.
- `rc2_argmax_concentration.rs` is the Phase 0 RC-2 reproduction harness
  unchanged in shape.
- `mod.rs` `seed_learned_windows` reset uses `Duration::hours` rather than
  `Duration::seconds(... * 3600)`. This is the CodeRabbit R1-F01 fix
  (`tmp/scratch/wu-11-01/coderabbit/loop-summary.md:16`).

### `README.md`

- Two updates (`README.md:228`-`246`): drops "pick = argmax" sentence from the
  per-window binding-rate scoring bullet, adds a paragraph on deterministic
  score-band fanout, adds a paragraph on the topology probe and its one-hour
  cooldown. Implements AC-5 (ticket WU-11-01.md AC-5 lines 64–68; proposal
  Test-intent row 17 at `proposals/11-routing-fanout.md:158`).

## Doc/artifact changes

These are workflow phase artifacts; each is the orchestrator-mandated output of
its phase. They are not drift:

- `proposals/11-routing-fanout.md` — Phase 3 proposal.
- `product-strategy/contracts/wu-11-01-routing-fanout.md` — Phase 6a orchestrator
  contract (with §13 Round 2/3 contract revisions documented in audit-history).
- `research/11-routing-fanout-{rca,problem-map,hookpoints}.md` — Phase 0 RCA,
  Phase 2.5 problem map, Phase 5 hookpoints.
- `risk/11-{audit,scope,shortcut,supported-surface,test-residuals}.md` — Phase 4
  risk gates (LOW after Round 2 revise) and Phase 6b residual artifact.
- `risk/11-process-tree-audit-phase{4,6}.md` — Phase 4/6 process-tree audits.

## Drift / drive-by check

I looked specifically for changes that don't trace to AC-1..AC-5, contract
§2/§5/§6/§7, or the proposal §Design and §Schema. None found:

- No edits to anti-scope surfaces: `refresh_quotas` IPC shape unchanged
  (no diff in `lib.rs` quota response paths); `state/repository.rs` not
  introduced; `session_replace/`, `session_export/`, `session_metadata/`,
  `setup/`, `src/`, `e2e/` untouched; `tests/rca_routing_claude_skipped.rs`
  unchanged.
- No new abstractions, services, DI shims, or backwards-compatibility readers.
- No stochastic fanout / RNG.
- `main.rs` has only the 4-line subscriber init — within the ticket's allowed
  "minor only" main.rs scope (`WU-11-01.md` Code Boundary).
- The two test-side fixture corrections (relative-time + `used_percent = 80`)
  are not "drive-by cleanup" of the test — they are explicit contract §13
  Round 2/3 corrections that the test/code separation rule routed through
  contract revisions rather than ad-hoc test edits.
- The CodeRabbit-applied fixes (R1-F01 reset durations, R1-F06 idempotent
  backfill plus its regression test, plus the doc-only R1-F02/R1-F04) are
  Phase 7 cleanup that ships in this PR by the workflow's design — not
  unrelated drive-bys.

## Verdict justification

Every meaningful code, dep, schema, test, doc, and artifact change traces to
AC-1, AC-2, AC-3, AC-4, AC-5, contract §2/§4/§5/§6/§7, the proposal's §Design
and §Schema, contract §13 Round 2/3, or a Phase 7 CodeRabbit applied finding
documented in `tmp/scratch/wu-11-01/coderabbit/loop-summary.md`. No
speculative abstraction, unrelated refactor, or anti-scope violation observed.

Verdict: **LOW_CONCERN**.
