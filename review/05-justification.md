# Justification: Initiative 05 — session migration

## Verdict: PARTIALLY JUSTIFIED

Almost every hunk in the merge of commits `15c121a` (docs
backfill), `a344bd0` (initiative + planning artifacts), and
`91403a0` (implementation) maps cleanly to one of the six
traceable sources required by Phase 8:

1. **Problem framing** — `research/05-session-migration-problem.md`
   and `initiatives/05-session-migration.md`.
2. **Locked answers Q1–Q8** —
   `research/05-session-migration-answers.md`.
3. **Approved problem map** —
   `research/05-session-migration-problem-map.md` (touched-surface
   inventory + risky-behavior list).
4. **Approved proposal Rev 4** — `proposals/05-session-migration.md`
   (§1.1 assumption register, §1.2 net-value, §11.1 test-intent,
   §13.1 supported surface).
5. **Hookpoints** —
   `research/05-session-migration-hookpoints.md`.
6. **Risk gates Rev 4** — `risk/05-{audit,scope,shortcut,
   supported-surface,test-residuals}.md`.

Three hunks do not trace cleanly:

1. **`StateDb::find_provider_for_session()` and its
   `ProviderSessionMatch` row type are still alive** at
   `src-tauri/src/state/db.rs:2795-2840` and `:138-141`. Proposal
   §13 line 727 directs the function to be **replaced**, not
   deprecated, with all callers switching over. `risk/05-shortcut.md`
   F1 (Rev 4 status: STILL RESOLVED) explicitly required the
   prohibition to hold. The PR retains the function, keeps four
   tests around it (`find_provider_for_session_returns_empty_for_unknown_session`,
   `_returns_single_provider_match_with_latest_timestamp`,
   `_uses_invocation_capture_when_no_turns_exist`,
   `_orders_by_latest_timestamp_then_provider_name` at
   `src-tauri/src/state/db.rs:4406-4521`), and reuses it from
   `run_repl` at `src-tauri/src/main.rs:834` to drive the
   non-TTY "matched … selected by latest turn timestamp" detail
   line. The detail-line use is *diagnostic* (the resume decision
   itself comes from `resolve_resume`), so this is **not** the
   F1 "legacy fallback in current binary" violation pattern, but
   it IS a `~/ai/conventions/no-backwards-compatibility.md`
   violation: the function and its row struct are kept alive
   instead of deleted.
2. **`discover_fixture_turn_session()` is a test-only escape
   hatch in production code** at `src-tauri/src/main.rs:603-628`.
   It reads `<dirname(XDG_CONFIG_HOME)>/turns.jsonl`, ingests it
   into `session_turns`, and returns the first session_id when
   `find_session_for_invocation_window` fails to associate a
   session. No proposal section authorizes this path; its only
   consumer is the `initiative_05_migration.rs` integration
   harness which writes `turns.jsonl` to a tempdir parent. This
   shape is the "make the test pass" anti-pattern proposal §14
   "no runtime fallback" was written to forbid.
3. **`resume_model_pool_mismatch_message()` and
   `should_emit_resume_detail_line()` are annotated
   `#[allow(dead_code)]`** at `src-tauri/src/main.rs:657, 671`
   despite being live callers (`main.rs:789, 996, 835`). Cosmetic
   only — the annotation is wrong but not load-bearing.

No other deletion-list violations were found. The Rev 4 `kind =
"config"` removal, `zstd` / `.zst` removal, and the three
explicitly-deleted test names
(`migration_zst_round_trip_preserves_post_offset_bytes`,
`migration_copies_codex_rollout_with_zst_extension`,
`migration_composes_codex_experimental_resume_argv`) are all
absent from the diff. The single `kind = "config"` reference is
the Rev 4-required negative test
(`compose_resume_args_rejects_config_kind` at
`src-tauri/src/config/model.rs:1842-1858`) that pins the
parse-time refusal.

## Hunks kept

### `src-tauri/src/state/db.rs` — schema

- **Add `is_compaction_boundary INTEGER NOT NULL DEFAULT 0`** to
  fresh `session_turns` schema (`:577`). Source: proposal §2 +
  hookpoints §1 + answers Q8.
- **Add `ensure_session_turns_schema` ALTER branch** for
  `is_compaction_boundary` at `:715-725`. Source: proposal §2
  ("idempotent ALTER") + hookpoints §1 line 18.
- **`CREATE TABLE IF NOT EXISTS session_chains`** at `:583-588`.
  Source: proposal §2 schema literal + hookpoints §1 line 17.
- **`CREATE TABLE IF NOT EXISTS session_chain_segments`** at
  `:590-601`. Source: proposal §2 + answers Q5.
- **`idx_segments_session` and `idx_segments_chain_active`
  indexes** at `:603-606`. Source: proposal §2.
- **`backfill_session_chains()` invocation** in `StateDb::open`
  at `:611-613`. Source: proposal §2 + hookpoints §1 line 20.
- **WAL-mode error message extended** with
  `"; run agents migrate-db first"` at `:449`. Source: proposal
  §14 ("clear error pointing the user at `agents migrate-db`
  for retry"); supported-surface gate observability.

### `src-tauri/src/state/db.rs` — types

- **`ResolvedResume` struct** at `:130-137`. Source: proposal §4
  + hookpoints §3 line 37. Adds `active_provider_index` (not in
  the proposal struct literal) — minor expansion to avoid
  re-resolving the index in the call site.
- **`ResumeError` enum** at `:139-170` — variants
  `InvalidUuid`, `NoChainFound`, `Ambiguous { input, previews }`,
  `ModelInferenceImpossible { chain_id, active_provider, hint }`,
  `ProviderModelMismatch { model_name, active_provider,
  suggestions }`, `UnknownModel`, `ActiveSegmentMissing`,
  `ProviderMissingResume`, `Db`. Source: proposal §4 + answers
  Q4 + hookpoints §3 line 38. The added `UnknownModel`,
  `ActiveSegmentMissing`, and `ProviderMissingResume` variants
  are forced by the resolver's actual return paths (model name
  inferred → not in the model store; chain row exists but no
  active segment; provider lacks `[providers.resume]`); each
  carries its own `ResumeError` variant rather than collapsing
  into `Db { message }`. Justified-but-loose against the
  proposal's variant list.
- **`ChainPreview` and `TurnPreview`** at `:172-187`. Source:
  proposal §4.1.
- **`BackfillReport`** at `:189-194`. Source: proposal §8.5.1
  (idempotence proof for `agents migrate-db` ↔ `StateDb::open`
  equivalence).
- **`SessionTurnIngest.is_compaction_boundary`** at `:125`.
  Source: proposal §3.4 + hookpoints §8.

### `src-tauri/src/state/db.rs` — write paths

- **`mint_chain_for_invocation_session(invocation_row_id)`** at
  `:1214-1276`. Source: proposal §3.1 + hookpoints §4. Wired
  from `emit_known_session_id` at `main.rs:646-648`.
- **`mint_imported_chain_if_absent(provider, session_id, ts,
  model)`** at `:3016-3061`. Source: proposal §3.1.1 +
  hookpoints §4 line 49.
- **`open_chain_segment(chain, provider, session, started_at,
  reason)`** at `:2998-3014`. Source: proposal §6 step 6 +
  hookpoints §6 line 71.
- **`close_active_segment_returning(chain_id, ended_at)`** at
  `:3063-3084`. Source: proposal §3.2 (RETURNING-pattern race
  guard).
- **`update_chain_last_used(chain_id)`** at `:3086-3094`.
  Source: proposal §3.3 + hookpoints §4 line 50. Pinned by
  `chain_last_used_at_updates_after_successful_invocation` test.
- **`latest_compaction_boundary(provider, session_id)`** at
  `:3096-3120`. Source: proposal §6.6 step 1 + hookpoints §6
  line 70.
- **`backfill_session_chains()`** at `:2247-2354`. Source:
  proposal §2 backfill loop. The `tx.execute` returning row
  count is summed into the `BackfillReport` — cleaner than the
  proposal's "manual progress lines" wording but pinned by
  `migrate_db_command_runs_backfill_to_completion` and
  `_idempotent_on_second_run` against the shape contract.
- **Single-row `INSERT OR IGNORE INTO session_turns` extended**
  with `is_compaction_boundary` literal `0` at `:2138-2152`.
  Source: proposal §3.4 + hookpoints §8 line 86. (Single-row
  path keeps the `parent_turn_id` / `is_sidechain` divergence
  from batch noted in the hookpoint as an
  implementer-discretion item.)
- **Batch `INSERT OR IGNORE INTO session_turns` extended** with
  `is_compaction_boundary` column + bind at `:2184-2206`.
  Source: proposal §3.4 + hookpoints §8 line 87.

### `src-tauri/src/state/db.rs` — resolver

- **`resolve_resume(config, models, input, model_override)`** at
  `:2566-2674`. Source: proposal §4 + answers Q4/Q5 +
  hookpoints §3 line 41. Implements the documented disambiguation
  steps (24h filter → max(last_used_at) → ambiguous), the model
  fallback chain (override → latest invocation → chain.model_name
  → provider default → `ModelInferenceImpossible`), and pool
  validation with `suggestions` building.
- **Helper SQL methods** `candidate_chain_ids` (`:2718`),
  `choose_resume_chain` (`:2731`), `active_segment_for_chain`
  (`:2761`), `chain_model_name` (`:2774`),
  `latest_invocation_model_for_chain` (`:2784`),
  `chain_previews` (`:2800`). Source: proposal §4 algorithm
  steps. All private — no API surface beyond the public
  resolver/preview endpoints.
- **`resume_previews(input)`** at `:2682-2685`. Source: proposal
  §8.5 (diagnostic-only `agents resume --list`).
- **`chain_id_for_segment(provider, session_id)`** at
  `:2687-2702`. Source: proposal §10 + hookpoints §11 line 109.
  Used by `trace::build_trace_session` to populate the new
  `chain_id` JSON field.

### `src-tauri/src/state/db.rs` — preserved-but-prohibited

- **`find_provider_for_session(session_id)`** at `:2795-2840`.
  **NOT JUSTIFIED.** Proposal §13 line 727: "Old function
  deleted, not deprecated." Hookpoints §2 line 24: "Delete in
  entirety per `~/ai/conventions/no-backwards-compatibility.md`."
  `risk/05-shortcut.md` F1 (Rev 4): "STILL RESOLVED" — Rev 4 was
  not supposed to weaken this prohibition. The implementation
  retains the function, leaving its three-table UNION SQL alive
  alongside `resolve_resume`.
- **`ProviderSessionMatch` struct** at `:138-141`.
  **NOT JUSTIFIED.** Hookpoints §3 line 30 explicitly states
  "delete; replaced by `ResolvedResume` in §3."
- **Four `find_provider_for_session_*` tests** at `:4406-4521`.
  **NOT JUSTIFIED.** Hookpoints §2 line 29: "delete or rewrite
  around `resolve_resume`." Implementation keeps them.

### `src-tauri/src/state/db.rs` — new tests

The diff adds 17 named §11.2 unit tests in `state/db.rs`'s
`#[cfg(test)] mod tests`:

- `backfill_creates_one_chain_per_provider_session_pair`
- `backfill_idempotent_on_second_open`
- `mint_chain_no_op_on_resume_of_existing_chain`
- `resolve_resume_returns_active_segment_for_single_chain`
- `resolve_resume_filters_by_24h_when_two_chains_share_session_id`
- `resolve_resume_errors_ambiguous_when_both_recent`
- `resolve_resume_falls_back_to_max_last_used_when_none_within_24h`
- `resolve_resume_infers_model_from_latest_invocation`
- `resolve_resume_falls_back_to_chain_model_name_when_no_invocations`
- `resolve_resume_falls_back_to_provider_default_model_for_ui_session`
- `resolve_resume_errors_when_model_inference_impossible`
- `resolve_resume_validates_provider_in_model_pool`
- `chain_last_used_at_updates_after_successful_invocation`
- `migration_returning_clause_aborts_on_concurrent_close`
- `mint_chain_on_first_session_capture` (runs in
  `initiative_05_migration.rs` end-to-end harness)

Source: proposal §11.2 named-test list.

### `src-tauri/src/balancer/mod.rs`

- **`compute_projections(model, state, ctx) -> Vec<ProviderProjection>`**
  at `:295-401`. Source: proposal §5.1 + hookpoints §5 line 59.
  Keeps the same refresh + scan + per-window projection loop as
  `score_by_density` (the file's prior contents are retained for
  the selection-side caller). The `ProviderProjection` /
  `WindowProjection` shapes match the proposal's named struct
  literals.
- **`decide_migration(state, model, resolved, threshold,
  manual_target) -> Result<MigrationDecision, MigrationError>`**
  at `:403-477`. Source: proposal §5 algorithm steps 1-7. The
  algorithm walks: single-provider → `Stay`; manual target with
  `[providers.resume]` and Claude-Code storage → `Migrate`;
  active provider exhausted_at lookup; `compute_projections` to
  read the active provider's `tightest_projected`; threshold
  short-circuit; then the eligible-sibling filter chain
  (provider≠active, has `resume`, has Claude-Code storage,
  `binding_score.is_some()`, strictly better than active under
  non-exhausted path).
- **`MigrationDecision` enum** at `:46-52`. Source: proposal §5.
- **`TransitionReason` enum + `as_str()`** at `:54-74`. Source:
  proposal §2 (CHECK constraint values) + §6 step 6.
- **`is_claude_code_storage(storage)`** helper at `:480-482`.
  Source: proposal §5 ("migration-eligible target = Claude-Code
  sibling").
- **Shared candidate filter** to `scan_provider` call sites
  (`:112`, `:295-303`) — adds `ctx.providers_cfg` to match
  `scan_provider`'s extended signature. Source: proposal §3.1.1
  ("`ProvidersConfig` reference threaded through `scan_provider`").
- **8 new tests** (`decide_migration_*`,
  `compute_projections_exposes_window_projection_used_by_selection`).
  Source: proposal §11.2.

### `src-tauri/src/migration/mod.rs` (new file)

Entire file is new, 262 lines. All sourced from proposal §6:

- **`MigrationError` enum** with variants
  `CodexMigrationDeferred`, `SourceMissing`,
  `SourcePathMalformed`, `SourceMissingStorage`,
  `TargetMissingStorage`, `TargetAlreadyExists`,
  `TargetDirectoryCreateFailed`, `TranscriptLocatorFailed`,
  `CompactionBoundaryNotInJsonl`, `ConcurrentSegmentClosed`,
  `ProviderNotInModelPool`, `ProviderMissingResume`, `Io`, `Db`.
  Source: proposal §6 step 1 / step 4 / step 5 + answers Q3 +
  research/05-codex-resume-verification.md (the
  `CodexMigrationDeferred` variant). The proposal does not
  enumerate `TargetAlreadyExists` or
  `TargetDirectoryCreateFailed`, but they are necessary
  failure modes for the §6 step 5 "rename atomically" contract;
  justified-but-loose under §6 step 5 ("failure modes
  enumerated").
- **`MigratedSegment` struct** carrying chain_id, source/target
  provider+session+jsonl_path, transition reason. Source:
  proposal §6 + §3.2.
- **`migrate_chain_segment(state, sessions_cfg, model, resolved,
  target_provider_index, reason, stderr)`** function. Body
  walks: pool lookup → resume strategy presence on both ends →
  Codex deferred guard (source OR target Codex returns the
  typed error before any file I/O) → `locate_transcript`
  primary, `find_claude_source_from_storage` fallback → absolute
  path validation → existence check → mint target session_id →
  Claude-Code target storage destructure → `cwd_hash` from
  source parent → bytes read → optional compaction-boundary
  truncation → `target_dir` create → tmp-write → atomic rename
  → segment close-then-open transaction → `[migrate]` stderr
  emission. Source: proposal §6 steps 1-7 + §3.2 + §6.6.
- **`find_claude_source_from_storage(provider, session_id)`**
  fallback. Source: proposal §6 step 1 ("fall back to globbing
  `<projects_dir>/*/<session_id>.jsonl`").
- **Compaction-aware truncation** at `:138-156`. Reads
  `latest_compaction_boundary`, scans the source JSONL line by
  line accumulating byte offsets, finds the line containing the
  boundary turn_id, slices `&bytes[offset..]`. Returns
  `MigrationError::CompactionBoundaryNotInJsonl` when the
  recorded turn_id is not present in the file. Source: proposal
  §6.6 steps 1-4 (preserves the "no silent offset=0 fallback"
  contract pinned by `risk/05-shortcut.md` F2).
- **`[migrate] <source> -> <target> reason=<reason>` stderr
  emission** at `:177-184`. Source: proposal §13.1 line 776.

### `src-tauri/src/sessions/mod.rs`

- **`ScriptTurn.is_compaction_boundary: Option<bool>`** at
  `:43-44`. Source: proposal §3.4 + §9.1.1 + hookpoints §8.
  `#[serde(default)]` matches existing `is_sidechain` precedent.
- **`scan_provider` extended signature** with `providers_cfg:
  &ProvidersConfig` at `:60-63`. Source: proposal §3.1.1
  + hookpoints §4 line 49.
- **`is_compaction_boundary` propagation** at `:121-123`.
  Source: proposal §3.4.
- **UI-session mint loop** at `:128-141`: after
  `ingest_session_turns_batch` succeeds, walks each turn and
  calls `mint_imported_chain_if_absent` with provider's
  `default_model` (or `'<unknown>'` fallback). Source: proposal
  §3.1.1 + answers Q4. Errors push to `report.errors` instead of
  failing the scan — matches the "ingestion errors don't abort"
  shape elsewhere in the file.
- **`scan_all` provides `ProvidersConfig::default()`** at `:152`.
  Source: implementation discretion under §3.1.1 — `scan_all`'s
  callers (legacy diagnostic path only) don't have a providers
  config in scope. Justified-but-loose; UI sessions ingested
  via `scan_all` mint with `'<unknown>'`.
- **2 new tests**:
  `turn_script_optional_compaction_field_defaults_false`,
  `turn_script_compaction_field_propagates_to_session_turns`.
  Source: proposal §11.2.
- **Internal test fixture argument additions** for
  `scan_provider` calls (4 sites). Mechanical follow-through.

### `src-tauri/src/config/model.rs`

- **`SessionStorage` enum** with `ClaudeCode { projects_dir }` /
  `Codex { sessions_dir }` variants and tagged-union serde at
  `:168-189`. Source: proposal §9.1 + hookpoints §9 line 92.
  `validate()` rejects empty paths.
- **`ProviderConfig.session_storage: Option<SessionStorage>`**
  at `:22-23` and `ProviderConfig::new` default at `:39`.
  Source: proposal §9.1 + hookpoints §9 line 93.
- **`ModelConfig.migration_threshold: f64`** with
  `#[serde(default = "default_migration_threshold")]` at
  `:249-251` and helper at `:253-255`. Source: proposal §5.1
  + §13.
- **`RawModelToml.session_storage` / `migration: Option<RawMigration>`**
  at `:329, 332`. Source: proposal §5.1 + §9.1.
- **`RawProvider.session_storage`** at `:345`. Source: proposal §9.1.
- **`from_toml` plumbing**: parse `[migration]` block (`:592-602`),
  reject out-of-range threshold, propagate `session_storage` into
  both providers and top-level provider arms. Source: proposal §5.1.
- **`to_toml` extension**: `append_session_storage_toml` helper
  at `:687-705` and `[migration]` block emission at `:475-481`.
  Source: proposal §5.1 + §9.1.
- **DELETE: the `interactive_args` requirement on
  `[providers.resume]`** at the prior `:656-660`. Justified-but-loose:
  not in the proposal; the proposal does not direct removal of
  this validator. The deletion lets the
  `compose_resume_args_rejects_config_kind` test (and other
  fixtures without `interactive_args`) parse, but the
  validation drop ripples to all `[providers.resume]` blocks
  pool-wide. **Worth flagging as a Phase-7-amendment-style
  scope expansion** — the change is small, but the
  commit-message body does not document it as intentional.
- **3 new tests**:
  `compose_resume_args_rejects_config_kind`,
  `session_storage_parses_claude_code_and_codex`,
  `migration_threshold_defaults_to_095`,
  `migration_threshold_rejects_out_of_range_values`. Source:
  proposal §11.2 + §11.1.

### `src-tauri/src/config/providers.rs`

- **`ProviderEntry.default_model: Option<String>`** at `:18-19`.
  Source: proposal §9.2 + hookpoints §9 line 94.
- **`RawEntry.default_model`** at `:35`, propagated through
  `entries` constructor at `:57`. Source: proposal §9.2.
- **NEW: `parses_default_model` test** at `:128-149`. Source:
  proposal §11.2 (resolver model-fallback test cluster).

### `src-tauri/src/config/mod.rs`

- **Re-export `SessionStorage`** at `:9`. Mechanical.

### `src-tauri/src/quota/mod.rs`

- **One `default_model: None` test fixture cleanup** at `:525`.
  Source: proposal §9.2 mechanical follow-through.

### `src-tauri/src/state/mod.rs`

- **Re-export `BackfillReport, ChainPreview, ModelStore,
  ResolvedResume, ResumeError, TurnPreview`** at `:7`. Source:
  proposal §4 / §8.5.1 surface.

### `src-tauri/src/lib.rs`

- **Add `pub mod migration;`** at `:6`. Source: proposal §6
  module placement (hookpoints §6 line 68 recommends a new
  `migration/mod.rs`).
- **`migration_threshold: 0.95` literal added** to four
  `ModelConfig` test fixtures. Mechanical follow-through from
  the new field.

### `src-tauri/src/executor/cli.rs`

- **`ResumePayload.target_jsonl_path: Option<&'a Path>`** at
  `:243`. Source: proposal §7 line 448.
- **`compose_resume_args(strategy, session_id,
  target_jsonl_path) -> Result<Vec<String>, String>`** new
  public test-friendly helper at `:247-257`. Source: proposal §7
  + §11.1 row "Resume strategy compatibility".
- **`compose_resume_provider_args(provider_args, resume)`** —
  the prior internal `compose_resume_args` signature, renamed.
  Body refactored to call the new shared `append_resume_args`
  helper. Justified-but-loose: refactor not directly named in
  the proposal but required to avoid duplicating argv-building
  logic between the public test surface and the existing
  resume-spawn callers. Behavior pinned by
  `compose_resume_args_ignores_target_jsonl_for_flag_strategy`
  and `_for_subcommand_strategy` (§11.1 row 633 invariance).
- **`target_jsonl_path: None` threading** at the two existing
  `ResumePayload` literal sites in `execute_resume` and
  `execute_interactive` callers. Source: proposal §7 line 452.
- **Mechanical `session_storage: None` literal cleanups** in 7
  test-fixture `ProviderConfig` literals. Source: §9.1 mechanical
  follow-through.
- **Mechanical `migration_threshold: 0.95` literal cleanups** in
  9 test-fixture `ModelConfig` literals. Source: §5.1.
- **2 new tests**:
  `compose_resume_args_ignores_target_jsonl_for_flag_strategy`,
  `_for_subcommand_strategy`. Source: §11.1 row 633.

### `src-tauri/src/main.rs`

- **`Cli.migrate: Option<String>`** at `:47-48`. Source:
  proposal §8.4 + hookpoints §10 line 100.
- **`Subcommands::Repl.model: Option<String>`** at `:104-105`,
  `Subcommands::Repl.migrate` at `:114-115`. Source: proposal
  §8.3 + §8.4.
- **`Subcommands::Resume.model: Option<String>`** at `:127-128`,
  `Subcommands::Resume.migrate` at `:135-136`. Source: proposal
  §8.2 + §8.4.
- **`Subcommands::ResumeList { uuid: String }` hidden variant**
  at `:154-155`. Source: proposal §8.5 + hookpoints §10 line 101
  ("introduce a new `Subcommands::ResumeList`").
- **`Subcommands::MigrateDb`** at `:156-157`. Source: proposal
  §8.5.1.
- **`run` dispatch updates** at `:298-329`: pass-through for
  `Option<String>` model + `--migrate`, plus dispatch for the
  two new variants. Source: §8.1-8.5.1.
- **DELETE: top-level `--resume requires --model` enforcement**
  at the prior `:318-321`. Source: proposal §8.1.
- **Resolver call wiring in top-level dispatch** (`:362-376`).
  Source: proposal §8.1.
- **`format_resume_error(err)`** at `:702-755`. Source: proposal
  §4.1 (ambiguous rendering) + §4 step 6 (mismatch wording).
  Renders all `ResumeError` variants to stderr text.
- **`run_repl` rewrite** at `:759-961` — calls `resolve_resume`,
  threads model fallback, drops the prior
  `find_provider_for_session` decision path. Source: proposal
  §4 + §8.3 + hookpoints §3 line 42.
- **`run_resume` rewrite** at `:953-1099` — calls
  `resolve_resume`, drops the prior
  `find_provider_for_session` decision path, calls
  `decide_migration` + `migrate_chain_segment` between resolution
  and spawn. Source: proposal §4 + §5 + §6 + §8.2 + hookpoints
  §3 line 43.
- **`run_migrate_db()`** at `:1337-1345`. Source: proposal §8.5.1
  + hookpoints §10 line 99.
- **`run_resume_list(uuid)`** at `:1347-1366`. Source: proposal
  §8.5.
- **`normalize_resume_list_args(args)`** at `:1368-1387`. Source:
  proposal §8.5 (the `agents resume --list <UUID>` user-facing
  syntax) + hookpoints §10 line 101 ("hidden subcommand for
  `resume --list`"). The argv-rewrite trick (`resume --list X`
  → `resume-list X`) avoids overloading the `Resume` variant's
  required positionals.
- **`emit_known_session_id` mint hook** at `:646-648`: after
  `update_session_capture` succeeds, calls
  `mint_chain_for_invocation_session(invocation_row_id)`. Source:
  proposal §3.1 + hookpoints §4 line 47.
- **`scan_provider` call site update** in
  `ingest_and_emit_session_id` at `:559-565`. Mechanical
  follow-through from the new providers_cfg argument.
- **`ProvidersConfig` load in `run_resume`** at `:976-981`.
  Source: proposal §4 step 5 + §9.2 (resolver needs the providers
  config for `default_model` lookup).
- **2 new tests**:
  `top_level_resume_parse_allows_missing_model_and_migrate_flag`,
  `resume_list_user_syntax_rewrites_to_hidden_subcommand`.
  Source: proposal §11.2.

### `src-tauri/src/main.rs` — unjustified hunks

- **`discover_fixture_turn_session(provider_name, state)`** at
  `:603-628`. **NOT JUSTIFIED.** No proposal section authorizes
  this fallback. The function reads
  `<dirname(XDG_CONFIG_HOME)>/turns.jsonl`, parses each line as
  `SessionTurnIngest`, batch-ingests them, and returns the first
  session_id — a path inverse to the proposal's
  "ingestion-driven session capture." Its only consumer is the
  `initiative_05_migration.rs` integration harness, which writes
  `turns.jsonl` to a tempdir parent (`tests:451, 457`). This is a
  test-shim in production code: the proposal explicitly forbids
  "no runtime fallback to legacy resolution" (§14) and
  `~/ai/conventions/no-deferred-stubs.md` forbids
  test-shim-shaped fallbacks. The cleaner fix is to drive the
  integration tests through the actual `scan_provider` adapter
  path the proposal specifies — the
  `<config>/sessions.toml`-configured `turn_script` mechanism
  already handles this.
- **`#[allow(dead_code)]` on `should_emit_resume_detail_line`
  and `resume_model_pool_mismatch_message`** at `:657, 671`.
  Both are still called (`:835`, `:789`, `:996`); the
  annotations are stale residue from a refactor pass and not
  load-bearing.

### `src-tauri/src/trace/mod.rs`

- **`TraceSession.chain_id: Option<String>`** at `:62`. Source:
  proposal §10 + hookpoints §11 line 109. Populated via
  `db.chain_id_for_segment(provider, session_id)` at `:296-298`.
- **`chain_id: None` literal added** to four
  `TraceSession` short-circuit branches (unresolved, no_locator,
  available, missing). Mechanical follow-through.
- **`seed_chain_segment` test helper** at `:556-571`. Source:
  proposal §11.2 trace test.
- **NEW: `trace_json_includes_chain_id`** at `:1493-1517`.
  Source: proposal §11.2.
- **6 mechanical `is_compaction_boundary: false` literal
  cleanups** in trace test fixtures. Source: §3.4 mechanical
  follow-through.

### `scripts/claude-code-turns`

- **`is_compaction_boundary` filter and emission** at
  `:67-84`: detects `obj.get("isCompactSummary") is True`,
  expands the kind filter to keep compaction records, emits the
  flag in the normalized JSON line. Source: proposal §9.1.1 +
  hookpoints §12 (the implementation-discovery item: "the exact
  record type Claude Code writes for compaction events must be
  confirmed against real JSONL samples"). The
  `risk/05-supported-surface.md` Rev 2 gate confirmed
  `isCompactSummary` is the predicate to use.

### `src-tauri/tests/initiative_05_migration.rs` (new file)

Entire 1161-line file is new. Implements the §11.2
proposal-named integration tests:

- `mint_chain_on_first_session_capture`,
  `agent_session_chain_records_model_at_mint`,
  `ui_session_chain_minted_at_ingestion_uses_provider_default`,
  `ui_session_chain_minted_with_unknown_when_no_provider_default`,
  `chain_mint_works_for_codex_ingestion`,
  `agent_resume_no_dash_m_uses_session_recorded_model`,
- `migration_copies_claude_jsonl_to_target_projects_dir`,
  `migration_appends_chain_segment_with_correct_reason`,
  `migration_errors_on_source_jsonl_missing`,
  `migration_errors_on_source_path_malformed`,
  `migration_truncates_target_jsonl_at_latest_compaction_boundary`,
  `migration_copies_full_jsonl_when_no_compaction_boundary`,
  `migration_picks_latest_of_multiple_compaction_boundaries`,
  `migration_errors_when_compaction_boundary_not_in_jsonl`,
  `pre_compaction_turns_remain_queryable_after_migration`,
- `migration_mechanic_errors_codex_deferred_on_codex_active_provider`,
  `migration_does_not_emit_migrate_stderr_on_codex_deferred`,
- `top_level_resume_without_model_succeeds_when_chain_exists`,
  `top_level_resume_without_model_errors_when_no_invocation_history`,
- `manual_migrate_flag_overrides_threshold_via_cli`,
  `resume_list_subcommand_prints_all_chains_for_session_id`,
- `migrate_db_command_runs_backfill_to_completion`,
  `migrate_db_command_idempotent_on_second_run`,
  `startup_refuses_chain_ops_on_backfill_failure`.

Source: proposal §11.2.

### Fixtures

- **`tests/fixtures/jsonl/adapter/with_compaction.jsonl`** —
  one-line normalized fixture with
  `is_compaction_boundary: true`. Source: §11.2
  `turn_script_compaction_field_propagates_to_session_turns`.
- **`tests/fixtures/jsonl/adapter/without_compaction.jsonl`** —
  one-line fixture with the boundary flag omitted. Source:
  §11.2 `turn_script_optional_compaction_field_defaults_false`.
- **`tests/fixtures/jsonl/claude/full_session.jsonl`** —
  10-line synthetic session for migration mechanic tests.
  Source: §11.2 migration mechanic tests.
- **`tests/fixtures/scripts/session_echo.sh`** — minimal stub
  CLI for end-to-end harness. Source: §11.1 fixture-application
  guidance.

### `src-tauri/tests/pr_f_resume_integration.rs`

- **One mechanical `is_compaction_boundary: false` literal
  cleanup** at `:164`. Source: §3.4 mechanical follow-through.

### Documentation files (commits `15c121a` + `a344bd0`)

The two doc-commits add the `initiatives/`, `proposals/`,
`research/`, `risk/`, and `review/` artifacts that Phase 8
treats as authoritative inputs. They are self-justifying for
this review's purpose: every implementation hunk traces against
them rather than the other way around.

- `initiatives/05-session-migration.md` — captures the user's
  problem framing verbatim (Phase 1).
- `proposals/05-session-migration.md` — Rev 4, all sections
  cited above.
- `research/05-codex-resume-verification.md` — A7 invalidator
  evidence for the Codex deferral.
- `research/05-session-migration-{problem,answers,problem-map,
  hookpoints}.md` — Phase 2/3/5 research.
- `risk/05-{audit,scope,shortcut,supported-surface,
  test-residuals}.md` — Phase 4 risk gates (all LOW under
  Rev 4).
- `initiatives/04-reactive-routing.md`, `initiatives/README.md`,
  `proposals/04-reactive-routing.md`,
  `research/03-rca-...md`, `research/04-...md`,
  `risk/04-...md`, `review/04-...md` — initiative-package
  backfill noted in commit `15c121a`'s body. They reconstruct
  04's planning artifacts after the fact and are out of scope
  for this review's "implementation matches proposal" check.

## Hunks that don't trace cleanly

Three. Two are real:

1. **`find_provider_for_session()` retained, four tests retained,
   `ProviderSessionMatch` retained, one diagnostic call site at
   `main.rs:834`** (`src-tauri/src/state/db.rs:2795-2840`,
   `:138-141`, `:4406-4521`). Proposal §13 line 727 directs
   "Old function deleted, not deprecated"; hookpoints §2
   line 24 reinforces. The function is unused for the resume
   *decision* (only for printing the non-TTY detail line) so
   the F1 prohibition is not strictly violated, but the
   "no-backwards-compatibility" deletion contract is. Recommend
   either deleting the function and routing the detail-line
   sibling-list query through a new `chain_siblings_for_session`
   helper, or documenting the retained function as a Phase-7
   amendment in the commit body.
2. **`discover_fixture_turn_session(provider_name, state)`**
   (`src-tauri/src/main.rs:603-628`). Test-shim in production:
   reads `<dirname(XDG_CONFIG_HOME)>/turns.jsonl`, parses, and
   ingests. No proposal source. Recommend deleting and routing
   the integration tests through `[providers.session_capture]` /
   `sessions.toml`'s configured `turn_script` mechanism.

The third is cosmetic:

3. **`#[allow(dead_code)]` on
   `should_emit_resume_detail_line` and
   `resume_model_pool_mismatch_message`** when both are live
   callers. Stale annotations.

One scope-expansion is justified-but-loose:

- **Removal of the `interactive_args` requirement on
  `[providers.resume]`** in `src-tauri/src/config/model.rs`. Not
  in the proposal; needed so that the
  `compose_resume_args_rejects_config_kind` test fixture (and
  the §11.2 migration-mechanic fixtures) parse without
  declaring `interactive_args`. The change is small and pool-wide
  config validation now accepts a previously-rejected shape;
  worth flagging as an undeclared scope expansion. Recommend a
  commit-message-body note matching the
  initiative-04 phase-7-amendment convention.

## Hunks that did NOT happen but should have

The proposal §11.2 named-test list is otherwise complete (35+
named tests present across `state/db.rs`, `balancer/mod.rs`,
`config/{model,providers}.rs`, `executor/cli.rs`,
`sessions/mod.rs`, `trace/mod.rs`, `main.rs`, and
`tests/initiative_05_migration.rs`). One test-audit
observation:

- The proposal lists
  `migration_does_not_emit_migrate_stderr_on_codex_deferred` as
  a §11.1 negative-emission test — present at
  `tests/initiative_05_migration.rs:846`.
- `compose_resume_args_*` invariance is pinned by the two new
  flag/subcommand `_ignores_target_jsonl` tests
  (`executor/cli.rs:1762, 1782`).
- The `[migrate]` positive-emission substring is asserted by
  `manual_migrate_flag_overrides_threshold_via_cli`'s stderr
  check (`tests/initiative_05_migration.rs:960`) — closing the
  Rev 4 watchpoint left open by `risk/05-shortcut.md`'s
  "Implementation-risk notes."

These are coverage points, not justification gaps.

## Cross-cutting cleanups

The diff stays scoped to the session-migration concern.
Specifically checked for:

- **`scripts/`** — only `claude-code-turns` is changed (and
  in-scope per §9.1.1). `codex-turns` is untouched, matching the
  Rev 4 deferral in §15.
- **CI / GitHub Actions** — none.
- **`agents/` config** — none.
- **Frontend (`src/`)** — none. Proposal §13 confirms PoolsView /
  StatusView remain unchanged in v1.
- **Unrelated dependency bumps** — none. `Cargo.toml` is not in
  the diff. `zstd` is correctly absent (Rev 4 deletion).
- **Drive-by formatting / clippy fixes** — none. The
  `src-tauri/src/state/db.rs` test-block reordering visible in
  the diff is a side effect of inserting many new tests at the
  end of the file; existing test bodies are unchanged.
- **`.zst` / `zstd` references anywhere in `src/`** — none.
  Search returned zero matches.
- **`kind = "config"` resume strategy** — present only as the
  refusal fixture in
  `compose_resume_args_rejects_config_kind`'s TOML literal
  (`config/model.rs:1851`). Per Rev 4 contract.
- **`experimental_resume`** — present only in the same refusal
  fixture (`config/model.rs:1852`). Per Rev 4 contract.
- **`migration_zst_round_trip_preserves_post_offset_bytes`,
  `migration_copies_codex_rollout_with_zst_extension`,
  `migration_composes_codex_experimental_resume_argv`** — all
  three deletions are honored. Search returns zero matches in
  any test file.

## Summary

PARTIALLY JUSTIFIED. The bulk of the diff (schema, types,
resolver, sticky-then-migrate decision, migration mechanic, CLI
surface, adapter contract, trace integration, and 35+ named
tests) traces cleanly to proposal §1.1-§13.1, answers Q1-Q8,
hookpoints §1-§13, and risk gates §1.1-§15. The Rev 4 deletion
contract holds for `kind = "config"` / `experimental_resume`,
the `zstd` crate / `.zst` code path, and the three named
test-deletions. Phase 4 risk gates (audit, scope, shortcut,
supported-surface) all returned LOW under Rev 4 and the
implementation does not undo any LOW-direction finding from
those gates.

Three deviations remain:

1. `find_provider_for_session()` is retained for a diagnostic
   stderr line in `run_repl`, with its `ProviderSessionMatch`
   row type and four `_returns_*` / `_orders_by_*` tests still
   live. Proposal §13 / hookpoints §2 / convention
   `~/ai/conventions/no-backwards-compatibility.md` direct
   deletion. The retained function is not the
   `risk/05-shortcut.md` F1 "legacy fallback in current binary"
   pattern (it does not influence resume selection), but it IS
   a deletion-list violation.
2. `discover_fixture_turn_session()` is a test-shim in
   production code (`main.rs:603-628`) with no proposal source
   and the inverse shape of the integration-test pattern §11.1
   prescribes (fixtures applied outside test bodies via
   `sessions.toml`-configured adapters). Recommend deleting the
   fallback and routing the integration tests through the real
   adapter pipeline.
3. `[providers.resume].interactive_args` requirement is removed
   from `config/model.rs` validation — needed for the new
   migration-mechanic fixtures, but not declared as a scope
   expansion in either the proposal or the commit body.

The verdict is PARTIALLY JUSTIFIED rather than UNJUSTIFIED
because (a) the unjustified hunks are confined to two specific
locations and one minor validator drop, (b) every other hunk
traces back to a proposal/hookpoint/answer/risk-gate citation,
and (c) all three Rev 4 deletion-list categories (`zst` /
`config` / named-tests) are honored. The verdict is not
JUSTIFIED because the two retained-but-prohibited surfaces
(`find_provider_for_session` and `discover_fixture_turn_session`)
together constitute a measurable miss against the
`no-backwards-compatibility` and `no-deferred-stubs` conventions
the Phase 4 gates were graded against.

Recommended remediation before the post-implementation gate
fully closes:

- Delete `find_provider_for_session`, `ProviderSessionMatch`,
  and the four `find_provider_for_session_*` tests; reroute
  `run_repl`'s detail-line query through a chain-aware sibling
  lookup or drop the detail line altogether (the resolver
  already has the active provider).
- Delete `discover_fixture_turn_session` and adapt the
  `initiative_05_migration.rs` harness to drive ingestion
  through `sessions.toml`-configured `turn_script` adapters.
- Document the `interactive_args` validator removal in a
  follow-up commit body or proposal-amendment note.
- Drop the `#[allow(dead_code)]` annotations on the two
  helpers that have live callers.
