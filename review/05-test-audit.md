# Test-Audit Gate: Initiative 05 — session migration

## Overall verdict: PARTIAL

The Initiative 05 test set is behaviorally broad and the full Rust suite
passes on the merged `main` snapshot under review (`91403a0`, diff
`58aa68d..HEAD`). Every proposal §11.1 theme has at least one passing test,
the named Rev 4 watchpoints exist, and the Codex-deferred path is pinned at
both the policy and mechanic layers. The kept Initiative 04 balancer suite
also passes after the `compute_projections` extraction, which is the main
guard for the claimed provider-selection equivalence.

The gate is **PARTIAL**, not PASS, because three audit rules are not fully
satisfied:

- **Fixture isolation**: several added tests still assemble scripts, TOML, SQL,
  or temp files inline in the test body instead of routing all fixture content
  through helpers/builders/fixture files as §11.1 and Phase 6 Step 6b require.
- **Level honesty**: four labels are inflated or too narrow: two parser-only
  tests are labeled `end-to-end`, one file-loading test is labeled `unit`, and
  one DB-backed projection test is labeled `unit`.
- **Negative-path coverage**: the suite has strong negative coverage for the
  public resolver and migration mechanic, but it does not provide at least one
  explicit error-path test for every newly added `Result`-returning helper.

These are test-quality gaps rather than evidence that the implementation is
wrong. They block a clean PASS under `~/ai/agents/test-audit-gate.md`.

## Verification run

Commands run from `src-tauri/`:

- `cargo test balancer::tests -- --nocapture`
  - **PASS**: 29 passed, including the kept Initiative 04 routing suite and
    the new `compute_projections_exposes_window_projection_used_by_selection`.
- `cargo test --test initiative_05_migration -- --nocapture`
  - **PASS**: 24 passed.
- `cargo test`
  - **PASS**: 295 lib tests, 33 main tests, 24 Initiative 05 integration tests,
    and all existing integration suites passed.

## Sub-audit 1 — §11.1 coverage

Verdict: PASS

Every §11.1 group has at least one passing landed test.

| §11.1 group | Passing evidence | Assessment |
| --- | --- | --- |
| Schema migration and backfill | `backfill_creates_one_chain_per_provider_session_pair`, `backfill_idempotent_on_second_open`, `migrate_db_command_runs_backfill_to_completion`, `migrate_db_command_idempotent_on_second_run`, `startup_refuses_chain_ops_on_backfill_failure` | Covered. Startup and explicit command paths both exercised. |
| Chain identity write paths | `mint_chain_on_first_session_capture`, `agent_session_chain_records_model_at_mint`, `ui_session_chain_minted_at_ingestion_uses_provider_default`, `ui_session_chain_minted_with_unknown_when_no_provider_default`, `chain_mint_works_for_codex_ingestion`, `mint_chain_no_op_on_resume_of_existing_chain`, `chain_last_used_at_updates_after_successful_invocation` | Covered. Agent, UI, Codex identity, idempotence, and last-used hook all have passing signals. |
| Resolver disambiguation and model inference | `resolve_resume_*`, `agent_resume_no_dash_m_uses_session_recorded_model`, `resolve_resume_falls_back_to_provider_default_model_for_ui_session` | Covered. Agent no-`-m` is end-to-end; UI default-model counterpart exists at resolver level. |
| Sticky-then-migrate decision | `decide_migration_*`, `manual_migrate_flag_overrides_threshold_via_cli` | Covered. Threshold, exhausted, manual, no-target, and Codex-mixed decisions all pass. |
| Migration mechanic: Claude copy, Codex deferral, ledger, races | `migration_copies_*`, `migration_appends_chain_segment_with_correct_reason`, `migration_returning_clause_aborts_on_concurrent_close`, `migration_errors_on_source_*`, `migration_mechanic_errors_codex_deferred_on_codex_active_provider` | Covered. Typed error coverage is present for the main risky branches. |
| Codex deferred negative emission | `migration_does_not_emit_migrate_stderr_on_codex_deferred` | Covered. Stderr and no-target-segment assertions both pass. |
| Compaction-aware target build | `migration_truncates_*`, `migration_copies_full_jsonl_when_no_compaction_boundary`, `migration_picks_latest_of_multiple_compaction_boundaries`, `migration_errors_when_compaction_boundary_not_in_jsonl`, `pre_compaction_turns_remain_queryable_after_migration` | Covered. The no-silent-fallback contract is pinned. |
| `is_compaction_boundary` ingest plumbing | `turn_script_optional_compaction_field_defaults_false`, `turn_script_compaction_field_propagates_to_session_turns` | Covered. Fixture JSONL files exercise absent and present optional fields. |
| `compute_projections` refactor equivalence | Existing balancer suite plus `compute_projections_exposes_window_projection_used_by_selection` | Covered for selection behavior; numeric projection exactness remains bounded by residual. |
| CLI surface | `top_level_resume_without_model_*`, `manual_migrate_flag_overrides_threshold_via_cli`, `resume_list_subcommand_prints_all_chains_for_session_id`, `migrate_db_command_*` | Covered. Parser-only label issues are handled under level honesty. |
| Resume strategy compatibility | `compose_resume_args_*`, `compose_resume_args_rejects_config_kind` | Covered. Rev 4 config-kind removal is pinned. |
| Trace integration | `trace_json_includes_chain_id` | Covered. JSON chain field has a passing assertion. |

## Sub-audit 2 — Specific requested checks

Verdict: PASS with one bounded projection-strength note

- **`compute_projections` equivalence** — `cargo test balancer::tests` passed
  all 29 balancer tests. The kept suite still pins exhausted filtering,
  oldest-exhausted fallback, past-reset skip, hidden-window penalty,
  invocation-count fallback, density picks, bootstrap cascade, and cumulative
  turn behavior (`src-tauri/src/balancer/mod.rs:856`, `:868`, `:895`, `:934`,
  `:1024`, `:1052`, `:1063`, `:1115`, `:1130`, `:1149`, `:1158`, `:1184`).
  This is sufficient to pin provider-selection equivalence over the existing
  fixture matrix. The new projection test at `src-tauri/src/balancer/mod.rs:1378`
  is weaker: it asserts projection existence and broad lower-bound shape, not
  exact projected values. That weakness is already consistent with the
  documented residual "Full numeric equivalence of `compute_projections` for
  all edge cases" in `risk/05-test-residuals.md`.
- **Codex-deferred policy test exists** —
  `decide_migration_returns_codex_deferred_for_codex_provider` exists at
  `src-tauri/src/balancer/mod.rs:1353` and pins mixed Codex/Claude as
  `Migrate` plus Codex-only as `Stay`.
- **Codex-deferred mechanic test exists** —
  `migration_mechanic_errors_codex_deferred_on_codex_active_provider` exists at
  `src-tauri/tests/initiative_05_migration.rs:816` and asserts
  `MigrationError::CodexMigrationDeferred { provider: "codex" }` plus no target
  Claude segment.
- **No `[migrate]` stderr on Codex deferred** —
  `migration_does_not_emit_migrate_stderr_on_codex_deferred` exists at
  `src-tauri/tests/initiative_05_migration.rs:846` and asserts stderr lacks
  `[migrate]` plus no target segment.
- **Rev 4 config-kind nit** —
  `compose_resume_args_rejects_config_kind` exists at
  `src-tauri/src/config/model.rs:1842` and rejects `[providers.resume]
  kind = "config"`.
- **No silent fallback when compaction boundary is missing from JSONL** —
  `migration_errors_when_compaction_boundary_not_in_jsonl` exists at
  `src-tauri/tests/initiative_05_migration.rs:750` and asserts the typed
  error plus no target output directory contents.
- **§3.3 last-used write hook** —
  `chain_last_used_at_updates_after_successful_invocation` exists at
  `src-tauri/src/state/db.rs:5672` and asserts `last_used_at` lands inside the
  call window.
- **No-`-m` resume agent path** —
  `agent_resume_no_dash_m_uses_session_recorded_model` exists at
  `src-tauri/tests/initiative_05_migration.rs:444` and exercises the CLI binary
  with no `--model`.
- **No-`-m` resume UI counterpart** —
  The UI/default-model counterpart is present as
  `resolve_resume_falls_back_to_provider_default_model_for_ui_session` at
  `src-tauri/src/state/db.rs:5584`. This pins the resolver behavior, but there
  is no separate end-to-end CLI test named `ui_resume_no_dash_m_*` that starts
  from an imported `<unknown>` UI chain plus `providers.toml default_model`.
  Existing end-to-end `top_level_resume_without_model_succeeds_when_chain_exists`
  at `src-tauri/tests/initiative_05_migration.rs:875` uses a chain with a known
  model, so it does not independently exercise the UI default fallback through
  the binary.

## Sub-audit 3 — Risk annotations

Verdict: PASS

Every test added by `git diff 58aa68d..HEAD` has the required
`// risk: <name>; level: <level>; source: <ref>` comment immediately above the
test. I found no missing annotation among the new Initiative 05 tests in:

- `src-tauri/src/balancer/mod.rs`
- `src-tauri/src/config/model.rs`
- `src-tauri/src/config/providers.rs`
- `src-tauri/src/executor/cli.rs`
- `src-tauri/src/main.rs`
- `src-tauri/src/sessions/mod.rs`
- `src-tauri/src/state/db.rs`
- `src-tauri/src/trace/mod.rs`
- `src-tauri/tests/initiative_05_migration.rs`

Existing pre-Initiative-05 tests in changed files do not all have risk comments,
but they were not added by this diff and are outside the Step 6b annotation
scope.

## Sub-audit 4 — Fixture isolation

Verdict: PARTIAL

The suite mostly uses helpers/builders, but several added tests still put
fixture content directly in the test body. That violates the Phase 6 line 159
rule as written: "no inline fixtures inside test bodies."

Flagged tests:

| Test | Location | Fixture issue |
| --- | --- | --- |
| `compose_resume_args_rejects_config_kind` | `src-tauri/src/config/model.rs:1842` | Inline TOML fixture in the body. Move to a helper like `config_kind_resume_toml()` or a fixture file. |
| `migration_threshold_defaults_to_095` | `src-tauri/src/config/model.rs:1890` | Inline TOML fixture in the body. |
| `migration_threshold_rejects_out_of_range_values` | `src-tauri/src/config/model.rs:1908` | Inline generated TOML fixture in the body. |
| `parses_default_model` | `src-tauri/src/config/providers.rs:132` | Writes inline TOML into a temp file inside the body. |
| `ui_session_chain_minted_at_ingestion_uses_provider_default` | `src-tauri/tests/initiative_05_migration.rs:382` | Inline shell/JSONL turn script in the body. |
| `ui_session_chain_minted_with_unknown_when_no_provider_default` | `src-tauri/tests/initiative_05_migration.rs:402` | Inline shell/JSONL turn script in the body. |
| `chain_mint_works_for_codex_ingestion` | `src-tauri/tests/initiative_05_migration.rs:422` | Inline shell/JSONL turn script in the body. |
| `agent_resume_no_dash_m_uses_session_recorded_model` | `src-tauri/tests/initiative_05_migration.rs:443` | Inline provider script and model TOML in the body. |
| `migration_errors_on_source_jsonl_missing` | `src-tauri/tests/initiative_05_migration.rs:598` | Inline locator script/path fixture in the body. |
| `migration_errors_on_source_path_malformed` | `src-tauri/tests/initiative_05_migration.rs:633` | Inline bare-path fixture in the body and writes `bare-session.jsonl` in the process cwd before cleanup. This should use the fixture tempdir. |
| `migration_errors_when_compaction_boundary_not_in_jsonl` | `src-tauri/tests/initiative_05_migration.rs:749` | Inline SQL fixture row in the body. |
| `top_level_resume_without_model_succeeds_when_chain_exists` | `src-tauri/tests/initiative_05_migration.rs:874` | Inline provider script and model TOML in the body. |
| `top_level_resume_without_model_errors_when_no_invocation_history` | `src-tauri/tests/initiative_05_migration.rs:919` | Inline provider script and model TOML in the body. |
| `manual_migrate_flag_overrides_threshold_via_cli` | `src-tauri/tests/initiative_05_migration.rs:959` | Inline provider script and multi-provider model TOML in the body. |

Not flagged: tests that pass scenario parameters to existing builders such as
`seed_test_chain`, `seed_windows_with_deltas`, `migration_fixture`, or
`seed_pre_backfill_db`. Those still use literal values, but the actual DB/config
fixture construction is centralized.

Tests that would close this gap:

- Move the inline model TOML strings into named helper functions or
  `src-tauri/tests/fixtures/models/*.toml`.
- Move one-line shell turn emitters into `src-tauri/tests/fixtures/scripts/`
  or a `Fixture::write_turn_script(provider, session, turn)` builder.
- Replace the direct SQL in
  `migration_errors_when_compaction_boundary_not_in_jsonl` with a
  `Fixture::seed_missing_boundary_turn(...)` helper.
- Replace `PathBuf::from("bare-session.jsonl")` with a tempdir-contained path
  or a helper that deliberately returns a relative path without touching the
  repository/process cwd.

## Sub-audit 5 — Observable signals

Verdict: PASS

The added tests assert observable behavior. I found no added test that merely
asserts "did not panic."

Examples of strong observable signals:

- SQL row counts and values:
  `backfill_creates_one_chain_per_provider_session_pair`,
  `mint_chain_no_op_on_resume_of_existing_chain`,
  `migration_appends_chain_segment_with_correct_reason`,
  `manual_migrate_flag_overrides_threshold_via_cli`.
- Typed structs and enum variants:
  `resolve_resume_*`, `decide_migration_*`,
  `migration_mechanic_errors_codex_deferred_on_codex_active_provider`.
- Exit codes and stderr substrings:
  `top_level_resume_without_model_errors_when_no_invocation_history`,
  `startup_refuses_chain_ops_on_backfill_failure`.
- File existence and byte/line contents:
  `migration_copies_claude_jsonl_to_target_projects_dir`,
  `migration_truncates_target_jsonl_at_latest_compaction_boundary`,
  `migration_copies_full_jsonl_when_no_compaction_boundary`.
- JSON field presence:
  `trace_json_includes_chain_id`.

Weak-but-not-failing signal:

- `compute_projections_exposes_window_projection_used_by_selection` asserts
  projection count, lower-bound projected usage, and non-null binding score.
  It is observable, but it does not pin exact projection values.

## Sub-audit 6 — Negative-path coverage

Verdict: PARTIAL

Strong negative-path coverage exists for the most user-visible and migration
critical flows:

- Resolver ambiguity and inference:
  `resolve_resume_errors_ambiguous_when_both_recent`,
  `resolve_resume_errors_when_model_inference_impossible`,
  `resolve_resume_validates_provider_in_model_pool`.
- Migration typed errors:
  `migration_errors_on_source_jsonl_missing`,
  `migration_errors_on_source_path_malformed`,
  `migration_errors_when_compaction_boundary_not_in_jsonl`,
  `migration_mechanic_errors_codex_deferred_on_codex_active_provider`,
  `migration_returning_clause_aborts_on_concurrent_close`.
- CLI failures:
  `top_level_resume_without_model_errors_when_no_invocation_history`,
  `startup_refuses_chain_ops_on_backfill_failure`.
- Config rejection:
  `compose_resume_args_rejects_config_kind`,
  `migration_threshold_rejects_out_of_range_values`.

The literal rule "each `Result`-returning fn has at least one error-path test"
is not fully met for newly added helpers. Missing or weak error-path coverage:

| Function | Current evidence | Gap |
| --- | --- | --- |
| `StateDb::open_chain_segment` | Positive idempotence through `mint_chain_no_op_on_resume_of_existing_chain` and migration success paths. | No explicit error-path test for failed insert/read. |
| `StateDb::mint_imported_chain_if_absent` | Positive UI/Codex ingestion tests. | No explicit DB error-path test. |
| `StateDb::update_chain_last_used` | Positive timestamp update test. | No error-path test, including nonexistent chain behavior if that is intended to remain non-error. |
| `StateDb::latest_compaction_boundary` | Positive boundary and no-boundary tests. | No bad-timestamp/error-path test. |
| `StateDb::resume_previews` | Positive through `resume_list_subcommand_prints_all_chains_for_session_id`. | No invalid UUID or DB-error test at the function level. |
| `StateDb::chain_id_for_segment` | Used by trace/resume plumbing. | No direct negative-path test for missing segment/DB error. |
| `run_migrate_db` | Positive and idempotent CLI tests. | Error path only indirectly covered by startup refusal, not by `migrate-db` command failure itself. |
| `run_resume_list` | Positive CLI list test. | No malformed UUID/error-path CLI test for the new list path. |
| `decide_migration` | Positive `Stay`/`Migrate` outcomes. | No error-path test for underlying state/projection read failures; most current reads are swallowed or converted to defaults, so this may warrant either a test or an explicit non-applicability note. |

Tests that would close this gap:

- `resume_list_subcommand_rejects_malformed_uuid`: run `agents resume --list not-a-uuid`, assert exit 1 and stderr contains `invalid session UUID`.
- `migrate_db_command_reports_backfill_error`: run `agents migrate-db` against a DB path that cannot be opened/written, assert exit 1 and stderr names the DB/backfill failure.
- `latest_compaction_boundary_errors_on_bad_timestamp`: seed a boundary row with an invalid timestamp and assert the typed DB error message.
- `update_chain_last_used_reports_locked_db` or an equivalent SQLite failure fixture, unless the team records a non-applicability rationale for low-level DB error legs.
- A small table in the report or residual doc explicitly carving out SQLite
  infrastructure failures that are not worth inducing per helper.

## Sub-audit 7 — Assumption and residual pinning

Verdict: PASS

`risk/05-test-residuals.md` names a residual class, rationale, invalidating
inputs, and net-value impact for every residual entry. The assumption register
in `risk/05-supported-surface.md` Rev 2 LOW is either positively pinned or
acknowledged as residual:

| Assumption | Test evidence / residual | Assessment |
| --- | --- | --- |
| A1 — Claude Code local JSONL replay | `migration_copies_claude_jsonl_to_target_projects_dir`, compaction copy tests, resume flag compatibility tests; residual "Real Claude Code acceptance of every copied JSONL". | Pinned within fixture budget, residual documented. |
| A2 — cache economics make migration valuable | `decide_migration_*`, `manual_migrate_flag_overrides_threshold_via_cli`; residual "Real provider cache economics...". | Policy boundary pinned, real economics residual documented. |
| A3 — ingestion captures needed turns | UI mint tests, compaction ingest tests, last-turn ledger tests; residual "Adapter timing and completeness...". | Pinned for deterministic adapters, temporal residual documented. |
| A4 — projection extraction preserves selection | Full balancer suite passes; `compute_projections_exposes_window_projection_used_by_selection`; residual "Full numeric equivalence...". | Selection pinned, numeric edge residual documented. |
| A5 — backfill acceptable / `migrate-db` foreground path | Backfill open tests, migrate-db tests, startup failure recovery hint; residual "Backfill performance...". | Correctness pinned, scale residual documented. |
| A6 — Claude compaction boundaries identifiable | Adapter field tests and compaction migration tests; residual "Claude compaction record predicate stability". | Normalized contract pinned, upstream drift residual documented. |
| A7 — Codex migration deferred, identity preserved | Codex chain mint, policy deferred, mechanic deferred, no-`[migrate]` emission; residual "Codex cross-account migration". | Pinned. |
| A8 — UI default_model fallback | UI chain mint default/unknown tests, resolver provider-default test, no-`-m` agent CLI tests; residual "Provider default_model matching real UI session intent". | Pinned at resolver level; no separate UI default fallback binary test. |

## Sub-audit 8 — Level honesty

Verdict: PARTIAL

Most levels are honest:

- Tests labeled `end-to-end` in `src-tauri/tests/initiative_05_migration.rs`
  actually invoke `env!("CARGO_BIN_EXE_oulipoly-agent-runner")` through
  `Fixture::command`.
- Migration and resolver tests labeled `particular-integration` use real temp
  SQLite state, temp files, config structs, or migration mechanics.
- Most unit tests parse pure TOML or compose argv without subprocesses.

Flagged labels:

| Test | Current label | Issue | Correct label |
| --- | --- | --- | --- |
| `top_level_resume_parse_allows_missing_model_and_migrate_flag` (`src-tauri/src/main.rs:1989`) | `end-to-end` | Parser-only `Cli::try_parse_from`; no subprocess and no CLI binary. | `unit` or `structural`. |
| `resume_list_user_syntax_rewrites_to_hidden_subcommand` (`src-tauri/src/main.rs:2011`) | `end-to-end` | Parser/rewrite-only; no CLI binary. | `unit` or `structural`. |
| `parses_default_model` (`src-tauri/src/config/providers.rs:132`) | `unit` | Uses `tempfile::NamedTempFile` and `ProvidersConfig::load`, so it performs real filesystem I/O. | `particular-integration`. |
| `compute_projections_exposes_window_projection_used_by_selection` (`src-tauri/src/balancer/mod.rs:1378`) | `unit` | Uses `StateDb::open(":memory:")` and seeded quota/window DB state. No real filesystem I/O, but it is an integration of balancer math with state DB fixtures. | `particular-integration` is more honest. |

Tests that would close this gap:

- Change the risk comments' `level:` fields for the four tests above.
- Keep a separate parser-only row in §11.1/Step 6b if the team wants those
  tests counted as structural CLI argument coverage rather than end-to-end
  CLI surface coverage.

## Walked test inventory

Verdict: PASS/PARTIAL by row; `OK` means the row satisfies annotation,
observable-signal, and level checks. `FIXTURE` and `LEVEL` mark the rule
violations described above.

### `src-tauri/src/balancer/mod.rs`

| Test | Signal | Result |
| --- | --- | --- |
| `decide_migration_stays_under_threshold` | `MigrationDecision::Stay` | OK |
| `decide_migration_migrates_above_threshold` | `Migrate { target_provider_index: 1, reason: QuotaThreshold }` | OK |
| `decide_migration_migrates_when_exhausted_flag_set` | `Migrate { reason: Exhausted }` | OK |
| `decide_migration_stays_when_no_better_sibling` | `Stay` | OK |
| `decide_migration_stays_when_single_provider_pool` | `Stay` | OK |
| `decide_migration_stays_when_no_sibling_has_session_storage` | `Stay` | OK |
| `decide_migration_manual_overrides_threshold` | `Migrate { reason: Manual }` | OK |
| `decide_migration_returns_codex_deferred_for_codex_provider` | Mixed pool migrates; Codex-only pool stays | OK |
| `compute_projections_exposes_window_projection_used_by_selection` | Projection length, lower-bound used, binding score present | LEVEL |

### `src-tauri/src/state/db.rs`

| Test | Signal | Result |
| --- | --- | --- |
| `backfill_creates_one_chain_per_provider_session_pair` | Chain/segment counts and imported active segments | OK |
| `backfill_idempotent_on_second_open` | Chain count unchanged after reopen | OK |
| `mint_chain_no_op_on_resume_of_existing_chain` | Same segment id, one active segment | OK |
| `resolve_resume_returns_active_segment_for_single_chain` | Resolved chain/provider/session/index fields | OK |
| `resolve_resume_filters_by_24h_when_two_chains_share_session_id` | Recent chain selected | OK |
| `resolve_resume_errors_ambiguous_when_both_recent` | `ResumeError::Ambiguous` previews | OK |
| `resolve_resume_falls_back_to_max_last_used_when_none_within_24h` | Max stale `last_used_at` selected | OK |
| `resolve_resume_infers_model_from_latest_invocation` | Latest invocation model selected | OK |
| `resolve_resume_falls_back_to_chain_model_name_when_no_invocations` | Chain model selected | OK |
| `resolve_resume_falls_back_to_provider_default_model_for_ui_session` | Provider default model selected | OK |
| `resolve_resume_errors_when_model_inference_impossible` | `ModelInferenceImpossible` with hint | OK |
| `resolve_resume_validates_provider_in_model_pool` | `ProviderModelMismatch` suggestions | OK |
| `chain_last_used_at_updates_after_successful_invocation` | `last_used_at` inside call window | OK |
| `migration_returning_clause_aborts_on_concurrent_close` | First close wins; second returns `None` | OK |

### Config, executor, parser, sessions, trace module tests

| Test | Signal | Result |
| --- | --- | --- |
| `compose_resume_args_rejects_config_kind` | TOML parse rejects `kind = "config"` | FIXTURE |
| `session_storage_parses_claude_code_and_codex` | Parsed `SessionStorage` variants | OK |
| `migration_threshold_defaults_to_095` | Default threshold equals `0.95` | FIXTURE |
| `migration_threshold_rejects_out_of_range_values` | Out-of-range thresholds reject | FIXTURE |
| `parses_default_model` | Loaded `default_model` field | FIXTURE, LEVEL |
| `compose_resume_args_ignores_target_jsonl_for_flag_strategy` | Flag argv unchanged | OK |
| `compose_resume_args_ignores_target_jsonl_for_subcommand_strategy` | Subcommand argv unchanged | OK |
| `top_level_resume_parse_allows_missing_model_and_migrate_flag` | Parsed resume/migrate/prompt fields | LEVEL |
| `resume_list_user_syntax_rewrites_to_hidden_subcommand` | User syntax rewrites to hidden subcommand | LEVEL |
| `turn_script_optional_compaction_field_defaults_false` | No boundary persisted | OK |
| `turn_script_compaction_field_propagates_to_session_turns` | Boundary turn id persisted | OK |
| `trace_json_includes_chain_id` | JSON `session.chain_id` equals chain id | OK |

### `src-tauri/tests/initiative_05_migration.rs`

| Test | Signal | Result |
| --- | --- | --- |
| `mint_chain_on_first_session_capture` | Chain model and segment count | OK |
| `agent_session_chain_records_model_at_mint` | Chain model | OK |
| `ui_session_chain_minted_at_ingestion_uses_provider_default` | Scan success and provider default model | FIXTURE |
| `ui_session_chain_minted_with_unknown_when_no_provider_default` | Scan success and `<unknown>` model | FIXTURE |
| `chain_mint_works_for_codex_ingestion` | Codex chain model and segment count | FIXTURE |
| `agent_resume_no_dash_m_uses_session_recorded_model` | CLI success and provider argv contains `--resume` | FIXTURE |
| `migration_copies_claude_jsonl_to_target_projects_dir` | Target path exists and lines match source | OK |
| `migration_appends_chain_segment_with_correct_reason` | Closed source segment, `last_turn_id`, target reason | OK |
| `migration_errors_on_source_jsonl_missing` | `MigrationError::SourceMissing` | FIXTURE |
| `migration_errors_on_source_path_malformed` | `MigrationError::SourcePathMalformed` | FIXTURE |
| `migration_truncates_target_jsonl_at_latest_compaction_boundary` | Target starts at turn 6, turn 5 absent | OK |
| `migration_copies_full_jsonl_when_no_compaction_boundary` | Target lines equal source | OK |
| `migration_picks_latest_of_multiple_compaction_boundaries` | Target starts at turn 8 | OK |
| `migration_errors_when_compaction_boundary_not_in_jsonl` | `CompactionBoundaryNotInJsonl`, no target output | FIXTURE |
| `pre_compaction_turns_remain_queryable_after_migration` | Pre-boundary SQL count remains 2 | OK |
| `migration_mechanic_errors_codex_deferred_on_codex_active_provider` | `CodexMigrationDeferred`, no target segment | OK |
| `migration_does_not_emit_migrate_stderr_on_codex_deferred` | No `[migrate]`, no target segment | OK |
| `top_level_resume_without_model_succeeds_when_chain_exists` | CLI exit 0 and provider argv contains `--resume` | FIXTURE |
| `top_level_resume_without_model_errors_when_no_invocation_history` | Nonzero exit, stderr has `Cannot infer model` and `default_model` | FIXTURE |
| `manual_migrate_flag_overrides_threshold_via_cli` | CLI exit 0 and target segment reason `manual` | FIXTURE |
| `resume_list_subcommand_prints_all_chains_for_session_id` | Stdout contains both chains/provider/turns | OK |
| `migrate_db_command_runs_backfill_to_completion` | CLI exit 0 and one chain row | OK |
| `migrate_db_command_idempotent_on_second_run` | Chain count unchanged after second run | OK |
| `startup_refuses_chain_ops_on_backfill_failure` | Nonzero exit and stderr names `agents migrate-db` | OK |

## Final action list

To turn this PARTIAL into PASS:

1. Move inline fixture content out of the flagged test bodies.
2. Correct the four inaccurate `level:` labels.
3. Add explicit negative-path tests or documented non-applicability notes for
   the newly added low-level `Result` helpers listed in Sub-audit 6.
4. Add an end-to-end UI default-model no-`-m` resume test if the project wants
   the UI counterpart pinned at the same level as the agent session path:
   seed/import a chain with `model_name = '<unknown>'`, configure
   `providers.toml default_model`, run `agents --resume <session> continue`,
   assert exit 0 and provider argv.
