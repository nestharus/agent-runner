# spec-discovery — Installed-provider discovery and REPL default resolution

## Source files

- `crates/oulipoly-runtime/src/discovery/mod.rs`
- `crates/oulipoly-runtime/src/repl_default_provider.rs`

## Preconditions

- A configured candidate set of provider identities to probe (claude,
  codex, openai_compat, possibly more).
- Per-provider detection rules (PATH executable name, optional env var,
  optional config file presence).
- The caller wants either (a) a snapshot of which providers are installed
  for the setup wizard or `--usage`, or (b) the resolved default provider
  for a REPL launch.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| All probes succeed; multiple providers found. | Return an `InstalledProviders` map with detection metadata per provider; `repl_default_provider` returns the configured-default or the first installed. |
| Only one provider installed. | `repl_default_provider` returns that provider. |
| No providers installed. | Return an empty map; `repl_default_provider` returns a typed "no default" outcome — the CLI surfaces this rather than crashing. |
| User-configured default points to an uninstalled provider. | `repl_default_provider` emits a diagnostic and falls back to the first installed; never silently picks something the user did not name. |
| Probe is run twice concurrently. | Both calls produce consistent results; discovery is idempotent and stateless. |

## Edge cases

- PATH contains a provider-named binary that is not actually executable
  (permissions) — treat as "not installed".
- A provider's CLI returns non-zero on `--version` — record as
  "installed but version probe failed"; surface a warning.
- A provider's CLI lives behind a wrapper script with a different name —
  honor the configured override.
- Multiple PATH entries shadow the same provider — pick the first (PATH
  resolution order); record the chosen path.

## Error conditions

- `DiscoveryProbeFailed` — IO error during PATH walk; surfaced
  per-provider so partial discovery still returns the other results.
- `DefaultUnresolved` — caller asked for a default and none is resolvable
  (zero installed + no override).

## Boundaries

- Discovery does NOT install or configure providers — that is
  `oulipoly-setup`'s job.
- Discovery does NOT decide WHICH provider to route to for a real
  invocation — that is the balancer; discovery's "default" is a UI-layer
  hint for the REPL launcher.
- Discovery does NOT refresh quota — `quota/mod.rs` is independent.

## Declared test patterns

Per `~/ai/conventions/testing.md`: per-provider detection fixture tests,
default-resolution table tests, no-provider edge tests.

- `crates/oulipoly-runtime/tests/age33_default_provider_characterization.rs`
- `src-tauri/tests/age31_new_repl_integration.rs`
- `src-tauri/tests/pr_e_repl_integration.rs`

## Cross-references

- `planning/coverage/spec-setup.md` — installs providers that discovery
  finds.
- `planning/coverage/spec-balancer.md` — separate, real routing
  decisions.
- `AGENTS.md` § Tech Stack.
