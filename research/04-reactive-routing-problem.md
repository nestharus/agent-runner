# Initiative 04 — Reactive Routing Problem Research

## 1. Surface to delete — exhaustive inventory

### 1.1 `RiskClass`

1. `RiskClass` is defined in the balancer module with `User` and `Background` variants. (`src-tauri/src/balancer/mod.rs:12-17`)
2. `select_provider` takes `risk_class: RiskClass` and returns `Result<Selection, BalanceError>`. (`src-tauri/src/balancer/mod.rs:79-84`)
3. `score_by_density` takes `risk_class: RiskClass`; the value is used in the user/background branch and passed to exhausted error construction. (`src-tauri/src/balancer/mod.rs:136-142`, `src-tauri/src/balancer/mod.rs:225-246`)
4. `ExhaustedError` stores `risk_class: RiskClass`; `exhausted_error` copies the selected class into the error payload. (`src-tauri/src/balancer/mod.rs:42-47`, `src-tauri/src/balancer/mod.rs:272-280`)
5. Balancer tests call `select_provider` with `RiskClass::User` / `RiskClass::Background` through helpers and direct calls. (`src-tauri/src/balancer/mod.rs:531-582`, `src-tauri/src/balancer/mod.rs:637-645`, `src-tauri/src/balancer/mod.rs:654-890`)
6. CLI `main.rs` imports `RiskClass`, defines `RiskClassArg`, maps it into `RiskClass`, and returns `RiskClass` from `resolve_risk_class`. (`src-tauri/src/main.rs:1-10`, `src-tauri/src/main.rs:62-82`, `src-tauri/src/main.rs:212-244`)
7. `run` resolves one `RiskClass` and passes it into direct-model and agent-model `run_with_balancing`; the `repl` subcommand passes `cli.risk_class.map(Into::into)` into `run_repl`. (`src-tauri/src/main.rs:282-295`, `src-tauri/src/main.rs:313-320`, `src-tauri/src/main.rs:340-347`)
8. `run_repl` takes `risk_class_override: Option<RiskClass>` and defaults to `RiskClass::User` when selecting a provider. (`src-tauri/src/main.rs:488-494`, `src-tauri/src/main.rs:586-591`)
9. `run_with_balancing` takes `risk_class: RiskClass` and passes it to `select_provider`. (`src-tauri/src/main.rs:670-677`, `src-tauri/src/main.rs:704-710`)
10. Main tests import `RiskClass` and assert cascade outcomes. (`src-tauri/src/main.rs:862-866`, `src-tauri/src/main.rs:1198-1325`)
11. Tauri `TestModelError` serializes `risk_class: balancer::RiskClass`; `test_model_with_db_path` hardcodes `balancer::RiskClass::User`; the structured-error test asserts `RiskClass::User`. (`src-tauri/src/lib.rs:35-42`, `src-tauri/src/lib.rs:519-536`, `src-tauri/src/lib.rs:809-813`, `src-tauri/src/lib.rs:902-942`)
12. The `quota_check` example imports `RiskClass` and calls `select_provider(..., RiskClass::Background)`. (`src-tauri/examples/quota_check.rs:10-12`, `src-tauri/examples/quota_check.rs:103-128`)
13. README CLI and quota docs describe `--risk-class`, `OULIPOLY_RISK_CLASS`, risk classes, and threshold behavior. (`README.md:117-130`, `README.md:217-234`)

### 1.2 `Selection`

1. `Selection` is defined with `provider_index` and `quota_tight_routing`. (`src-tauri/src/balancer/mod.rs:19-23`)
2. `select_provider` returns `Result<Selection, BalanceError>` and constructs `Selection` in single-provider, missing-window fallback, fully-unlearned fallback, user soft-degrade, and normal-return paths. (`src-tauri/src/balancer/mod.rs:79-90`, `src-tauri/src/balancer/mod.rs:123-133`, `src-tauri/src/balancer/mod.rs:218-223`, `src-tauri/src/balancer/mod.rs:236-251`)
3. Balancer tests unwrap `.provider_index`, return `Selection` from a helper, and assert `quota_tight_routing`. (`src-tauri/src/balancer/mod.rs:531-582`, `src-tauri/src/balancer/mod.rs:637-645`, `src-tauri/src/balancer/mod.rs:747-758`, `src-tauri/src/balancer/mod.rs:881-890`)
4. `run_repl` reads `selection.provider_index` and `selection.quota_tight_routing`. (`src-tauri/src/main.rs:586-605`)
5. `run_with_balancing` reads `selection.provider_index` and `selection.quota_tight_routing`. (`src-tauri/src/main.rs:704-727`)
6. Tauri `test_model_with_db_path` reads `selection.provider_index`. (`src-tauri/src/lib.rs:519-545`)
7. The `quota_check` example reads `selection.provider_index` and `selection.quota_tight_routing`. (`src-tauri/examples/quota_check.rs:116-128`)

### 1.3 `BalanceError` / `ExhaustedError` / `ExhaustedProviderInfo`

1. `BalanceError` is defined with the single `Exhausted(ExhaustedError)` variant, a `Display` message, and an `Error` impl. (`src-tauri/src/balancer/mod.rs:25-40`)
2. `ExhaustedError` and `ExhaustedProviderInfo` carry model name, risk class, provider names, projected usage, and thresholds. (`src-tauri/src/balancer/mod.rs:42-55`)
3. `score_by_density` returns `Err(BalanceError::Exhausted(...))` when no hard-eligible providers remain outside the fully-unlearned fallback case. (`src-tauri/src/balancer/mod.rs:213-228`)
4. `exhausted_error` builds the provider list and copies both thresholds from `model.balancer`. (`src-tauri/src/balancer/mod.rs:272-289`)
5. `run_repl` catches `Err(err @ BalanceError::Exhausted(_))`, emits the balance error, and returns exit code `1`. (`src-tauri/src/main.rs:586-596`)
6. `run_with_balancing` catches the same error shape before starting an invocation. (`src-tauri/src/main.rs:704-709`)
7. `emit_balance_error` prints the error string and `[diagnostics: quota_exhausted]`. (`src-tauri/src/main.rs:810-816`)
8. Tauri `test_model_with_db_path` catches `BalanceError::Exhausted`, returns `TestModelResult { success: false, exit_code: 1, error: Some(...) }`, and maps `ExhaustedError` into `TestModelError`. (`src-tauri/src/lib.rs:519-568`)
9. Balancer test `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail` pattern-matches `Err(BalanceError::Exhausted(err))`. (`src-tauri/src/balancer/mod.rs:775-797`)
10. Tauri test `test_model_returns_structured_quota_exhausted_error` depends on the exhausted error being surfaced through `TestModelResult.error`. (`src-tauri/src/lib.rs:902-942`)

### 1.4 `BalancerConfig`, `user_threshold`, `failure_threshold`, and `[balancer]`

1. `ModelConfig` owns `balancer: BalancerConfig`. (`src-tauri/src/config/model.rs:203-211`)
2. `BalancerConfig` defines `user_threshold` and `failure_threshold`, defaults to `0.70` and `0.95`, and validates finiteness, unit interval, and ordering. (`src-tauri/src/config/model.rs:214-251`)
3. Raw TOML parsing includes `balancer: Option<RawBalancerBlock>` with optional threshold fields. (`src-tauri/src/config/model.rs:316-333`)
4. `ModelConfig::from_toml` returns `balancer` in the constructed model. (`src-tauri/src/config/model.rs:638-644`)
5. `parse_balancer` applies default thresholds or raw overrides and validates them. (`src-tauri/src/config/model.rs:648-659`)
6. `append_balancer_toml` writes `[balancer]`, `user_threshold`, and `failure_threshold` when config differs from defaults. (`src-tauri/src/config/model.rs:661-673`)
7. Tauri `save_model` calls `model.balancer.validate()` before writing TOML. (`src-tauri/src/lib.rs:266-286`)
8. `score_by_density` reads `model.balancer.failure_threshold` and `model.balancer.user_threshold` during projection gating. (`src-tauri/src/balancer/mod.rs:184-191`)
9. `exhausted_error` copies both thresholds into provider info. (`src-tauri/src/balancer/mod.rs:282-287`)
10. Config tests pin default parsing, override parsing, invalid threshold validation, ordering validation, and TOML round-trip. (`src-tauri/src/config/model.rs:1180-1278`)
11. README documents the optional `[balancer]` TOML block and the default threshold values. (`README.md:221-232`)

### 1.5 `--risk-class`, `OULIPOLY_RISK_CLASS`, `resolve_risk_class`, and cascade branches

1. The CLI struct has global `#[arg(long = "risk-class", value_enum, global = true)] risk_class: Option<RiskClassArg>`. (`src-tauri/src/main.rs:62-67`)
2. `RiskClassArg` has `User` and `Background` variants and converts into `RiskClass`. (`src-tauri/src/main.rs:69-82`)
3. `resolve_risk_class` implements this cascade: explicit flag, `repl` subcommand, `OULIPOLY_RISK_CLASS`, `-f` / `OULIPOLY_PARENT_INVOCATION` / non-TTY heuristic, then `User`. (`src-tauri/src/main.rs:212-244`)
4. `run` passes `cli.risk_class.map(Into::into)` to `run_repl`; no `cli.risk_class.is_some()` occurrence is present in current main plumbing. (`src-tauri/src/main.rs:282-288`)
5. `run` calls `resolve_risk_class` for non-subcommand direct/agent execution and passes the resulting class to `run_with_balancing`. (`src-tauri/src/main.rs:292-320`, `src-tauri/src/main.rs:323-347`)
6. Main tests pin flag precedence, env precedence, file heuristic, TTY heuristic, parent-invocation heuristic, piped-stdin heuristic, repl defaulting, and global flag parsing. (`src-tauri/src/main.rs:906-953`, `src-tauri/src/main.rs:1198-1325`)
7. README documents the flag, env var, repl behavior, and heuristic cascade. (`README.md:117-130`, `README.md:226-234`)

If `resolve_risk_class` returns no routing class in the current code shape, direct-model and agent-model `run_with_balancing` call sites lose the `risk_class` argument they currently pass. (`src-tauri/src/main.rs:295-320`, `src-tauri/src/main.rs:340-347`, `src-tauri/src/main.rs:670-677`)

### 1.6 `quota_tight_routing`

1. `Selection` contains `quota_tight_routing`. (`src-tauri/src/balancer/mod.rs:19-23`)
2. `select_provider` sets `quota_tight_routing: false` for single-provider, missing-window fallback, fully-unlearned fallback, and normal return; it sets `true` only in the user soft-degrade branch. (`src-tauri/src/balancer/mod.rs:85-90`, `src-tauri/src/balancer/mod.rs:129-133`, `src-tauri/src/balancer/mod.rs:218-223`, `src-tauri/src/balancer/mod.rs:236-251`)
3. `InvocationRecord` and `InvocationStart` both include `quota_tight_routing: bool`. (`src-tauri/src/state/db.rs:138-166`)
4. Existing DB migration adds `invocations.quota_tight_routing` to current-schema databases when the column is missing. (`src-tauri/src/state/db.rs:522-555`)
5. New `invocations` schema includes `quota_tight_routing BOOLEAN NOT NULL DEFAULT 0`. (`src-tauri/src/state/db.rs:691-719`)
6. Legacy invocation migration creates and populates `quota_tight_routing` as `0` in `invocations_new`. (`src-tauri/src/state/db.rs:786-825`)
7. `start_invocation` inserts `quota_tight_routing` from `InvocationStart`. (`src-tauri/src/state/db.rs:898-929`)
8. Invocation lookup queries select `quota_tight_routing`; row mapping reads it from column 12. (`src-tauri/src/state/db.rs:1051-1097`, `src-tauri/src/state/db.rs:1131-1147`)
9. `run_repl` carries `quota_tight_routing` from selection into `InvocationStart`; resumed sessions set it to `false`. (`src-tauri/src/main.rs:522-605`, `src-tauri/src/main.rs:619-626`)
10. `run_with_balancing` writes `selection.quota_tight_routing` into `InvocationStart`. (`src-tauri/src/main.rs:704-727`)
11. Stderr warning text is emitted in `run_repl` and `run_with_balancing`: `[warn: no provider below user_threshold; routing via quota-tight path]`. (`src-tauri/src/main.rs:598-600`, `src-tauri/src/main.rs:711-713`)
12. Balancer test helper sets `quota_tight_routing: false` in `InvocationStart`. (`src-tauri/src/balancer/mod.rs:483-501`)
13. Main tests include `InvocationStart` literals with the field in parent-resolution and finalizer-guard tests. (`src-tauri/src/main.rs:1348-1364`, `src-tauri/src/main.rs:1441-1525`)
14. State test helper and invocation lifecycle tests include the field; `quota_tight_routing_column_persisted_to_invocations` directly asserts persistence. (`src-tauri/src/state/db.rs:2157-2172`, `src-tauri/src/state/db.rs:2929-2951`, `src-tauri/src/state/db.rs:2953-3290`, `src-tauri/src/state/db.rs:3533-3554`)
15. The `quota_check` example prints `quota_tight`. (`src-tauri/examples/quota_check.rs:123-128`)
16. README describes the persisted flag and warning. (`README.md:221-226`)

### 1.7 `TestModelResult.error`, `TestModelError`, `TestModelProviderInfo`

1. Rust `TestModelResult` has `error: Option<TestModelError>` with serde default/skip behavior. (`src-tauri/src/lib.rs:25-33`)
2. `TestModelError` and `TestModelProviderInfo` are Rust structs for structured quota-exhausted pre-flight errors. (`src-tauri/src/lib.rs:35-50`)
3. `test_model_with_db_path` populates `error: Some(...)` only for `BalanceError::Exhausted`; normal execution returns `error: None`. (`src-tauri/src/lib.rs:519-545`)
4. `test_model_error_from_exhausted` maps `ExhaustedError` fields into the structured shape. (`src-tauri/src/lib.rs:548-568`)
5. Frontend `TestModelResult` currently exposes only `success`, `stdout`, `stderr`, and `exit_code`; it does not mirror the Rust `error` field. (`src/lib/types.ts:109-114`)
6. `testModel` uses the frontend `TestModelResult` type for the Tauri command. (`src/lib/tauri.ts:1-11`, `src/lib/tauri.ts:73-75`)
7. Rust test `test_model_returns_structured_quota_exhausted_error` asserts the structured error. (`src-tauri/src/lib.rs:902-942`)

### 1.8 `ProviderEval.hard_blocked` / `user_blocked`, eligibility filters, and soft-degrade branch

1. `ProviderEval` includes `hard_blocked` and `user_blocked`. (`src-tauri/src/balancer/mod.rs:57-65`)
2. Recent-error avoidance currently creates a `ProviderEval` with both blocked flags set. (`src-tauri/src/balancer/mod.rs:146-160`)
3. Projection computes `projected = used + turns × burn_rate`, then sets `hard_blocked` from `failure_threshold` and `user_blocked` from `user_threshold`. (`src-tauri/src/balancer/mod.rs:162-191`)
4. `binding_score` is set to `None` when `hard_blocked`, `unlearned`, or unscored. (`src-tauri/src/balancer/mod.rs:199-210`)
5. `hard_eligible` filters out hard-blocked and unlearned providers. (`src-tauri/src/balancer/mod.rs:213-216`)
6. Empty `hard_eligible` returns round-robin only for fully-unlearned, not-hard-blocked pools; otherwise it returns `BalanceError::Exhausted`. (`src-tauri/src/balancer/mod.rs:218-228`)
7. The user branch builds `user_eligible` by filtering out `user_blocked`; if empty, it returns the best hard-eligible provider with `quota_tight_routing: true`. (`src-tauri/src/balancer/mod.rs:230-243`)
8. Main emits the quota-tight warning after both `run_repl` and `run_with_balancing` selection. (`src-tauri/src/main.rs:598-600`, `src-tauri/src/main.rs:711-713`)

### 1.9 Tests pinning delete behavior

1. Balancer threshold/error behavior tests: `user_threshold_hides_provider_from_user_class_only`, `user_threshold_soft_degrades_with_quota_tight_flag_when_all_fail`, `failure_threshold_hard_blocks_all_classes`, `failure_threshold_returns_exhausted_not_roundrobin_when_all_fail`. (`src-tauri/src/balancer/mod.rs:732-797`)
2. Balancer API tests with current `RiskClass`/`Selection` signature: `single_provider_always_zero`, `round_robin_on_fresh_state`, `avoids_errored_providers`, `density_scoring_picks_lowest_used_when_windows_match`, `density_picks_account_with_more_time_when_used_equal`, `binding_constraint_avoids_account_with_pressed_short_window`, `falls_back_to_invocation_count_when_windows_missing`, `high_weekly_account_stops_winning_after_cumulative_turns`, `bootstrap_uses_sibling_pool_when_own_delta_absent`, `fresh_pool_falls_through_to_invocation_count_round_robin`. (`src-tauri/src/balancer/mod.rs:531-582`, `src-tauri/src/balancer/mod.rs:654-730`, `src-tauri/src/balancer/mod.rs:812-825`, `src-tauri/src/balancer/mod.rs:881-890`)
3. Main risk cascade tests: `risk_class_cli_flag_overrides_env_var`, `risk_class_env_var_overrides_heuristic`, `risk_class_heuristic_classifies_file_flag_as_background`, `risk_class_heuristic_classifies_tty_prompt_as_user`, `risk_class_heuristic_classifies_parent_invocation_as_background`, `risk_class_heuristic_classifies_piped_stdin_as_background`, `repl_subcommand_always_user_class`, `risk_class_flag_reaches_repl_subcommand`. (`src-tauri/src/main.rs:1198-1325`)
4. Config balancer tests: `parse_balancer_defaults_when_block_absent`, `parse_balancer_overrides_thresholds`, `rejects_balancer_threshold_outside_unit_interval`, `rejects_balancer_user_threshold_above_failure_threshold`, `roundtrip_model_with_balancer_config`. (`src-tauri/src/config/model.rs:1180-1278`)
5. State quota-tight persistence test: `quota_tight_routing_column_persisted_to_invocations`. (`src-tauri/src/state/db.rs:2929-2951`)
6. Tauri structured pre-flight error test: `test_model_returns_structured_quota_exhausted_error`. (`src-tauri/src/lib.rs:902-942`)

Count: the delete inventory above contains 9 requested categories and 70 concrete line-range entries across `README.md`, `src-tauri/examples/quota_check.rs`, `src-tauri/src/balancer/mod.rs`, `src-tauri/src/config/model.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/main.rs`, `src-tauri/src/state/db.rs`, `src/lib/tauri.ts`, and `src/lib/types.ts`. Direct deletion or signature reshaping is approximately 430-520 LOC, plus mechanical one-line struct/call-site edits for `InvocationStart` literals and `select_provider` callers. The line estimate comes from the ranges enumerated in sections 1.1-1.9.

## 2. Surface to keep — confirm

1. `score_by_density` currently computes projection as `project_used_percent(window.used_percent, turns, burn_rate)`, tracks max projected usage, and computes the ranking score as `(1.0 - projected).max(0.0) * hours`; `best_binding_score` picks the max binding score. (`src-tauri/src/balancer/mod.rs:162-196`, `src-tauri/src/balancer/mod.rs:248-269`)
2. The bootstrap cascade is implemented as learned per-window rate, then sibling pool average by `window_id`, then duration-ratio fallback. (`src-tauri/src/balancer/mod.rs:296-316`, `src-tauri/src/balancer/mod.rs:318-377`)
3. Per-window delta learning lives in `upsert_quota_refresh`: it compares each new window to the same `window_id`, counts assistant turns since the prior refresh, and writes `last_delta_percent` / `last_delta_calls` on `provider_quota_windows`. (`src-tauri/src/state/db.rs:1282-1297`, `src-tauri/src/state/db.rs:1350-1437`)
4. The three delta-learning guards are current constants and branches: `MIN_LEARN_SAMPLE_CALLS`, `NEAR_EXHAUSTED_USED_PERCENT`, and `MAX_LEARNABLE_BURN_RATE`. (`src-tauri/src/state/db.rs:8-45`, `src-tauri/src/state/db.rs:1391-1418`)
5. Fully-unlearned pools still reach `round_robin_fallback`: `score_by_density` returns it when every eval is unlearned and none is hard-blocked, and the test `fresh_pool_falls_through_to_invocation_count_round_robin` pins that path. (`src-tauri/src/balancer/mod.rs:213-224`, `src-tauri/src/balancer/mod.rs:455-474`, `src-tauri/src/balancer/mod.rs:881-890`)
6. Recent-error avoidance remains in both density scoring and invocation-count fallback: 3 or more errors in a 30-minute window block or penalize a provider. (`src-tauri/src/balancer/mod.rs:8-9`, `src-tauri/src/balancer/mod.rs:146-160`, `src-tauri/src/balancer/mod.rs:422-450`)
7. `recent_error_count` counts failed invocations by `(model_name, provider_index)` newer than a cutoff, and the state test `recent_errors` pins the count. (`src-tauri/src/state/db.rs:1188-1208`, `src-tauri/src/state/db.rs:3261-3289`)

## 3. Current exhausted-classification plumbing

1. `ErrorCategory::QuotaExhausted` exists and serializes as `quota_exhausted`. (`src-tauri/src/diagnostics/mod.rs:14-35`)
2. `diagnose_error` asks the diagnostics model to classify stderr and includes `quota_exhausted: Quota exceeded, billing limit, usage cap` in the category list. (`src-tauri/src/diagnostics/mod.rs:37-67`)
3. `parse_diagnosis` maps a first-line `quota_exhausted` response into `ErrorCategory::QuotaExhausted`. (`src-tauri/src/diagnostics/mod.rs:78-100`)
4. The heuristic fallback maps stderr containing `"quota"`, `"billing"`, or `"usage limit"` into `ErrorCategory::QuotaExhausted`. (`src-tauri/src/diagnostics/mod.rs:102-132`)
5. `run_with_balancing` runs diagnostics only after a subprocess returns a non-zero exit code; it passes `error_category.as_deref()` to `finalize_invocation`. (`src-tauri/src/main.rs:763-779`)
6. `finalize_invocation` writes `error_category` into `invocations.error_category` in the terminal update. (`src-tauri/src/state/db.rs:931-986`)
7. `recent_error_count` aggregates recent failed invocations by `model_name` and `provider_index`; it does not filter by `error_category`. (`src-tauri/src/state/db.rs:1188-1208`)
8. The `providers` table has `error_count`, `last_error`, and `last_error_at`, keyed by `(model_name, provider_index)`. (`src-tauri/src/state/db.rs:51-61`, `src-tauri/src/state/db.rs:377-387`)
9. `finalize_invocation` increments `providers.error_count` for any failed invocation and stores a stderr snippet as `last_error`; it does not check whether the failure was quota-related. (`src-tauri/src/state/db.rs:988-1017`)
10. `provider_quotas` is keyed by `provider_name` and stores quota refresh metadata; `provider_quota_windows` is keyed by `(provider_name, window_id)` and stores per-window quota readings and learned deltas. (`src-tauri/src/state/db.rs:389-406`)

## 4. Current refresh plumbing

1. `select_provider` performs lazy refresh only when it receives a `BalanceContext`; for each provider in a multi-provider model, it calls `is_stale`, then `refresh_provider`, then `scan_provider`. (`src-tauri/src/balancer/mod.rs:67-77`, `src-tauri/src/balancer/mod.rs:93-108`)
2. `run_repl` builds a `BalanceContext` from `providers.toml`, `sessions.toml`, and a new `InFlight` tracker before calling `select_provider`. (`src-tauri/src/main.rs:504-518`, `src-tauri/src/main.rs:586-591`)
3. `run_with_balancing` builds the same context before calling `select_provider`. (`src-tauri/src/main.rs:683-704`)
4. Tauri `refresh_quotas` collects provider names from multi-provider models, skips providers where `is_stale` is false, and calls `refresh_provider` for stale providers. (`src-tauri/src/lib.rs:324-375`)
5. The `quota_check` example calls `is_stale` for display, then calls `refresh_provider` for each distinct provider name. (`src-tauri/examples/quota_check.rs:37-68`)
6. `refresh_provider` returns `NoScript` when no quota script exists, `AlreadyInFlight` when another refresh owns the provider slot, `Failed` when the script or DB write fails, and `Updated` after `state.upsert_quota_refresh` succeeds. (`src-tauri/src/quota/mod.rs:86-127`)
7. `is_stale` returns true for a missing quota row, missing `refreshed_at`, or an existing quota row with zero windows; otherwise it compares cache age to `dynamic_ttl_secs`. (`src-tauri/src/quota/mod.rs:129-147`)
8. `dynamic_ttl_secs` returns `MAX_TTL_SECS` for empty windows; otherwise it takes the minimum seconds until reset across windows, divides by `REFRESH_WINDOW_DIVISOR`, and clamps to `[MIN_TTL_SECS, MAX_TTL_SECS]`. (`src-tauri/src/quota/mod.rs:13-20`, `src-tauri/src/quota/mod.rs:149-163`)
9. TTL tests pin stale behavior for empty/missing quota rows and dynamic TTL behavior for short, nearly expired, and empty windows. (`src-tauri/src/quota/mod.rs:327-397`)

## 5. Risk class CLI plumbing — trace for deletion

1. Current root CLI parsing stores `risk_class: Option<RiskClassArg>`; `RiskClassArg` maps into balancer `RiskClass`. (`src-tauri/src/main.rs:62-82`)
2. The current code has no `cli.risk_class.is_some()` branch; the observed direct uses are `cli.risk_class` in `resolve_risk_class` and `cli.risk_class.map(Into::into)` for `run_repl`. (`src-tauri/src/main.rs:212-220`, `src-tauri/src/main.rs:282-288`)
3. `OULIPOLY_RISK_CLASS` is read in `resolve_risk_class`, and the main test helper mutates/restores it around cascade tests. (`src-tauri/src/main.rs:226-233`, `src-tauri/src/main.rs:906-953`)
4. `run_repl` does not call `resolve_risk_class`; it receives `risk_class_override` from the parsed CLI flag and otherwise uses `RiskClass::User`. (`src-tauri/src/main.rs:282-288`, `src-tauri/src/main.rs:488-494`, `src-tauri/src/main.rs:586-591`)
5. `run_with_balancing` receives a concrete `RiskClass`; direct-model and agent-model execution obtain that value from `resolve_risk_class`. (`src-tauri/src/main.rs:292-320`, `src-tauri/src/main.rs:340-347`, `src-tauri/src/main.rs:670-677`, `src-tauri/src/main.rs:704-710`)
6. Tauri `test_model` does not read CLI risk state; it hardcodes `balancer::RiskClass::User`. (`src-tauri/src/lib.rs:519-536`)
7. With the current signatures unchanged, removing the return value from `resolve_risk_class` leaves non-subcommand execution without the `risk_class` argument currently required by `run_with_balancing` and `select_provider`. (`src-tauri/src/main.rs:295-320`, `src-tauri/src/main.rs:340-347`, `src-tauri/src/main.rs:670-677`, `src-tauri/src/balancer/mod.rs:79-84`)

## 6. Open questions the orchestrator must answer

1. Where does the new exhausted flag live? Current candidate tables have different keys: `providers` is keyed by `(model_name, provider_index)`, `provider_quotas` is keyed by `provider_name`, and `provider_quota_windows` is keyed by `(provider_name, window_id)`. (`src-tauri/src/state/db.rs:377-406`)
2. What refresh result clears the flag? Current refresh success writes quota metadata through `upsert_quota_refresh`, while empty-window refreshes preserve prior windows and record only `last_empty_refresh_at`. (`src-tauri/src/quota/mod.rs:118-124`, `src-tauri/src/state/db.rs:1282-1348`, `src-tauri/src/state/db.rs:1370-1441`)
3. Does exhausted status follow a provider entry in one model pool or a provider account globally? Current invocation/provider aggregate data keys by `(model_name, provider_index)`, while quota data keys by `provider_name`. (`src-tauri/src/state/db.rs:377-406`, `src-tauri/src/state/db.rs:988-1003`, `src-tauri/src/state/db.rs:1212-1280`)
4. What happens when every provider in a pool has the exhausted flag? Current all-hard-blocked learned pools return `BalanceError::Exhausted`, current fully-unlearned pools return `round_robin_fallback`, and current missing-window pools use invocation-count fallback. (`src-tauri/src/balancer/mod.rs:123-133`, `src-tauri/src/balancer/mod.rs:213-228`, `src-tauri/src/balancer/mod.rs:422-474`)
5. Does reactive classification apply to interactive `repl` exits? Current `run_repl` finalizes non-spawn subprocess exits with `error_category = None`; non-interactive `run_with_balancing` runs diagnostics on failed subprocess results. (`src-tauri/src/main.rs:639-663`, `src-tauri/src/main.rs:763-779`)

## 7. Non-goals

1. Initiative 04 is not deleting projection math, bootstrap burn-rate learning, per-window delta learning, `round_robin_fallback`, or recent-error avoidance; those mechanisms are current keep surfaces listed in section 2. (`src-tauri/src/balancer/mod.rs:162-196`, `src-tauri/src/balancer/mod.rs:305-377`, `src-tauri/src/state/db.rs:1282-1437`, `src-tauri/src/balancer/mod.rs:422-474`)
2. Initiative 04 is not changing quota script execution or JSON parsing; refresh currently runs `quota_script` through `refresh_provider` and parses `windows` or legacy single-window output before writing state. (`src-tauri/src/quota/mod.rs:100-127`, `src-tauri/src/quota/mod.rs:165-270`)
3. Initiative 04 is not changing session scanning as a quota signal; `select_provider` currently calls `scan_provider`, and scoring counts assistant turns since `refreshed_at`. (`src-tauri/src/balancer/mod.rs:93-108`, `src-tauri/src/balancer/mod.rs:162-169`)
4. Initiative 04 is not changing general invocation lifecycle finalization; `start_invocation` inserts a running row and `finalize_invocation` writes terminal status, success, exit code, error category, and provider aggregates. (`src-tauri/src/state/db.rs:898-929`, `src-tauri/src/state/db.rs:931-1021`)
5. Initiative 04 is not changing frontend model-management surfaces except where the Tauri `test_model` response shape removes the current Rust-only structured `error` field; the frontend type currently omits that field. (`src-tauri/src/lib.rs:25-50`, `src/lib/types.ts:109-114`)
