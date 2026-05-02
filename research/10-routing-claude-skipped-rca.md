# RCA — claude pool 0% usage while claude2/claude3 absorb all routing

## Symptom

User report: a deployment has three account pools, `claude`, `claude2`, and
`claude3`. Each runtime provider wraps the same executable command (`claude`,
or a prefixed form whose executable still resolves to `claude`). Real `agents`
CLI invocations route to `claude2` / `claude3` while `claude` remains at 0%
usage.

Baseline commands requested for Phase 0:

```bash
cd src-tauri
cargo test -- --list 2>&1 | head -50
cargo test balancer --no-fail-fast 2>&1 | tail -80
cargo test executor::cli --no-fail-fast 2>&1 | tail -80
```

`cargo test -- --list 2>&1 | head -50` showed the existing balancer unit test
inventory after compilation, including `balancer::tests::round_robin_on_fresh_state`,
`balancer::tests::falls_back_to_invocation_count_when_windows_missing`,
`balancer::tests::select_provider_filters_exhausted_accounts`, and
`balancer::tests::fresh_pool_falls_through_to_invocation_count_round_robin`.
The targeted `balancer` and `executor::cli` filters completed successfully;
their integration-test tails contained only filtered-out test targets and
`test result: ok`.

The reproduced skip does not require quota windows, recent failures, or command
deduplication. It occurs in the invocation-count fallback when historical state
exists for the same `model_name` and stale provider indexes whose current
provider names no longer match those rows.

## Reproduction (red, against pre-fix HEAD)

Reproduction harness:
`src-tauri/tests/rca_routing_claude_skipped.rs`

The harness creates the current model order `[claude, claude2, claude3]`, then
seeds successful historical invocations for `provider_name='claude2'` at
`provider_index=0` and `provider_name='claude3'` at `provider_index=1`.
`claude` has no history by provider name. With no quota windows present,
selection enters fallback count scoring and picks `claude3` instead of the
history-free `claude`.

Command:

```bash
cd src-tauri
cargo test --test rca_routing_claude_skipped fallback_count_routing_uses_current_provider_identity_not_stale_index_history -- --nocapture
```

Captured output at HEAD `9cadc90dd295b796c7b9ff7db1fd3d8e68838731`:

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.99s
     Running tests/rca_routing_claude_skipped.rs (target/debug/deps/rca_routing_claude_skipped-5043fcdd88dfdfa5)

running 1 test

thread 'fallback_count_routing_uses_current_provider_identity_not_stale_index_history' (13250) panicked at tests/rca_routing_claude_skipped.rs:48:5:
assertion `left == right` failed: provider claude has no invocation history by provider_name, but stale provider_index rows made selection pick claude3
  left: "claude3"
 right: "claude"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
test fallback_count_routing_uses_current_provider_identity_not_stale_index_history ... FAILED

failures:

failures:
    fallback_count_routing_uses_current_provider_identity_not_stale_index_history

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

error: test failed, to rerun pass `--test rca_routing_claude_skipped`
```

## Root causes

### RC-1 — Fallback usage stats are keyed by provider index, not provider identity

Routing enters invocation-count fallback when at least one non-exhausted
candidate has no quota windows: `src-tauri/src/balancer/mod.rs:160-167`.
That path scores each candidate by `state.get_provider(&model.name, i)`:
`src-tauri/src/balancer/mod.rs:586-607`. `round_robin_fallback` uses the same
lookup: `src-tauri/src/balancer/mod.rs:618-639`.

The persisted aggregate table is keyed only by `(model_name, provider_index)`:
`src-tauri/src/state/db.rs:508-518`. `finalize_invocation` reads only
`model_name` and `provider_index` from the invocation row before upserting the
aggregate: `src-tauri/src/state/db.rs:1186-1195` and
`src-tauri/src/state/db.rs:1229-1244`. `get_provider` then reads aggregates by
the same two fields: `src-tauri/src/state/db.rs:1495-1531`.

Effect: if the provider list/order changes, or if rows were written under a
different account at the same index, the fallback scorer charges that old
history to whichever provider currently occupies the index. The red harness
seeds history for `claude2` at index 0 and `claude3` at index 1, then loads a
current model where index 0 is `claude`. The selector treats `claude` as having
six prior invocations and picks index 2 (`claude3`), even though `claude` has
zero invocation rows by provider name.

This reproduces `claude` staying at 0% while traffic goes to `claude2` /
`claude3`. It is order-dependent and DB-history-dependent. It appears with one
model pool containing one entry per account; sharing the same runtime command
is not necessary for this root cause.

## Hypotheses (unreproduced)

- Live deployment state may contain a provider/index mismatch matching RC-1.
  Evidence that would confirm it: rows in `invocations` for the affected model
  where `provider_index=0` has `provider_name!='claude'`, plus current model
  config where provider index 0 is `claude`.
- A recent-error variant can produce the same symptom because
  `recent_error_count` also filters by `(model_name, provider_index)`:
  `src-tauri/src/balancer/mod.rs:590-595` and
  `src-tauri/src/state/db.rs:1533-1553`. Evidence that would confirm it:
  three or more recent failed invocation rows at `provider_index=0` whose
  `provider_name` is not current `claude`.
- A persisted quota-exhausted flag can intentionally remove `claude` from
  candidates because quotas are read by provider name and candidates with
  `exhausted_at` are filtered: `src-tauri/src/balancer/mod.rs:116-138`.
  Evidence that would confirm this path: a `provider_quotas` row for
  `provider_name='claude'` with non-null `exhausted_at`. The Phase 0
  reproduction does not seed quota state.
- The shared executable command is likely a false lead for selection. The
  executor helper `provider_name(command)` does return the last token of
  `command`: `src-tauri/src/executor/cli.rs:796-823`, so prefixed commands such
  as `env -u CLAUDECODE claude` resolve to `claude`. However, the CLI dispatch
  path selects a provider index first and then resolves the effective provider:
  `src-tauri/src/main.rs:1999-2003`. `ProvidersConfig::effective_provider`
  preserves the model provider key as `ProviderConfig.name`:
  `src-tauri/src/config/providers.rs:116-127` and
  `src-tauri/src/config/providers.rs:157-190`. `derive_pools` also groups by
  model provider names, not by parsed executable command:
  `src-tauri/src/lib.rs:53-77`.

## Files involved

- `src-tauri/tests/rca_routing_claude_skipped.rs` — red reproduction harness for
  stale index history causing `claude` to be skipped.
- `src-tauri/src/balancer/mod.rs` — provider selection, quota filtering,
  invocation-count fallback, and recent-error penalty.
- `src-tauri/src/state/db.rs` — invocation rows, provider aggregate schema, and
  aggregate read/write methods keyed by provider index.
- `src-tauri/src/main.rs` — real CLI dispatch path calls `select_provider`,
  resolves the selected effective provider, and records both provider name and
  provider index.
- `src-tauri/src/config/providers.rs` — runtime provider resolution preserves
  account/provider keys (`claude`, `claude2`, `claude3`) even when commands are
  identical.
- `src-tauri/src/executor/cli.rs` — `shell_split` and `provider_name` command
  parsing; relevant to display/migration hypotheses, not the reproduced
  selection failure.
- `src-tauri/src/lib.rs` — pool derivation groups current model provider names
  after sorting/deduping.

## Out of scope

- No fix is proposed in this Phase 0 document.
- No product source was changed; only the reproduction harness and RCA were
  added.
- This RCA does not characterize the user's live SQLite DB because it was not
  available in the worktree.
- This RCA does not change quota refresh, cooldown, migration, or command
  parsing behavior.
