# spec-usage — `--usage` accessor, dispatch, and renderer

## Source files

- `src-tauri/src/usage/accessor.rs`
- `src-tauri/src/usage/cli.rs`
- `src-tauri/src/usage/dispatch.rs`
- `src-tauri/src/usage/fetcher.rs`
- `src-tauri/src/usage/filter.rs`
- `src-tauri/src/usage/mapper.rs`
- `src-tauri/src/usage/mod.rs`
- `src-tauri/src/usage/renderer.rs`
- `src-tauri/src/usage/row.rs`
- `src-tauri/src/usage/vendor.rs`

## Preconditions

- A configured set of provider accounts in `RuntimeConfig`.
- For each account, either an installed provider executable or a
  configured quota-fetch script (the path that yields a `QuotaWindow`).
- The caller invoked the CLI with `--usage` (with or without filter
  arguments).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| `--usage` with no filters, all providers installed, all scripts succeed. | Render one row per `(provider, account)` with limit/used/remaining + reset_at. |
| `--usage` with `--provider claude` filter. | Render only claude rows. |
| `--usage` with a provider configured but not installed. | Emit a warning row noting "provider `<name>` not installed — skipping" and continue. |
| `--usage` with a provider whose config entry has no quota-fetch script. | Emit a warning row "no usage source configured for `<provider>/<account>` — skipping" and continue (AGE-162 WU-C behavior). |
| `--usage` when one provider's script fails but others succeed. | Render the successful rows; emit a warning row for the failed provider with the script's exit code; do NOT abort the full snapshot. |
| `--usage --json`. | Emit a single JSON object containing one entry per included account, including the same warning entries as structured records. |

## Edge cases

- Provider account name contains shell-special characters — quote
  correctly in the script invocation; render verbatim in the table.
- A provider returns a window with `used > limit` — render verbatim;
  display "exhausted" status rather than negative remaining.
- A provider returns `reset_at` in the past — render as "expired"; the
  caller may treat this as `unknown` for routing purposes.
- Zero providers configured at all — print a friendly "no providers
  configured" banner with exit code 0; do not crash.

## Error conditions

- `UsageDispatchFailed` — internal plumbing error in `dispatch.rs`; the
  CLI exits non-zero with a structured error.
- `UsageRenderFailed` — `renderer.rs` could not format the rows (should
  never happen with valid `UsageRow` inputs; programmer error).
- `UsageVendorMisconfigured` — `vendor.rs` cannot identify the script
  shape for a provider (config schema mismatch); CLI exits non-zero with
  a clear "config schema" error pointing to the offending account.

## Boundaries

- Usage module does NOT mutate quota state — it reads via `accessor.rs`
  and renders. Refreshing the underlying `QuotaWindow` is the
  responsibility of `quota/mod.rs`, invoked by the accessor.
- Usage module does NOT decide routing — it is a read-side view. The
  balancer is the policy engine.
- Usage module does NOT modify config — `--usage` is read-only with
  respect to `RuntimeConfig`.
- Usage module does NOT short-circuit on partial failure — the
  warn-and-skip pattern is intentional so operators see all working
  providers' data even when some accounts are misconfigured.

## Declared test patterns

Per `~/ai/conventions/testing.md`: integration tests on the full
`--usage` CLI path, per-vendor fixture tests on the mapper, and
warn-and-skip path coverage per failure class.

- `src-tauri/tests/age15_usage_cli_characterization.rs`
- `src-tauri/tests/age162_usage_missing_provider_warn_and_skip.rs`
- `src-tauri/tests/age8_cli_characterization.rs`

## Cross-references

- `planning/coverage/spec-quota.md` — the upstream data source.
- `planning/coverage/spec-tauri-client.md` — sibling CLIs sharing the
  Tauri `src-tauri/src/` root.
- `AGENTS.md` § Commands.
