# spec-tauri-client — Top-level Tauri client wiring, CLIs, adapters

## Source files

- `src-tauri/src/lib.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/wiring.rs`
- `src-tauri/src/balanced_cli.rs`
- `src-tauri/src/cli_inputs.rs`
- `src-tauri/src/config_migration_cli.rs`
- `src-tauri/src/repl_cli.rs`
- `src-tauri/src/resume_acceptance_adapter.rs`
- `src-tauri/src/resume_cli.rs`
- `src-tauri/src/session_import_replace_cli.rs`
- `src-tauri/src/session_ingest_cli.rs`
- `src-tauri/src/session_metadata_cli.rs`
- `src-tauri/src/terminal_outcome_adapter.rs`
- `src-tauri/src/trace_cli.rs`
- `src-tauri/src/main/owned_turn_event_ingest.rs`
- `src-tauri/src/setup/flow.rs`
- `src-tauri/src/setup/mod.rs`

## Preconditions

- A built Tauri binary (or `cargo run -p oulipoly-tauri --bin ...`).
- The runtime crate(s) compiled and wired via `wiring.rs`.
- For per-CLI flows: a configured deployment whose `oulipoly-config` and
  `oulipoly-state` surfaces resolve.

## Input → Expected output

| Input situation | Expected output |
|-----------------|-----------------|
| `main.rs` invoked with no args. | Launch the Tauri GUI. |
| `main.rs` invoked with a sub-CLI (`balanced`, `repl`, `resume`, etc.). | Dispatch to the matching `*_cli.rs` driver via `cli_inputs.rs`. |
| `balanced_cli.rs` invoked with a prompt. | Route via balancer + executor; return result; emit trace/diagnostics. |
| `repl_cli.rs` invoked. | Open an interactive session per `repl_default_provider.rs`'s resolution. |
| `resume_cli.rs` invoked with a session id. | Resolve via session_lifecycle, attach to existing session, continue. |
| `session_ingest_cli.rs` invoked. | Import an externally-produced session transcript into the local state DB. |
| `session_import_replace_cli.rs` invoked. | Replace an existing local session with an imported payload. |
| `session_metadata_cli.rs` invoked. | Read or update metadata for a known session. |
| `trace_cli.rs` invoked. | Surface the diagnostics/trace history for an invocation. |
| `config_migration_cli.rs` invoked. | Migrate config schema forward. |
| Setup flow invoked (`setup/flow.rs`). | Drive the wizard from `oulipoly-setup`; persist results via `oulipoly-config`. |
| Tauri owned-turn event arrives. | `main/owned_turn_event_ingest.rs` parses and persists per `oulipoly-state` schema. |

## Edge cases

- CLI arg parse fails — exit non-zero with usage; do not panic.
- Tauri context exists but no GUI display (headless host) — CLI sub-mode
  still works; GUI mode reports a clear error.
- Adapter (e.g. `terminal_outcome_adapter.rs`) receives an envelope
  whose schema does not match — typed error; do not swallow.
- Resume acceptance adapter sees a session in `mutability: read-only` —
  refuses gracefully (delegates to `oulipoly-runtime/session_metadata/
  mutability.rs`).

## Error conditions

- `CliParseFailed` — invalid CLI arguments.
- `WiringFailed` — `wiring.rs` could not construct the runtime service
  graph (typically an internal-config mismatch; programmer error).
- `TauriBootFailed` — GUI surface could not initialize.
- `AdapterError` — a typed `*_adapter.rs` translation failure.

## Boundaries

- Tauri client does NOT implement balancer policy, recognizer logic,
  quota refresh, or session metadata storage — those live in
  `oulipoly-runtime` and `oulipoly-state`. The client is composition +
  CLI + adapter only.
- Tauri client does NOT bypass the documented service ports — every
  runtime call goes through `services/` wired by `wiring.rs`.
- Tauri client does NOT mutate config files directly — it goes through
  `oulipoly-config`.

## Declared test patterns

Per `~/ai/conventions/testing.md`: integration tests on each CLI driver,
wiring smoke tests, adapter contract tests, workspace-layout invariants.

- `src-tauri/tests/age36_wiring.rs`
- `src-tauri/tests/age37_wiring.rs`
- `src-tauri/tests/age38_test_model_services.rs`
- `src-tauri/tests/age38_wiring.rs`
- `src-tauri/tests/age39_main_thinning_source_guard.rs`
- `src-tauri/tests/age151_source_guard.rs`
- `src-tauri/tests/age154_test_model_disposition.rs`
- `src-tauri/tests/age8_cli_characterization.rs`
- `src-tauri/tests/initiative_07_canonical_reader_unification.rs`
- `src-tauri/tests/nes_259_returned_artifacts_integration.rs`
- `src-tauri/tests/pr_a_invocation_integration.rs`
- `src-tauri/tests/pr_c_locator_scripts.rs`
- `src-tauri/tests/pr_f_resume_integration.rs`
- `src-tauri/tests/release_yml_contract.rs`
- `src-tauri/tests/workflow_yml_contract.rs`
- `src-tauri/tests/wiring_smoke.rs`
- `src-tauri/tests/workspace_layout.rs`
- `src-tauri/tests/claude_path_hash_rca/age158_characterization.rs`
- `src-tauri/tests/claude_path_hash_rca/rc1_non_alnum_encoding.rs`
- `src-tauri/tests/claude_path_hash_rca/rc2_windows_backslash_encoding.rs`
- `src-tauri/tests/claude_path_hash_rca/rc3_symlink_canonicalization.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs`
- `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs`
- `src-tauri/tests/routing_fanout_rca/age158_characterization.rs`

## Cross-references

- `planning/coverage/spec-balancer.md`, `spec-quota.md`,
  `spec-recognizer.md`, `spec-executor.md` — runtime surfaces this
  client invokes.
- `planning/coverage/spec-session-lifecycle.md` — session CLIs depend
  on this.
- `planning/coverage/spec-config.md` — config-migration CLI.
- `planning/coverage/spec-setup.md` — setup flow this client drives.
- `AGENTS.md` § Commands.
