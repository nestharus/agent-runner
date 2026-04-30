# Phase 5 Hookpoints — Session Migration

> **Note (pre-change evidence):** All hookpoint claims below describe the
> codebase state **after** Initiative 04 lands but **before** Initiative
> 05. Provider-quota `exhausted_at` schema, `mark_exhausted`, the
> exhausted-filter inside `select_provider`, and the
> `score_by_density` projection body are all present
> (`src-tauri/src/state/db.rs:389-397`,
> `src-tauri/src/state/db.rs:646-653`,
> `src-tauri/src/balancer/mod.rs:78-119`,
> `src-tauri/src/balancer/mod.rs:121-244`). 05 layers chain identity on
> top of that surface and replaces `find_provider_for_session` (call
> sites at `main.rs:669` and `main.rs:843`).

## 1. Schema migration hookpoints

- `CREATE TABLE IF NOT EXISTS session_chains` and `session_chain_segments` placement: append both `CREATE TABLE` blocks to the schema-batch literal at `src-tauri/src/state/db.rs:377-506`, immediately after the existing `session_turns` block (closes at line 505) and before the closing `");` at line 506. Indexes `idx_segments_session` and `idx_segments_chain_active` belong in the same batch.
- `ALTER TABLE session_turns ADD COLUMN is_compaction_boundary` ensure-branch: extend `ensure_session_turns_schema` at `src-tauri/src/state/db.rs:598-617` with a third absent-column branch following the `parent_turn_id` (`:600-606`) and `is_sidechain` (`:607-613`) precedents. Use `INTEGER NOT NULL DEFAULT 0` to match the `is_sidechain` shape.
- Fresh `session_turns` schema declaration: add `is_compaction_boundary INTEGER NOT NULL DEFAULT 0` to the `CREATE TABLE` literal at `src-tauri/src/state/db.rs:493-505`, parallel to the existing `is_sidechain` column at line 501.
- Backfill loop hookpoint: insert a new private fn `backfill_session_chains(conn: &Connection)` invocation in `StateDb::open` at `src-tauri/src/state/db.rs:365-514`, after the three `ensure_*_schema` calls at `:509-511` and before `Ok(StateDb { conn })` at `:513`. Body wraps `SELECT EXISTS(SELECT 1 FROM session_chains)` gate (proposal §2) and the per-(provider, session_id) loop in one transaction.

## 2. Resolver and `find_provider_for_session()` deletion hookpoints

- Function definition: `pub fn find_provider_for_session(...)` at `src-tauri/src/state/db.rs:2062-2107`. Delete in entirety per `~/ai/conventions/no-backwards-compatibility.md`.
- Production call sites:
  - `run_repl` resume branch at `src-tauri/src/main.rs:669` (inside the Some(session_id) block at `:663-690`).
  - `run_resume` (one-shot) at `src-tauri/src/main.rs:843` (inside the resume validation block at `:820-876`).
  - No Tauri command calls the resolver (`grep find_provider_for_session src-tauri/src/lib.rs` is empty).
- Test-only call sites at `src-tauri/src/state/db.rs:3666-3776` (four `#[test]` fns) — delete or rewrite around `resolve_resume`.
- `ProviderSessionMatch` struct at `src-tauri/src/state/db.rs:133-137`: delete; replaced by `ResolvedResume` in §3.
- Mismatch error helper `resume_model_pool_mismatch_message()` at `src-tauri/src/main.rs:599-628`: keep the suggestion-builder logic but reframe the error message per proposal §4 step 6 ("active segment's owning provider"). The function is invoked from both call sites at `:697-701` and `:870-875`.
- Top-level `--resume requires --model` enforcement at `src-tauri/src/main.rs:318-321`: delete the `ok_or_else` rejection. Replace with the resolver call.

## 3. New resolver hookpoints

- `resolve_resume()` location: place inside `state/db.rs` next to the deleted `find_provider_for_session` (around `:2062`). Recommendation — keep it in `db.rs` rather than a new `resume/mod.rs`, because (a) the SQL it issues is local to chain tables and (b) the existing `StateDb` impl already exposes the `get_quota` / `count_assistant_turns_since` helpers `decide_migration` will call. Group `decide_migration` next to it for proximity.
- `ResolvedResume` struct: top of `state/db.rs` near `ProviderSessionMatch` (currently at `:133-137`), or in a new public types section just before `impl StateDb`.
- `ResumeError` enum: same file, with variants `NoChainFound`, `Ambiguous { previews }`, `ModelInferenceImpossible { hint }`, `ProviderModelMismatch { active_provider, suggestions }`, `InvalidUuid`. Mirror existing `String` error pattern in `StateDb` methods or upgrade the whole module to a `thiserror` enum (latter is cleaner; in-scope per proposal §4).
- `ChainPreview` and `TurnPreview` structs: same module, public, used by `ResumeError::Ambiguous` and `agents resume --list`.
- Call-site switchovers (post-deletion):
  - `src-tauri/src/main.rs:313-360` (top-level `--resume` block) — call `resolve_resume(&state, &models, session_id, cli.model.as_deref())` before dispatching to `run_resume` or `run_repl`.
  - `src-tauri/src/main.rs:663-690` (`run_repl` resume branch) — replace `find_provider_for_session` + post-validation block with one resolver call.
  - `src-tauri/src/main.rs:820-876` (`run_resume`) — same pattern.

## 4. Chain mint write-path hookpoints

- **Agent sessions, post-execution** (§3.1): the runner writes session_id via `update_session_capture` at three production sites in `main.rs` — `:1057-1062` (one-shot via `run_with_balancing`), `:956-963` (direct-model agent path), `:772-792` (agent-based execution). Each is preceded by `ingest_and_emit_session_id` (`main.rs:512-563`) or `emit_known_session_id` (`main.rs:565-584`). Hook the chain-mint INSERT inside `emit_known_session_id` at `:572-576`, immediately after `update_session_capture` succeeds and before the `OULIPOLY_SESSION` stderr emit. Single point of insertion covers all three production paths.
- **Resume paths** (§3.1, ON CONFLICT no-op): `update_session_capture(..., "resumed")` runs before spawn at `main.rs:752` (REPL resume, inside `run_repl`) and `main.rs:900` (one-shot resume, inside `run_resume`). The resolver opened the chain earlier in the same call, so the mint INSERT here is `ON CONFLICT DO NOTHING` — keep the same code path (hook in `emit_known_session_id` covers this only if those resume sites are routed through it; verify at implementation time and add a parallel write-path if not).
- **UI sessions, ingestion-time** (§3.1.1): the `INSERT OR IGNORE INTO session_turns` statements at `src-tauri/src/state/db.rs:1962-1974` (single-row `ingest_session_turn`) and `:1998-2014` (batch `ingest_session_turns_batch`) are the only writers for UI sessions. Both are reached via `scan_provider` in `src-tauri/src/sessions/mod.rs:58-100`. Add the existence-check + mint sequence (proposal §3.1.1) inside both insert paths, gated on `changed > 0` (single-row) and per-row in the batch loop. The provider's `default_model` lookup needs a `ProvidersConfig` reference threaded through `scan_provider` — extend its signature.
- **`last_used_at` update** (§3.3): every successful invocation tied to a chain. Hook in two places:
  - `finalize_invocation` succeeds at `main.rs:1077-1085` (one-shot), `:946-952` (direct-model), `:920-925` (agent), `:1078-1090` (resume), `:769-774` (REPL exit).
  - Cleanest: write a helper `update_chain_last_used(state, invocation_row_id)` and call it once per finalized success. The chain_id is derivable from the invocation row's session_id via `session_chain_segments`.

## 5. Sticky-then-migrate / `compute_projections` refactor hookpoints

- `score_by_density` body: `src-tauri/src/balancer/mod.rs:121-244` (proposal §5.1 acknowledges the `:121-232` typo; actual end is `:244`). The per-candidate eval loop spans `:151-232`; the eligibility filter and selection are `:234-243`.
- `ProviderEval` struct: `src-tauri/src/balancer/mod.rs:20-25`. Currently private (`struct`, not `pub struct`). Either (a) make `pub` and reuse, or (b) introduce `pub struct ProviderProjection { provider_index, projections_per_window: Vec<WindowProjection>, binding_score: Option<f64>, recent_error_count: u32 }` alongside it (proposal §5.1 names this shape).
- `WindowProjection` does NOT exist today — the per-window projection result (`projected`, `hours`, `remaining_headroom`) is computed inline at `:216-219` and dropped at the end of the iteration. Introduce as `pub struct WindowProjection { window_id: i64, projected_used: f64, hours_until_reset: f64, remaining_headroom: f64 }` and have `compute_projections` retain a `Vec<WindowProjection>` per provider.
- `compute_projections` signature: `pub fn compute_projections(model, state, ctx) -> Vec<ProviderProjection>` extracted from the refresh-then-eval flow at `:53-77` (refresh + scan loop) and `:121-232` (per-candidate scoring loop). `score_by_density` becomes a thin caller that owns only the eligibility filter and `best_binding_score` selection (`:234-243`).
- `decide_migration` location: place adjacent to `compute_projections` in `balancer/mod.rs`. Body is small (proposal §5 algorithm steps 1-7). Returns `MigrationDecision`. Imports `provider_quotas.exhausted_at` via existing `state.get_quota(&provider_name)` at `balancer/mod.rs:71`.
- `MigrationDecision` and `TransitionReason` enums: same module, `pub`. `TransitionReason` mirrors the `CHECK` constraint values in §2's segment schema literal.
- `migration_threshold` field: add to `ModelConfig` at `src-tauri/src/config/model.rs:214-221` as `pub migration_threshold: f64` with `#[serde(default = "default_migration_threshold")]`. Parse from `[migration]` block in `RawModelToml` at `:286-297` (add `migration: Option<RawMigration>` field) and the `from_toml` flatten at `:533-580`.

## 6. Migration mechanic hookpoints (§6)

- JSONL source resolution: `transcript_locator` invocation already exists in `trace::build_trace_session` at `src-tauri/src/sessions/mod.rs:164` (where `entry.transcript_locator` is dispatched). Factor the locator-call into a reusable `pub fn locate_transcript(provider_name, session_id, sessions_cfg) -> Result<PathBuf, ...>` helper inside `sessions/mod.rs`, and call from both the existing trace path and the new migration path. Codex glob fallback is also implemented there — same helper.
- cwd_hash extraction: pure Rust — `path.parent().and_then(|p| p.file_name())`. No new code site beyond the migration helper.
- Atomic copy paths (`std::fs::rename`): new code in a new `migration/mod.rs` (recommended; otherwise inside `executor/cli.rs` near `execute_resume` at `:400`). The migration step runs **before** `compose_resume_args` in the resume path, since the target session_id (and target absolute path) is what feeds into `compose_resume_args`.
- Codex migration is deferred to a follow-up PR per `research/05-codex-resume-verification.md` and proposal §15. Do not add a Codex compressed-transcript copy path in v1; the migration helper returns `MigrationError::CodexMigrationDeferred` when a Codex source or target would otherwise enter the mechanic.
- Compaction-anchor lookup SQL: new `StateDb` method `latest_compaction_boundary(provider_name, session_id) -> Option<(turn_id, timestamp)>` next to `find_session_for_invocation_window` at `src-tauri/src/state/db.rs:2109` (the cluster of session-related queries).
- Segment-close `UPDATE ... RETURNING id` (§3.2): new `StateDb` method, same neighborhood. Open a new segment via existing transaction primitives (`unchecked_transaction` precedent at `:1992-1995`).

## 7. `compose_resume_args()` extension hookpoint

- Function signature change: `src-tauri/src/executor/cli.rs:246-274`. Add `target_jsonl_path: Option<&Path>` parameter for the deferred Codex migration follow-up, but do not add a new resume strategy arm in v1. Existing `Flag` and `Subcommand` behavior stays unchanged, and config-kind TOML must reject during validation per proposal §7 and §11.
- `ResumePayload` struct at `src-tauri/src/executor/cli.rs:241-244` gains a parallel field (or callers pass the path separately).
- Call sites:
  - `src-tauri/src/executor/cli.rs:410` inside `execute_resume` (one-shot resume).
  - `src-tauri/src/executor/cli.rs:528` inside `execute_interactive` (interactive resume).
- Codex path-resume/config strategy work is deferred to a follow-up PR per `research/05-codex-resume-verification.md` and proposal §15; v1 does not introduce config-key plumbing.

## 8. `is_compaction_boundary` ingest plumbing hookpoints

- `ScriptTurn` at `src-tauri/src/sessions/mod.rs:33-43`: add `#[serde(default)] pub is_compaction_boundary: Option<bool>,` parallel to `parent_turn_id` (`:39-40`) and `is_sidechain` (`:41-42`). `#[serde(default)]` already covers absent-field tolerance — confirmed by `risk/05-audit.md` §8.
- `SessionTurnIngest` at `src-tauri/src/state/db.rs:117-124`: add `pub is_compaction_boundary: bool,` parallel to the existing `is_sidechain: bool` (`:123`). Default from `Option<bool>.unwrap_or(false)` at the `ScriptTurn`→`SessionTurnIngest` boundary in `sessions/mod.rs` (currently around `:119` per audit).
- Single-row INSERT at `src-tauri/src/state/db.rs:1962-1974`: extend column list to 8 columns; add `is_compaction_boundary` to params. Note: today this insert does NOT bind `parent_turn_id` or `is_sidechain` — only the batch path does. Decide whether to extend single-row to match the batch path (recommended) or scope this PR to adding only `is_compaction_boundary` to both.
- Batch INSERT at `src-tauri/src/state/db.rs:1998-2014`: extend column list (currently 9 columns including the literal `''` source_file at `:2012`) and add the bind in the param tuple at `:2016-2024`.

## 9. Provider config hookpoints

- `[providers.session_storage]` parser: extend `RawProvider` at `src-tauri/src/config/model.rs:299-309` with `session_storage: Option<SessionStorage>`. Mirror at `RawModelToml` (`:286-297`) for top-level provider declarations. Include in the `from_toml` flatten at `:533-580` and in the `to_toml` emitter (the `append_*_toml` helpers around `:404-470`).
- `SessionStorage` enum: define in `config/model.rs` near `ResumeStrategy`/`SessionCapture` (`:48-163`), tagged-union via `#[serde(tag = "kind", rename_all = "snake_case")]` with variants `ClaudeCode { projects_dir: PathBuf }` and `Codex { sessions_dir: PathBuf }`. Implement `validate()` parallel to `ResumeStrategy::validate` (`:64-87`).
- `ProviderConfig` struct at `src-tauri/src/config/model.rs:7-21`: add `pub session_storage: Option<SessionStorage>,` field; update `ProviderConfig::new` default at `:24-37`.
- `default_model` in `providers.toml`: extend `ProviderEntry` at `src-tauri/src/config/providers.rs:8-19` with `pub default_model: Option<String>,`. Extend `RawEntry` at `:26-32` with the same field plus `#[serde(default)]`. Map through in the `entries` constructor at `:44-55`.
- Validation: at `ProvidersConfig::load` (`src-tauri/src/config/providers.rs:36-57`) — proposal §9.2 requires that if `default_model` is set, it must name a model present in the models directory. Cross-config validation belongs at the model-load site (e.g. `lib.rs` startup or `main.rs:load_models`); hook there rather than inside `ProvidersConfig::load` (which doesn't see the models dir).

## 10. CLI surface hookpoints

- `agents migrate-db` subcommand: extend the `Subcommands` enum at `src-tauri/src/main.rs:72-141` with a `MigrateDb` variant (no args). Dispatch in the match at `:265-307` to a new `run_migrate_db(state)` fn. Body invokes the same `backfill_session_chains` helper as `StateDb::open`.
- `--migrate <provider>` flag: add `#[arg(long = "migrate")] migrate: Option<String>` to (a) the top-level `Cli` struct at `:23-70` (alongside `resume` at `:44-45`), (b) `Subcommands::Repl` at `:99-114`, (c) `Subcommands::Resume` at `:115-140`. Thread through `run_repl`, `run_resume`, and the top-level dispatch as `manual_target: Option<&str>` to `decide_migration`.
- `agents resume --list <UUID>`: ambiguous — `Subcommands::Resume` already exists. Either (a) add a `--list` flag to that variant, or (b) introduce a new `Subcommands::ResumeList { uuid: String }` for clarity. Recommend (b): the `Resume` variant requires `model` and `session_id` (both currently `String`, would need to become `Option<String>`); a separate variant avoids overloading. Dispatch to a new `run_resume_list(uuid)` that calls the resolver's preview-builder and prints matching chains.
- `-m`/`--model` becomes optional:
  - Top-level `Cli.model` at `:35-36` — already `Option<String>`. Delete the `ok_or_else` at `main.rs:318-321`.
  - `Subcommands::Repl.model` at `:101` — currently `String`. Change to `Option<String>`; require non-None unless `resume` is `Some`. Validation in the dispatch at `:280-290`.
  - `Subcommands::Resume.model` at `:117-119` — currently `String`. Change to `Option<String>`; resolver fills in.

## 11. Trace integration hookpoint

- `TraceSession` struct at `src-tauri/src/trace/mod.rs:60-70`: add `pub chain_id: Option<String>,` field. Populate in `build_trace_session` (called from `:161` inside `build_trace_node`) by querying `session_chain_segments` for the segment matching the invocation's session_id.
- Human-readable trace output: add a `Chain: <chain_id>` line in the rendering path. The render fn `render_ascii_trace` is at `:138`; the per-node session block emits `Session: <UUID>` and `Resume target: <UUID>` (verify exact line at implementation time).

## 12. Adapter script hookpoint

- `scripts/claude-code-turns:57-86` is the JSONL→normalized-turn emitter. Currently filters `obj.get("type")` against `("user", "assistant")` only (`:68-70`) and drops anything else. Compaction records likely have a different `type` (Claude Code's record types include `summary`, `system`, etc.). Implementation discovery: inspect representative `~/.claude/projects/<cwd_hash>/*.jsonl` files for the compaction record shape, then extend the filter and JSON output (`:76-83`) to emit `is_compaction_boundary: true` for the matching record type. The output schema gains the optional field (no schema migration on adapter contract — `Option<bool>` with `#[serde(default)]` is forward-compat, per §8 above).
- `scripts/codex-turns`: NOT updated in this PR (proposal §15 — Codex compaction format unknown). No hookpoint here; adapter remains capable of ingestion without the flag.

## 13. README hookpoints

- `README.md:131-136` (CLI synopsis) — verified `repl <model> [--resume ...]` and `resume -m <model> --session-id ...` synopsis lines. Mark `-m, --model` optional when `--resume` is present; document `--migrate <provider>`; document `agents resume --list <UUID>`.
- `README.md:222-258` (`providers.toml` reference) — verified existing block declares `quota_script` + `auth_refresh_command` per provider. Extend each example with `default_model = "..."` line.
- `README.md:298-310` (turn-script JSONL contract) — verified existing 6-field shape (`session_id, turn_id, timestamp, role, parent_turn_id?, is_sidechain?`). Add `is_compaction_boundary?` as a 7th optional field.
- `README.md:324-336` (`transcript_locator` block) — verified existing "lazy at trace time — never at invocation time" wording. Reword to "lazy — invoked only when a chain is being inspected (`trace`) or migrated (`resume` with cross-provider migration)." Cross-link to migration §6.
- `README.md:417-477` ("Resuming a session" subsection) — verified existing wording centered on per-session ownership and `[providers.resume]`. Replace with chain-aware language per proposal §12.
- `README.md:467-475` (resume failure-modes enumeration) — verified existing four-bullet list (`No session found`, `Invalid session UUID`, `Provider/model mismatch`, `Provider has no [providers.resume] block`). Add `ResumeError::Ambiguous` and `ResumeError::ModelInferenceImpossible` per proposal §4.5 fallback chain. Reframe `Provider/model mismatch` to reference "active segment's owning provider."

## 14. Non-hookpoints — what NOT to touch

- `score_by_density` projection math at `src-tauri/src/balancer/mod.rs:151-232` is **moved**, not changed. The hidden-window penalty (`:178-196`), per-window projection (`:198-221`), bootstrap cascade (via `bootstrap_burn_rate` at `:211`), and the eligibility filter + `best_binding_score` (`:234-243`) all stay bit-for-bit. Pin via the existing balancer test suite (kept by initiative 04).
- Initiative 04's `provider_quotas.exhausted_at` schema (`:389-397`, `:646-653`), `mark_exhausted` write path (`main.rs:1071-1075`), and the exhausted filter inside `select_provider` (`balancer/mod.rs:78-108`) are read-only consumers for §5 step 3. Do not modify.
- `session_turns` ingestion code paths beyond the §3.1.1 mint hook and the §8 column extension stay as-is. The `UNIQUE(provider_name, session_id, turn_id)` constraint at `:504` continues to make ingestion idempotent.
- `trace --json` existing fields (`TraceSession.id`, `transcript_path`, `transcript_state`, `turn_count`, `assistant_turn_count`, `sidechain_turn_count`, `resume_acceptance` at `:60-70`) stay unchanged. `chain_id` is purely additive.
- `compose_resume_args`'s existing `Flag` and `Subcommand` arms (`executor/cli.rs:250-271`) are unchanged. No `Config` arm is added in v1; Codex migration is deferred per `research/05-codex-resume-verification.md` and proposal §15.
- Frontend / Tauri command surface: no Tauri command currently calls `find_provider_for_session` (verified — empty grep in `src-tauri/src/lib.rs`), so the resolver migration does not touch any `#[tauri::command]` handler. PoolsView/StatusView remain unchanged per proposal §13.
