# Test residuals — routing-claude-skipped

## WS-3 rollback during mid-rebuild failure

- Residual class: temporal/concurrency
- Technique attempted or considered: chaos
- Scope: `ensure_providers_schema` rollback if a failure is injected after `ALTER TABLE providers RENAME TO providers_legacy_index_keyed` and post-fix `CREATE TABLE providers`, but before the rebuild `INSERT ... SELECT ... FROM invocations` completes.
- Budget or bound: considered test-only `CHECK` constraints, mid-transaction triggers, sibling-connection lock contention, and OS-level fault injection. All require test-only product-source hooks or runner-level fault injection outside the unit / particular-integration levels selected by the approved test-intent track.
- Result: not verified by Phase 6b runtime tests.
- Remaining residual: transactional rollback for a mid-rebuild failure must be verified by Phase 6c code review of the explicit transaction boundary in `ensure_providers_schema`.
- Invalidating inputs: a product-supported failure-injection hook, a repository convention allowing test-only migration fault hooks, or an approved end-to-end / chaos-test level for this risk.
- Net-value impact: no change. The proposal already records this as a code-review residual, and the emitted unexpected-shape rejection test covers the reachable early-rejection contract without claiming mid-rebuild rollback coverage.
