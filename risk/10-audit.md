# Audit Risk Assessment (round 3): proposals/10-routing-claude-skipped.md

Line-number references in this audit are evidence snapshots from the
round-3 review. Prefer the named symbols in adjacent prose for future
implementation work, and re-check line ranges before using them as
navigation.

## Verdict: LOW

The round-3 revision closes the audit-relevant portions of the round-2 shortcut-gate findings without regressing the round-1 audit closure. The proposal still carries the migration error contract from round 2, now adds a self-contained operator recovery procedure for unexpected `providers` shapes, and correctly records mid-rebuild rollback as a code-review residual rather than a runtime-test claim. The inadequate rollback test-intent row has been replaced by an idempotency-across-reopens test that exercises the concrete post-fix-shape no-op branch and includes the required risk, level, fixture source, assumption link, observable signal, and residual fields.

The audit checklist remains covered: validation/contracts are explicit for accepted/rejected `providers` shapes, migrations are specified by shape and transaction boundaries, fixture sources are named for each test-intent row, residuals are recorded where runtime coverage is intentionally not claimed, and the supported-surface/rollback/observability tracks remain present.

## Prior finding status

- `R1-F01` (medium -> closed): closed, still closed
  - Closure evidence: `proposals/10-routing-claude-skipped.md:91-97` still defines accepted pre/post shapes, rejects unexpected shapes through `StateDb::open`, states transactional rollback, and defines deterministic retry/empty-table behavior. The strengthened malformed-shape test-intent row at `proposals/10-routing-claude-skipped.md:241` now also requires `providers` and `invocations` to be byte-identical after failed open and exercises recovery after operator cleanup.
- `R2-F01` (medium -> closed): closed
  - Closure evidence: the round-2 "Migration rollback on failed rebuild" test entry is no longer present. The proposal instead records the mid-rebuild rollback property as an explicit residual verified by code review at `proposals/10-routing-claude-skipped.md:108-110`, and adds `Migration ensure_providers_schema is idempotent across reopens` at `proposals/10-routing-claude-skipped.md:242` to cover a real branch that does not require test-only product-source failure injection.
- `R2-N01` (low note -> closed): closed
  - Closure evidence: `proposals/10-routing-claude-skipped.md:99-106` gives a self-contained recovery procedure: restore `state.db` from backup, or manually drop malformed `providers`/leftover legacy tables and reopen so the missing-table branch creates the post-fix shape. It also states the expected data-loss boundary for aggregate counts and why this is operator-level.
- `R1-N01` and `R1-N02`: remain closed
  - `last_error_at` remains constrained to most recent failed rows at `proposals/10-routing-claude-skipped.md:85` and covered by the test-intent row at `proposals/10-routing-claude-skipped.md:243`. `src-tauri/examples/quota_check.rs` remains in-scope for the `get_provider` signature update at `proposals/10-routing-claude-skipped.md:168`.

## New findings (round 3)

None.

## Watch signals

- `WS-1`: holds
  - The proposal keeps the migration transactional and rejection-oriented: exact pre/post shapes only, no heuristic recovery, and explicit failed-open behavior at `proposals/10-routing-claude-skipped.md:91-97`. The operator recovery procedure does not add binary-side heuristic repair; it is manual recovery after rejection.
- `WS-2`: holds
  - The proposal keeps `providers` keyed by `(model_name, provider_name)` at `proposals/10-routing-claude-skipped.md:25-42`, requires `get_provider` to read by provider name and keep no index-keyed alias at `proposals/10-routing-claude-skipped.md:130-138`, requires `recent_error_count` to filter by provider name at `proposals/10-routing-claude-skipped.md:140-157`, and explicitly excludes backwards-compatibility/index-keyed fallback readers at `proposals/10-routing-claude-skipped.md:181`.
- `WS-3`: holds
  - Mid-rebuild rollback is not re-promoted to a runtime test claim. The residual says runtime proof would require fault injection or test-only product-source changes and assigns verification to implementation-time code review of `ensure_providers_schema`'s transaction wrapper at `proposals/10-routing-claude-skipped.md:108-110`.

## Notes

- Prior-finding counters: closed 4, weakened 0, regressed 0, not closed 0.
- New-finding counters: high 0, medium 0, low 0.
- The idempotency-across-reopens test row has all required risk-annotation fields in the test-intent table: risk, level, fixture source, assumption link, observable signal, and residual (`proposals/10-routing-claude-skipped.md:232-242`).
- The unexpected-shape rejection test now verifies the failed open does not mutate `providers` or `invocations`, and it also exercises post-cleanup recovery through the missing-table branch (`proposals/10-routing-claude-skipped.md:241`).
- Spot-checks against source still match the proposal's problem surface and planned diff: current `providers` DDL is index-keyed at `src-tauri/src/state/db.rs:509-517`; current aggregate finalization is index-keyed at `src-tauri/src/state/db.rs:1186-1255`; current `get_provider` and `recent_error_count` remain index-keyed at `src-tauri/src/state/db.rs:1495-1547`; balancer and the example call those APIs with indexes at `src-tauri/src/balancer/mod.rs:261`, `src-tauri/src/balancer/mod.rs:591`, `src-tauri/src/balancer/mod.rs:600`, `src-tauri/src/balancer/mod.rs:628`, and `src-tauri/examples/quota_check.rs:123`.
