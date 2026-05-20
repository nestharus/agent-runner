# spec-config — Provider/model/app config resolution

## Source files

- `crates/oulipoly-config/src/lib.rs`
- `crates/oulipoly-config/src/agent.rs`
- `crates/oulipoly-config/src/app.rs`
- `crates/oulipoly-config/src/claude_tool_filter.rs`
- `crates/oulipoly-config/src/model.rs`
- `crates/oulipoly-config/src/providers.rs`
- `crates/oulipoly-config/src/repositories/mod.rs`
- `crates/oulipoly-config/src/sessions.rs`

## Preconditions

- A configured config root path (per-deployment).
- For load: existing config files on disk in the documented format
  (`AGENTS.md` § Tech Stack covers the schema). For save: a writable
  config root.
- The caller wants either (a) a full `AppConfig` snapshot, or (b) a
  specific subsection (providers, sessions, agent, model).

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| Load with all files present and schema-current. | Parse + return `AppConfig` populated end-to-end. |
| Load with optional provider section absent. | Parse with that section defaulted; do not fail. |
| Load with required field missing. | Return a structured validation error naming the field and offending path. |
| Load with a provider entry that has duplicate account names. | Return a structured "duplicate account" error. |
| Save a mutated `AppConfig`. | Persist atomically (write-temp + rename) so an interrupted save does not corrupt the on-disk file. |
| Resolve effective model for a (provider, account) pair. | `model.rs` returns the configured model name (or provider default if no override). |
| Resolve claude tool-filter for a session. | `claude_tool_filter.rs` returns the tool allow/deny set with documented precedence: per-session > per-account > per-provider > default. |
| Apply a session-config preset. | `sessions.rs` returns the merged effective config. |

## Edge cases

- Config file has BOM — strip before parse.
- Provider account name contains spaces — preserved verbatim; not
  munged.
- An unknown top-level config key is present — log a warning, keep the
  rest parseable (forward-compat for newer config-schema versions).
- Concurrent save with a reader — atomic rename + mtime gate; reader
  sees either the old or the new file in full.
- Repository operation on a config row whose underlying file rotated —
  detect via `repositories/mod.rs` and re-load.

## Error conditions

- `ConfigLoadFailed` — IO or syntax error during parse.
- `ConfigValidationFailed` — schema-valid but semantically invalid
  (duplicate keys, missing required-when-conditional fields, etc).
- `ConfigSaveFailed` — IO or temp-rename failure.
- `ModelUnresolvable` — neither override nor provider default produces a
  model name (provider has no shipped default for that account class).
- `ToolFilterMisconfigured` — claude tool-filter references unknown
  tools.

## Boundaries

- Config does NOT execute provider processes — executor's domain.
- Config does NOT touch the state DB — `oulipoly-state` is the SQLite
  surface; config writes go to filesystem config files.
- Config does NOT discover installed providers — that is
  `oulipoly-runtime/discovery/`.
- Config does NOT decide routing — balancer's domain.

## Declared test patterns

Per `~/ai/conventions/testing.md`: schema-round-trip tests, validation
boundary tests, repositories contract tests, examples-load tests.

- `crates/oulipoly-config/tests/examples_contract.rs`
- `crates/oulipoly-config/tests/examples_load.rs`
- `crates/oulipoly-config/tests/repositories_contract.rs`
- `src-tauri/tests/age151_config_migration_cli.rs`
- `src-tauri/tests/age33_config_state_characterization.rs`
- `src-tauri/tests/load_app_config_characterization.rs`

## Cross-references

- `planning/coverage/spec-state-db.md` — separate DB-backed state.
- `planning/coverage/spec-setup.md` — initial config authoring.
- `planning/coverage/spec-balancer.md` — consumer of provider config.
- `AGENTS.md` § Tech Stack.
