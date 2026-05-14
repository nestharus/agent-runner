# Agent Runner Quota Refresh Eval

## Behavior

Routing treats a persisted `exhausted_at` flag as reset-implied only when the
provider also has stored quota windows and every stored `resets_at` value has
elapsed. The mechanism is lazy-on-route: the first routing decision after all
stored windows have elapsed can re-admit the provider and clear the stale
`exhausted_at` flag.

The reset signal is adapter-uniform. Routing keys only on persisted
adapter-supplied `resets_at` timestamps; it does not encode provider-specific
reset schedules, polling timers, or background monitors.

## Boundaries

- All stored windows elapsed: the exhausted provider is eligible for the current
  routing decision, and `provider_quotas.exhausted_at` is cleared.
- Any stored window still live: the provider remains excluded when the live
  window is at quota, and the exhausted flag remains set.
- No stored windows: the provider remains excluded while exhausted. This eval
  does not claim recovery for zero-window exhausted accounts.
- Clear-write failure: the current routing decision uses the in-memory
  reset-implied predicate; the next routing call can retry the idempotent clear.

## Verification

Runtime coverage lives in `crates/oulipoly-runtime/src/balancer/mod.rs`:

- `select_provider_readmits_exhausted_account_when_all_windows_elapsed`
- `select_provider_keeps_exhausted_account_excluded_while_a_window_is_live`
- `select_provider_keeps_zero_window_exhausted_account_excluded`

State coverage lives in `crates/oulipoly-state/src/db.rs`:

- `clear_exhausted_nulls_the_flag`
