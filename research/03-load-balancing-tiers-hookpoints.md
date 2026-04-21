# Phase 5 Hookpoints — Load-Balancing Redesign

This document maps `proposals/03-load-balancing-tiers.md` onto the current codebase. It is organized by the locked three-PR structure and does not revise the design.

## 1. PR 1 hookpoints — `chatgpt-usage`

### 1.1 Target script

- Target file: `/home/nes/.local/bin/chatgpt-usage`.
- Current script is Bash (`/home/nes/.local/bin/chatgpt-usage:1`, `/home/nes/.local/bin/chatgpt-usage:15`).
- Current docs advertise the legacy flat single-window output shape, not a `windows` array (`/home/nes/.local/bin/chatgpt-usage:10-13`).
- Current credential validation reads `tokens.access_token` and `tokens.account_id`; unreadable file exits before emission (`/home/nes/.local/bin/chatgpt-usage:17-29`).
- Current HTTP request calls `https://chatgpt.com/backend-api/wham/usage` with bearer token and account id headers (`/home/nes/.local/bin/chatgpt-usage:31-34`).
- Exact rewrite site is the response comment plus `jq` emitter: it currently says `secondary_window` is the 7-day window and emits only `.rate_limit.secondary_window` as `{used_percent, resets_at}` (`/home/nes/.local/bin/chatgpt-usage:36-46`).

### 1.2 Tracked source precedent

- Tracked precedent file: `scripts/anthropic-usage`.
- Shebang/interpreter and strict Bash mode are at `scripts/anthropic-usage:1` and `scripts/anthropic-usage:21`.
- Comment block documents the multi-window JSON shape and the current positional-window caveat (`scripts/anthropic-usage:8-19`).
- Credential validation exits before JSON emission for unreadable or tokenless inputs (`scripts/anthropic-usage:23-34`).
- Emit block builds `windows` with seven-day first and five-hour second via conditional `jq` entries (`scripts/anthropic-usage:41-54`).
- `scripts/README.md` is the local adapter-script convention document; it says scripts are standalone executables wired through TOML, not linked into the binary (`scripts/README.md:1-5`).
- `scripts/README.md` quota section documents multi-window quota scripts and legacy single-window compatibility (`scripts/README.md:191-209`).
- Top-level README also documents quota adapters as scripts in `scripts/` and shows manual installation to `~/.local/bin/` (`README.md:247-258`).

### 1.3 Deploy relationship

- The installed `chatgpt-usage` is currently outside the repo at `/home/nes/.local/bin/chatgpt-usage`; no tracked `scripts/chatgpt-usage` file exists in the documented reference-adapter inventory (`README.md:247-258` documents only tracked reference quota adapters; `scripts/README.md:207-209` names `anthropic-usage` and `zai-usage` only).
- Current repo docs show manual deployment, not generation: `install -m 755 scripts/anthropic-usage scripts/zai-usage ~/.local/bin/` (`README.md:254-258`).
- The implementation hookpoint is therefore: add tracked `scripts/chatgpt-usage`, update `/home/nes/.local/bin/chatgpt-usage` from it manually, and update docs if the tracked reference-adapter list needs to include the new file (`README.md:247-258`, `scripts/README.md:207-209`).

### 1.4 Parser contract

- Rust parser surface is `QuotaScriptOutput` and `QuotaScriptWindow`; `windows` is preferred and legacy `used_percent`/`resets_at` is fallback (`src-tauri/src/quota/mod.rs:65-84`).
- `refresh_provider` calls `run_script`, then writes the parsed `Vec<QuotaWindowInput>` through `StateDb::upsert_quota_refresh` (`src-tauri/src/quota/mod.rs:100-127`).
- `parse_output` accepts either `windows` or legacy flat shape, normalizes `used_percent` values above `1.0` as percentages, parses `resets_at` as RFC3339, and returns `Vec<QuotaWindowInput>` (`src-tauri/src/quota/mod.rs:222-265`).

### 1.5 Non-hookpoints

- Do not change `scripts/anthropic-usage` for PR 1; it is the precedent, not the target (`scripts/anthropic-usage:1-54`).
- Do not change the Rust quota parser for PR 1; it already accepts the target `windows` array (`src-tauri/src/quota/mod.rs:65-84`, `src-tauri/src/quota/mod.rs:222-265`).
- Do not change `providers.toml` parsing; it only stores the shell command string and is already format-agnostic (`src-tauri/src/config/providers.rs:6-23`, `src-tauri/src/config/providers.rs:25-51`).

## 2. PR 2 hookpoints — `is_stale` + `upsert_quota_refresh`

### 2.1 `is_stale` function

- Current `is_stale` is `src-tauri/src/quota/mod.rs:129-143`.
- It returns stale when the provider quota row is absent or `refreshed_at` is absent (`src-tauri/src/quota/mod.rs:132-138`).
- Exact semantic change site is after `let windows = state.get_windows(provider_name).unwrap_or_default();`; today line 140 immediately passes possibly-empty windows to `dynamic_ttl_secs` (`src-tauri/src/quota/mod.rs:139-142`).
- The proposal's empty-window guard belongs between current lines 139 and 140 (`src-tauri/src/quota/mod.rs:139-142`).

### 2.2 `dynamic_ttl_secs` function

- Current `dynamic_ttl_secs` is `src-tauri/src/quota/mod.rs:145-159`.
- It currently treats an empty window set as `MAX_TTL_SECS` (`src-tauri/src/quota/mod.rs:145-151`).
- Non-empty TTL math computes minimum seconds until reset, then divides by `REFRESH_WINDOW_DIVISOR` and clamps to `[MIN_TTL_SECS, MAX_TTL_SECS]` (`src-tauri/src/quota/mod.rs:152-158`).
- Per proposal, this helper does not change because the semantic bug is "a provider row with zero current windows is stale"; that state is known in `is_stale`, which has both the quota row and window query in hand (`src-tauri/src/quota/mod.rs:132-142`).

### 2.3 `upsert_quota_refresh` function

- Current body is `src-tauri/src/state/db.rs:1148-1242`.
- Prior reads already happen before mutation: `prior = get_quota`, `prior_windows = get_windows`, and `longest_prior` are computed before the transaction opens (`src-tauri/src/state/db.rs:1162-1166`).
- Provider-level delta computation is currently longest-window based and must be bypassed/reworked for empty-input rejection (`src-tauri/src/state/db.rs:1168-1182`).
- Legacy provider mirror currently converts empty input into `used_percent = 0.0` and `resets_at = NULL`; this must not run in the "prior windows exist and input is empty" preservation path (`src-tauri/src/state/db.rs:1184-1189`).
- Transaction opens at `src-tauri/src/state/db.rs:1191-1194`.
- Provider quota upsert currently resets `calls_since_refresh`, overwrites `refreshed_at`, and writes provider-level deltas on every call; the empty-input branch must update only the proposal-approved audit fields when prior windows exist (`src-tauri/src/state/db.rs:1196-1217`).
- Window deletion is the destructive site that PR 2 must skip on empty input (`src-tauri/src/state/db.rs:1219-1223`).
- Window insertion loop is the non-empty replacement path and remains for normal writes (`src-tauri/src/state/db.rs:1225-1238`).
- Commit/return are at `src-tauri/src/state/db.rs:1240-1242`.

### 2.4 `last_empty_refresh_at` column

- New-database `provider_quotas` table declaration lives in `StateDb::open` at `src-tauri/src/state/db.rs:352-360`.
- Existing schema-ensure pattern is implemented inline before/after table creation: `ensure_invocations_schema` is called before broad table creation, and `ensure_session_turns_schema` is called after it (`src-tauri/src/state/db.rs:338-470`).
- Column-add ensure examples exist for `invocations.session_id`, `invocations.session_capture_method`, `session_turns.parent_turn_id`, and `session_turns.is_sidechain` (`src-tauri/src/state/db.rs:482-546`).
- `QuotaRecord` currently exposes only `provider_name`, `calls_since_refresh`, `refreshed_at`, and provider-level deltas (`src-tauri/src/state/db.rs:24-40`).
- `get_quota` currently selects and maps no audit timestamp field (`src-tauri/src/state/db.rs:1077-1107`).
- The proposal frames `last_empty_refresh_at` as audit-only; it does not need to surface on `QuotaRecord` unless an implementation test or UI needs to read it through the typed Rust API (`src-tauri/src/state/db.rs:24-40`, `src-tauri/src/state/db.rs:1077-1107`).

### 2.5 Existing tests to preserve

- `quota/mod.rs` TTL tests are the direct current coverage for `dynamic_ttl_secs`: short-window shrink (`src-tauri/src/quota/mod.rs:322-340`), min clamp (`src-tauri/src/quota/mod.rs:342-353`), and empty-window max fallback (`src-tauri/src/quota/mod.rs:355-358`).
- The empty-window TTL test is no longer a behavior test for `is_stale`; it should either remain as a pure helper test if `dynamic_ttl_secs` stays unchanged, or be supplemented/replaced by an `is_stale` forced-refresh test (`src-tauri/src/quota/mod.rs:355-358`, `src-tauri/src/quota/mod.rs:132-143`).
- `balancer/mod.rs` missing-window fallback test currently depends on one provider having no windows and `select_provider` falling to invocation-count mode (`src-tauri/src/balancer/mod.rs:416-433`). It remains valid for "never refreshed/no windows" unless PR 3 replaces the fallback semantics.
- No `state/db.rs` unit test currently pins `upsert_quota_refresh` empty-input behavior by name; quota write coverage is mostly indirect through balancer test seeding (`src-tauri/src/balancer/mod.rs:327-423`) and through `upsert_quota_refresh` call sites found in the current source (`src-tauri/src/quota/mod.rs:118-127`, `src-tauri/src/state/db.rs:1155-1242`).

### 2.6 Non-hookpoints

- Do not change `refresh_provider`'s script execution contract in PR 2; it already passes parsed windows through to `upsert_quota_refresh` (`src-tauri/src/quota/mod.rs:100-127`).
- Do not change `parse_output`; PR 2 rejects empty successful refreshes at the DB write path, not at the parser contract (`src-tauri/src/quota/mod.rs:222-265`).
- Do not touch `scripts/anthropic-usage`; Rust must defend against any scraper emitting `windows: []` (`scripts/anthropic-usage:45-54`, `src-tauri/src/quota/mod.rs:222-265`).
- Do not change scoring in PR 2; `select_provider` only needs the new `is_stale` behavior to self-heal empty-window rows before its existing window-gather/scoring gate (`src-tauri/src/balancer/mod.rs:32-69`).

## 3. PR 3 hookpoints — scoring redesign

### 3.1 Schema migrations (§4.2)

- `provider_quotas` `CREATE TABLE` is `src-tauri/src/state/db.rs:352-360`; it currently includes provider-level `last_delta_percent` and `last_delta_calls`.
- `provider_quota_windows` `CREATE TABLE` is `src-tauri/src/state/db.rs:362-368`; it currently has no per-window delta fields.
- `invocations` fresh schema lives in `invocations_schema_sql` (`src-tauri/src/state/db.rs:564-590`).
- Existing `invocations` schema ensure/migration pattern is `ensure_invocations_schema`: inspect columns, add simple missing columns, otherwise rebuild legacy rows (`src-tauri/src/state/db.rs:482-511`).
- Legacy rebuild creates `invocations_new`, copies rows, drops old table, renames, and recreates indexes (`src-tauri/src/state/db.rs:616-735`). If `quota_tight_routing` must be present during legacy rebuilds, add it to both the new table shape and insert mapping here (`src-tauri/src/state/db.rs:658-727`).
- `InvocationRecord`, `InvocationStart`, `start_invocation`, and `map_invocation_row` are the typed plumbing that must reflect any persisted invocation column (`src-tauri/src/state/db.rs:103-129`, `src-tauri/src/state/db.rs:768-797`, `src-tauri/src/state/db.rs:919-1013`).

### 3.2 Model TOML (§4.3)

- `ModelConfig` currently contains `name`, `prompt_mode`, `providers`, and `inputs`; add the `[balancer]` typed field here (`src-tauri/src/config/model.rs:203-210`).
- `RawModelToml` currently accepts command/provider/session/input fields and has no balancer block (`src-tauri/src/config/model.rs:273-285`).
- Existing validation pattern lives on typed structs such as `ResumeStrategy::validate`, `SessionCapture::validate`, `ProviderConfig::validate_interactive_args`, and `parse_inputs` (`src-tauri/src/config/model.rs:37-42`, `src-tauri/src/config/model.rs:61-83`, `src-tauri/src/config/model.rs:111-151`, `src-tauri/src/config/model.rs:326-388`).
- `to_toml` is the serializer hookpoint; it currently emits single-provider or multi-provider blocks, optional resume/session capture blocks, and `[[inputs]]` blocks (`src-tauri/src/config/model.rs:390-508`).
- Existing optional block serializer helpers are `append_resume_toml` and `append_session_capture_toml` (`src-tauri/src/config/model.rs:595-642`).
- `from_toml` is the parser hookpoint; parse errors are returned as `String`, inputs are parsed before providers, providers are built from either `[[providers]]` or single command, and validation errors are decorated with model/provider context (`src-tauri/src/config/model.rs:510-591`).
- `load_models` propagates `ModelConfig::from_toml` errors while reading all TOML files from a models directory (`src-tauri/src/config/model.rs:693-725`).

### 3.3 Risk class plumbing (§4.4)

- CLI parser hookpoint: `Cli` currently has no risk-class option; existing main flags end at repeated `--input` (`src-tauri/src/main.rs:17-61`).
- `Subcommands` currently has `Trace` and `Repl`; `Repl` has model, resume, project, and models-dir fields (`src-tauri/src/main.rs:63-105`).
- Runtime env-var reads currently include `OULIPOLY_PARENT_INVOCATION` in `resolve_parent_invocation_id` and `OPENAI_API_KEY` in setup detection (`src-tauri/src/main.rs:706-714`, `src-tauri/src/setup/detection.rs:334-345`).
- Test-only env-var manipulation for `OULIPOLY_PARENT_INVOCATION` is in main tests, and test-only `XDG_CONFIG_HOME` manipulation is in state DB tests (`src-tauri/src/main.rs:776-802`, `src-tauri/src/state/db.rs:1901-1926`, `src-tauri/src/state/db.rs:2246-2260`).
- `resolve_prompt` is the stdin TTY hookpoint; it checks file, positional prompt, then `std::io::stdin().is_terminal()` before reading piped stdin (`src-tauri/src/main.rs:165-188`).
- `resolve_parent_invocation_id` currently loses "env var was set but invalid" as a distinct signal because it returns `None` for unset, malformed, unknown, invalid UUID, and provider mismatch (`src-tauri/src/main.rs:706-714`, `src-tauri/src/main.rs:1060-1115`). The risk cascade needs an env presence check, not just the resolved parent row id.
- `select_provider` current signature returns `usize` and accepts only model, state, and optional `BalanceContext` (`src-tauri/src/balancer/mod.rs:22-70`).
- Current production callers are `run_repl` (`src-tauri/src/main.rs:525`), `run_with_balancing` (`src-tauri/src/main.rs:622`), and Tauri `test_model` (`src-tauri/src/lib.rs:490-504`).
- Current non-production diagnostic example caller is `src-tauri/examples/quota_check.rs:117-119`; it must also compile after the signature change.
- `InvocationStart` is the call-site payload for the new `quota_tight_routing` column (`src-tauri/src/state/db.rs:123-129`).
- `start_invocation` currently inserts no quota-tight column (`src-tauri/src/state/db.rs:768-797`).
- Start call sites to update include `run_repl` and `run_with_balancing` (`src-tauri/src/main.rs:539-545`, `src-tauri/src/main.rs:628-634`), balancer test helper (`src-tauri/src/balancer/mod.rs:228-245`), main/state tests (`src-tauri/src/main.rs:1160-1227`, `src-tauri/src/state/db.rs:1872-1886`, `src-tauri/src/state/db.rs:2313-2379`, `src-tauri/src/state/db.rs:2460-2544`), and other state tests found through the repeated `InvocationStart` pattern (`src-tauri/src/state/db.rs:2878-2920`).

### 3.4 Per-window delta learning (§4.5)

- PR 3 layers on PR 2's `upsert_quota_refresh` rewrite; the current function already reads prior quota and prior windows before mutation at `src-tauri/src/state/db.rs:1164-1166`.
- `QuotaWindow` is the window read type used by the balancer; add per-window delta fields here after schema migration (`src-tauri/src/state/db.rs:42-52`).
- `get_windows` is the window read path and currently selects/maps only `window_id`, `used_percent`, and `resets_at` ordered by `window_id` (`src-tauri/src/state/db.rs:1109-1146`).
- Current provider-level delta computation and carry-forward are in `upsert_quota_refresh`; PR 3 replaces this with per-window keyed learning (`src-tauri/src/state/db.rs:1162-1182`).
- Current window replacement deletes all rows and reinserts by enumerated input position; PR 3 should preserve replacement semantics while adding per-window delta columns to insert rows (`src-tauri/src/state/db.rs:1219-1238`).
- `count_assistant_turns_since` already provides the assistant-turn count since prior `refreshed_at` (`src-tauri/src/state/db.rs:1809-1837`).
- Provider-level delta exposure is in `QuotaRecord`, `get_quota`, and `get_quotas`; those are the delete/update hookpoints when deltas move to windows (`src-tauri/src/state/db.rs:29-40`, `src-tauri/src/state/db.rs:1077-1107`, `src-tauri/src/state/db.rs:1277-1317`).

### 3.5 Bootstrap cascade (§4.6)

- Delete/replace `global_avg_percent_per_call`; it is private and only called by `score_by_density` (`src-tauri/src/balancer/mod.rs:94`, `src-tauri/src/balancer/mod.rs:144-165`).
- Current implementation sums provider-level deltas across `QuotaRecord`s and returns `0.0` when no data exists; PR 3 replaces that scalar with per-window pool average and duration-ratio fallback helpers (`src-tauri/src/balancer/mod.rs:144-165`).
- New `pool_window_avg_percent_per_call` and `duration_ratio_fallback_percent_per_call` should live near the scoring helpers in `balancer/mod.rs`, where `score_by_density`, `global_avg_percent_per_call`, `score_by_invocation_count`, and `round_robin_fallback` currently live together (`src-tauri/src/balancer/mod.rs:88-218`).
- The helpers should use `QuotaWindow.window_id` and `get_windows`'s ordered positional identity rather than adding plan-class metadata (`src-tauri/src/state/db.rs:42-52`, `src-tauri/src/state/db.rs:1109-1119`).

### 3.6 Scoring function (§4.7)

- Current `score_by_density` body is `src-tauri/src/balancer/mod.rs:88-142`.
- Projection scalar setup is line 94 (`src-tauri/src/balancer/mod.rs:88-95`).
- Per-provider loop and operational error block are `src-tauri/src/balancer/mod.rs:97-107`.
- Assistant-turn count since refresh is `src-tauri/src/balancer/mod.rs:109-116`.
- Projection loop currently applies the same scalar to each window and computes `remaining / hours` (`src-tauri/src/balancer/mod.rs:118-130`).
- Binding fold-min is `src-tauri/src/balancer/mod.rs:120-131`.
- Score push and descending sort are `src-tauri/src/balancer/mod.rs:132-136`.
- All-`NEG_INFINITY` fallback and best-index return are `src-tauri/src/balancer/mod.rs:138-141`.
- Existing error constants for operational failure blocking are at `src-tauri/src/balancer/mod.rs:7-8`.
- `ProviderEval`, `Selection`, `RiskClass`, and `BalanceError` should live in `balancer/mod.rs` near the public selection API they describe (`src-tauri/src/balancer/mod.rs:10-26`).
- Existing error enum style in the repo is simple typed enums with `as_str` or `db_value` mapping; examples are `diagnostics::ErrorCategory`, `InvocationStatus`, and `SessionCaptureMethod` (`src-tauri/src/diagnostics/mod.rs:14-35`, `src-tauri/src/state/db.rs:131-174`, `src-tauri/src/executor/mod.rs:22-39`).

### 3.7 Error surfacing (§4.8)

- `run_with_balancing` selection happens before invocation creation; add the hard-exhaustion branch between current parent resolution and provider indexing (`src-tauri/src/main.rs:618-634`).
- Existing one-shot diagnostics only run after subprocess execution failure, and `[diagnostics: {cat}]` is printed after stderr on failure (`src-tauri/src/main.rs:670-700`, `src-tauri/src/main.rs:717-741`).
- `run_repl` non-resume selection is currently direct tuple construction from `select_provider`, before provider indexing and invocation creation (`src-tauri/src/main.rs:460-527`, `src-tauri/src/main.rs:539-568`).
- Tauri `TestModelResult` currently has only `success`, `stdout`, `stderr`, and `exit_code` (`src-tauri/src/lib.rs:25-31`).
- Tauri `test_model` opens the app-adjacent DB, calls `select_provider`, executes, and returns the raw result without diagnostics (`src-tauri/src/lib.rs:471-504`).
- Tauri invoke registration includes `test_model`; any response type change is exposed through this command (`src-tauri/src/lib.rs:700-727`).
- `refresh_quotas` is not an error-surfacing hookpoint for scoring; it returns per-provider refresh status and raw windows (`src-tauri/src/lib.rs:290-390`).

### 3.8 Diagnostic category (§4.8)

- Existing diagnostic categories include `QuotaExhausted` with string `quota_exhausted` (`src-tauri/src/diagnostics/mod.rs:14-35`).
- LLM diagnostic prompt lists `quota_exhausted` as "Quota exceeded, billing limit, usage cap" (`src-tauri/src/diagnostics/mod.rs:47-65`).
- LLM output parser maps `quota_exhausted` into `ErrorCategory::QuotaExhausted` (`src-tauri/src/diagnostics/mod.rs:78-100`).
- Heuristic fallback maps stderr containing `"quota"`, `"billing"`, or `"usage limit"` to `QuotaExhausted` (`src-tauri/src/diagnostics/mod.rs:102-132`).
- `[diagnostics: quota_exhausted]` is currently printed only on the CLI one-shot post-execution failure path after `run_diagnostics` returns the category string (`src-tauri/src/main.rs:694-700`, `src-tauri/src/main.rs:717-735`).
- Pre-flight hard refusal can reuse the existing category slot; no new diagnostic category is required unless implementation wants to distinguish pre-flight refusal from subprocess-reported exhaustion (`src-tauri/src/diagnostics/mod.rs:14-35`, `src-tauri/src/main.rs:694-700`).

### 3.9 Test hooks

- Existing four scoring tests to rewrite/reseed are `density_scoring_picks_lowest_used_when_windows_match` (`src-tauri/src/balancer/mod.rs:319-335`), `density_picks_account_with_more_time_when_used_equal` (`src-tauri/src/balancer/mod.rs:337-350`), `binding_constraint_avoids_account_with_pressed_short_window` (`src-tauri/src/balancer/mod.rs:352-414`), and `falls_back_to_invocation_count_when_windows_missing` (`src-tauri/src/balancer/mod.rs:416-433`).
- Existing balancer test helpers are `record_invocation_for_test`, `two_provider_model`, `three_provider_model`, and `one_window` (`src-tauri/src/balancer/mod.rs:228-317`).
- `record_invocation_for_test` must gain any new `InvocationStart` fields, including `quota_tight_routing` if it is added to the start payload (`src-tauri/src/balancer/mod.rs:228-245`, `src-tauri/src/state/db.rs:123-129`).
- `one_window` only seeds `used_percent` and `resets_at`; per-window delta tests need either updated `upsert_quota_refresh` sequencing or a dedicated test helper to seed delta columns (`src-tauri/src/balancer/mod.rs:311-317`, `src-tauri/src/state/db.rs:1155-1242`).
- `StateDb::set_refreshed_at_for_test` can remain useful for constructing refresh-to-refresh learning scenarios (`src-tauri/src/state/db.rs:1245-1260`).
- CLI parser tests currently cover existing subcommands and no-risk-class behavior; risk-class flag tests belong in this main test module (`src-tauri/src/main.rs:808-1057`).
- Parent-invocation env tests already exercise unset, valid, malformed, unknown UUID, and invalid UUID cases; risk heuristic tests can reuse that env-lock pattern but must check env presence separately from resolution (`src-tauri/src/main.rs:776-802`, `src-tauri/src/main.rs:1060-1115`).
- Existing state DB schema/migration tests should be expanded for new columns and legacy rebuilds (`src-tauri/src/state/db.rs:1995-2028`, `src-tauri/src/state/db.rs:2121-2310`).

### 3.10 Non-hookpoints

- Do not redesign session ingestion; `sessions/mod.rs` already ingests adapter output and degrades scan failures into `ScanReport.errors` (`src-tauri/src/sessions/mod.rs:1-18`, `src-tauri/src/sessions/mod.rs:53-127`).
- Do not add risk-class logic to session capture; `SessionCaptureMethod` describes capture mechanics, not caller tolerance (`src-tauri/src/executor/mod.rs:16-39`, `src-tauri/src/executor/cli.rs:406-544`).
- Do not reuse setup auth detection as runtime risk plumbing; `OPENAI_API_KEY` is read only for setup detection (`src-tauri/src/setup/detection.rs:334-345`).
- Do not change frontend beyond the `test_model` response shape that the Tauri command exposes; quota refresh UI data is a separate command shape (`src-tauri/src/lib.rs:25-31`, `src-tauri/src/lib.rs:290-390`, `src-tauri/src/lib.rs:471-504`).
- Do not change `setup/` paths for quota balancing; setup paths are CLI/account discovery and first-run flow, not selection scoring (`src-tauri/src/setup/detection.rs:330-362`, `src-tauri/src/lib.rs:99-119`).
- Do not treat `src-tauri/src/config/providers.rs` as the model threshold home; it only parses provider-name keyed `quota_script` entries (`src-tauri/src/config/providers.rs:6-23`, `src-tauri/src/config/providers.rs:25-51`).

## 4. Cross-PR shared utilities

- `CompositeInvocationId` already formats stderr line output and parses env payloads; reuse it for invocation-id handling instead of adding a new env payload shape (`src-tauri/src/state/db.rs:176-199`).
- `stderr_line` is the existing helper that prints `OULIPOLY_INVOCATION=...` (`src-tauri/src/state/db.rs:183-190`).
- One-shot invocation parent propagation already flows from `run_with_balancing` through `execute_with_inputs_and_env` to `cmd.env("OULIPOLY_PARENT_INVOCATION", ...)` (`src-tauri/src/main.rs:624-646`, `src-tauri/src/executor/mod.rs:78-95`, `src-tauri/src/executor/cli.rs:241-268`).
- Interactive parent propagation already flows from `run_repl` through `execute_interactive` to the same `build_command` helper (`src-tauri/src/main.rs:535-563`, `src-tauri/src/executor/cli.rs:344-404`, `src-tauri/src/executor/cli.rs:241-268`).
- `FinalizerGuard` is the existing lifecycle safety pattern for started invocation rows in CLI code (`src-tauri/src/main.rs:346-379`).
- Existing simple error-category/string mapping conventions are `ErrorCategory::as_str`, `InvocationStatus::as_str`, and `SessionCaptureMethod::db_value` (`src-tauri/src/diagnostics/mod.rs:24-35`, `src-tauri/src/state/db.rs:139-174`, `src-tauri/src/executor/mod.rs:30-39`).
- Existing balancer helpers that survive are `score_by_invocation_count` and `round_robin_fallback`; PR 3 narrows when round-robin fallback is used but should not duplicate its invocation-count logic (`src-tauri/src/balancer/mod.rs:167-218`).
- Existing test helpers that survive with signature updates are `record_invocation_for_test`, `two_provider_model`, `three_provider_model`, and `one_window` (`src-tauri/src/balancer/mod.rs:228-317`).

## 5. Parallel-system risks

- `refresh_quotas` looks quota-related but is a Tauri refresh command, not a scoring hookpoint; use it only if the response shape needs to show PR 2/PR 3 data, not as a separate balancer path (`src-tauri/src/lib.rs:304-390`).
- `src-tauri/examples/quota_check.rs` is a diagnostic example that duplicates parts of the balancer readout. It must be updated to compile after signature/field changes, but scoring logic must stay in `balancer/mod.rs` (`src-tauri/examples/quota_check.rs:1-13`, `src-tauri/examples/quota_check.rs:71-149`).
- `providers.toml` config looks adjacent because it names `quota_script`, but model thresholds belong in model TOML, not provider quota-script config (`src-tauri/src/config/providers.rs:6-23`, `src-tauri/src/config/model.rs:203-210`).
- Setup detection reads `OPENAI_API_KEY` and account files; do not reuse it for quota state or runtime routing (`src-tauri/src/setup/detection.rs:334-345`).
- Session ingestion is adjacent because scoring reads assistant turns, but the proposal does not change adapter JSONL contracts or ingestion semantics (`src-tauri/src/sessions/mod.rs:8-18`, `src-tauri/src/sessions/mod.rs:53-127`, `src-tauri/src/state/db.rs:1691-1741`).
- Existing provider-level delta columns are legacy code paths PR 3 removes: `QuotaRecord.last_delta_*`, `provider_quotas.last_delta_*`, `get_quota`, `get_quotas`, `upsert_quota_refresh` provider-level delta write, and `global_avg_percent_per_call` (`src-tauri/src/state/db.rs:29-40`, `src-tauri/src/state/db.rs:352-360`, `src-tauri/src/state/db.rs:1077-1107`, `src-tauri/src/state/db.rs:1148-1242`, `src-tauri/src/state/db.rs:1277-1317`, `src-tauri/src/balancer/mod.rs:144-165`).
- `global_avg_percent_per_call` has only one current caller, `score_by_density`, so deleting it as part of the scoring rewrite does not leave external callers (`src-tauri/src/balancer/mod.rs:94`, `src-tauri/src/balancer/mod.rs:144-165`).
- `get_quotas` is currently uncalled in `src-tauri/src` but still selects provider-level deltas; if retained, it must not become a stale compatibility surface for deleted columns (`src-tauri/src/state/db.rs:1277-1317`; current source search found `get_quota` callers but no `get_quotas` caller outside its definition in `src-tauri/src`).
- Top-level README and `scripts/README.md` still describe/reference legacy aspects of quota scripts; update docs as part of the same PR that adds a tracked `scripts/chatgpt-usage`, but do not use docs as a runtime hookpoint (`README.md:230-258`, `scripts/README.md:191-209`).

## 6. Discrepancies

- The prompt's explicit `select_provider` caller list names `run_with_balancing`, `run_repl`, and Tauri `test_model`, but the current repo also has `src-tauri/examples/quota_check.rs` calling `select_provider` (`src-tauri/src/main.rs:525`, `src-tauri/src/main.rs:622`, `src-tauri/src/lib.rs:492`, `src-tauri/examples/quota_check.rs:117-119`).

## 7. Human-gate decisions (locked 2026-04-21)

- **A. `src-tauri/examples/quota_check.rs`** — **Update**, not delete. PR 3 rewrites its `select_provider` call site to match the new `Result<Selection, BalanceError>` signature, drops provider-level delta reads, and adds a `RiskClass::Background` argument (diagnostic tooling is non-interactive). The example continues to exist.
- **B. `get_quotas` (`state/db.rs:1277-1317`)** — **Delete**. No current `src-tauri/src` callers; retaining it post-column-drop would create a stale compatibility surface, which the no-compat-shims policy forbids. Deletion lands in PR 3 alongside the other provider-level delta removals.
- **C. Docs update in PR 1** — **Yes, fold into PR 1**. `scripts/chatgpt-usage` is added alongside matching updates to `scripts/README.md:207-209` and `README.md:247-258` so the tracked reference-adapter list is in sync as of the commit. Resolves prior scope-risk finding G1.
- The proposal cites `/home/nes/.local/bin/anthropic-usage` as installed precedent in places, but the tracked repo precedent is `scripts/anthropic-usage`; the tracked file has the same relevant multi-window emit structure and is the PR 1 source precedent (`scripts/anthropic-usage:1-54`).
- Current `scripts/README.md` says quota-script reference adapters are `anthropic-usage` and `zai-usage`, while top-level README has the same reference list; neither mentions `chatgpt-usage` because it is not tracked yet (`scripts/README.md:207-209`, `README.md:247-258`).
- Current `providers.toml` local comment still documents the legacy flat quota output shape, while the repo README/scripts README document multi-window scripts with legacy fallback; this is external local config, not a repo source hookpoint (`/home/nes/.config/oulipoly-agent-runner/providers.toml:1-9`, `README.md:230-245`, `scripts/README.md:191-205`).
