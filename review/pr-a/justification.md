# Justification Review: feat/pr-a-invocation-lifecycle

## Verdict: TIGHT

Every hunk traces back to the PR-A contract; two micro-additions are
defensible adjacent cleanup. No PR-B/C/D material leaked in, and no
process/docs artifacts are present.

## Per-change classification

| Change area | Lines | Classification | Justification |
|---|---|---|---|
| `state/db.rs` — schema rebuild + `ensure_invocations_schema` | ~90 | in scope | Contract §"Schema contract" + inline-schema rule. |
| `state/db.rs` — `migrate_legacy_invocations` + `provider_name_lookup` | ~160 | in scope | Contract §"Migration". Txn + provider-name resolution are mandated. |
| `state/db.rs` — `InvocationStart` / `InvocationStatus` / `CompositeInvocationId` / widened `InvocationRecord` | ~110 | in scope | Contract §"Struct contract" verbatim. |
| `state/db.rs` — `start_invocation` / `finalize_invocation` / `get_invocation_by_uuid` / `map_invocation_row` | ~170 | in scope | Contract §"Method contract" verbatim. `record_invocation` deletion is mandated. |
| `state/db.rs` — `impl FromStr for InvocationStatus` | ~13 | adjacent cleanup | Contract only asked for inherent `from_str: Option<Self>`. The trait impl is a small idiomatic add triggered by the `should_implement_trait` lint on the contracted shape. Trivial, defensible. |
| `state/db.rs` — new tests (migration, lifecycle, round-trips, fixtures) | ~480 | in scope | All map 1:1 to test-contract items #1–#6. |
| `state/mod.rs` — re-export | 1 | in scope | Contract §"Files expected to change" names this file. |
| `main.rs` — lifecycle reorder, env-var read, stderr emission, `resolve_parent_invocation_id` | ~62 | in scope | Contract §"CLI contract" + §"Lifecycle ordering". Hoisting the existing `provider_name` binding upward is the minimum needed to build `InvocationStart`. |
| `executor/cli.rs` — `parent_invocation_env` parameter + `cmd.env(...)` | ~12 | in scope | Contract §"Env var write at spawn". |
| `executor/mod.rs` — new `execute_with_inputs_and_env` wrapper | ~27 new | in scope (light verbosity) | The `cli::execute` signature gained a required parameter; since `cli` is a private module, `main.rs` needs a public entry point. Adding a third wrapper rather than widening the two existing ones preserves caller surface area. Arguably could have collapsed to a single `Option<&str>`-bearing wrapper, but the present shape is not creep. |
| `balancer/mod.rs` — `record_invocation_for_test` helper + 3 test-call migrations | ~30 | in scope | Contract deletes `record_invocation` per V14 ("no compatibility shim"). Balancer tests reach into `StateDb`, so they had to migrate with the API. No non-test code in this file was touched. |
| `tests/pr_a_invocation_integration.rs` — stderr emission + parent propagation | 183 | in scope | Test-contract items #7 and #8 require exactly this integration coverage. |

## Findings

None rising to scope creep. Two small judgment calls worth naming so
future PRs don't accrete more of the same pattern:

1. **`FromStr` trait alongside inherent `from_str`.** The contract
   pinned the inherent `Option<Self>` shape; the trait impl is
   additive. A stricter reading of V14 ("change the code, don't
   shim") would say: pick one surface. But the comment acknowledges
   the collision and cites the contract, so this is transparent, not
   sneaky. Leave as-is.

2. **Triple executor wrapper (`execute` / `execute_with_inputs` /
   `execute_with_inputs_and_env`).** The first two now just forward
   to the third with `None`. Collapsing to one function taking
   `Option<&str>` would be cleaner, but both existing callers
   (`main.rs` outside `run_with_balancing`, tests) would churn.
   Acceptable as-is for PR-A; worth revisiting if PR-B/C add more
   entry points.

## Patterns followed correctly

- No CLI-name sniffing; provider identity flows through
  `provider_name` strings resolved from declarative config (V1, V2).
- Migration lives inline in `open` with `unchecked_transaction()`,
  matching `upsert_quota_refresh` style. No new "migrations" module
  (contract anti-pattern respected).
- `record_invocation` is deleted outright — no deprecated alias,
  every caller migrated in the same PR (V14).
- `load_models` failure during migration degrades to
  `status='legacy'` with a stderr warning rather than silent success
  (V10 — failures observable).
- CLI surface stays flat; no `trace` subcommand, no `session_capture`
  plumbing, no `session_turns.parent_turn_id`. PR-B/C/D material was
  kept out.
- `.gitignore` / process artifacts are absent from this diff — they
  landed separately on `main` as expected.
