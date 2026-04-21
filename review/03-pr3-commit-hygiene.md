# PR 3 — Commit Hygiene Audit

**Branch:** `feat/03-pr3-scoring-redesign`
**Base:** `feat/03-pr2-empty-windows`
**Commits under review** (`git log feat/03-pr2-empty-windows..HEAD --oneline`):

```
d3be311 feat(pr3): per-window scoring, risk classes, threshold gates
1d81f84 test(pr3): scoring redesign, risk class, bootstrap cascade contract
```

---

## 1. Commit messages

### `1d81f84` — test commit
`test(pr3): scoring redesign, risk class, bootstrap cascade contract`

- **Imperative / concise / accurate:** yes. Names the three test areas
  that the diff actually adds (scoring formula + binding-rate behavior,
  risk-class gate thresholds + CLI resolve, bootstrap cascade with
  learned-rate / pool-average / duration-ratio fallbacks).
- **WHY:** no separate body. For a test commit whose justification is
  "lock the contract the next commit will satisfy", this is acceptable;
  the *why* lives in the feat commit it pairs with.

### `d3be311` — feat commit
`feat(pr3): per-window scoring, risk classes, threshold gates`

- **Imperative / concise subject:** yes (~70 chars, verb-first).
- **WHY stated plainly in the first paragraph:** *"Replaces the
  provider-level percent-per-hour density score with per-window
  turns-per-hour binding rate, fixing the bug where a high-weekly-usage
  account kept winning round-robin picks even as its runway projected
  into failure territory."* This is the load-bearing motivation.
- **Body covers:** formula (§4.7 reference + inline equations),
  bootstrap cascade + direction, risk-class gate semantics, CLI
  precedence chain, per-model `[balancer]` validation, schema
  changes (add/drop columns on `provider_quota_windows` /
  `provider_quotas` / `invocations` with idempotent migrations),
  deletions with rationale (`global_avg_percent_per_call`,
  `StateDb::get_quotas`, `QuotaRecord.last_delta_*`), example update,
  and a verification line ("All 239 tests pass. Examples build.").
- **Verification claims re-checked:** lib test count is exactly 239 at
  `cargo test` (309 across all test binaries); `cargo build --examples`
  succeeds. Both claims accurate.

**Message quality verdict:** good. The feat body is long but each
section corresponds to a reviewable concern, so a reviewer scanning
the message can jump to the right file.

---

## 2. Scope (per-commit single-concern)

### Test commit — CLEAN
Every hunk lands inside a `#[cfg(test)] mod tests { ... }` block or on a
`#[cfg(test)]`-gated helper. Spot-checked:

- `src/balancer/mod.rs` hunks: all four at `@@ -308 / -324 / -339 /
  -418 ... mod tests` — test-only.
- `src/state/db.rs` hunks: `@@ -1339 impl StateDb` adds
  `set_window_delta_for_test`, attribute-gated `#[cfg(test)]`; others
  inside `mod tests`.
- `src/main.rs`, `src/lib.rs`, `src/config/model.rs`: all hunks inside
  `mod tests` blocks (imports + new `#[test]` fns).

No production code lands in the test commit.

### Feat commit — multi-concern by design
Bundles: balancer scoring rewrite, `select_provider` signature +
return-type change, risk-class CLI plumbing (`--risk-class` +
`OULIPOLY_RISK_CLASS` + heuristic), per-model `BalancerConfig`
validation, `lib::test_model` restructure, schema migrations
(add/drop columns across three tables), `global_avg_percent_per_call`
+ `StateDb::get_quotas` + `QuotaRecord.last_delta_*` deletions,
call-site updates across `main.rs`, `lib.rs`, `examples/quota_check.rs`,
`tests/pr_b_trace_integration.rs`.

Each concern is listed in the commit message, and the proposal-level
review already rejected splits. Re-validation below confirms that
holds at the commit level.

---

## 3. Ordering (red → green)

Ran `cargo build --tests` at each commit in the worktree:

- **`1d81f84` (test commit):** 46 compile errors — e.g. `no field
  last_delta_percent on type QuotaWindow`, `no field
  quota_tight_routing on db::InvocationStart`, `use of undeclared type
  BalanceError`, `cannot find function test_model_for_test in this
  scope`. This is the **intended red state** per the audit brief: the
  tests assert the contract that the feat commit will introduce.
- **`d3be311` (feat commit):** clean build, `cargo test` → 239/239 lib
  tests pass (309 total across all test targets). `cargo build
  --examples` succeeds.

TDD ordering: **CLEAN**.

---

## 4. Size — re-validation at commit level

Feat commit: 814 insertions, 195 deletions, 9 files.

Proposed in-commit splits and why each fails:

### Split A — schema + type shells first, wiring second
`last_delta_percent/calls` moves *out of* `provider_quotas` into
`provider_quota_windows`. The SQL statements that read/write those
columns (`upsert_quota_window`, quota refresh paths, invocation record
join) all live in `state/db.rs` and must migrate in the same commit —
otherwise persistence queries reference non-existent columns. Orphan
types (`RiskClass`, `Selection`, `BalanceError`, `BalancerConfig`) with
no consumer would be dead code a reviewer can't evaluate. **Leaves a
red build mid-branch.**

### Split B — balancer core, then call sites
`select_provider` changes return type from `usize` to
`Result<Selection, BalanceError>` and gains a `RiskClass` argument. If
`main.rs`, `lib.rs`, the integration test, and the example aren't
updated in the same commit, the crate stops compiling. Defers work
but doesn't yield a reviewable intermediate state.

### Split C — risk-class CLI plumbing as a separate earlier commit
`resolve_risk_class` + the `--risk-class` flag + env-var precedence
could land before the scoring rewrite if it passed
`RiskClass::Background` as a throwaway default into the old
`select_provider`. But the test commit's
`risk_class_flag_reaches_repl_subcommand` would then fail to compile
against the old signature, breaking the red→green handoff. Net: ~100
lines moved at the cost of a complicating intermediate commit.

### Split D — `BalancerConfig` validation alone
`config::model::BalancerConfig` + `parse_balancer` + `validate` +
`append_balancer_toml` is self-contained (~78 lines). Technically
separable, but the tests for it live in the test commit and reference
thresholds that only become meaningful once the balancer consumes
them. A standalone config commit either (a) defines a validated type
no code uses, or (b) pairs with test-suite additions that would then
need to move out of the test commit. Either way the TDD pairing
breaks.

**Size verdict:** the proposal-level rejection of splits holds at the
commit level. The concerns are load-bearing on each other through
type signatures, return shapes, and schema columns. The detailed
commit message gives reviewers per-concern entry points into the
single diff.

---

## 5. Per-commit buildability

| Commit   | `cargo build --tests`             | Expected? |
|----------|-----------------------------------|-----------|
| `1d81f84` (test) | FAIL — 46 errors           | yes, intentional red state |
| `d3be311` (feat) | PASS, 239/239 lib tests    | yes, green after TDD pairing |

The mid-branch red build is intrinsic to the `test(...)` → `feat(...)`
TDD pattern used throughout this initiative; PR 1 and PR 2 use the
same shape. It is not accidental.

---

## Verdict

**CLEAN.**

Both commits are well-messaged, cleanly scoped to their roles (test vs
feat), correctly ordered as red→green, and the feat commit's size is
justified: every candidate in-commit split leaves either a
non-compiling intermediate state or orphaned dead code. The detailed
feat commit body mitigates the size for reviewers.

No reorganization recommended.
