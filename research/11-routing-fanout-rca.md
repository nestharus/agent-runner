# Routing fanout RCA

## Symptom

Reported by the user:

- "No requests are being routed to claude despite 0% usage."
- "All codex requests are going through codex2."
- "On codex it shows how much is remaining. On claude it shows how much is used."

Observed from the pre-collected live baseline instead:

- In the last hour, `claude-opus` routed `22/22` invocations to `claude`.
- In the last hour, `gpt-high` routed `24/24` invocations to `codex`.
- The six-hour timeline flipped around the #25 / `fa8b38b` rollout boundary: before #25 traffic concentrated on `claude2` / `codex2`; after #25 traffic concentrated on `claude` / `codex`.

So the user-named providers are inverted relative to the DB. The real observed defect is not "claude skipped" or "codex2 selected"; it is per-pool concentration onto one provider account at a time.

The remaining/used display report is not reproduced in the current frontend. `PoolCard` renders command chips and model counts, not quota values (`src/components/PoolCard.tsx:100`), and the only `remaining` hit in `PoolsView` is a local array name while removing pool commands (`src/views/PoolsView.tsx:129`). The runner stores quota windows as used fractions (`src-tauri/src/state/db.rs:157`) after parsing scripts that emit used percent on a 0..100 scale (`src-tauri/src/quota/mod.rs:329`, `src-tauri/src/quota/mod.rs:366`). The pre-collected upstream check also verified both APIs return used %, not remaining %.

## Root Cause(s)

### RC-1: Cached incomplete quota topology can stay fresh and dominate density scoring

Code path:

- CLI routing calls `select_provider` with a balance context before execution (`src-tauri/src/main.rs:1990`, `src-tauri/src/main.rs:2000`).
- `select_provider` only refreshes providers that `is_stale` marks stale (`src-tauri/src/balancer/mod.rs:98`, `src-tauri/src/balancer/mod.rs:104`).
- `is_stale` is provider-local: it checks whether the provider has any cached windows and whether `refreshed_at` exceeds a TTL derived only from that provider's cached windows (`src-tauri/src/quota/mod.rs:183`, `src-tauri/src/quota/mod.rs:198`, `src-tauri/src/quota/mod.rs:206`).
- If every candidate has at least one window, routing enters density scoring (`src-tauri/src/balancer/mod.rs:160`).
- The missing-window penalty only applies when the provider has fewer live windows than siblings and one visible window is at least `0.85` used (`src-tauri/src/balancer/mod.rs:288`, `src-tauri/src/balancer/mod.rs:292`).
- Otherwise, the provider's binding score is just the minimum over visible windows of `(1 - projected_used) * hours_until_reset` (`src-tauri/src/balancer/mod.rs:297`, `src-tauri/src/balancer/mod.rs:308`, `src-tauri/src/balancer/mod.rs:311`).

Violated assumption:

The code assumes a low-used single-window cache is benign. In the observed Claude state, `claude` had only a weekly cached window at low usage while `claude2` and `claude3` had weekly plus short windows. Because `claude` was not near the `0.85` missing-window threshold, it avoided the short-window binding constraint and received a multi-day score. Complete siblings were constrained by their short windows and lost.

What surfaced it:

#25 / `fa8b38b` corrected provider aggregate keying by identity. That removed the earlier index-history concentration mode and exposed the density scorer's deterministic preference for the now correctly keyed account with the largest visible binding score. The live baseline shows the boundary: traffic moved from `claude2`/`codex2` before #25 to `claude`/`codex` after #25.

### RC-2: Learned-quota routing is deterministic argmax with no fanout term

Code path:

- The README contract says learned quota routing computes a binding score and `pick = argmax` (`README.md:232`).
- The implementation follows that contract: density scoring computes projections, filters eligible providers, then returns `best_binding_score(...).index` (`src-tauri/src/balancer/mod.rs:170`, `src-tauri/src/balancer/mod.rs:189`, `src-tauri/src/balancer/mod.rs:198`).
- `best_binding_score` is a plain `max_by` over binding score (`src-tauri/src/balancer/mod.rs:444`, `src-tauri/src/balancer/mod.rs:447`).
- Invocation-count balancing is only the fallback when density scoring cannot produce learned binding scores (`src-tauri/src/balancer/mod.rs:160`, `src-tauri/src/balancer/mod.rs:166`; README fallback contract at `README.md:233`).
- Successful executions increment `calls_since_refresh` (`src-tauri/src/main.rs:2097`, `src-tauri/src/main.rs:2100`; DB write at `src-tauri/src/state/db.rs:2120`), but learned density scoring projects from ingested assistant turns since `refreshed_at`, not from invocation count or `calls_since_refresh` (`src-tauri/src/balancer/mod.rs:272`, `src-tauri/src/balancer/mod.rs:276`; quota delta learning falls back to `calls_since_refresh` only during refresh-time learning at `src-tauri/src/state/db.rs:1947`).

Violated assumption:

"Load balanced" is user-facing language that implies traffic fans out across healthy provider accounts. The implemented learned-quota path is capacity ranking: if one provider's score is modestly higher, every selection returns that provider until scores change through refresh/session-turn projection/error state. Repeated successful selections alone do not introduce any distribution pressure.

What surfaced it:

The #25 identity-keying change made the selected provider names accurate, so the remaining behavior is no longer hidden behind stale index aggregates. With live Codex-like scores, `codex` beats `codex2` by a modest binding gap, and deterministic argmax sends all `gpt-high` requests to `codex`.

## Files Involved

- `README.md`
- `scripts/README.md`
- `src-tauri/src/balancer/mod.rs`
- `src-tauri/src/quota/mod.rs`
- `src-tauri/src/state/db.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/lib.rs`
- `src/components/PoolCard.tsx`
- `src/views/PoolsView.tsx`
- `src-tauri/tests/routing_fanout_rca.rs`
- `src-tauri/tests/routing_fanout_rca/mod.rs`
- `src-tauri/tests/routing_fanout_rca/rc1_incomplete_quota_topology.rs`
- `src-tauri/tests/routing_fanout_rca/rc2_argmax_concentration.rs`

## Reproduction

### RC-1

Harness: `src-tauri/tests/routing_fanout_rca/rc1_incomplete_quota_topology.rs`

Command:

```bash
cd src-tauri && cargo test --test routing_fanout_rca rc1_incomplete_cached_topology_does_not_dominate_pool_routing 2>&1 | tee ../.tmp/rc1-red-run.log
```

Verbatim failure output:

```text
   Compiling oulipoly-agent-runner v0.1.0 (/home/nes/projects/agent-runner/worktrees/rca-routing-fanout/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.06s
     Running tests/routing_fanout_rca.rs (target/debug/deps/routing_fanout_rca-40afb839e2d2e173)

running 1 test
test routing_fanout_rca::rc1_incomplete_quota_topology::rc1_incomplete_cached_topology_does_not_dominate_pool_routing ... FAILED

failures:

---- routing_fanout_rca::rc1_incomplete_quota_topology::rc1_incomplete_cached_topology_does_not_dominate_pool_routing stdout ----

thread 'routing_fanout_rca::rc1_incomplete_quota_topology::rc1_incomplete_cached_topology_does_not_dominate_pool_routing' (62985) panicked at tests/routing_fanout_rca/rc1_incomplete_quota_topology.rs:30:5:
assertion `left == right` failed: post-fix routing should not let claude's stale single-window cache dominate complete siblings
  left: "claude"
 right: "claude3"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    routing_fanout_rca::rc1_incomplete_quota_topology::rc1_incomplete_cached_topology_does_not_dominate_pool_routing

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.00s

error: test failed, to rerun pass `--test routing_fanout_rca`
```

### RC-2

Harness: `src-tauri/tests/routing_fanout_rca/rc2_argmax_concentration.rs`

Command:

```bash
cd src-tauri && cargo test --test routing_fanout_rca rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers 2>&1 | tee ../.tmp/rc2-red-run.log
```

Verbatim failure output:

```text
   Compiling oulipoly-agent-runner v0.1.0 (/home/nes/projects/agent-runner/worktrees/rca-routing-fanout/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.85s
     Running tests/routing_fanout_rca.rs (target/debug/deps/routing_fanout_rca-40afb839e2d2e173)

running 1 test
test routing_fanout_rca::rc2_argmax_concentration::rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers ... FAILED

failures:

---- routing_fanout_rca::rc2_argmax_concentration::rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers stdout ----

thread 'routing_fanout_rca::rc2_argmax_concentration::rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers' (63664) panicked at tests/routing_fanout_rca/rc2_argmax_concentration.rs:25:5:
post-fix routing should fan out across eligible providers; selected only {"codex"}
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    routing_fanout_rca::rc2_argmax_concentration::rc2_repeated_learned_quota_selections_fan_out_across_eligible_providers

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1 filtered out; finished in 0.01s

error: test failed, to rerun pass `--test routing_fanout_rca`
```

## Open Questions

- Hypothesis (unreproduced): the user saw "remaining" vs "used" in a surface outside the current Solid frontend, such as an upstream CLI status display, `quota_check`, an older branch, or a local wrapper. Evidence that would confirm/refute it: the exact screenshot/terminal line or code path that renders those labels.
- Hypothesis (unreproduced): `refresh_quotas` could become a future UI unit footgun because it serializes the normalized 0..1 internal value in a field named `used_percent` (`src-tauri/src/lib.rs:293`, `src-tauri/src/lib.rs:363`), while script docs reserve `used_percent` for 0..100 (`scripts/README.md:204`). Evidence that would confirm/refute it: a current frontend caller or a failing UI/API test that treats that IPC field as 0..100.
- The live baseline already confirms both upstream APIs report used %, not remaining %, and `parse_output` normalizes those used percentages by `/100.0` before storage (`src-tauri/src/quota/mod.rs:360`, `src-tauri/src/quota/mod.rs:366`). I did not reproduce any routing path that compares remaining for one provider against used for another.
