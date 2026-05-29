# spec-balancer — Provider selection and routing matrix

## Source files

- `crates/oulipoly-runtime/src/balancer/mod.rs`
- `crates/oulipoly-runtime/src/balancer/context.rs`
- `crates/oulipoly-runtime/src/balancer/snapshot.rs`
- `crates/oulipoly-runtime/src/balancer/refresh_inputs.rs`

## Preconditions

- A `RuntimeConfig` enumerating zero or more provider account configurations,
  each carrying provider identity (`claude`, `codex`, `openai_compat`),
  account/profile name, and optional quota window snapshot.
- A `QuotaSnapshot` (possibly stale or empty) keyed by `(provider, account)`
  with at-most-once entry per key; missing keys are interpreted as "quota
  unknown", not "exhausted".
- Terminal-signal history is per-attempt; the balancer reads, never writes,
  the recognizer-classified outcome of prior attempts within the current
  invocation.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| One healthy provider, no prior failures this invocation. | Select that provider. |
| Multiple healthy providers, no quota data. | Select per the configured selection policy (round-robin or first-eligible) — deterministic per invocation when inputs are identical. |
| One quota-exhausted account, others healthy. | Skip the exhausted account; do not retry it during the current invocation. |
| One quota-window in `unknown` state (no recent refresh), others healthy. | Treat unknown as eligible; emit a diagnostic note "quota unknown for `<provider>/<account>` — including in candidate set". |
| All accounts exhausted. | Return a `NoEligibleProvider` outcome; do not silently fall back to an exhausted account. |
| Provider previously failed with `rate_limited` during this invocation. | Skip that account in subsequent attempts within the same invocation. |
| Provider previously failed with `auth_required`. | Skip that account in subsequent attempts within the same invocation; surface the auth error rather than fail-over silently. |
| Provider previously failed with `quota_exhausted`. | Skip that account in subsequent attempts within the same invocation. |
| Provider previously failed with generic `error` or `timeout`. | The account remains eligible for future attempts within the same invocation; bounded retry count applies. |

## Edge cases

- Tie between two equally-eligible providers — selection is deterministic and
  reproducible given identical inputs (no time-of-day randomness).
- Quota snapshot lists an account that no longer exists in `RuntimeConfig` —
  ignore the stale snapshot entry; do not synthesize a phantom account.
- `RuntimeConfig` lists an account that has never been refreshed — treat
  quota as `unknown` (eligible).
- All providers in the candidate set are exhausted **except** one in
  `unknown` quota — that one is selected; this is the intended behavior of
  treating unknown as eligible.
- Single-provider configuration (only `claude` configured) — selection
  collapses to "use the one account or fail with `NoEligibleProvider`".

## Error conditions

- `NoEligibleProvider` — every configured account is either exhausted or has
  failed in a non-retryable way during this invocation. Error carries
  per-account reason codes for diagnostics.
- `MalformedRuntimeConfig` — caller passed a config with duplicate
  `(provider, account)` keys or empty provider identity. Surface as a
  configuration error, not a routing error.

## Boundaries

- Balancer does NOT call provider executables directly — that is
  `executor/cli.rs`'s job. The balancer's output is a provider selection,
  not a process invocation.
- `select_provider(Some(ctx))` owns contextual refresh orchestration before
  route selection, including stale-cache refresh calls and session scans, but
  quota-script internals remain `quota/mod.rs`'s job.
- Balancer does NOT classify terminal signals — that is
  `executor/terminal_signal.rs` and the per-provider recognizers' job. The
  balancer reads classifications, not raw stderr.

## Declared test patterns

Per `~/ai/conventions/testing.md` § "Declared test patterns": exhaustive
matrix tests on the selection rule, plus per-failure-class skip tests, plus
NoEligibleProvider error-shape tests.

- `crates/oulipoly-runtime/tests/routing_matrix.rs`
- `crates/oulipoly-runtime/tests/age15_runtime_refresh_provider_contract_guard.rs`
- `crates/oulipoly-runtime/tests/age153_balancer_signal_isolation.rs`
- `crates/oulipoly-runtime/tests/age35_routing_characterization.rs`
- `crates/oulipoly-runtime/tests/age34_runtime_launcher_service_routing.rs`
- `src-tauri/tests/age162_dispatch_stderr_marks_exhausted.rs`
- `src-tauri/tests/age28_provider_policy_routing.rs`
- `src-tauri/tests/rca_routing_claude_skipped.rs`
- `src-tauri/tests/routing_fanout_rca/rc1_incomplete_quota_topology.rs`
- `src-tauri/tests/routing_fanout_rca/rc2_argmax_concentration.rs`

## Cross-references

- `planning/coverage/spec-quota.md` — quota refresh that feeds this surface.
- `planning/coverage/spec-recognizer.md` — terminal-signal taxonomy this
  surface consumes.
- `planning/coverage/spec-executor.md` — process execution downstream of
  selection.
- `AGENTS.md` § Rust Workspace Structure.
