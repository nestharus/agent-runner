# Multi-Concern Review: feat/pr-a-invocation-lifecycle

## Verdict: SINGLE-CONCERN

The diff implements exactly the coupled mechanism the proposal and
contract pinned as PR-A: a single durable-invocation lifecycle whose
parts are mutually load-bearing. Splitting would force shipping
dead-code halves.

## What's in this diff

- `state/db.rs` (+1120): `invocations` schema rebuild, transactional
  legacy migration, new types (`InvocationStart`, `InvocationStatus`,
  widened `InvocationRecord`, `CompositeInvocationId`), new methods
  (`start_invocation`, `finalize_invocation`, `get_invocation_by_uuid`),
  deletion of `record_invocation`.
- `main.rs` + `executor/{mod,cli}.rs`: lifecycle reorder to
  insert-on-spawn / update-on-finish, stderr emission of
  `OULIPOLY_INVOCATION`, read of `OULIPOLY_PARENT_INVOCATION` at
  startup, write at subprocess spawn.
- `balancer/mod.rs`: test-only call-site updates driven by the
  `record_invocation` removal (no behavior change).
- `tests/pr_a_invocation_integration.rs` (+183): end-to-end CLI
  fixture covering stderr line + env propagation.

## Decomposition assessment

**Schema migration vs. lifecycle methods** — not separable. The new
columns (`status`, `finished_at`, nullable `success`/`exit_code`) exist
*because* of the insert-on-spawn / update-on-finish split. Shipping
the schema first would either (a) leave `record_invocation` writing
rows that violate the new semantics, or (b) require a throwaway shim
the contract explicitly forbids (V14). Shipping lifecycle first is
impossible — the columns don't exist yet. Split cost: pure churn,
zero review savings.

**Env-var propagation vs. stderr emission** — not separable. They are
the two ends of the same IPC channel: a parent process's stderr
`OULIPOLY_INVOCATION=...` line is exactly what the child parses out
of `OULIPOLY_PARENT_INVOCATION`. Shipping emission without read
produces a line nothing consumes; shipping read without emission
produces a reader nothing feeds. Neither half delivers user value
alone. The integration test in `pr_a_invocation_integration.rs`
literally asserts the loop closes end-to-end.

**`CompositeInvocationId` as its own PR** — not useful. The type
exists to serialize/parse the env-var payload and format the stderr
line. Without callers it is dead code. V14 bars compat shims; the
same logic bars pre-landing inert type modules.

**Anything bundled that doesn't belong?** No. Files touched
(`state/db.rs`, `state/mod.rs`, `main.rs`, `executor/cli.rs`,
`executor/mod.rs`, plus the integration test) are precisely the
hookpoints listed in the contract's "Files expected to change". The
`executor/mod.rs` addition (`execute_with_inputs_and_env`) is a
wrapper added to plumb the parent-env payload through — part of the
same mechanism, not a separable feature. The `balancer/mod.rs` delta
is a mechanical test fix required by the `record_invocation` removal;
carving it out is nonsense.

No out-of-scope work crept in: no `trace` subcommand, no
`session_capture` / `transcript_locator`, no `parent_turn_id`, no CLI
restructure, no README sweep. The `idx_invocations_session` index
correctly deferred to PR-C per the contract.

## Findings

None. PR-A as shipped is a single coherent concern — the proposal's
original 4-PR split call holds. Approving.
