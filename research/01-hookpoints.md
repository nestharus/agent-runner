# Hookpoint Research: proposals/01-trace-inspection.md

## Pre-existing code that must change (per PR)

### PR-A
- `src-tauri/src/state/db.rs:209-364` (`StateDb::open`) owns schema creation inline via one `execute_batch`. The current `invocations` table lives at `233-245`; the proposal's `invocations_new` rebuild belongs here, next to the existing schema/backfill work for `provider_quota_windows` at `264-269`. There is no separate migration framework.
- `src-tauri/src/state/db.rs:376-433` (`record_invocation`) is the current completion-time lifecycle. It both updates aggregate `providers` stats and inserts the final `invocations` row in one call. This does not survive intact under insert-on-spawn/update-on-finish; it is the code path to split or replace with start/finalize methods.
- `src-tauri/src/main.rs:226-295` (`run_with_balancing`) is the only production caller of `record_invocation`. Today the executor runs first (`255-257`), diagnostics happen second (`261-265`), then the finished invocation is recorded (`267-276`). PR-A's lifecycle change has to reorder this function around a start insert, stderr ID emission, subprocess execution, and finalization.
- `src-tauri/src/state/db.rs:52-57` (`QuotaWindowInput`), `src-tauri/src/state/db.rs:73-82` (`InvocationRecord`), and `src-tauri/src/sessions/mod.rs:32-39` (`ScriptTurn`) show the current naming split between input payloads and stored records. Any `InvocationStart`/expanded `InvocationRecord` should follow this local convention instead of inventing a second naming scheme.
- `src-tauri/src/executor/cli.rs:214-290` (`execute_provider`) is where child `Command` assembly happens. The runner currently inherits env by default and never calls `env_clear`, `env_remove`, or `envs`; this is the hookpoint for writing the current `OULIPOLY_PARENT_INVOCATION` value onto the spawned child before `spawn()` at `268-270`.
- `src-tauri/src/main.rs:149-203` and `226-295` are where startup env parsing must land. There is currently no parent-invocation parsing anywhere in the runtime, so PR-A's read/validate/resolve step belongs before `executor::execute_with_inputs`.
- `src-tauri/src/main.rs:234`, `276`, `283`, `288-290`, `309-317`, and `335` are every current `eprintln!` site in the CLI runtime. stderr already carries warnings, provider stderr, diagnostics, and fatal errors; the new `OULIPOLY_INVOCATION=...` line belongs in `run_with_balancing` immediately after the durable row is inserted and before the wrapped CLI is spawned.
- `src-tauri/src/lib.rs:103-134`, `161-193`, `src-tauri/src/executor/cli.rs:245-250`, and `src-tauri/Cargo.toml:10-23` confirm the project already uses `uuid::Uuid::new_v4()` and already depends on `uuid`. Reuse that pattern for invocation UUID generation rather than adding a second ID crate or bespoke formatter.

### PR-B
- `src-tauri/src/main.rs:15-55` defines a flat `Cli` struct with `#[derive(Parser)]`; there is no `Subcommand` enum or command dispatch today. `trace` therefore requires refactoring the CLI shape, not slotting another flag into the existing single-command parser.
- `src-tauri/src/main.rs:149-203` (`run`) assumes every CLI path resolves to agent/model execution. Subcommand dispatch will have to split here so `trace` bypasses prompt resolution, model loading for execution, and `run_with_balancing`.
- `src-tauri/examples/quota_check.rs:47-145` and `src-tauri/examples/session_scan.rs:44-73` are the existing manual terminal-formatting patterns: section headers, aligned columns, and explicit `println!` control. Trace's ASCII tree should imitate this lightweight style rather than introducing a formatting library.
- `src-tauri/src/setup/context.rs:76-92` and `src-tauri/src/setup/sync.rs:122-124` are the existing pretty-JSON emission pattern: build a serializable struct and call `serde_json::to_string_pretty`. There is no current `to_writer` use in-tree.
- `src-tauri/src/state/db.rs:436-567`, `698-737`, and `1153-1180` show the project's query style: small focused methods, `prepare`/`query_map` when iterating rows, `query_row` for scalar reads, positional `params![]`, and explicit row-to-struct mapping. Trace tree queries should extend `StateDb` in this style rather than reaching into `rusqlite` directly from `main.rs`.
- `src-tauri/src/lib.rs:296-380` contains Tauri IPC shapes such as `QuotaRefreshEntry`, but nothing in `lib.rs` references invocations or trace data. That confirms the proposal's expectation: trace is a CLI feature, not a new Tauri command.

### PR-C
- The proposal text says `ProviderConfig` in `src-tauri/src/config/providers.rs`, but the executable provider contract actually lives in `src-tauri/src/config/model.rs:6-27`. That is where `session_capture` belongs. `src-tauri/src/config/providers.rs:6-47` is only the quota-script registry (`ProvidersConfig` / `ProviderEntry`).
- `src-tauri/src/config/model.rs:150-165` (`RawModelToml` / `RawProvider`), `363-403` (`ModelConfig::from_toml`), and `260-294` (`to_toml`) are the exact TOML parse/serialize hookpoints for a new provider field. Adding `session_capture` only to the runtime struct would leave model parsing and round-trip tests inconsistent.
- `src-tauri/src/config/model.rs:472-596` contains the provider TOML parsing/round-trip tests that will need to absorb the new optional field.
- `src-tauri/src/config/sessions.rs:12-22` (`SessionSourceEntry`) and `36-64` (`SessionsConfig::load`) are the hookpoints for adding `transcript_locator`. This file already models declarative per-provider session config with optional `state_dir`; the locator should extend that struct rather than creating a second config loader.
- `src-tauri/src/executor/cli.rs:214-290` (`execute_provider`) is the execution hookpoint for `session_capture` dispatch. Today it only appends configured args and input flags (`226-237`), sets `current_dir` (`239-241`), optionally writes a prompt temp file (`243-255`), and waits for output. There is no capture-strategy layer yet.
- `src-tauri/src/executor/cli.rs:243-255` plus `28-37` in the same file show the only current temp-file lifecycle: prompt spillover. Nothing today reads a post-run temp file to reconstruct stdout, so Codex `--json -o <tmpfile>` handling is greenfield logic that must still plug into `execute_provider`'s cleanup path.
- `src-tauri/src/executor/cli.rs:226-237` confirms the runner does not currently mutate user-supplied provider args beyond appending fixed config args and resolved input flags. Claude `--session-id` injection therefore needs a clean addition here; there is no prior "runtime-added provider flag" pattern.
- `src-tauri/src/sessions/mod.rs:141-202` (`run_turn_script`) is the existing shell-script adapter runner with `sh -c`, `STATE_DIR`, timeout handling, and stdout/stderr capture. A transcript locator invocation should reuse or generalize this path instead of cloning the same mechanics into a second helper.
- No current code resolves transcript paths at execution time. Per the proposal, `transcript_path` lookup belongs in the future trace codepath, not in `execute_provider` or `run_with_balancing`.

### PR-D
- `src-tauri/src/state/db.rs:348-361` defines the current `session_turns` table. This is the exact schema hookpoint for `parent_turn_id` and `is_sidechain`; there is no existing `ALTER TABLE` helper, only inline schema bootstrap in `StateDb::open`.
- `src-tauri/src/sessions/mod.rs:32-39` (`ScriptTurn`) is currently the four-field adapter contract. It must widen to optional parent/sidechain metadata, following the project's existing `Option<T>` style rather than introducing sentinel strings.
- `src-tauri/src/sessions/mod.rs:81-111` is the in-memory scan-to-batch path. The batch tuple is currently `(session_id, turn_id, timestamp, role)` and is constructed directly from `ScriptTurn`; this code must widen together with the database batch insert.
- `src-tauri/src/state/db.rs:1112-1150` (`ingest_session_turns_batch`) is the bulk-ingest hookpoint. Its tuple signature, prepared statement, and insert column list all assume the current four-field shape.
- `src-tauri/src/state/db.rs:1079-1106` (`ingest_session_turn`) is the single-row insert sibling and will also need widening if it is kept as a maintained public method.
- `scripts/claude-code-turns:57-82` is where the current Claude adapter strips raw JSONL down to four fields. `parentUuid` and `isSidechain` already exist upstream; this script is the exact place that currently drops them.
- `README.md:232-269`, `scripts/README.md:7-31`, and the module docs in `src-tauri/src/sessions/mod.rs:1-18` all still describe the four-field turn contract and must be updated with the optional fields in the same PR as the code.

## Pre-existing patterns to follow (per PR)

### PR-A
- Schema/backfill work should follow the inline migration convention already in `src-tauri/src/state/db.rs:221-364`, especially the in-place backfill pattern at `264-269` and the transactional write pattern in `upsert_quota_refresh` at `612-662`.
- UUID generation should follow `uuid::Uuid::new_v4().to_string()` as used in `src-tauri/src/lib.rs:103`, `161`, and `src-tauri/src/executor/cli.rs:249`.
- The preferred data-shape pattern is "record struct plus input struct", as shown by `QuotaRecord`/`QuotaWindow`/`QuotaWindowInput` in `src-tauri/src/state/db.rs:27-57`. For session/script-facing input, `src-tauri/src/sessions/mod.rs:32-39` is the lighter-weight example.

### PR-B
- Human-readable terminal output should follow the existing examples in `src-tauri/examples/quota_check.rs:47-145` and `src-tauri/examples/session_scan.rs:44-73`: explicit layout, no external formatter, no ANSI assumptions.
- JSON output should follow the current pretty-print idiom from `src-tauri/src/setup/context.rs:77-92` and `src-tauri/src/setup/sync.rs:122-124`.
- SQLite access should follow `StateDb` query methods like `get_windows` (`src-tauri/src/state/db.rs:535-567`), `get_quotas` (`698-737`), and `count_assistant_turns_since` (`1155-1180`): keep SQL inside `state/db.rs`, return typed structs, use prepared statements when iterating.

### PR-C
- Declarative config parsing should follow `src-tauri/src/config/sessions.rs:29-58`: raw serde struct, optional fields via `#[serde(default)]`, then explicit conversion into a stable config struct.
- If an enum is needed for capture strategies, the closest in-tree serde-tagged patterns are `ParamType` (`src-tauri/src/state/db.rs:87-98`), `AuthMethod` (`130-143`), and `InputType` (`src-tauri/src/config/model.rs:96-123`). Prefer that style over procedural string matching.
- Adapter script execution should prefer `src-tauri/src/sessions/mod.rs:141-202` over `src-tauri/src/quota/mod.rs:161-220`, because the locator contract also needs `STATE_DIR` and shares the same shell/timeout surface as turn scripts.

### PR-D
- Optional contract evolution should follow the quota-script backward-compat pattern in `src-tauri/src/quota/mod.rs:65-84`: add optional serde fields, keep old emitters valid, and collapse defaults in Rust.
- `Option<T>` field style should follow existing structs like `InvocationRecord.error_category` (`src-tauri/src/state/db.rs:80`) and `SessionSourceEntry.state_dir` (`src-tauri/src/config/sessions.rs:19-22`).

## Pre-existing code that must be deleted

- `src-tauri/src/state/db.rs:376-433` (`record_invocation`) as the single completion-time lifecycle entrypoint is obsoleted by PR-A's start/finalize design.
- `src-tauri/src/main.rs:267-276` is the old single-call invocation-recording site and disappears once lifecycle recording is split.
- `scripts/claude-code-turns:76-81` and the four-field contract text in `README.md:254-258`, `scripts/README.md:21-24`, and `src-tauri/src/sessions/mod.rs:11-13` are obsolete once sidechain fields are part of the adapter contract.

## Parallel-systems risks

- Do not add `session_capture` to `src-tauri/src/config/providers.rs`; despite the similar name, that file is quota-only. Use `src-tauri/src/config/model.rs:6-27` (`ProviderConfig`) and its TOML loader at `150-165` / `363-403`.
- Do not create a new invocation-ID table. Extend `invocations` in `src-tauri/src/state/db.rs:233-245`; the integer PK already anchors local joins, and the proposal explicitly adds a caller-visible UUID alongside it.
- Do not add a second session-metadata table for sidechains. Use `session_turns` in `src-tauri/src/state/db.rs:348-361` and widen `ScriptTurn` / `ingest_session_turns_batch`.
- Do not introduce runner-owned transcript-path or transcript-content persistence. The existing split is already present in the codebase: SQLite metadata in `state/db.rs`, raw content in adapter-land (`sessions.toml`, `scripts/`). Use `session_id` plus `transcript_locator`; keep content out of SQLite.
- Do not add CLI-name sniffing in `executor/cli.rs`. The correct extension point is a declarative field on `ProviderConfig`, matching project values and the existing `quota_script` / `turn_script` patterns.
- Do not build a second script-runner helper just for locators. Reuse or generalize `src-tauri/src/sessions/mod.rs:141-202`.
- Do not surface trace through `src-tauri/src/lib.rs` IPC unless scope changes later. `lib.rs:296-380` has no invocation/trace surface today, and the proposal is CLI-only.

## New code with no existing counterpart

- A `trace` subcommand implementation has no counterpart today. There is no invocation-tree module, no trace query API on `StateDb`, and no trace output struct.
- Composite invocation ID parsing/formatting (`{"source","id"}`) is greenfield. Existing UUID use only covers setup session IDs and temp filenames.
- `session_capture` strategy definitions are greenfield as a runtime contract, even though they should live inside the existing provider config machinery.
- Transcript-locator adapters are greenfield in `scripts/`; the repository currently ships only `claude-code-turns`, `codex-turns`, and quota scripts.
- Transcript rendering / inline raw-record export is greenfield. Nothing currently reads provider transcript files back into a CLI response surface.

## Test patterns to follow

- DB tests: `src-tauri/src/state/db.rs:1198-1246` uses `StateDb::open(Path::new(":memory:"))` and focused unit tests around schema/method behavior.
- Adapter-script tests: `src-tauri/src/sessions/mod.rs:214-326` uses tempdirs plus executable fixture scripts written to disk, then asserts DB side effects and collected errors.
- Balancer tests: `src-tauri/src/balancer/mod.rs:221-415` show how to seed quota windows / invocation history and assert deterministic selection outcomes.
- Provider-config parse/round-trip tests: `src-tauri/src/config/model.rs:472-596` are the pattern for new optional TOML fields on providers.
- Sessions-config parse tests: `src-tauri/src/config/sessions.rs:75-126` are the pattern for extending `sessions.toml` with another optional field.

## README + scripts/README sections to update

- `README.md` `## Session Ingestion` (`232-269`)
- `README.md` `### Diagnostic tools` (`271-279`)
- `README.md` `## Configuration` (`291-300`) because it describes `sessions.toml`
- `scripts/README.md` `## Turn scripts (sessions.toml)` (`7-91`)
- `scripts/README.md` `### Contract` (`14-31`)
- `scripts/README.md` `### Wiring` (`33-52`)
- `scripts/README.md` `### Bundled reference scripts` (`54-65`)

## Cargo dependencies to add

None. `src-tauri/Cargo.toml:10-23` already contains `clap`, `serde`, `serde_json`, `rusqlite`, `dirs`, `tempfile`, `uuid`, and `chrono`.

## Risk-gate findings re-checked against existing code

- Shortcut F1 / Scope F1 map cleanly: declarative capture belongs in `src-tauri/src/config/model.rs:6-27`, `150-165`, `260-294`, `363-403`, with runtime dispatch in `src-tauri/src/executor/cli.rs:214-290`. That avoids a parallel CLI-specific branch.
- Shortcut F2 maps cleanly: `transcript_locator` extends `src-tauri/src/config/sessions.rs:12-22` and can execute through the existing shell adapter runner in `src-tauri/src/sessions/mod.rs:141-202`. No runner-side storage-layout logic is required.
- Shortcut F3 maps cleanly: the `legacy` migration path belongs inside `src-tauri/src/state/db.rs:221-364`, where schema/backfill logic already lives. No sentinel `provider_name` column needs to be invented.
- Shortcut F6 maps cleanly: verified readback belongs inside the executor hookpoint at `src-tauri/src/executor/cli.rs:214-290`, with degraded outcomes persisted on `invocations`; there is no existing heuristic fallback code to untangle.
- Scope F6 also maps cleanly: the repository already divides concerns the same way the proposal does, with schema/runtime in `state/db.rs` and `main.rs`, adapter contracts in `config/*.rs` plus `scripts/`, and CLI formatting examples in `src-tauri/examples/`. The 4-PR split attaches to those existing seams rather than cutting across them.
