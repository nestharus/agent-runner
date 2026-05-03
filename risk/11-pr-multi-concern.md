Verdict: MULTI_CONCERN_ACCEPTABLE

# WU-11-01 multi-concern review

## What the PR contains

The single squashed commit `74f05e5` against `main` (`fa8b38b`) carries
four distinguishable groups:

1. **Phase 0 RCA + reproduction harnesses** — `research/11-routing-fanout-rca.md`
   plus `src-tauri/tests/routing_fanout_rca/{mod,rc1_incomplete_quota_topology,
   rc2_argmax_concentration}.rs` and the `tests/routing_fanout_rca.rs` shim.
   These were authored on the inherited `rca/routing-fanout` branch
   (`1ab5602`, not on `main`) and are bundled here.
2. **RC-1 fix — topology-aware quota probe** — `provider_quotas` schema
   migration + `QuotaRecord` extension + `record_topology_probe` +
   `upsert_quota_refresh` peak-update in `state/db.rs`;
   `is_topology_probe_due` + `TOPOLOGY_PROBE_COOLDOWN_SECS` in `quota/mod.rs`;
   topology-probe pass in `balancer::select_provider`; tracing-subscriber
   wiring in `main.rs` and `Cargo.{toml,lock}`.
3. **RC-2 fix — deterministic score-band fanout** — `FANOUT_SCORE_BAND_RATIO`
   + `select_binding_score_with_fanout` in `balancer/mod.rs`, plus the
   fanout-selected `tracing::info!` event.
4. **Phase 0 fixture correction** — relative-time + `used_percent = 80`
   in the rc1 harness (the §13 contract revision noted in the commit body),
   plus the WU artifacts under `proposals/`, `research/`, `risk/`, and
   `product-strategy/contracts/`.

The proposal §Design explicitly chooses RC-1 and RC-2 as **two
independent fixes** with disjoint blast radius. The ticket's
"Notes for Phase 2.5+" allows either bundled or split delivery so long
as AC-1, AC-2, AC-3 all pass. So the diff is genuinely factorable —
this is not file-overlap-only cohesion.

## Why not split

A clean decomposition (e.g. PR-A: Phase 0 reproduction; PR-B: RC-1
topology probe + schema migration; PR-C: RC-2 fanout) is logically
possible but would create more churn than value:

- **AC-3 in intermediate states.** Splitting RC-1 and RC-2 means one
  PR lands while the other harness remains RED in CI. Either RC-1 ships
  first (rc2 RED) or RC-2 ships first (rc1 RED). Neither intermediate
  state satisfies the AC-3 contract that the suite stays green, and
  bypassing it would require temporarily disabling a harness — exactly
  the kind of test-state churn the squashed-PR convention exists to
  avoid.
- **Workflow-encoded bundling.** The commit body documents the policy:
  "Bundles Phase 0 RCA + reproduction harnesses with the WU-11-01
  implementation so the squashed PR is reviewable as one self-contained
  unit." A Phase 0-only PR landing intentionally-RED reproduction
  harnesses is not the project convention.
- **Fixture is fix-coupled.** The §13 contract revision changed the rc1
  fixture (relative-time + `used_percent = 80`) so claude's binding score
  lands below claude3's *under the post-probe-refresh state*. That
  fixture only makes sense alongside the RC-1 implementation; landing it
  in a Phase 0 PR by itself would be incoherent, and landing the
  original (broken) fixture first would force a fix-up commit later.
- **Shared observability footprint.** `tracing-subscriber` wiring in
  `main.rs` + `Cargo.toml` is shared infrastructure for both the
  topology-probe and fanout-selected `tracing::info!` events. Splitting
  RC-1 and RC-2 forces an arbitrary choice of which PR owns the
  subscriber init.
- **Single user-visible symptom.** The ticket scopes a single bug —
  100%-to-one-provider routing for both `claude-opus` (RC-1) and
  `gpt-high` (RC-2) — as one work unit. Shipping only half closes only
  half of what the user reported.
- **Single README §Load Balancing edit.** The doc change describes both
  the topology probe and the score-band fanout as one coherent routing
  policy; splitting it produces two amendments that the user reads as
  one section.

## What does *not* justify the bundle

For honesty: the WU artifacts under `proposals/`, `research/`, `risk/`,
and `product-strategy/contracts/` are workflow trail and could in
principle be a doc-only PR. They ride with the implementation here
because the project convention treats them as the same artifact set as
the fix; that is a documentation-locality choice, not a strict
dependency.

## Conclusion

The PR contains recognizable concerns (Phase 0 reproduction; RC-1
topology probe + schema migration; RC-2 fanout; WU artifacts) and is
not single-concern in the strict sense. But each candidate split would
either produce a CI-broken intermediate state (AC-3 violation), split a
fixture from the code that justifies it, or fragment shared
observability and documentation. The ticket explicitly authorizes
bundling, and the workflow convention reinforces it. Decomposition is
possible but net-negative. Advance as-is.
