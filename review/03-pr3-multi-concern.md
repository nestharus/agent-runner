# Multi-Concern Check: PR 3

## Verdict: single-concern

PR 3 is one coherent unit of work: *replace the single scalar
percent-per-hour density score with a per-window binding-rate
scoring path gated by risk-class thresholds, and migrate storage
and all call sites to feed that one new selection contract.* Every
file in the diff — schema, TOML, CLI flag, Tauri error, example,
validator — traces back to the new signature of `select_provider`
and the new `Selection` / `BalanceError` types. None of the six
candidate seams produce an independently-shippable unit; each
splits the code along a line where one half is dead plumbing and
the other half refuses to compile.

Scope rev 2's rejections hold; the three additional seams
(get_quotas cleanup, quota_check.rs example, save_model validator)
are all forced follow-through of the same core change, not
independent concerns.

## Evaluation of the three scope-rev-2 seams against the actual diff

### Seam 1 — schema migrations ship independently — **rejected (confirms scope rev 2)**

Three schema changes land in this PR:

- `provider_quota_windows` **gains** `last_delta_percent` /
  `last_delta_calls` (`src-tauri/src/state/db.rs:596-612`,
  `:361-365`).
- `provider_quotas` **drops** `last_delta_percent` /
  `last_delta_calls` (`src-tauri/src/state/db.rs:584-590`).
- `invocations` **gains** `quota_tight_routing`
  (`src-tauri/src/state/db.rs:506-512`, `:659`).

Isolating the migrations isn't coherent. Dropping the provider-
level delta columns requires simultaneously stopping every reader:
`get_quota`'s `SELECT last_delta_percent, last_delta_calls`
(pre-PR shape at `state/db.rs:1113-1119`) and the balancer's
`global_avg_percent_per_call` consumer
(pre-PR `balancer/mod.rs`). Both readers are rewritten in this PR.
A migrations-only PR would either (a) leave those readers selecting
nonexistent columns and crash the first refresh cycle, or (b) land
the `ALTER TABLE` statements without the column-dropping side
(and then the real PR still has to drop them anyway). Answer Q2
("no dual-write shims") forbids the middle-state solution that
would make this seam buildable. Dead-middle-state → not a seam.

Symmetrically, the `invocations.quota_tight_routing` column is
written by `start_invocation` through a new `InvocationStart`
field (`src-tauri/src/state/db.rs:127`, `:877`). Every caller of
`InvocationStart` in the diff — balancer tests, cli.rs tests,
main.rs `run_with_balancing` / `run_repl`, db.rs tests — gains the
new field. Shipping the column without the struct field would not
break compilation (the column has a DEFAULT 0), but shipping the
struct field without the column would fail at INSERT time.
Keeping them together is the only shape that actually builds.

### Seam 2 — risk-class plumbing without scoring redesign — **rejected (confirms scope rev 2)**

`select_provider`'s signature changes from `(model, state, ctx) -> usize`
to `(model, state, ctx, RiskClass) -> Result<Selection, BalanceError>`
(`src-tauri/src/balancer/mod.rs:78-88`). The `Selection`'s
`quota_tight_routing` field (`:18-22`) is **only** produced by
`score_by_density`'s user-band branch (`balancer/mod.rs` at the
`if user_eligible.is_empty()` arm in the rewritten scorer) and
`BalanceError::Exhausted` is **only** raised by that same scorer
when no provider is below `failure_threshold`.

Without §4.7's scoring redesign, the `--risk-class` CLI flag
(`main.rs:62-82`) would parse fine, `resolve_risk_class`
(`main.rs:212-243`) would resolve fine, but the resolved value
would feed a `select_provider` call that can't produce anything
other than `Ok(Selection { quota_tight_routing: false })` — the
whole new return type would be dead shape. Similarly,
`emit_balance_error` (`main.rs:806-813`) and the Tauri
`test_model_error_from_exhausted` (`lib.rs:555-577`) would be
unreachable. Dead plumbing → not a seam.

### Seam 3 — `[balancer]` TOML block independently — **rejected (confirms scope rev 2)**

`BalancerConfig { user_threshold, failure_threshold }`
(`src-tauri/src/config/model.rs:214-218`) has exactly two
consumers in the diff:

- `score_by_density` reads `model.balancer.failure_threshold` /
  `user_threshold` on every window projection
  (`balancer/mod.rs:180-187` in the new scorer).
- `exhausted_error` reports them in the structured error
  (`balancer/mod.rs:280-286`).
- `save_model` validates before write (`lib.rs:279`).

Shipping the TOML block standalone would add a parsed config
section that nothing reads — the validator would run and the
getter would exist, but no code path would compare a projected
used-percent against either threshold. The test
`rejects_balancer_threshold_outside_unit_interval` (model.rs:1233)
would exercise only the validator. Dormant config → not a seam.

## Evaluation of the three additional seams (new in phase 8)

### Seam 4 — delete `get_quotas` + drop provider-level delta columns as cleanup PR — **rejected**

This looked tempting: `get_quotas` (the batch-fetch helper) has no
in-tree callers (confirmed by grep: only `research/01-hookpoints.md`
mentions it, a prose reference, not a call site), and the provider-
level delta columns on `provider_quotas` are being deleted — a
classic "land the deletions first to shrink the real PR" shape.

But the cleanup isn't separable:

- `QuotaRecord.last_delta_percent` / `last_delta_calls`
  (`src-tauri/src/state/db.rs:31-38` pre-PR) are read by the
  pre-PR balancer's `global_avg_percent_per_call`. You cannot
  remove them from the struct without simultaneously removing the
  reader, and removing the reader means replacing density scoring's
  source-of-truth — which is exactly §4.5's per-window learning
  rewrite. The deletion and the rewrite are the same change viewed
  from two sides.
- `get_quotas` is unused *today*, but its SELECT statement
  references the same `last_delta_percent` / `last_delta_calls`
  columns. Deleting the function without dropping the columns
  leaves a dead function referencing live columns; dropping the
  columns without deleting the function breaks the SELECT. They
  must land together, and once they land together the deletion
  has taken on the provider-level → per-window migration shape.

A "cleanup-first" PR would either leave `global_avg_percent_per_call`
reading a field that no longer exists on `QuotaRecord` (compile
error) or leave a version of `QuotaRecord` that still carries the
fields only to satisfy one now-dead reader (pointless churn, and
forbidden by answer Q2's no-shim rule). No clean intermediate
state exists.

### Seam 5 — `quota_check.rs` example update self-contained — **rejected**

`src-tauri/examples/quota_check.rs` changes 19 lines:

- Imports `RiskClass` (`examples/quota_check.rs:10`) — a type
  that does not exist before this PR.
- Calls `select_provider(m, &db, None, RiskClass::Background)`
  and pattern-matches on `Result<Selection, _>`
  (`examples/quota_check.rs:116-124`) — against a signature that
  does not exist before this PR.
- Drops `q.last_delta_percent.zip(q.last_delta_calls)` from the
  printout (`examples/quota_check.rs:91-95`) — against a field
  that was removed in this PR.

The example is a forced follow-through, not an optional polish.
Shipping the example update alone would fail to build (the new
signature doesn't exist). Shipping the balancer changes without
updating the example would break `cargo build --examples`, which
the commit message explicitly calls out ("Examples build."). The
change is the minimum edit required to keep the example compiling
on the new signature. Zero reviewable content as a standalone PR.

### Seam 6 — `save_model` validation as its own small PR — **rejected**

The `save_model` validation is a single line:
`model.balancer.validate()?;` (`src-tauri/src/lib.rs:279`). It
defends the Tauri-frontend path against a `ModelConfig` with a
bad balancer block that bypasses the TOML parser's
`parse_balancer` validator (`config/model.rs:669-680`).

`BalancerConfig` is introduced by this same PR
(`config/model.rs:214`). `BalancerConfig::validate` is defined by
this same PR (`config/model.rs:223-241`). The Tauri path with the
bypass (`save_model` taking a `ModelConfig` directly from the
frontend, not a TOML string) was already present, but had nothing
to validate until the balancer block existed. There is no
pre-existing thing to validate earlier; there is no separable unit
of work.

A standalone "add save_model validation" PR would be one line of
production code plus its test. The test itself —
`config::model::rejects_balancer_*` — is already in this PR and
validates the TOML path, which is the shared validator. A second
test hitting `save_model` would be strict duplication of the
validator's own unit tests against a now-introduced caller. Pure
ceremony.

## Why the files/commits belong together

The PR has two commits (`test` then `feat`) arranged as a standard
red/green TDD split (same shape as PR 1 and PR 2 seams 3). Every
one of the nine files touched is load-bearing for at least one
other:

- `balancer/mod.rs` rewrites scoring and defines `RiskClass`,
  `Selection`, `BalanceError` — consumed by `main.rs`, `lib.rs`,
  and `examples/quota_check.rs`.
- `config/model.rs` adds `BalancerConfig` — consumed by
  `score_by_density` (thresholds) and `save_model` (validation).
- `state/db.rs` moves deltas from provider-level to window-level
  storage and adds `quota_tight_routing` — produced by the new
  balancer's `Selection`, consumed by the new learning cascade.
- `main.rs` plumbs `--risk-class` and the env-var precedence
  chain, and surfaces `BalanceError::Exhausted` via
  `emit_balance_error` — depends on the new signature of
  `select_provider`.
- `lib.rs` mirrors the same error surfacing into the Tauri
  `test_model` command with `TestModelError` — depends on the same.
- `executor/cli.rs`, `quota/mod.rs`, `pr_b_trace_integration.rs`
  are mechanical struct-field follow-through
  (`balancer: Default::default()`, `last_delta_percent: None`,
  `quota_tight_routing: false`) forced by `ModelConfig`,
  `QuotaWindow`, and `InvocationStart` gaining fields.
- `examples/quota_check.rs` is forced follow-through of the
  `select_provider` signature change.

Splitting any seam produces one of three shapes: (a) a half-wired
intermediate that fails to build (seams 1, 5), (b) a dormant
plumbing PR with no producer or consumer (seams 2, 3, 4), or (c)
a one-line ceremonial PR with no standalone review value (seam 6).

The proposal's §4.10 stance — *"no TODO-gated rollout, no feature
flags, no hidden fallback to old scalar scoring"*
(`proposals/03-load-balancing-tiers.md`) — is the human-gate
decision that locks this in: the scoring redesign is a one-shot
replacement, and every piece that feeds or consumes the new
scoring ships with it. Revision 2's tightening (explicit
`ProviderEval`, `Option<f64>` bootstrap, `run_repl` exhaustion
surface) only increased the internal coupling; none of the six
seams survive the revised scope.

## Cross-checks against the `AGENTS.md` split rules

- **"Large deletion is its own PR."** N/A as an *isolatable* PR —
  see seam 4. The deletions (`get_quotas`,
  `global_avg_percent_per_call`, `QuotaRecord.last_delta_*`, the
  provider-level delta columns) are each structurally fused with
  the replacement they clear space for.
- **"Additive changes go before behavioral changes."** Satisfied
  *within* this PR: the `test(pr3)` commit adds the contract
  first, the `feat(pr3)` commit lands the implementation. The
  schema-ensure additions (per-window delta columns,
  `quota_tight_routing`) land as the substrate for the same
  behavioral change inside the same commit — splitting them
  further would only be useful if there were a consumer on a
  separate track, and there isn't.
- **"Dependency order matters."** PR 3 depends on PR 2 (empty-
  windows self-heal) and PR 1 (second-window emission), per
  `proposals/03-load-balancing-tiers.md:472`. That dependency is
  inter-PR, not an intra-PR split signal.

## Summary

Ship as one PR. 1,515 insertions / 266 deletions across 9 files,
two commits test-then-feat, every file load-bearing for at least
one other. The scope gate in phase 4 already rejected three splits
for the right reasons; the three additional seams surfaced for
phase 8 fail for the same class of reasons (dead plumbing,
forced-build follow-through, or one-line ceremony). No seam
proposed would produce a strictly better reviewer experience, and
the split cost (two stacked PRs, cross-review for what is
conceptually one redesign) is real.
