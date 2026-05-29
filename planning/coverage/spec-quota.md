# spec-quota — Quota refresh, parse, and cache

## Source files

- `crates/oulipoly-runtime/src/quota/mod.rs`
- `crates/oulipoly-runtime/src/quota/in_flight.rs`
- `crates/oulipoly-runtime/src/quota/outcome.rs`
- `crates/oulipoly-runtime/src/quota/parse.rs`
- `crates/oulipoly-runtime/src/quota/process.rs`

## Preconditions

- A configured per-provider quota-fetch script path (typically a CLI
  wrapper exposing the provider's account usage endpoint).
- An in-memory `QuotaCache` map keyed by `(provider, account)` carrying the
  last successful refresh result and timestamp.
- Caller has decided refresh is wanted (the balancer requests refresh
  before selection in fresh-routing mode; otherwise the cache is read
  as-is).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Refresh request with a valid script that exits 0 and writes well-formed JSON. | Parse the JSON, write the resulting `QuotaWindow` (limit, used, reset_at) into the cache, return success. |
| Refresh request with a script that exits 0 and writes empty stdout. | Cache entry remains in `unknown` state; return success-with-no-update. |
| Refresh request with a script that exits non-zero. | Do NOT mutate the cache; return a refresh error carrying the script's exit code and stderr tail. |
| Refresh request with a script that emits malformed JSON. | Do NOT mutate the cache; return a parse error. |
| Refresh request when no script is configured. | Cache entry remains `unknown`; return a configuration error noting the missing script path. |
| Refresh request when the script emits a window in the past. | Parse; cache the window; the consumer (balancer) is responsible for treating a past `reset_at` as expired. |
| Cache read with a valid recent entry. | Return the cached `QuotaWindow`. |
| Cache read with no entry. | Return `unknown`. |
| Cache read with a stale entry (older than configured TTL). | Return the cached value with a `stale: true` flag so callers can choose whether to refresh. |

## Edge cases

- Script writes both stdout and stderr — only stdout is parsed; stderr is
  preserved verbatim in the error path.
- Script writes UTF-8 with a BOM — strip the BOM before parse.
- Script writes a `limit` of zero — treat as "no quota information"
  (`unknown`), not as "fully exhausted".
- Concurrent refresh requests for the same `(provider, account)` —
  serialize per key; one in-flight refresh per key at a time.
- Cache resize during refresh — refresh result still lands in the cache,
  no orphan entries.

## Error conditions

- `QuotaScriptMissing` — the configured script path does not exist or is
  not executable.
- `QuotaScriptFailed` — the script ran but exited non-zero.
- `QuotaParseFailed` — the script's stdout was not valid JSON or did not
  match the expected `QuotaWindow` shape.
- `QuotaTimeout` — the script exceeded the configured refresh timeout.

## Boundaries

- Quota module does NOT decide eligibility — that is `balancer/mod.rs`'s
  job. Quota is the data source, not the policy.
- Quota module does NOT invoke provider executables — only the configured
  quota-fetch script. The provider's own CLI (`claude`, `codex`) is
  invoked exclusively by `executor/cli.rs`.
- Quota module does NOT classify terminal signals — that is
  `executor/terminal_signal.rs`'s job. A `rate_limited` recognizer outcome
  does NOT short-circuit a quota refresh; the recognizer signal feeds the
  balancer separately.

## Declared test patterns

Per `~/ai/conventions/testing.md`: contract tests on the script-result
parse, table tests on cache read/write, error-shape tests on each failure
class.

- `crates/oulipoly-runtime/tests/age34_runtime_quota_service_routing.rs`
- `src-tauri/tests/age162_usage_missing_provider_warn_and_skip.rs`
- `src-tauri/tests/age100_one_shot_quota_migration.rs`
- `src-tauri/tests/age100_resume_quota_migration.rs`
- `src-tauri/tests/age15_usage_cli_characterization.rs`

## Cross-references

- `planning/coverage/spec-balancer.md` — the consumer of this surface.
- `planning/coverage/spec-usage.md` — surfaces the same quota data to the
  CLI `--usage` flow.
- `AGENTS.md` § Testing.
