# Hookpoint Research: proposals/02-interactive-resume.md

## Pre-existing code that must change (per PR)

### PR-E
- `src-tauri/src/main.rs:17-61` (`Cli`) is the parser surface that must stay backward-compatible. The flat positional `agent` + `prompt_args` path remains the no-subcommand path; the existing regression at `src-tauri/src/main.rs:539-568` is the proof point to preserve.
- `src-tauri/src/main.rs:17-22` carries `args_conflicts_with_subcommands = true`; `Subcommands::Repl` must be added in-place at `src-tauri/src/main.rs:63-89` without weakening that decoration. `Trace` is the existing model for how a new subcommand is introduced.
- `src-tauri/src/main.rs:183-202` is the actual `match cli.command` dispatch site. `Subcommands::Repl { ... }` joins this match beside `Subcommands::Trace`, so interactive launch bypasses `resolve_prompt()` and the agent/model one-shot path cleanly.
- `src-tauri/src/config/model.rs:6-16` is the real `ProviderConfig`; `src-tauri/src/config/providers.rs:6-17` is quota-only and must not be reused. `interactive_args: Option<Vec<String>>` and `resume: Option<ResumeStrategy>` belong in `config/model.rs` and its parse/serialize pipeline, not in `providers.toml`.
- `src-tauri/src/config/model.rs:221-238`, `442-507`, and `510-563` are the TOML round-trip hookpoints. `RawModelToml`, `RawProvider`, `ModelConfig::from_toml()`, `ModelConfig::to_toml()`, and `toml_string_literal()` must all learn the new provider fields together.
- `src-tauri/src/config/model.rs:57-97` (`SessionCapture::validate`) is the exact validation pattern to imitate. `ResumeStrategy::validate()` should follow this load-time rejection style rather than deferring malformed config to executor-time failures.
- `src-tauri/src/executor/cli.rs:221-310` (`execute_provider`) is the existing subprocess assembly path. Interactive execution should live beside it in the same module so command splitting, `current_dir`, and `OULIPOLY_PARENT_INVOCATION` reuse stay local; otherwise PR-E would duplicate command construction in a parallel module.
- `src-tauri/src/executor/cli.rs:275-283` is also the boundary where the current path becomes unusable for REPLs: stdin is `null`/`piped`, stdout/stderr are always `Stdio::piped()`, and `wait_with_output()` at `297-299` assumes captured IO. `Stdio::inherit()` is greenfield in this codebase and should be treated as a separate path, not a small flag on the captured pipeline.
- `src-tauri/src/main.rs:321-428` (`run_with_balancing`) is the invocation lifecycle template PR-E must reuse: open DB, resolve parent invocation, select provider, insert `running`, emit invocation metadata, spawn child, update session capture, finalize row, then emit model output or diagnostics. `repl` needs the same lifecycle ordering around a different executor.
- `src-tauri/src/state/db.rs:760-881` (`start_invocation` / `finalize_invocation`) are the right persistent lifecycle hookpoints. There is no older single-call invocation writer left to extend.
- `src-tauri/src/main.rs:360-362` is the existing `OULIPOLY_INVOCATION` emission and `src-tauri/src/main.rs:158-159` is the repo’s only `is_terminal()` gate pattern. PR-E’s interactive path needs the existing emission string plus that existing TTY-test style; there is no helper that already combines them.
- `src-tauri/src/executor/cli.rs:253-258` is the existing `OULIPOLY_PARENT_INVOCATION` propagation hook. Interactive spawn must set the same env var through the same `Command` path.

### PR-F
- `src-tauri/src/state/db.rs:1736-1759` (`count_session_turns`) is the closest existing query helper for provider-scoped session lookups. The new bare-`session_id` provider lookup is greenfield, but it should follow this same `query_row`/typed-return style and stay in `StateDb`.
- `src-tauri/src/state/db.rs:599-605` (`session_turns_index_sql`) is where `idx_session_turns_session_lookup (session_id, timestamp)` must be added for fresh bootstrap. `src-tauri/src/state/db.rs:522-540` (`ensure_session_turns_schema`) is the additive ensure path that already re-applies `CREATE INDEX IF NOT EXISTS` on existing DBs.
- `src-tauri/src/state/db.rs:448-460` is the bootstrap `session_turns` table creation site. The proposal does not need a new table or a second lookup corpus; it needs an extra index on the existing table.
- `src-tauri/src/main.rs:204-230` loads `HashMap<String, ModelConfig>` and resolves a named model, while `src-tauri/src/main.rs:346-348` selects one provider index from that model’s `providers` list. PR-F’s “resolved provider must belong to requested model” check should reuse `model.providers` directly; no extra provider-pool index is justified.
- `src-tauri/src/config/model.rs:149-156` and `587-619` confirm `ModelConfig.providers` is already an ordinary iterable vector and `load_models()` already returns an iterable `HashMap<String, ModelConfig>`. Suggestion text for “try model X” should scan that loaded map; there is no missing registry to build first.
- `src-tauri/src/lib.rs:47-75` (`derive_pools`) is a concrete in-tree example of iterating every loaded model and each model’s providers. The “find every model containing provider P” suggestion path should imitate this pattern rather than inventing a persistent reverse index.
- `src-tauri/src/state/db.rs:893-908` (`update_session_capture`) already accepts arbitrary method strings and writes both `session_id` and `session_capture_method`. PR-F should reuse it directly for `"resumed"` exactly as the proposal says; no DB API change is needed for the write itself.
- `src-tauri/src/main.rs:383-385`, `423-425`, and `454-462` are the current stderr-emission patterns for short runner-owned lines. `[resume] -> <provider>` belongs alongside these `eprintln!` patterns, not in executor stdout/stderr streams.
- `src-tauri/src/trace/mod.rs:220-349` (`build_trace_session`) is the only session-state read path in trace and already special-cases `"failed"` at `226-230`. PR-F must extend this function in place for `"resumed"`; no parallel trace renderer is needed.
- `src-tauri/src/trace/mod.rs:378-390` (`format_ascii_node`) is the only human text formatter. The proposal’s wording change (“Resume target:” vs “Session:”) maps here even though the current output is compact `session=<id>` text rather than a labeled line.
- `src-tauri/src/state/db.rs:186-192` (`CompositeInvocationId::parse_env_value`) and `src-tauri/src/trace/mod.rs:110-111` are existing `Uuid::parse_str` validation sites. PR-F’s full-UUID `--resume` validation should match this style.

## Pre-existing patterns to follow (per PR)

### PR-E
- Clap subcommand tests already exist at `src-tauri/src/main.rs:492-590`. `repl` parsing and “reserved first token” regressions should live beside the `trace` parser tests.
- The local RAII `Drop` pattern is `src-tauri/src/quota/mod.rs:52-63` (`InFlightGuard`). It is not lifecycle-specific, but it is the only current “guard owns cleanup on drop” pattern to imitate.
- The safest lifecycle assertions live in `src-tauri/src/state/db.rs:2221-2380` and especially `2366-2379`, which proves `finalize_invocation()` rejects double-finalize. PR-E’s guard tests should extend this expectation, not weaken it.
- Executor private-helper style is `src-tauri/src/executor/cli.rs:325-386` (`build_capture_plan`). If PR-E needs a shared argv builder under `execute_provider()` and `execute_interactive()`, that helper style is the in-tree model.

### PR-F
- Provider-config parse/round-trip tests live at `src-tauri/src/config/model.rs:716-1050`. `interactive_args`, `ResumeStrategy`, and canonical Claude/Codex shapes should extend this test block.
- DB helper tests already use the in-memory `StateDb` fixture at `src-tauri/src/state/db.rs:1815-1817`; `count_session_turns` coverage at `2577-2639` is the closest template for the new ordered session lookup helper.
- Trace tests seed invocation rows and session metadata in-place at `src-tauri/src/trace/mod.rs:428-500`, then assert ASCII/JSON output at `742-1145`. A `"resumed"` warning test should follow this exact fixture pattern.

## Pre-existing code that must be deleted

- None identified. Init-02 is additive if implemented against the existing seams above. The main deletion risk is accidental duplication, not actual obsolete code.

## Parallel-systems risks

- Do not add interactive or resume fields to `src-tauri/src/config/providers.rs:6-17`; that file is quota-only. Use `src-tauri/src/config/model.rs:6-16` and its TOML hooks.
- Do not add `mark_resumed_session()` or any second writer for `session_id` + `session_capture_method`. `src-tauri/src/state/db.rs:893-908` is already the atomic writer for that column pair.
- Do not build a new session-ownership table from `invocations.session_id`. Provider lookup must use `session_turns` at `src-tauri/src/state/db.rs:448-460`, because that is the populated corpus and the proposal already scopes the new work to an index plus a helper.
- Do not create a provider-to-model reverse index in memory or SQLite. Suggestions can scan `HashMap<String, ModelConfig>` from `load_models()` (`src-tauri/src/config/model.rs:587-619`) exactly the way `derive_pools()` scans models at `src-tauri/src/lib.rs:47-75`.
- Do not force interactive launch through `execute_provider()` by bolting on conditionals while leaving `stdout`/`stderr` piped. The current captured executor at `src-tauri/src/executor/cli.rs:221-310` is fundamentally one-shot.
- Do not add Tauri commands for `repl` or resume. `src-tauri/src/lib.rs:693-726` wires the current IPC surface, and none of those commands are invocation/trace launch surfaces.
- Do not fork trace into a second “resume-aware” renderer. Extend `build_trace_session()` and `format_ascii_node()` in `src-tauri/src/trace/mod.rs:220-390`.

## New code with no existing counterpart

- `Subcommands::Repl` itself is greenfield; only `Trace` exists today.
- `ResumeStrategy` / `ResumeKind` are greenfield config types, even though they belong in the existing model-config machinery.
- `execute_interactive()` with inherited stdio is greenfield in production code. No current executor path uses `Stdio::inherit()` or `wait()`.
- Production Unix signal handling is greenfield. There is no `signal_hook`, `nix`, `SIGINT`, `SIGTERM`, or `SIGHUP` handling in `src-tauri/src`; current `cfg(unix)` usage is test-only (`executor/cli.rs:706-1050`, `trace/mod.rs:396-1146`, `sessions/mod.rs:276`).
- A lifecycle finalizer guard around invocation rows is effectively greenfield. The repo has `Drop` guards, but nothing today wraps `start_invocation()`/`finalize_invocation()` that way.
- The “ordered providers for bare session_id” DB helper is greenfield; only provider-scoped session queries exist today.

## Test patterns to follow

- Subcommand parsing: `src-tauri/src/main.rs:492-590`.
- Provider-config round-trip and validation: `src-tauri/src/config/model.rs:716-1050`.
- Lifecycle/state assertions around start/finalize/update: `src-tauri/src/state/db.rs:2221-2503`.
- DB lookup/counting on `session_turns`: `src-tauri/src/state/db.rs:2577-2639`.
- Trace fixture seeding and ASCII/JSON assertions: `src-tauri/src/trace/mod.rs:428-1145`.
- Executor integration fixtures with temp scripts: `src-tauri/src/executor/cli.rs:534-1050`.

## README + scripts/README sections to update

- `README.md:294-374` under `## Inspecting a Run`, especially `### trace subcommand` and `### Configuring session capture`.
- `README.md:232-292` under `## Session Ingestion` and `### Optional: transcript_locator`, because PR-F changes how `"resumed"` and `"none"` should be described in trace output.
- `scripts/README.md:104-181` under `## Transcript locators` and `## Session capture`, because PR-F explicitly composes with and bypasses `[providers.session_capture]`.

## Cargo dependencies to add

- `uuid`, `clap` with `derive`, `serde`, and `rusqlite` are already present at `src-tauri/Cargo.toml:10-23`.
- `signal-hook` is not present, and there is no other signal-registration crate in the manifest. If PR-E implements the proposal’s Unix signal handling without direct platform FFI, this dependency must be added; the proposal’s “no new crate” claim does not match the current manifest.

## Risk-gate findings re-checked against existing code

- Round 0 maps cleanly. The session lookup performance finding lands exactly in `src-tauri/src/state/db.rs:599-605` plus `522-540`; the `interactive_args` drift caveat lands in `src-tauri/src/config/model.rs:332-507` and its tests; the resume observability fix fits existing `eprintln!` usage in `src-tauri/src/main.rs:383-425`.
- Round 1 also maps cleanly. Provider/model mismatch checking belongs on the already-loaded `model.providers` vector (`src-tauri/src/main.rs:211-230`, `346-348`); Codex composition verification belongs in executor integration tests (`src-tauri/src/executor/cli.rs:786-1050`); the no-op-after-finalize requirement is enforced by the existing double-finalize error at `src-tauri/src/state/db.rs:2366-2379`.
- Round 2 does not require any parallel system. The explicit “no fallback from `interactive_args` to `args`” revision fits the current model-config loader because the two arrays are parsed independently in one place (`src-tauri/src/config/model.rs:221-238`, `442-507`). The Unix-only caveat is accurate because production code has no signal abstraction today. The “do not reconcile stranded rows at startup” deferral matches the current `StateDb::open()` shape at `320-540`, which already does additive schema work but no invocation-status inference.
- Round 3 is especially clean against existing code. Reusing `update_session_capture()` at `src-tauri/src/state/db.rs:893-908` avoids a second writer and exactly matches the one-source-of-truth concern flagged in the risk review.
- Round 4 also maps cleanly without a new module. `src-tauri/src/trace/mod.rs:220-349` already owns session-state warnings, transcript lookup, and turn counts, and `226-230` is the precise existing special-case branch to extend for `"resumed"`. Extending this function and the existing text formatter at `378-390` is sufficient; there is no need for a second trace rendering path.
