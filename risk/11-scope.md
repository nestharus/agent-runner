Termination signal: none
Verdict: LOW

The proposal stays cleanly within the WU-11-01 scope on every axis.

A. In-scope only. Every committed code change lives in the ticket Code Boundary: `balancer/mod.rs` (new `select_binding_score_with_fanout`, topology-probe gather/probe step in `select_provider`, two new constants), `quota/mod.rs` (additive `is_topology_probe_due` helper plus `TOPOLOGY_PROBE_COOLDOWN_SECS`), `state/db.rs` (additive `topology_peak_live_window_count` / `last_topology_probe_at` columns, `ensure_provider_quotas_topology_schema`, `record_topology_probe`, peak-update inside `upsert_quota_refresh`), `main.rs` (no change proposed beyond integration glue), and `README.md` §Load Balancing. No reach into `state/repository.rs`, no new Initiative-B services, no frontend.

B. No anti-scope drift. Each ticket Anti-scope item is explicitly preserved in the proposal's Anti-scope section: `refresh_quotas` IPC response shape unchanged, `QuotaRefreshEntry`/`QuotaRefreshWindow`/`QuotaWindow.used_percent` field names unchanged, the 0..100 vs 0..1 unit-naming mismatch deferred, no `state/repository.rs` or Initiative-B abstractions, no `session_replace`/`session_export`/`session_metadata` edits, no `setup/`/frontend/e2e edits, no backwards-compat shims (the migration is forward-only and shape-based), and no stochastic fanout — the new selector is pure over binding scores, existing aggregate counts, and provider order.

C. Single-WU coherence. RC-1 and RC-2 are presented as two independent fixes inside the same WU, which the ticket "Notes for Phase 2.5+" explicitly permits. They share the routing surface and the AC-1/AC-2 RED→GREEN harnesses, and the proposal does not bundle in the deferred empty-bodies WU (`research/12-empty-bodies-ref-rca.md`), session-storage work, or any unrelated cleanup.

D. Migration scope discipline. The schema change is the minimum necessary for RC-1: two additive columns on `provider_quotas` plus a backfill from existing window-row counts. No schema-wide refactor, no rename, no compat reader, no incidental column cleanup. RC-2 explicitly requires no schema change.

E. Test scope discipline. Test changes target only the in-scope Test Boundary: the two existing `routing_fanout_rca` harnesses turn RED→GREEN, new inline tests are added in `balancer::tests`, `quota::tests`, and `state::db::tests`, and an optional new top-level file `tests/routing_fanout_topology_migration.rs` is allowed by the ticket's "new `tests/routing_fanout_*.rs` files are allowed if needed" language. The #25 harness `tests/rca_routing_claude_skipped.rs` is explicitly preserved unchanged. No e2e or setup tests proposed.

F. README scope. The README change is gated to §Load Balancing per AC-5 and is described narrowly (topology probe, deterministic score-band fanout, hard-pin wide gaps, unchanged unit semantics, plus a `quota_check` mention). It is not a wholesale rewrite.

Termination check. No assumption from the Phase 2.5 problem map is credibly invalidated: the proposal accepts and builds on the map's RC-1 (sibling-blind `is_stale`) and RC-2 (argmax pin) findings rather than contradicting them. The supported-surface track and net-value statement together do claim a real reduction of a current-state, supported-surface risk: multi-provider CLI routing concentration on the `run_with_balancing` and interactive paths, with two RED reproduction harnesses as evidence and a bounded blast radius (additive schema, deterministic selector, single observability event per new branch). So neither `invalidated_assumption` nor `non_positive_value` triggers.
