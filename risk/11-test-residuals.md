# WU-11-01 Step 6b Test Residuals

This artifact records named residual risks from contract section 10 that are
not fully encoded as deterministic unit, component, or particular-integration
tests in Step 6b. These residuals do not change the net-value case.

## Persistent One-Window Probes Beyond A Single Call

- residual class: temporal/concurrency
- technique attempted or considered: model-checking
- scope: repeated balanced invocations against a provider that persistently exposes fewer live windows than siblings.
- budget or bound: Step 6b encodes one cooldown-suppressed repeat selection in `balancer::tests::topology_probe_respects_cooldown_for_persistent_short_topology`; it does not model many cooldown windows over time.
- result: single-call cooldown behavior is covered; long-horizon behavior remains residual.
- remaining residual: a provider that is permanently one-window may still produce unacceptable hourly probe cadence in real usage.
- invalidating inputs: production evidence that mixed one-window and two-window pools are common and hourly probes are too frequent.
- whether the residual changes the net-value case: no.

## Wide-Score Hard-Pin Coverage At The High End Of The Score Gap

- residual class: bounded-model
- technique attempted or considered: property-based
- scope: score-band boundary and extreme score ratios for deterministic fanout hard-pin behavior.
- budget or bound: Step 6b encodes one deterministic outside-band case in `balancer::tests::density_hard_pins_when_score_gap_exceeds_band`.
- result: the contractual `best / 2.0` boundary is covered by example; the full numeric space is not exhaustively searched.
- remaining residual: floating-point edge cases or extreme score magnitudes could affect the hard-pin boundary if implementation does not compare carefully.
- invalidating inputs: observed binding scores near the exact boundary, non-finite score propagation, or score magnitudes large enough to expose precision bugs.
- whether the residual changes the net-value case: no.

## Long Real-Run Distribution At Hundreds Of Selections

- residual class: bounded-model
- technique attempted or considered: property-based
- scope: distribution quality across hundreds of repeated learned-quota selections with invocation recording between picks.
- budget or bound: existing RC-2 harness loops 8 selections and Step 6b unit tests cover selector ordering.
- result: near-term fanout is covered; long-run fairness shape is not.
- remaining residual: lifetime invocation-count tie pressure may not yield the desired distribution across hundreds of selections in all score/count histories.
- invalidating inputs: production or simulation evidence that within-band providers remain materially concentrated after many successful selections.
- whether the residual changes the net-value case: no.

## Real Upstream API Rate Safety Under Topology Probing

- residual class: integration-hidden
- technique attempted or considered: chaos
- scope: actual provider quota scripts and upstream usage APIs under topology-triggered refreshes.
- budget or bound: Step 6b uses local shell-script fixtures and never calls real upstream APIs.
- result: local call gating is covered; real upstream API rate safety is not.
- remaining residual: upstream rate limits, script-side caching, or authentication behavior may make hourly topology probes too expensive.
- invalidating inputs: upstream API rate-limit errors or provider scripts that are not safe to invoke at the topology cooldown cadence.
- whether the residual changes the net-value case: no.

## Clock Skew Outside Test-Controlled Timestamps

- residual class: temporal/concurrency
- technique attempted or considered: fuzzing
- scope: system clock movement around `last_topology_probe_at`, `refreshed_at`, TTL, and cooldown comparisons.
- budget or bound: Step 6b uses fixed rows and near-now assertions; it does not skew wall-clock time backward or forward.
- result: normal timestamp behavior is covered; host clock skew is residual.
- remaining residual: clock jumps could cause an early or delayed topology probe.
- invalidating inputs: environments with non-monotonic system clocks or manual clock adjustment while the app is running.
- whether the residual changes the net-value case: no.

## Historical Peak Reconstruction For Never-Cached Topologies

- residual class: integration-hidden
- technique attempted or considered: graph
- scope: legacy DBs whose cached `provider_quota_windows` never contained the upstream's complete topology.
- budget or bound: Step 6b migration test verifies backfill from cached window count only.
- result: contract-required cached-count backfill is covered.
- remaining residual: the migration cannot reconstruct historical peaks that were never stored.
- invalidating inputs: a requirement to infer prior complete topology from external history or provider-specific semantics.
- whether the residual changes the net-value case: no.

## Peak Decay After Upstream Product Changes

- residual class: emergent-interaction
- technique attempted or considered: model-checking
- scope: providers whose real upstream quota topology permanently shrinks after a product or plan change.
- budget or bound: Step 6b encodes monotonic peak preservation and does not encode decay.
- result: no-lowering behavior is covered by `state::db::tests::upsert_quota_refresh_updates_topology_peak_without_lowering_on_shrink`.
- remaining residual: a real permanent topology shrink may leave a high peak that causes periodic topology probes until policy changes.
- invalidating inputs: upstream product changes that intentionally reduce live-window count for a provider.
- whether the residual changes the net-value case: no.

## Concurrent Refresh Races Between IPC `refresh_quotas` And Runtime Topology Probing

- residual class: temporal/concurrency
- technique attempted or considered: chaos
- scope: simultaneous Tauri IPC quota refreshes and CLI runtime topology probes mutating the same quota rows.
- budget or bound: Step 6b tests single-threaded state writes only.
- result: timestamp-only write semantics are covered in isolation.
- remaining residual: interleavings between refresh replacement and topology probe timestamp writes may need separate stress or integration coverage.
- invalidating inputs: observed race failures, SQLite busy errors, or lost peak/probe metadata under concurrent refreshes.
- whether the residual changes the net-value case: no.

## README/Doc Semantic Automation

- residual class: integration-hidden
- technique attempted or considered: symbolic
- scope: semantic correctness of README Load Balancing documentation after implementation.
- budget or bound: Step 6b emits no automated documentation semantic assertion.
- result: AC-5 remains an implementation and review obligation.
- remaining residual: documentation could be updated syntactically but still fail to explain topology probing, deterministic fanout, hard-pin behavior, or unchanged quota units clearly.
- invalidating inputs: reviewer or user evidence that README wording misstates the behavior.
- whether the residual changes the net-value case: no.

