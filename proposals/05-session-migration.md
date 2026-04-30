# 1. Scope statement (Rev 5)

Initiative 05 introduces a session-chain abstraction that decouples conversation identity from the upstream provider's session_id, so a single logical conversation can move across provider accounts (e.g. `claude` → `claude2`) without renaming or losing history. The PR adds two SQLite tables (`session_chains`, `session_chain_segments`), per-provider runtime config in `providers.toml`, a Claude-Code transcript-copy step in the executor, a best-on-resume policy keyed off initiative 04's projection scoring, and lifts `--resume`'s `--model` requirement by inferring the model from the chain's invocation history or falling through to provider-only spawn. This ships as one PR because chain identity, the resolver, and the Claude migration mechanic are mutually dependent: schema, write paths, resolver, executor, and CLI all participate in the same data flow; splitting them produces dead intermediate code with the same shape as the rejected splits in initiative 04.

Initiative 05 depends on initiative 04 — the migration trigger reads `provider_quotas.exhausted_at` and reuses initiative 04's per-window projection. Land 04 first.

**Rev 2 changes** (in response to risk gates Rev 1):

- §3.4 added: `is_compaction_boundary` ingest-path plumbing enumerated (`ScriptTurn`, `SessionTurnIngest`, INSERT statements at `state/db.rs:1962-1974` and `1998-2014`).
- §5 Step 4: replaced ambiguous "run `score_by_density`" with explicit refactor commit — factor out `compute_projections(model, state, ctx) -> Vec<ProviderProjection>` that both `score_by_density` and `decide_migration` call.
- §6.1: cwd_hash derived from source JSONL's parent directory name, not by replicating Claude Code's encoder. No new adapter required.
- §6.5: explicit `.zst` handling — `zstd` crate dependency declared; decompress-to-mem → slice → recompress-to-tmp → rename order specified; failure modes enumerated. Superseded by Rev 4: Codex migration is deferred, so `zstd` is not added in v1.
- §6.6 step 3: missing JSONL line for a recorded compaction boundary returns `MigrationError::CompactionBoundaryNotInJsonl` — no silent offset=0 fallback.
- §6 cross-references: "§6.4 / §6.5 / §6.9" replaced with "step 4 / step 5 / step 10 of §6" since the linear list inside §6 is not subsection-anchored.
- §11: added `chain_last_used_at_updates_after_successful_invocation` and `migration_errors_when_compaction_boundary_not_in_jsonl`.
- §12: README scope extended with `:131-136` (CLI synopsis where `-m` becomes optional) and `:467-475` (resume failure-mode list gains `Ambiguous` and `ProviderNotConfigured` variants).
- §14: backfill-performance escape hatch revised — chain-aware code paths do NOT fall back to `find_provider_for_session()`. Backfill is mandatory at first open; if perf exceeds the 2s budget, ship the backfill as a one-shot `agents migrate-db` step but never as a runtime fallback (per `~/ai/conventions/no-backwards-compatibility.md`).

**Phase 3 compliance amendments (post-Rev-2-LOW)**: §1.1 assumption register validated and extended from problem-map §7; §1.2 net-value statement; §11.1 test-intent track grouping the §11.2 test list by change-risk theme; §13.1 supported-surface track covering deployment, cohort, adjacent paths, migration, rollback, observability.

**Rev 4 changes** (Codex migration deferral):

- §6 Step 1 / Step 3: Codex providers (`kind = "codex"` in `[providers.session_storage]`) remain declarable but cannot be migration sources or targets in v1. If `--migrate` or the best-on-resume policy reaches the migration mechanic for a Codex chain, return `MigrationError::CodexMigrationDeferred { provider }`.
- §6 Step 5 / §6.6: Codex `.zst` migration is deferred; only plaintext Claude-Code JSONL copy ships in v1, so the `zstd` crate dependency is NOT added.
- §7: drop `kind = "config"` / `experimental_resume`. Keep `kind = "flag"` for Claude and `kind = "subcommand"` for Codex one-shot/REPL fresh-session resume. `compose_resume_args()` still gains `target_jsonl_path: Option<&Path>` for the deferred follow-up; only the Claude migration path uses the path-aware plumbing in v1.
- §9.1: `kind = "codex"` remains for forward-compatible chain identity (chain_id mint at ingestion, segment ledger, resume-by-id within the same provider), but is ignored for migration in v1 pending a documented Codex path-resume mechanism.
- §11 / §13.1: remove Codex `.zst` and `experimental_resume` tests/surface; add Codex-deferred mechanic coverage and Codex chain-identity coverage.
- §15: replace the Codex compaction-format migration residual with a broader Codex migration deferral entry citing `research/05-codex-resume-verification.md`.

**Rev 5 changes** (policy fix):

- §5: migration policy reframed as best-on-resume. At every resume, pick the best-ranked sibling provider with session storage and migrate if it differs from the active segment's provider; drop the threshold gate entirely. Rev 6 changes the ranking metric from binding score to lowest load.
- Per user feedback, resume is rare and happens between invocations, not per turn, so thrashing is not a concern. Cache continuity rarely benefits because agents fan out and miss cache anyway.
- Drop `migration_threshold` from `ModelConfig` and remove `[migration] threshold = ...` parsing/emission entirely.

**Rev 6 changes** (resume ranking fix):

- §5: best-on-resume still reuses `compute_projections` for projected usage, but ranks providers by `max(projected_used)` across visible windows, lowest first. It does not use `binding_score`; that remains specific to active load balancing where reset timing matters.
- Providers with no learned windows have an empty projection vector and count as load `0.0`, preserving their existing eligibility.

## 1.1 Assumption register

This is the approved register validated and extended from `research/05-session-migration-problem-map.md` §7. It replaces the draft register there for Phase 3/4 review; do not maintain a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | Claude Code `--resume <UUID>` replays local JSONL state rather than requiring server-side session state, and migration must reuse the source UUID on the target side. | Locked answer Q2 says the JSONL is the source of truth and cites Claude Code session docs plus `claude --help` (`research/05-session-migration-answers.md:25-36`); its first sentence verifies `~/.claude2/projects/<hash>/<UUID>.jsonl` plus `--resume <UUID>` works. Live QA empirically rejected the safer-practice idea of minting a new target UUID because Claude Code compares `--resume` against embedded JSONL `sessionId` fields. | A Claude Code release or observed session where copied JSONL plus `--resume` cannot continue without server-side state. | §6 migration mechanic, §7 `flag` resume path, §9.1 `claude_code` storage, §11.1 migration tests. |
| A2 | Resume is a rare, between-invocation event. Picking the least-loaded provider at resume entry does not cause thrashing. | Live user feedback for Rev 5: resume is not a per-turn operation, and agents fan out enough that cache stickiness rarely buys continuity. | Observed resume workflows that repeatedly bounce the same chain between providers in a way that materially harms reliability or cost. | §5 best-on-resume policy, §12 README migration explanation, §14 cross-org cache residual, §1.2 net-value statement. |
| A3 | `session_turns` ingestion captures every adapter-emitted turn soon enough for resolver, quota projection, and compaction-boundary decisions. | Problem-map notes `select_provider` scans providers before scoring and scripts use cursors (`research/05-session-migration-problem-map.md:133-135`); hookpoints identify `scan_provider` and both DB insert paths (`research/05-session-migration-hookpoints.md:45-52`, `research/05-session-migration-hookpoints.md:84-89`). | Adapter cursor bugs, delayed/batched emission, skipped malformed lines, partial writes, or script failures that leave the DB missing turns the proposal relies on. | §3.1.1 UI chain mint, §3.3 last-used updates, §4 resolver previews, §6.6 compaction-aware copy, §10 trace counts. |
| A4 | `score_by_density` can be extracted into `compute_projections` without changing provider selection. | Problem-map identifies the local projection body and existing density tests (`research/05-session-migration-problem-map.md:136`); hookpoints locate the exact extraction surface and non-hookpoint keep-list (`research/05-session-migration-hookpoints.md:54-63`, `research/05-session-migration-hookpoints.md:128-131`). | Any branch reorder, tie behavior change, hidden-window penalty change, bootstrap/fallback change, or balancer test regression after extraction. | §5.1 refactor, §5 migration decision, §13/§13.1 blast-radius notes, §11.1 projection-equivalence tests. |
| A5 | First-open backfill is acceptable when run in one transaction, with `agents migrate-db` as the explicit retry/foreground path if user-visible delay or write failure occurs. | Problem-map calls out synchronous open-path risk and backfill data sources (`research/05-session-migration-problem-map.md:107-117`); Rev 2 risk gates accepted the mandatory-backfill/no-fallback design (`risk/05-audit.md`, `risk/05-shortcut.md`). | Representative user DBs, slow I/O, locks, or write failures make startup backfill too slow or unreliable without a different migration path. | §2 backfill, §8.5.1 `agents migrate-db`, §14 backfill risk, §13.1 migration/rollback. |
| A6 | Claude Code compaction records can be identified well enough for `claude-code-turns` to emit `is_compaction_boundary`. | Locked answer Q8 defines the compaction-aware strategy and confidence (`research/05-session-migration-answers.md:131-150`); hookpoints identify the reference adapter update site (`research/05-session-migration-hookpoints.md:114-117`). | Claude Code JSONL record drift, multiple incompatible compaction formats, missing turn IDs, or summaries that cannot be replayed cleanly from a line boundary. | §3.4 ingest plumbing, §6.6 compaction-aware target build, §9.1.1 adapter contract, §15 Claude compaction unresolved. |
| A7 | Codex cross-account file-copy migration is not verified for v1 because the CLI has no documented path-resume surface. Codex chain identity still works through ingestion and same-provider resume-by-id. | Rev 4 verification found `experimental_resume` is not documented or working, `codex resume <SESSION_ID>` requires the target HOME's `state_5.sqlite`, and `ThreadResumeParams.path` is internal-only (`research/05-codex-resume-verification.md`). | Codex exposes a documented path-resume/import mechanism, or a later PR deliberately implements a state-DB-aware migration path. | §6 Codex deferred guard, §9.1 `codex` storage identity-only note, §11 Codex deferral/identity tests, §15 Codex migration deferred residual. |
| A8 | UI-started sessions have no reliable per-session model provenance, so the runner must avoid inventing one and let the upstream CLI default apply when no model can be inferred. | Locked answer Q4 distinguishes agent vs UI sessions; problem-map notes direct CLI sessions lack invocation model provenance (`research/05-session-migration-problem-map.md:100`). | Upstream CLI exposes reliable per-session model metadata through ingestion. | §3.1.1 UI mint, §4 model resolution, §8.6 resume without `-m`, §12 README. |

## 1.2 Net-value statement

Yes: the proposal clearly reduces concrete current-state risk on the supported resume/session surface. It addresses the resume cliff when a Claude-Code owning provider approaches or hits quota, the current inability for UI-started sessions to resume through agent-runner without model/provider guesswork, ambiguous same-UUID ownership, and long-session context-overflow risk from naive full-JSONL copying (`research/05-session-migration-problem-map.md:40-76`, `research/05-session-migration-problem-map.md:92-105`, `research/05-session-migration-problem-map.md:119-128`). The reduction is broad for Claude-style same-CLI migrations, Codex chain identity, and model inference; residuals remain for Codex cross-account migration, cross-CLI migration, transcript-preview snippets, and cache isolation across org/workspace boundaries (§15, §14). The added blast radius is real but bounded: first-open backfill, additive chain tables/column, Claude JSONL copy failures, `compute_projections` refactor risk, and concurrent segment-close races. Migration and rollback burden is low because schema changes are idempotent/additive, `agents migrate-db` runs the same backfill explicitly, and prior binaries ignore the new tables/column. The net judgment is positive: the current resume/migration risk reduction outweighs the added blast radius and the low migration/rollback burden, contingent on Phase 4 supported-surface review validating §13.1 and the assumptions above.

# 2. Schema migration

Match initiative 04's schema-ensure pattern (`src-tauri/src/state/db.rs:522-558`). Two new tables, both unconditional `CREATE TABLE IF NOT EXISTS`:

```sql
CREATE TABLE IF NOT EXISTS session_chains (
    chain_id     TEXT PRIMARY KEY,
    created_at   TEXT NOT NULL,
    last_used_at TEXT NOT NULL,
    model_name   TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS session_chain_segments (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    chain_id          TEXT NOT NULL REFERENCES session_chains(chain_id),
    provider_name     TEXT NOT NULL,
    session_id        TEXT NOT NULL,
    started_at        TEXT NOT NULL,
    ended_at          TEXT,
    last_turn_id      TEXT,
    transition_reason TEXT NOT NULL CHECK (transition_reason IN
        ('initial', 'manual', 'quota_threshold', 'exhausted', 'imported')),
    UNIQUE(chain_id, provider_name, session_id)
);

CREATE INDEX IF NOT EXISTS idx_segments_session
  ON session_chain_segments(session_id);
CREATE INDEX IF NOT EXISTS idx_segments_chain_active
  ON session_chain_segments(chain_id, ended_at);
```

`ended_at IS NULL` is the active-segment marker. `last_turn_id` is the turn_id of the latest observed turn on this segment, written when the segment closes.

Add a column to the existing `session_turns` table (idempotent, matches initiative 04's pattern at `src-tauri/src/state/db.rs:611-622`):

```sql
ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0;
```

Update fresh `session_turns` schema declaration (`src-tauri/src/state/db.rs:493-505`) to include the column.

This flag marks turns that represent the upstream CLI's compaction-summary records — i.e. when the CLI replaced a span of earlier turns with a single summarizing turn to fit the context window. Used by §6.6 to build a truncated target JSONL during migration.

**Backfill** runs once at `StateDb::open` after table creation, gated by `SELECT EXISTS(SELECT 1 FROM session_chains)` returning 0:

For each distinct `(provider_name, session_id)` in `session_turns`:

1. Mint `chain_id = uuid_v4()` in Rust (SQLite's `randomblob(16)` is not a valid UUID v4).
2. Insert `session_chains` row with `created_at = MIN(timestamp)`, `last_used_at = MAX(timestamp)`, and `model_name` from the most recent `invocations` row tied to that session_id (fallback `'<unknown>'` if none).
3. Insert one `session_chain_segments` row with `transition_reason = 'imported'`, `started_at = MIN(timestamp)`, `ended_at = NULL`, `last_turn_id` = turn_id at MAX(timestamp).

Wrap the entire backfill in one transaction. Benchmark on a representative DB before merge — if the user has >100K `session_turns` rows, the inserts must complete under ~2s; otherwise gate behind a one-time migration flag and ship as a separate PR step.

After backfill, every previously-ingested (provider, session_id) is its own chain. Existing call sites that today walk `session_turns` directly continue to work because the resolver's first lookup falls through to a one-segment chain.

# 3. Data write paths

## 3.1 Mint chain on first session_id capture (agent sessions)

Hook: after `set_session_id` (or equivalent) writes session_id to the in-progress invocation row in `executor::execute()` and `executor::execute_interactive()` paths. Identify the exact site during implementation — the survey notes that `session_capture_method` and `session_id` are written post-execution to `invocations`.

Sequence (one transaction):

```sql
INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
VALUES (?, ?now, ?now, ?model_name_from_invocation)
ON CONFLICT DO NOTHING;

INSERT INTO session_chain_segments
    (chain_id, provider_name, session_id, started_at, transition_reason)
VALUES (?, ?, ?, ?now, 'initial')
ON CONFLICT DO NOTHING;
```

The `ON CONFLICT DO NOTHING` handles the resume case: a chain already exists for this (provider, session_id) pair (because the resolver opened it earlier in the same call). No-op.

The mint is keyed on (provider, session_id) — the natural unique pair from the upstream CLI's perspective. The chain_id is opaque and not surfaced unless the user runs `agents resume --list` or `trace --json`.

**Recording model at mint** is the central value-add for agent sessions: the next `agents --resume <UUID>` call resolves to the same model without the caller passing `-m`. This eliminates the model arg from agent-driven workflows entirely — agents resume sessions by id alone and inherit the model their original invocation used.

## 3.1.1 Mint chain on first ingestion observation (UI sessions)

Sessions started outside agent-runner (a user running `claude` or `codex` in a terminal directly) surface only via `session_turns` ingestion. They have no `invocations` row, and the runner does not know which model the upstream CLI used.

Hook: in the `session_turns` ingestion code path (the writer that consumes turn-script stdout), after `INSERT OR IGNORE INTO session_turns ...` succeeds for a new `(provider_name, session_id)` pair, check if a chain exists:

```sql
SELECT 1 FROM session_chain_segments
WHERE provider_name = ? AND session_id = ?
LIMIT 1;
```

If absent, mint with the same shape as §3.1 and `model_name = '<unknown>'`:

```sql
INSERT INTO session_chains (chain_id, created_at, last_used_at, model_name)
VALUES (?, ?turn_timestamp, ?turn_timestamp, '<unknown>');

INSERT INTO session_chain_segments
    (chain_id, provider_name, session_id, started_at, transition_reason)
VALUES (?, ?, ?, ?turn_timestamp, 'imported');
```

The resolver's fallback chain (§4.5) will then either use an explicit/inferred model or fall through to the upstream provider CLI's built-in default model.

Migration of UI sessions is identical to agent sessions for the same provider family — same `[providers.session_storage]` layout, same chain ledger, and for Claude Code the same copy mechanic. Codex UI sessions still mint chain identity through ingestion, but Codex cross-account file-copy migration is deferred in v1 (§15). The only distinction between agent and UI sessions is the model resolution fallback.

## 3.2 Close active segment on migration

At migration time (§6), before opening the next segment:

```sql
UPDATE session_chain_segments
SET ended_at     = ?now,
    last_turn_id = (
        SELECT turn_id FROM session_turns
        WHERE provider_name = (SELECT provider_name FROM session_chain_segments WHERE id = ?id)
          AND session_id    = (SELECT session_id    FROM session_chain_segments WHERE id = ?id)
        ORDER BY timestamp DESC LIMIT 1
    )
WHERE id = ?id
  AND ended_at IS NULL
RETURNING id;
```

If `RETURNING` is empty, another concurrent migration beat us — abort this migration and re-resolve (§14).

If `session_turns` has no rows for this segment yet (capture happened but ingestion hasn't run), `last_turn_id` is `NULL`. Acceptable; the time boundary is still recorded.

## 3.3 Update last_used_at after every successful invocation tied to a chain

```sql
UPDATE session_chains SET last_used_at = ?now WHERE chain_id = ?;
```

Used by the 24-hour disambiguation window in the resolver.

## 3.4 `is_compaction_boundary` ingest-path plumbing

The new column on `session_turns` is touched by more than schema-ensure. Three additional sites:

- `ScriptTurn` struct in `src-tauri/src/sessions/mod.rs:33-43`: add `pub is_compaction_boundary: Option<bool>` (parallel to existing `parent_turn_id: Option<String>` and `is_sidechain: Option<bool>`). Serde already tolerates absent fields via `Option`; today's adapter scripts that don't emit the field continue to deserialize cleanly.
- `SessionTurnIngest` (the writer-side struct that crosses the parse → DB boundary): add `pub is_compaction_boundary: bool` (defaulting to `false` when `ScriptTurn.is_compaction_boundary.is_none()`).
- The two `INSERT OR IGNORE INTO session_turns` SQL statements at `src-tauri/src/state/db.rs:1962-1974` (single-row insert) and `1998-2014` (batch insert): extend the column list and bind the new value.

The schema's `DEFAULT 0` on the column is necessary but not sufficient — without these struct/SQL changes, the runner ingests turns and silently drops the boundary signal. This must be wired in this PR or §6.6's truncation never observes a boundary in production.

# 4. Resolver

Extend or wrap the existing `find_provider_for_session()` (`src-tauri/src/state/db.rs:2062-2107`) into a new `resolve_resume` API. The old function is replaced (no compatibility shim — `~/ai/conventions/no-backwards-compatibility.md`).

```rust
pub struct ResolvedResume {
    pub chain_id: String,
    pub model_name: Option<String>,
    pub model: Option<ModelConfig>,
    pub active_provider: String,
    pub active_session_id: String,
}

pub fn resolve_resume(
    state: &StateDb,
    user_input: &str,           // session_id or chain_id, full UUID
    model_override: Option<&str>,
) -> Result<ResolvedResume, ResumeError>
```

Algorithm:

1. **Validate input**: must parse via `uuid::Uuid::try_parse`. Reject prefix matching (today's `repl --resume` rejects partial UUIDs at `main.rs:469`).

2. **Find candidate chains**:
   ```sql
   SELECT DISTINCT chain_id FROM session_chain_segments
   WHERE session_id = :input OR chain_id = :input
   ```

3. **Disambiguate**:
   - 0 chains → `ResumeError::NoChainFound { input }`.
   - 1 chain → proceed.
   - >1 chains → filter by `session_chains.last_used_at >= now − 24h`. If exactly 1 remains: proceed. If 0 remain: pick max(last_used_at). If still >1: `ResumeError::Ambiguous { previews }` (§4.1).

4. **Resolve active segment**:
   ```sql
   SELECT provider_name, session_id FROM session_chain_segments
   WHERE chain_id = :chain_id AND ended_at IS NULL
   ORDER BY started_at DESC LIMIT 1
   ```
   If no active segment exists (defensive — shouldn't happen): pick the segment with max(started_at).

5. **Resolve model** (user input → invocation history → chain model → CLI default):
   - If `model_override` is `Some(m)`: use `m`.
   - Else: SELECT `model_name` FROM `invocations` WHERE `session_id` IN (chain's segment session_ids) ORDER BY `created_at` DESC LIMIT 1. If non-empty: use it. (Hits for any session that has ever been agent-mediated, including UI sessions resumed once through agent-runner.)
   - Else: use `session_chains.model_name`. If non-`'<unknown>'`: use it. (Hits for agent sessions that minted with a known model.)
   - Else: return success with `model_name = None` and `model = None`. The spawn path will use the active provider's CLI command shape without any `--model`/`-m` override and let the CLI choose its built-in default model.

6. **Validate provider/model pool inclusion** only when `model` is `Some`: the active provider must appear in `model.providers`. If not, return `ResumeError::ProviderModelMismatch { active_provider, suggestions }`. Suggestions are model names whose pool includes `active_provider`. When `model` is `None`, skip this check; the spawn path performs the provider lookup.

7. Return `ResolvedResume`.

## 4.1 Ambiguous chain previews

When >1 chain remains after the 24h filter, build:

```rust
pub struct ChainPreview {
    pub chain_id: String,
    pub last_used_at: DateTime<Utc>,
    pub active_provider: String,
    pub active_session_id: String,
    pub turn_count: usize,
    pub recent_turns: Vec<TurnPreview>,    // last 3, oldest-first
}

pub struct TurnPreview {
    pub role: String,                 // 'user' | 'assistant'
    pub timestamp: DateTime<Utc>,
    pub snippet: Option<String>,      // first 120 chars; None in v1
}
```

Snippet content is `None` in v1 (deferred to a follow-up that adds a `transcript_preview` adapter pattern alongside the existing turn/quota adapters). User disambiguates by recency and provider — adequate for the rare case of two chains sharing a UUID.

`ResumeError::Ambiguous` rendering on stderr (always exit 1 — never auto-pick on ambiguity):

```
[resume] session 9e69e8cc-... matches 2 chains:
  chain a1b2c3d4 — last used 2026-04-29T14:02 — claude2 — 12 turns
  chain f5e6d7c8 — last used 2026-04-29T11:18 — claude  — 4 turns
Re-run with: agents resume <chain_id> ...
```

# 5. Best-on-resume policy

After `resolve_resume` succeeds, the executor decides whether to stay or migrate.

When `ResolvedResume.model` is `None`, the spawn path first picks the
lexicographically first model TOML whose provider pool contains the active
provider, then runs this same migration policy against that pool. This preserves
manual `--migrate` and best-on-resume behavior for UI-only sessions while still
spawning the final provider command without a model override.

```rust
pub enum MigrationDecision {
    Stay,
    Migrate { target_provider_index: usize, reason: TransitionReason },
}

pub enum TransitionReason {
    Initial,
    Manual,
    QuotaThreshold,
    Exhausted,
    Imported,
}

pub fn decide_migration(
    state: &StateDb,
    model: &ModelConfig,
    resolved: &ResolvedResume,
    manual_target: Option<&str>,
) -> Result<MigrationDecision, MigrationError>
```

Algorithm:

1. If `manual_target` is `Some(name)`: validate name is in `model.providers` and has `[providers.session_storage]` declared. Return `Migrate { reason = Manual }`.
2. Look up the active provider's index in `model.providers`. If absent: caller already handled the mismatch via the resolver's pool validation step — defensive return `Stay`.
3. Read `provider_quotas.exhausted_at` for the active provider. If non-NULL: pick the lowest-load sibling with `[providers.session_storage]` declared; return `Migrate { reason = Exhausted }`. If no eligible sibling: `Stay`.
4. Call `compute_projections(model, state, ctx) -> Vec<ProviderProjection>` (new helper — see §5.1) which performs the same refresh-then-evaluate flow `score_by_density` uses today (`src-tauri/src/balancer/mod.rs:93-121`) and returns the per-provider projection vector instead of selecting a single index.
5. For each storage-backed provider, compute resume load as `max(projected_used)` across `projections_per_window`. If the projection vector is empty because the provider has no learned windows, treat load as `0.0`. Tie-break by lowest provider index. Call the lowest-load provider `best`.
6. If `best.provider_index == active_provider_index`: `Stay`; the active provider is already the least loaded.
7. Else: `Migrate { target_provider_index: best.provider_index, reason: QuotaThreshold }`. The historical `quota_threshold` reason string is retained, but it now means "active provider is not the least-loaded provider at resume entry."

Single-provider models always `Stay`. Pools where no provider has `[providers.session_storage]` always `Stay` (no migration target available).

Rev 4 v1 limitation remains in the migration mechanic: `kind = "codex"` is declarable for chain identity, but if policy or `--migrate` routes through a Codex source or target, §6 returns `MigrationError::CodexMigrationDeferred { provider }` before writing files or chain segments.

## 5.1 `compute_projections` refactor

`score_by_density` (`src-tauri/src/balancer/mod.rs:121-232`, exact range to confirm at implementation time) currently builds a local `evals: Vec<ProviderEval>` and returns a single `usize` (the selected provider index). The projection data is dropped at function exit.

Initiative 05 needs the projection data without selecting. Refactor:

- Extract the refresh + per-provider evaluation loop into `pub fn compute_projections(model, state, ctx) -> Vec<ProviderProjection>` where `ProviderProjection` carries `{ provider_index, projections_per_window: Vec<WindowProjection>, binding_score: Option<f64>, recent_error_count: u32 }`. The function does NO selection.
- `score_by_density` becomes a thin caller: `compute_projections` → `argmax(binding_score)` → fall through to invocation-count if all-unlearned (preserving existing behavior).
- `decide_migration` calls `compute_projections` directly and ranks storage-backed providers by `max(projected_used)` across windows, lowest first. It intentionally ignores `binding_score`; binding score remains the active load-balancing metric because it incorporates time-to-reset pressure.

This is a refactor, not a behavior change — `score_by_density`'s output is bit-for-bit identical. Pin with the existing balancer test suite (kept by initiative 04). The refactor lives in the same file as §3.8's keep-list per initiative 04, so the §13 cross-cutting note about "no incidental change to balancer projection or refresh" is satisfied: the math moves but does not change.

# 6. Migration mechanic

When `decide_migration` returns `Migrate { target_provider_index, reason }`:

1. **Locate source JSONL** via active provider's `[providers.session_storage]`:
   - `kind = "claude_code"`: source path resolution — first call the active provider's `[transcript_locator]` (existing `sessions.toml` adapter) which already returns the absolute path. If no locator configured, fall back to globbing `<projects_dir>/*/<session_id>.jsonl` (single match expected). The `cwd_hash` is then **read from the located file's parent directory name** — no encoder is needed in this PR. Step 3's target path reuses this same `cwd_hash` verbatim because the project directory is unchanged across the migration.
   - `kind = "codex"`: return `MigrationError::CodexMigrationDeferred { provider }`. v1 supports Claude-Code migration only. Codex chain identity is preserved (segments are still recorded for ingestion-observed Codex sessions, and resume-by-id within the same provider still works through Codex's native `resume` subcommand), but cross-account file copy is deferred per §15.

   If source path doesn't exist: `MigrationError::SourceMissing { provider, session_id }`. Caller surfaces as a hard error. If the located source file's parent directory cannot be extracted (e.g. malformed path): `MigrationError::SourcePathMalformed`.

   **Encoder note**: replicating Claude Code's project-path-to-cwd_hash encoding is explicitly out of scope. Migration only needs the *current* hash for the *current* session, which is already encoded in the source path on disk. New session creation under agent-runner is unchanged — the upstream CLI continues to do its own encoding when it writes new JSONLs.

2. **Reuse source session_id** on the target side. Cross-HOME UUID collisions don't happen because each Claude HOME has its own JSONL space; this matches answers Q2's first sentence which already verified `~/.claude2/projects/<hash>/<UUID>.jsonl` resolves cleanly.

3. **Compute target path** via target's `[providers.session_storage]`:
   - `kind = "claude_code"`: `<target.projects_dir>/<cwd_hash>/<source_session_id>.jsonl`. `cwd_hash` is the value extracted from the source path in step 1.
   - `kind = "codex"`: return `MigrationError::CodexMigrationDeferred { provider }`. Codex cannot be a migration target in v1.

4. **Determine compaction-anchor offset** (see §6.6). If a compaction boundary exists for the source `(provider, session_id)`, find its byte offset in the source JSONL; otherwise offset = 0. If a boundary is recorded in `session_turns` but the matching JSONL line cannot be located (turn_id mismatch, JSONL truncated, format drift): `MigrationError::CompactionBoundaryNotInJsonl { session_id, turn_id }` — **no silent offset=0 fallback** (would mask the very overflow §6.6 prevents).

5. **Copy atomically**:
   - **Plain JSONL only** (`kind = "claude_code"`): open source, seek to `offset`, write `source[offset..]` bytes to `<target_path>.tmp`, then `rename(<target_path>.tmp, <target_path>)`. POSIX rename is atomic on the same filesystem.
   - Failure modes:
     - Read/write error → delete `<target_path>.tmp`, hard error.
     - Rename error → leave `<target_path>.tmp` on disk for forensic inspection, hard error.

6. **Open new segment** (one transaction with §3.2's close):
   ```sql
   INSERT INTO session_chain_segments
       (chain_id, provider_name, session_id, started_at, transition_reason)
   VALUES (?, ?target_provider, ?source_session_id, ?now, ?reason);
   ```
   The transaction-with-RETURNING from §3.2 guarantees no concurrent migration races to write two open segments.

7. **Compose target argv** via `[providers.resume]`:
   - `kind = "flag"`: `--resume <source_session_id>` (Claude).
   - `kind = "subcommand"`: `<subcommand> <source_session_id>` (Codex one-shot/REPL fresh-session resume; not used for Codex migration in v1).

8. **Spawn target provider** with composed argv and the user's prompt. The target's `command` field already encodes the right HOME-switching env (e.g. `env -u CLAUDECODE claude2`); no new env handling needed.

9. **On success**: existing post-invocation flow records the new invocation row and (if session_capture is configured on the target) ingests turns. Chain remains coherent.

10. **On target CLI failure**: leave the new segment open; record the failed invocation. The resolver's "max(started_at)" tiebreak picks the failed segment on next resume — defensive but correct. Document that a user can re-issue `--migrate <other-provider>` to fork again. Cleanup of stale target JSONLs is deferred to a follow-up GC pass.

11. **Format normalization caveat**: first-pass implementation copies bytes unchanged within the post-compaction slice and keeps the source session_id so record-level JSONL metadata and the resume UUID agree. If a future CLI requires additional normalization, that requires a deliberate normalization PR — **do not silently sed-edit JSONL bytes**. Document in §14 as a watched failure mode.

## 6.6 Compaction-aware target build

Long sessions get compacted by the upstream CLI: a span of earlier turns is rewritten into a single summary turn so the live API request stays under the model's context window. The on-disk JSONL keeps growing, but the live conversation state is `[summary, post-compaction turns...]`. Byte-for-byte JSONL copy to the target HOME would force the target CLI to replay the full uncompacted history and get a context-overflow rejection.

The migration mechanic compensates by truncating the target JSONL to start from the latest compaction boundary.

**Procedure** (called from step 4 of §6):

1. Query for the latest compaction boundary on the source:
   ```sql
   SELECT turn_id, timestamp FROM session_turns
   WHERE provider_name = ? AND session_id = ?
     AND is_compaction_boundary = 1
   ORDER BY timestamp DESC
   LIMIT 1;
   ```
2. If no row: no compaction has occurred. Return offset = 0 (full file copy).
3. If a row: locate the matching JSONL line by scanning the source plaintext JSONL and parsing each line's JSON for the `turn_id` (or whichever field the upstream CLI uses to identify a turn — `claude-code-turns` documents that mapping). If the scan completes without finding a line whose JSON identifies as the recorded turn_id: return `MigrationError::CompactionBoundaryNotInJsonl { session_id, turn_id }`. **Never silently fall back to offset = 0** — that would feed the target the full uncompacted history, defeating the section's purpose and masking adapter-format drift as the very context-overflow bug we're preventing.
4. Return the byte offset of the start of that matching line. Step 5 of §6 uses `source[offset..]` as the bytes-to-write.

**Pre-compaction turns are NOT deleted.** The source JSONL keeps the full byte stream; `session_turns` keeps every ingested turn (already the case). After migration:

- The target HOME has a truncated JSONL representing the live conversation state.
- The source HOME has the original full JSONL untouched.
- `session_turns` retains every turn from both sides — pre-compaction turns are flagged via their position relative to `is_compaction_boundary` rows and remain queryable.

**Search/audit benefits**: a query for "what was discussed about X" can scan all `session_turns` rows for the chain regardless of compaction. Adding a `WHERE timestamp >= (SELECT MAX(timestamp) FROM session_turns WHERE chain_id = ? AND is_compaction_boundary = 1)` filter scopes to "live state only." Both views are first-class.

**Multi-compaction**: if a session has compacted multiple times, the strategy is unchanged — pick the LATEST boundary. Earlier compaction summaries are themselves part of the pre-compaction history of the latest boundary.

Codex migration is deferred in v1, so the compaction-aware target build is only exercised for Claude-Code plaintext JSONL. Codex compaction format remains subsumed by the broader deferral in §15; `codex-turns` can continue ingesting turns without `is_compaction_boundary`.

# 7. Resume strategies remain `flag` and `subcommand`

`ResumeStrategy` in `src-tauri/src/config/model.rs` stays limited to the existing supported strategies:

```rust
pub enum ResumeStrategyKind {
    Flag,
    Subcommand,
}

pub struct ResumeConfig {
    pub kind: ResumeStrategyKind,
    pub flag: Option<String>,             // for Flag
    pub subcommand: Option<Vec<String>>,  // for Subcommand
}
```

The `ConfigArgument` enum and `ResumeStrategyKind::Config` variant are NOT introduced in v1.

`compose_resume_args()` at `src-tauri/src/executor/cli.rs:246-274` gains an optional `target_jsonl_path: Option<&Path>` parameter for future use, but does not add a new strategy arm:

```rust
fn compose_resume_args(
    strategy: &ResumeConfig,
    session_id: &str,
    target_jsonl_path: Option<&Path>,
) -> Result<Vec<String>, ComposeError>
```

Update both call sites at `executor/cli.rs:410` (one-shot resume) and `:528` (interactive resume) to thread the optional target path. The parameter is reserved for the deferred Codex migration follow-up — see §15. In v1, the only migration path that passes a target path is Claude-Code JSONL copy, and the existing `flag` strategy still composes `--resume <source_session_id>`.

Codex provider example:

```toml
[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
```

Claude provider example unchanged:

```toml
[providers.resume]
kind = "flag"
flag = "--resume"
```

# 8. CLI surface changes

## 8.1 Top-level `--resume` no longer requires `--model`

Delete the explicit error at `src-tauri/src/main.rs:318-321`. Replace with the resolver. Top-level `--resume <UUID>` with no `--model`: call `resolve_resume(state, config, uuid, None)`. With `--model <m>`: pass `Some(m)` as override.

## 8.2 `resume` subcommand: `-m` becomes optional

`src-tauri/src/main.rs:116-140` — change `model: String` to `model: Option<String>`. `None` runs the resolver; `Some` overrides.

## 8.3 `repl --resume`: same treatment

`src-tauri/src/main.rs:104-114` — `model` positional becomes optional when `--resume` is present; required otherwise. Plain `repl <model>` (no resume) keeps current behavior.

## 8.4 New `--migrate <provider>` flag

On `resume` and `repl --resume` subcommands and the top-level form, accept `--migrate <provider_name>` to force migration to the named provider regardless of automatic ranking. Validates pool inclusion and `[providers.session_storage]` presence. Sets `transition_reason = 'manual'`.

## 8.5 New `agents resume --list <UUID>`

Diagnostic-only: list all chains matching the input session_id with their previews. Reuses the resolver's preview-building code path. Always exits 0; does not spawn anything. Useful for users who hit the ambiguous-chain error path.

## 8.5.1 New `agents migrate-db` subcommand

Foreground command that runs the §2 backfill on demand. Behavior:

- Idempotent: safe to run repeatedly; backfill checks `SELECT EXISTS(SELECT 1 FROM session_chains)` before inserting and skips if already populated.
- Prints a one-line progress every N rows for user feedback on large DBs.
- Exits 0 on success, 1 on DB write failure (with stderr message naming the failing row group).
- Does not spawn provider subprocesses; pure DB work.

Same backfill loop as the `StateDb::open` synchronous path — `migrate-db` is a user-facing wrapper, not a separate implementation. Pin equivalence with a test that runs both paths on the same fixture and asserts identical resulting `session_chains` and `session_chain_segments` rows.

## 8.6 Eliminating `-m` from agent-driven resume workflows

The combined effect of §3.1 (chain records model at mint), §4.5 (resolver fallback), and §8.1–8.3 (`-m` becomes optional) is that **agents which resume sessions never need to pass `-m`**. Once a session has been started via `agents -m <model> ...`, every subsequent `agents --resume <UUID>` (or `agents resume <UUID>`, or `agents repl --resume <UUID>`) returns to the same model automatically.

UI sessions get the same ergonomics without runner-side model inference: a user who runs `claude` directly and later wants to resume through agent-runner does so with `agents --resume <UUID>`; the active provider's CLI is spawned without a model override, so the CLI uses its own default model. They can override with `-m` if they want a specific runner model/pool for the continuation.

# 9. Provider config

## 9.1 Provider runtime config in `providers.toml`

Per-provider runtime config lives in `~/.config/oulipoly-agent-runner/providers.toml`, keyed by provider/account name. Model TOMLs do not carry command, prompt, resume, capture, acceptance, or storage config.

```toml
[claude2]
quota_script         = "anthropic-usage ~/.claude2/.credentials.json"
auth_refresh_command = "claude auth status"
command              = "env"
args                 = ["-u", "CLAUDECODE", "claude2", "--dangerously-skip-permissions"]
interactive_args     = ["-u", "CLAUDECODE", "claude2", "--dangerously-skip-permissions"]
prompt_mode          = "stdin"

[claude2.resume]
kind = "flag"
flag = "--resume"

[claude2.session_capture]
kind = "forced_flag_verified"
flag = "--session-id"

[claude2.session_storage]
kind = "claude_code"
projects_dir = "~/.claude2/projects"

[claude2.resume_acceptance]
accepted_output_patterns = ["\"session_id\":\"{session_id}\""]
```

Spawn composition:

- With a model: `providers[name].command + providers[name].args + model.providers[name].args + resume_args`.
- Without a model: `providers[name].command + providers[name].args + resume_args`; no model TOML is consulted.
- REPL uses `interactive_args` on both sides with the same provider-first ordering.

Session storage is parsed from `providers.toml`:

```rust
pub enum SessionStorage {
    ClaudeCode { projects_dir: PathBuf },
    Codex      { sessions_dir: PathBuf },
}
```

Parse via tagged union with `kind = "claude_code" | "codex"`. Path fields use `~` expansion.

Claude-Code storage example:

```toml
[claude2.session_storage]
kind         = "claude_code"
projects_dir = "~/.claude2/projects"
```

Codex storage example:

```toml
[codex.session_storage]
kind         = "codex"
sessions_dir = "~/.codex2/sessions"
```

**v1 limitation**: `kind = "codex"` is read by the chain layer (chain_id mint at ingestion, segment ledger, resume-by-id within the same provider), but the migration mechanic in §6 returns `MigrationError::CodexMigrationDeferred` if a migration trigger fires for a Codex chain. Codex cross-account migration awaits a follow-up PR after Codex exposes a documented path-resume mechanism. See §15.

Validation at config load:
- `projects_dir` / `sessions_dir` must exist on disk (warn but don't fail — provider is opt-in).
- Two `kind = "claude_code"` providers in the same model pool must not declare the same `projects_dir` (would cause source-equals-target collisions during migration). Reject at config load.
- `kind = "codex"` providers are not validated against migration-target-pair uniqueness in v1 because they do not participate in the migration mechanic.

Providers without session storage cannot be migration targets. Providers with `kind = "codex"` also cannot be migration targets in v1; their storage declaration is identity-only. A Claude-Code provider can still be the source of an outbound migration **only** if the runner can locate the source JSONL via convention — but for v1, **require both source and target Claude-Code providers to declare storage**. Simplifies the resolver and guarantees deterministic failure modes.

## 9.2 Model TOML provider entries are model flags only

Each model TOML provider entry contains the provider name and model-specific flags only:

```toml
[[providers]]
name = "claude2"
args = ["-p", "--model", "opus", "--output-format", "json"]
interactive_args = ["--model", "opus"]
```

Config load rejects old per-provider blocks in model TOMLs with: `Old per-provider config detected in <file>; run agents migrate-config to migrate.`

`agents migrate-config` lifts old runtime blocks into `providers.toml`, rewrites model TOMLs to the reduced shape, aborts on conflicting per-provider runtime declarations, and is idempotent.

## 9.1.1 Turn-script adapter contract: `is_compaction_boundary`

Extend the existing turn-script JSON line shape (documented at `README.md:298-310`) with an optional field:

```json
{
  "session_id": "...",
  "turn_id": "...",
  "timestamp": "<RFC 3339>",
  "role": "user|assistant",
  "parent_turn_id": "<turn_id|null>",
  "is_sidechain": true,
  "is_compaction_boundary": true
}
```

`is_compaction_boundary` is **optional** (parallel to existing `parent_turn_id` / `is_sidechain` optional fields, per README convention). Adapters that don't track compaction omit the field; the runner treats omitted as `false`.

Update the `claude-code-turns` reference adapter (`scripts/claude-code-turns`) to emit the flag when it observes a compaction record in Claude Code's JSONL. Implementation discovery: inspect existing `~/.claude/projects/...` JSONLs for compaction-marker record types and update the adapter accordingly. Treat the adapter update as in-scope for this PR.

`codex-turns` is **not** updated in this PR. The adapter remains capable of ingestion without the flag; Codex chain identity works without compaction-boundary detection because Codex cross-account migration is deferred in v1 (§15).

# 10. Trace integration

`trace --json` adds `chain_id` to each session block, sourced from the latest segment row tied to the invocation's session_id:

```json
{
  "session": {
    "id": "9e69e8cc-...",
    "chain_id": "a1b2c3d4-...",
    "transcript_state": "available",
    "transcript_path": "...",
    "sidechain_turn_count": 2
  }
}
```

Human-readable trace output adds a `Chain: <chain_id>` line above the existing `Session: <UUID>` line. When `--resume` was used, the existing `Resume target: <UUID>` line stays unchanged — the target UUID is the segment's `session_id`, distinct from `chain_id`.

# 11. Test plan

## 11.1 Test-intent track

Fixtures are applied outside test bodies per Phase 6: DB state comes from dedicated SQLite fixture builders, config state from TOML fixture files/builders, provider commands from stub scripts, and transcript files from tempdir fixture trees.

| Theme / expected test group | Change risk or verification risk | Acceptance condition | Level | Fixture source / application point | Assumption-register link | Observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Schema migration and backfill (`backfill_*`, `migrate_db_command_*`, `startup_refuses_chain_ops_on_backfill_failure`) | Existing DBs may open without chain rows, double-backfill, or fall back to deleted resolver behavior. | `CREATE TABLE IF NOT EXISTS` and `ALTER TABLE ... DEFAULT 0` are idempotent; each distinct `(provider_name, session_id)` becomes one imported chain/segment; `StateDb::open` and `agents migrate-db` produce equivalent rows; failures stop chain-aware startup with a recovery hint. | particular-integration | SQLite temp DB fixtures seeded from `session_turns`/`invocations`; CLI fixture invokes `agents migrate-db` against the temp DB. | A5 | SQL row counts and values in `session_chains`/`session_chain_segments`; command exit code 0/1; stderr substring naming `agents migrate-db` on startup failure. | Does not prove performance on every user DB or filesystem; backfill scale remains a §14 watched risk. |
| Chain identity write paths (`mint_chain_*`, `agent_session_chain_records_model_at_mint`, `ui_session_chain_*`, `chain_last_used_at_updates_after_successful_invocation`) | New agent/UI sessions may fail to mint stable chain identity, record wrong model provenance, or leave stale `last_used_at`, breaking resume without `-m` and 24h disambiguation. | Agent session capture mints an initial segment with model name; UI ingestion mints an imported segment with `'<unknown>'`; resume no-ops against existing chain; successful chain-tied invocation advances `last_used_at`. | particular-integration | Invocation/ingestion fixture builders with stub provider output and temp DB applied through existing state APIs. | A3, A8 | SQL row values: `transition_reason`, `model_name`, active segment count, and `last_used_at` within the call window. | Does not verify direct upstream CLI behavior beyond adapter-emitted turns; delayed adapters remain an A3 invalidator. |
| Resolver disambiguation and model inference (`resolve_resume_*`, `agent_resume_no_dash_m_*`) | `--resume` may select the wrong logical conversation, accept ambiguous ownership, or fail to infer the model where proposal promises CLI-default fallback. | Full UUID only; single-chain resolves active segment; duplicate session IDs filter by 24h then max `last_used_at`; true ambiguity returns previews; model precedence is override → latest invocation → chain model → `None`; provider/model pool mismatch reports suggestions only when a model is known. | particular-integration | Resolver DB/config builders with seeded chains, segments, invocations, and model pools; agent CLI fixture for no-`-m` resume. | A8 for CLI-default fallback; none for pure disambiguation mechanics. | Returned `ResolvedResume` fields, `ResumeError` variants/previews, optional model name, suggestions list, CLI exit code/stderr. | Preview snippets are intentionally absent until the §15 `transcript_preview` follow-up; UUID collisions outside seeded cases are not exhaustively generated. |
| Best-on-resume decision (`decide_migration_*`, `manual_migrate_flag_overrides_best_score_via_cli`) | Migration may fail to happen when a less-loaded provider exists at resume, ignore exhaustion, or choose a provider without session storage. | Every resume picks the lowest-load provider with `[providers.session_storage]`; active least-loaded stays; ties break by lower provider index; `exhausted_at` triggers migration to the lowest-load sibling; single-provider/no-storage pools stay; manual target overrides automatic ranking and records `manual`; Codex source/target policy decisions are deferred by the migration mechanic before writes. | particular-integration | Balancer state/config fixture builders with quota-window rows, exhausted flags, model pools, and session-storage blocks; CLI fixture applies `--migrate`. | A2, A4, A7 | `MigrationDecision` enum value, target provider index, transition reason, logged Codex-deferred reason, and resulting segment `transition_reason = 'manual'` for CLI path. | Does not model real provider API quota correctness; Codex cross-account migration remains §15 residual. |
| Migration mechanic: Claude JSONL copy, Codex deferred guard, segment ledger, and races (`migration_copies_*`, `migration_appends_chain_segment_*`, `migration_returning_clause_aborts_on_concurrent_close`, source-path errors, `migration_mechanic_errors_codex_deferred_*`) | Copying transcripts may write the wrong target, corrupt source/target files, leave multiple active segments, hide missing/malformed source paths, or accidentally exercise unverified Codex migration. | Source JSONL remains unchanged; Claude target path is provider-kind correct; plain copy writes `source[offset..]`; Codex source/target returns `MigrationError::CodexMigrationDeferred`; missing/malformed sources produce typed errors; close/open segment transaction records `ended_at`, `last_turn_id`, new active segment, and aborts concurrent close losers. | particular-integration | Tempdir transcript trees for Claude layouts, transcript-locator stub outputs, SQLite chain/turn fixtures, Codex storage config fixtures, and transaction-race harness. | A1 for Claude replay layout; A7 for Codex deferral; A3 for `last_turn_id` from ingested turns. | File existence/path, byte contents, absence/presence of `.tmp`, `MigrationError` variant, SQL segment fields, and one failed concurrent close result. | Does not prove real CLIs accept every copied JSONL; Codex cross-account migration is deferred to a follow-up PR per §15; chain identity for Codex sessions is verified but the file-copy path is not exercised. |
| Migration mechanic: Codex deferred negative emission (`migration_does_not_emit_migrate_stderr_on_codex_deferred`) | Observability may claim a migration occurred even when the Codex-deferred guard short-circuits before segment insertion. | When Codex migration returns `MigrationError::CodexMigrationDeferred`, the `[migrate]` stderr line is not emitted and no target segment row is inserted. | particular-integration | CLI/migration fixture with Codex active provider, eligible Claude-Code sibling, stderr capture, and SQLite segment-count assertion. | A7 | Stderr does not contain `[migrate]`; `session_chain_segments` has no newly inserted target row. | Does not prove future Codex migration observability; the path remains deferred to §15. |
| Migration mechanic: compaction-aware Claude target build (`migration_truncates_*`, `migration_errors_when_compaction_boundary_not_in_jsonl`, `pre_compaction_turns_remain_*`) | Compacted Claude sessions may be copied from the wrong offset, causing context overflow or data loss. | Latest compaction boundary wins; no boundary copies full file; missing boundary line errors without partial target; pre-compaction `session_turns` remain queryable. | particular-integration | Plaintext transcript fixtures with known JSON lines and turn IDs; DB fixture marks `is_compaction_boundary`; tempdir copy target. | A3, A6 | Target line set, error variant `CompactionBoundaryNotInJsonl`, missing partial target, SQL query returning pre-compaction rows. | Tests use synthetic boundaries and cannot prove future upstream JSONL stability; Claude compaction record format remains a §15 implementation-discovery item. |
| `is_compaction_boundary` ingest plumbing (`turn_script_optional_compaction_field_defaults_false`, `turn_script_compaction_field_propagates_to_session_turns`) | Schema default may exist while adapters/structs/INSERT statements silently drop the boundary signal. | Missing optional field stores `0`; emitted `true` stores `1`; both single-row and batch insert paths bind the value. | particular-integration | Dedicated adapter-output JSONL fixtures parsed through `scan_provider`; direct DB ingest fixture for single-row path. | A3, A6 | `session_turns.is_compaction_boundary` SQL value equals 0/1 for seeded turns. | Does not validate all real Claude Code compaction record variants; format drift is A6/§15 residual. |
| `compute_projections` refactor equivalence (existing balancer test suite plus extraction-specific coverage if needed) | Refactor may change provider selection, fallback order, hidden-window penalty, or tie behavior while migration consumes projection data. | `score_by_density` returns the same provider index before/after extraction for existing fixtures; extracted projections expose the same per-window values used by the selection path. | unit | Existing balancer fixtures retained from initiative 04; optional projection fixture calls `compute_projections` with the same seeded quota/window state. | A4 | Selected provider index, binding score/order, projected window fields, and unchanged existing balancer test output. | Does not prove behavioral parity for all numeric edge cases beyond fixture coverage; any failing balancer regression invalidates A4. |
| CLI surface (`top_level_resume_without_model_*`, `manual_migrate_flag_*`, `resume_list_subcommand_*`, `migrate_db_command_*`) | Public commands may keep requiring `-m`, route to the wrong mode, hide ambiguity, or expose a new subcommand that does not share the startup backfill implementation. | `agents --resume <UUID>` works without `-m` when model can be inferred and errors clearly when it cannot; `--migrate` forces manual transition; `resume --list` prints all matching chains and exits 0; `migrate-db` is idempotent and equivalent to startup backfill. | end-to-end | CLI runner fixture with temp config dir/DB, stub provider binaries, chain DB fixtures, and transcript tempdirs. | A8 for no-`-m` UI fallback; A5 for `migrate-db`. | Exit code, stderr substrings, stdout list containing both chain IDs/providers/turn counts, and SQL row equality after command. | Does not verify shell completion/help text unless README/CLI tests add it; real provider command quirks remain integration-hidden residual. |
| Resume strategy compatibility (`compose_resume_args_*`, `compose_resume_args_rejects_config_kind`) | Adding the future `target_jsonl_path` parameter may accidentally change existing resume argv or reintroduce the removed config strategy. | Existing `flag` and `subcommand` argv remain unchanged with or without `target_jsonl_path`; no `config` strategy parses in v1. | unit | Model TOML fixture parsed into `ResumeStrategy`; compose-args fixture provides `None` and `Some(path)`; config-kind TOML fixture asserts rejection. | A1, A7 | Returned argv vector for Claude `flag` and Codex `subcommand`; config-kind TOML parse error. | Does not implement Codex path-resume; that is explicitly deferred in §15. |
| Trace integration (`trace_json_includes_chain_id`) | Adding chain identity to trace may omit migrated nodes or break existing trace fields. | `trace --json` includes `session.chain_id` for nodes tied to a chain; existing session fields remain present; human trace gains chain line without changing resume target semantics. | particular-integration | Trace DB fixture with invocation tree, session rows, and matching chain segments; JSON renderer invoked against fixture DB. | None for additive serialization; A3 for turn counts if asserted. | JSON field `session.chain_id` equals segment row chain ID; existing fields still deserialize; ASCII output contains `Chain:`. | Does not add frontend chain visibility; PoolsView/StatusView remain out of v1 scope (§13, §15). |

## 11.2 Test plan implementation list

Unit tests (Rust, `#[test]`):

- `mint_chain_on_first_session_capture`: invoke a model with session_capture configured, assert `session_chains` row exists with chain_id, model_name, and one segment with `transition_reason = 'initial'`.
- `mint_chain_no_op_on_resume_of_existing_chain`: resume a chain, assert no new chain row is created and the existing segment remains active.
- `backfill_creates_one_chain_per_provider_session_pair`: seed `session_turns` with two providers each owning a session_id, run `StateDb::open`, assert two chains and two segments exist with `transition_reason = 'imported'`.
- `backfill_idempotent_on_second_open`: run `StateDb::open` twice, assert chain count unchanged.
- `resolve_resume_returns_active_segment_for_single_chain`: seed one chain with two segments (one closed, one active), assert resolver returns the active segment's provider/session.
- `resolve_resume_filters_by_24h_when_two_chains_share_session_id`: two chains sharing session_id, one used 1h ago, one 48h ago. Resolver returns the 1h-old chain without erroring.
- `resolve_resume_errors_ambiguous_when_both_recent`: same setup but both chains within 24h, assert `ResumeError::Ambiguous` lists both chain_ids.
- `resolve_resume_falls_back_to_max_last_used_when_none_within_24h`: both chains older than 24h, assert resolver picks max(last_used_at).
- `resolve_resume_infers_model_from_latest_invocation`: chain with two invocations, latest carrying `claude-opus`. Resolver with no override returns model `claude-opus`.
- `resolve_resume_falls_back_to_chain_model_name_when_no_invocations`: chain with no invocation rows (backfilled only), `session_chains.model_name = "claude-haiku"`. Resolver returns `claude-haiku`.
- `resolve_resume_returns_none_model_when_no_inference_source`: chain with no invocations and `chain.model_name = '<unknown>'`. Assert `resolved.model_name.is_none()` and `resolved.model.is_none()`.
- `agent_session_chain_records_model_at_mint`: invoke `agents -m claude-opus ...`, capture session_id, assert `session_chains.model_name = "claude-opus"`.
- `ui_session_chain_minted_with_unknown`: ingest a turn for a fresh `(provider, session_id)` pair. Assert chain row exists with `model_name = '<unknown>'`.
- `chain_mint_works_for_codex_ingestion`: ingest a Codex turn for a fresh `(provider, session_id)` pair; assert `session_chains` and `session_chain_segments` rows exist. This pins that Codex chain identity is preserved even though migration is deferred.
- `agent_resume_no_dash_m_uses_session_recorded_model`: start an agent session under `claude-opus`, then run `agents --resume <UUID>` with no `-m`. Assert the second invocation runs against `claude-opus`.
- `resolve_resume_validates_provider_in_model_pool`: chain owned by `claude2`, request a model whose pool excludes `claude2`, assert `ResumeError::ProviderModelMismatch` with non-empty suggestions.
- `decide_migration_picks_best_scored_sibling_on_resume`: active at 83% in one long window, sibling at 19% long / 9% short. Binding score would prefer active because of the sibling's short reset window, but resume load picks the sibling. Assert `Migrate { target = sibling, reason = QuotaThreshold }`.
- `decide_migration_stays_when_active_is_least_loaded`: active has the lowest `max(projected_used)`. Assert `Stay`.
- `decide_migration_ignores_short_window_pressure_on_siblings`: pin the reported `claude` 83% vs `claude3` 19% / 9% case. Assert `Migrate { target = claude3, reason = QuotaThreshold }`.
- `decide_migration_breaks_ties_by_provider_index`: two siblings with identical loads. Assert the lower-index provider wins.
- `decide_migration_migrates_when_exhausted_flag_set`: active provider has `exhausted_at` non-null, sibling clear, assert `Migrate { reason = Exhausted }` regardless of projection.
- `decide_migration_stays_when_single_provider_pool`: one-provider model at 99%, assert `Stay`.
- `decide_migration_stays_when_no_sibling_has_session_storage`: sibling lacks `[providers.session_storage]`, assert `Stay`.
- `decide_migration_manual_overrides_best_score`: `manual_target = Some("claude2")`, active would otherwise win automatic ranking, assert `Migrate { reason = Manual }`.
- `decide_migration_returns_codex_deferred_for_codex_provider`: active provider has `kind = "codex"` storage. With a better storage-backed sibling, assert `Migrate`; the migration mechanic returns `MigrationError::CodexMigrationDeferred` before writes.
- `migration_copies_claude_jsonl_to_target_projects_dir`: stage a fake JSONL under source projects_dir, run migration, assert target projects_dir contains the file at `<cwd_hash>/<source_session_id>.jsonl`.
- `migration_reuses_source_session_id_on_target_side`: run a Claude-Code migration and assert `MigratedSegment.target_session_id == source_session_id`, target path uses the source UUID, and the new segment is unique by `(chain_id, target_provider, source_session_id)`.
- `migration_mechanic_errors_codex_deferred_on_codex_active_provider`: invoke the migration mechanic with a Codex source provider and a Claude-Code target candidate, assert `MigrationError::CodexMigrationDeferred { provider }` and no target file/segment is written.
- `migration_does_not_emit_migrate_stderr_on_codex_deferred`: invoke the migration mechanic with a Codex active provider, triggering `MigrationError::CodexMigrationDeferred`; assert stderr does NOT contain `[migrate]`; assert no segment row was inserted.
- `migration_truncates_target_jsonl_at_latest_compaction_boundary`: source JSONL has 10 turns with `is_compaction_boundary = 1` on turn 6. Target JSONL contains turns 6-10 only; turns 1-5 are absent. Source JSONL is unchanged.
- `migration_copies_full_jsonl_when_no_compaction_boundary`: no `is_compaction_boundary` row exists for source, target equals source.
- `migration_picks_latest_of_multiple_compaction_boundaries`: source has compaction at turn 4 and turn 8; target starts at turn 8.
- `migration_errors_when_compaction_boundary_not_in_jsonl`: `session_turns` records `is_compaction_boundary = 1` for turn_id `T` but the source JSONL contains no line whose JSON identifies as `T`. Assert `MigrationError::CompactionBoundaryNotInJsonl` with `turn_id = T`. No partial target file written.
- `pre_compaction_turns_remain_queryable_after_migration`: assert `session_turns` rows for the pre-compaction span are still SELECT-able after migration completes.
- `turn_script_optional_compaction_field_defaults_false`: ingest a turn with no `is_compaction_boundary` field, assert column is `0`.
- `turn_script_compaction_field_propagates_to_session_turns`: ingest a turn with `is_compaction_boundary: true`, assert column is `1`.
- `migrate_db_compaction_backfill_idempotent_on_second_run`: seed an existing `session_turns` row with `is_compaction_boundary = 0`, re-read a Claude JSONL containing `isCompactSummary: true`, assert the first pass flags one row and the second pass flags zero rows.
- `session_storage_expands_tilde_in_projects_dir`: parse model TOML with `projects_dir = "~/.claude/projects"` and `sessions_dir = "~/.codex/sessions"`, assert both paths are expanded against `dirs::home_dir()`.
- `agent_session_chain_records_initial_reason_even_if_ingestion_minted_first`: mint an imported segment from ingestion, then mint the agent-session chain for the same `(provider, session_id)`, assert `transition_reason = 'initial'`.
- `imported_session_stays_imported_when_no_agent_mint_fires`: mint only from ingestion and assert `transition_reason = 'imported'`.
- `chain_last_used_at_updates_after_successful_invocation`: invoke a chain-tied invocation, assert `session_chains.last_used_at` advances to within the call window. Pins §3.3's write hook directly so the 24h disambiguation rule is verified end-to-end (not just on seeded data).
- `migration_errors_on_source_path_malformed`: stage a `[providers.session_storage]` whose `transcript_locator` returns a path with no parent (e.g. a bare filename). Assert `MigrationError::SourcePathMalformed`. Defensive but pins the `MigrationError` variant against accidental removal during refactor.
- `migrate_db_command_runs_backfill_to_completion`: invoke `agents migrate-db` against a fresh DB with seeded `session_turns` rows. Assert exit 0; assert resulting `session_chains` and `session_chain_segments` rows match what `StateDb::open`'s synchronous backfill produces on the same seed. Pins the equivalence guarantee in §8.5.1.
- `migrate_db_command_idempotent_on_second_run`: run `agents migrate-db` twice; assert chain count unchanged after second run.
- `startup_refuses_chain_ops_on_backfill_failure`: simulate a DB write failure during startup backfill (read-only DB, e.g.); assert the runner exits with a clear error message that names `agents migrate-db` as the recovery action and that NO chain-aware code path runs in degraded mode.
- `migration_appends_chain_segment_with_correct_reason`: trigger migration via QuotaThreshold, assert closed segment has `ended_at` set and `last_turn_id` populated, new segment has `transition_reason = 'quota_threshold'` and `ended_at = NULL`.
- `migration_returning_clause_aborts_on_concurrent_close`: simulate two concurrent closes; one succeeds (RETURNING non-empty), one aborts.
- `migration_errors_on_source_jsonl_missing`: source path absent, assert `MigrationError::SourceMissing`.
- `compose_resume_args_rejects_config_kind`: parse a model TOML fixture with `[providers.resume] kind = "config"` and assert validation rejects it. Pins the v1 invariant that no config resume strategy is recognized.
- `top_level_resume_without_model_succeeds_when_chain_exists`: invoke `agents --resume <UUID>` with no `-m`, assert resolver picks the model and the run completes.
- `run_resume_spawns_without_model_flag_when_model_none`: chain seeded only via backfill with `model_name = '<unknown>'`; `agents --resume <UUID>` spawns from `providers.toml` only and succeeds without model flags.
- `run_repl_spawns_without_model_flag_when_model_none`: same assertion for `interactive_args`.
- `manual_migrate_flag_overrides_best_score_via_cli`: `agents resume --migrate <other-provider> <UUID>`, assert migration occurs and reason is `'manual'` even when the active provider would otherwise win automatic ranking.
- `resume_list_subcommand_prints_all_chains_for_session_id`: two chains share session_id, assert `agents resume --list <UUID>` prints both with last_used_at, active provider, and turn count.
- `trace_json_includes_chain_id`: invoke `trace --json <invocation_uuid>` over a chained invocation, assert `session.chain_id` matches the segment row.

Integration tests:
- `pr_05_migration_integration.rs`: end-to-end migration via fake JSONLs and stub provider commands.

Tests to update (mechanical):
- All `InvocationStart` literals gain no new fields (chain mint is separate from invocation row writes).
- `resume_model_pool_mismatch_message` test: rename to `resume_active_segment_provider_pool_mismatch_message`, update to verify suggestions list non-empty across multiple sibling models.

# 12. README updates

Replace the "Resuming a session" subsection (`README.md:417-477`) with chain-aware language:

- A chain is the stable identity of a logical conversation; session_id is per-segment and may change at migration.
- `--resume <UUID>` accepts a session_id or chain_id; if a session_id matches multiple chains, disambiguates by 24h-window and falls back to user choice.
- `-m` is now optional on `resume` and the top-level form. For agent sessions, the model is inferred from the chain's recorded model (set when the agent first started the session). For UI sessions started outside agent-runner, the runner spawns the active provider CLI without a model override and lets the CLI use its own default. Pass `-m` to override.
- Both agent and UI sessions can be migrated for Claude-Code providers — the chain layer abstracts over how the session was started. Codex sessions preserve chain identity but not cross-account migration in v1.
- `--migrate <provider>` forces migration; otherwise the runner picks the least-loaded storage-backed provider at resume entry.
- `[providers.session_storage] kind = "claude_code"` is required on any provider that participates in migration (source or target). `kind = "codex"` is declarable for chain identity but migration is deferred in v1.
- Resume strategies remain `kind = "flag"` and `kind = "subcommand"`; no `kind = "config"` strategy ships in v1.

Add a new subsection "Session migration" under Load Balancing covering the best-on-resume policy and the retained historical `quota_threshold` transition reason string.

Add a `[providers.session_storage]` example to the existing Adding a Model section.

Update the Session Ingestion turn-script contract (`README.md:298-310`) to document the new optional `is_compaction_boundary` field. Note that `claude-code-turns` emits it; `codex-turns` does not yet (limitation in §15).

Update the `transcript_locator` subsection (`README.md:324-336`) to note that the locator is now invoked at **migration time** as well as trace time. Today's wording ("lazy at trace time — never at invocation time") becomes stale once §6.1 calls the locator during a `resume`-triggered migration to resolve the source JSONL path. Reword to "lazy — invoked only when a chain is being inspected (`trace`) or migrated (`resume` with cross-provider migration). Unused providers cost nothing." Cross-link to §6.1's source-path resolution path.

Update the CLI synopsis (`README.md:131-136`) to mark `-m, --model` as optional when `--resume` is present, and document the new `--migrate <provider>` flag plus the `agents resume --list <UUID>` subcommand.

Update the resume failure-modes list (`README.md:467-475`) — the "Resume failures all exit 1 with a specific stderr message" enumeration today lists four error classes (`No session found`, `Invalid session UUID`, `Provider/model mismatch`, `Provider has no [providers.resume] block`). Add `ResumeError::Ambiguous` (multiple chains share session_id, all within 24h — user must rerun with `--resume <chain_id>`) and `ResumeError::ProviderNotConfigured` (the active provider is no longer present in any loaded model TOML). The existing `Provider/model mismatch` text is kept; phrasing now references "active segment's owning provider" rather than "owning provider" since segments can change.

# 13. Cross-cutting considerations

- `find_provider_for_session()` (`src-tauri/src/state/db.rs:2062-2107`) is replaced by `resolve_resume()`. All callers — `run_resume`, `run_repl` resume branches, the Tauri command if any — switch over. Old function deleted, not deprecated.
- The `resume_acceptance` field in invocation rows (already present) reflects the **target** CLI's acceptance after migration. The chain ledger answers "was the migration recorded"; `resume_acceptance` answers "did the new provider accept the copied transcript". Document the distinction in §10's trace output.
- `compose_resume_args()` signature changes (gains optional `target_jsonl_path`). Update both call sites.
- `ModelConfig` does not carry migration policy knobs. Removed policy config is deleted, not retained as ignored compatibility surface.
- E2E: defer Tauri mock scenarios for chain disambiguation; the UI does not surface chains in v1. PoolsView/StatusView remain unchanged.
- The chain abstraction is observable only via CLI (`resume --list`, `trace --json`). No frontend changes.

## 13.1 Supported-surface track

**Deployment mode**: the runner ships as the Tauri/Rust binary built by `cargo tauri build` and installed at `~/.local/bin/agents`. Initiative 05 ships as one PR; release follows the existing tag-and-bump cadence.

**Customer cohort**: existing agent-runner users who already have providers configured are the primary supported cohort. UI-only Claude Code / Codex users become reachable post-PR through session ingestion when `sessions.toml` is configured.

**Adjacent public or user-reachable paths** (from `research/05-session-migration-problem-map.md` §4):

- `agents repl <model>` with no resume must keep working without chain input (`research/05-session-migration-problem-map.md:94`).
- `agents repl <model> --resume <UUID>` must keep working, with the model argument now optional on resume (`research/05-session-migration-problem-map.md:95`).
- `agents resume -m <model> --session-id <UUID> -f <file>` must keep working, with the model argument now optional (`research/05-session-migration-problem-map.md:96`).
- `agents -m <model> --resume <UUID> "prompt"` must keep working, with the same optional-model treatment (`research/05-session-migration-problem-map.md:97`).
- `agents --resume <UUID>` with no prompt and `-m <model>` must keep working through the `run_repl` route, with the model argument now optional on resume (`research/05-session-migration-problem-map.md:98`).
- `agents trace <invocation_uuid>` and `agents trace --json` must keep working with additive `chain_id` output (`research/05-session-migration-problem-map.md:99`).
- Direct user-terminal CLI usage of `claude` / `codex` outside agent-runner must keep working through `session_turns` ingestion; those turns continue to affect quota projection and chain import/model fallback without requiring invocation rows (`research/05-session-migration-problem-map.md:100`; hookpoint: §3.1.1).
- `cargo run --example session_scan --release` must keep working as the supported ingestion-health diagnostic path and should remain provider-count oriented unless a later proposal changes that surface (`research/05-session-migration-problem-map.md:101`).
- `agents quota_check` example output must keep working unchanged; do not add chain/session-density fields to this diagnostic surface in Initiative 05 (`research/05-session-migration-problem-map.md:102`; output-shape note: `research/05-session-migration-problem-map.md:83`).
- Tauri `test_model_with_db_path` must keep working; it does not touch chains (`research/05-session-migration-problem-map.md:103`).
- Session ingestion through ordinary balanced execution must keep working: `select_provider` still scans providers through `BalanceContext` before scoring, and chain mint/import behavior is additive to the existing turn scan (`research/05-session-migration-problem-map.md:104`; hookpoint: §3.1.1).
- Post-success session capture through `ingest_and_emit_session_id` must keep working for direct model, agent, one-shot resume, and REPL flows; capture failure remains non-fatal to invocation finalization but less observable (`research/05-session-migration-problem-map.md:105`; hookpoints: §3.1 and §3.3).
- Frontend PoolsView/StatusView remain read-only on chain data in v1 and have no Tauri-command dependency for chain operations (`research/05-session-migration-problem-map.md:86`).

**Cohort coverage check**:

- Existing agent-runner users with configured providers are covered by the current CLI surfaces (`repl`, `resume`, top-level prompt resume, no-prompt resume), trace, diagnostics, GUI `test_model`, balanced execution, and post-success capture listed above. These are the paths they can use today (`research/05-session-migration-problem-map.md:94-105`).
- UI-only Claude Code / Codex users are covered explicitly through the direct terminal usage + `session_turns` ingestion path and ordinary balanced-execution scan path. They remain reachable only when `sessions.toml` is configured, and their chain/model fallback is the §3.1.1 imported-chain path (`research/05-session-migration-problem-map.md:100`, `research/05-session-migration-problem-map.md:104`).

**Blast-radius notes for unchanged adjacent paths**:

- `provider_quotas.exhausted_at` from initiative 04 is a read-only consumer for §5 step 3. Initiative 05 does not change the schema, write path, or clear path for quota exhaustion.
- `score_by_density` math is refactored into `compute_projections` but claimed bit-for-bit equivalent. Pin through the existing balancer test suite and keep the selection/fallback semantics unchanged.
- `session_turns` ingestion contract changes additively: `is_compaction_boundary` column defaults to `0`, and the adapter field is optional. Existing adapters that omit it continue to ingest with `false`.

**Migration path**: schema migration uses unconditional `CREATE TABLE IF NOT EXISTS` for `session_chains` and `session_chain_segments`, plus idempotent `ALTER TABLE session_turns ADD COLUMN is_compaction_boundary INTEGER NOT NULL DEFAULT 0`. Backfill runs at first open from existing `session_turns` rows. `agents migrate-db` is available unconditionally for users who want to run backfill explicitly. Codex providers participate in chain identity (ingestion mint, segment ledger, same-provider resume-by-id) but not cross-account migration in v1. There is no data loss, no rolling restart requirement, and no double-write window.

**Rollback path**: rolling back to the prior version means uninstalling 05's binary. The new tables (`session_chains`, `session_chain_segments`) and the new column (`session_turns.is_compaction_boundary`) are inert under the prior binary because the prior binary does not read them. The prior binary's `find_provider_for_session()` still works against unmodified `session_turns`. Because Rev 4 removes the `kind = "config"` resume strategy, there is no v1 schema or config drift to undo for Codex. Rollback is safe and requires no schema downgrade.

**Observability**:

- `trace --json` adds `chain_id` for every node tied to a chain.
- `agents resume --list <UUID>` provides ad-hoc chain inspection.
- Power users can query `session_chains` and `session_chain_segments` directly, with related `session_turns` / `invocations` joins for live-state and orphan checks.
- `OULIPOLY_INVOCATION` and `OULIPOLY_SESSION` stderr lines remain unchanged, so existing wrappers keep working.
- `[resume] -> <provider>` continues to fire on successful resume; `[migrate] <source-provider> -> <target-provider> reason=<reason>` fires when migration occurs.

The `[migrate] <source-provider> -> <target-provider> reason=<transition_reason>` line is emitted on stderr from the migration helper (§6 step 6, after the segment row is opened and before §6 step 7 composes target argv). Mirrors the existing `[resume] -> <provider>` line at `src-tauri/src/main.rs` (find the resume selection log site at implementation time). Always emitted, regardless of TTY, exactly once per migration event.

Concrete SQL observability queries for operators:

```sql
-- Q1: Find every chain currently active on a given provider.
SELECT chain_id, session_id, started_at FROM session_chain_segments
WHERE provider_name = ? AND ended_at IS NULL
ORDER BY started_at DESC;

-- Q2: List all migrations in the past 24h, oldest first.
SELECT chain_id, provider_name, transition_reason, started_at, last_turn_id
FROM session_chain_segments
WHERE transition_reason IN ('manual', 'quota_threshold', 'exhausted')
  AND started_at > datetime('now', '-1 day')
ORDER BY started_at ASC;

-- Q3: Find chains that share a session_id (potential ambiguity points).
SELECT session_id, GROUP_CONCAT(chain_id) AS chains, COUNT(DISTINCT chain_id) AS chain_count
FROM session_chain_segments
GROUP BY session_id
HAVING chain_count > 1;

-- Q4: Show live-state turns for a chain after the latest compaction boundary.
WITH active AS (
  SELECT provider_name, session_id FROM session_chain_segments
  WHERE chain_id = ? AND ended_at IS NULL
  ORDER BY started_at DESC
  LIMIT 1
),
boundary AS (
  SELECT MAX(st.timestamp) AS boundary_ts
  FROM session_turns st
  JOIN active a
    ON a.provider_name = st.provider_name
   AND a.session_id = st.session_id
  WHERE st.is_compaction_boundary = 1
)
SELECT st.turn_id, st.timestamp, st.role, st.source_file
FROM session_turns st
JOIN active a
  ON a.provider_name = st.provider_name
 AND a.session_id = st.session_id
WHERE st.timestamp >= COALESCE((SELECT boundary_ts FROM boundary), '0000-01-01T00:00:00Z')
ORDER BY st.timestamp ASC;

-- Q5: Count quota-threshold migrations per chain.
SELECT chain_id, COUNT(*) AS quota_threshold_migrations
FROM session_chain_segments
WHERE transition_reason = 'quota_threshold'
GROUP BY chain_id
ORDER BY quota_threshold_migrations DESC, chain_id ASC;

-- Q6: Find open segments with no invocation recorded in the past 24h.
SELECT scs.chain_id, scs.provider_name, scs.session_id, scs.started_at
FROM session_chain_segments scs
WHERE scs.ended_at IS NULL
  AND NOT EXISTS (
    SELECT 1 FROM invocations i
    WHERE i.provider_name = scs.provider_name
      AND i.session_id = scs.session_id
      AND strftime('%s', i.created_at) >= strftime('%s', 'now', '-24 hours')
  )
ORDER BY scs.started_at ASC;
```

**Phase 4 Rev 3 audit/scope amendment**: enumerated all problem-map §4 paths; added SQL observability queries; mechanized the `[migrate]` stderr line.

# 14. Risk surface for phase 4

**Audit risk: chain-segment race conditions.** Two concurrent `agents resume` calls with `--migrate` on the same chain could both try to close the same active segment. Guard with the `RETURNING` pattern in §3.2: `UPDATE ... WHERE id = ? AND ended_at IS NULL RETURNING id`. If empty, abort and re-resolve. SQLite 3.35+ supports RETURNING; the bundled version is 3.51.1 (initiative 03 §123-125 confirms `libsqlite3-sys 0.36.0` ships 3.51.1). Supported.

**Audit risk: backfill performance.** A user with 100K `session_turns` rows triggers 100K chain inserts on first open. Run backfill in one transaction; benchmark before merge.

**Decision (locked):** ship `agents migrate-db` unconditionally in this PR — it is not gated on a perf threshold. Default behavior at `StateDb::open` runs backfill synchronously when `session_chains` is empty AND `session_turns` is non-empty. If that runs to completion under any user-visible delay, no further action is required. If a user wants to run backfill explicitly (e.g. before a tight-deadline session, or after restoring a DB), `agents migrate-db` does the same work as a foreground command with progress output. The CLI surface entry is added in §8.

**No runtime fallback to `find_provider_for_session()` is permitted** — that function is deleted in §13 per `~/ai/conventions/no-backwards-compatibility.md`. If startup backfill fails (disk full, DB locked, panic), the runner exits with a clear error pointing the user at `agents migrate-db` for retry. There is no graceful-degradation path that keeps the old code alive.

**Audit risk: copy-then-resume failure modes.** If the file copy succeeds but the target CLI rejects the JSONL (format mismatch, permissions, dup UUID inside record-level fields), the new segment row stays open and a stray target JSONL exists. Mitigation: copy to `.tmp`, rename atomically AFTER segment row is written (so a crash before rename leaves no stale segment); the failed invocation row is sufficient evidence; user reissues `--migrate` to retry. Document in step 10 of §6. GC of stray JSONLs is deferred.

**Scope risk: no incidental change to balancer projection or refresh behavior.** §5.1 commits to extracting `compute_projections` from `score_by_density` — a refactor that moves code, not a behavior change. The mathematics, refresh ordering, fall-through to invocation-count, and recent-error penalty must be bit-for-bit equivalent before and after. Pin via the existing balancer test suite kept by initiative 04. Watch for diff under `src-tauri/src/balancer/mod.rs` in this PR: per-window projection math, bootstrap cascade, and selection logic must be unchanged. Any branch reorder or fall-through condition change is a separate proposal.

**Shortcut risk: no silent JSONL byte edits.** First-pass copies bytes unchanged for Claude Code. If a target CLI rejects the copy because record-level fields embed the source session_id, that's a deliberate normalization PR — `sed`-style rewrites are explicitly rejected. Provider-specific locator/copy helpers can be extended later to provide canonical copy-with-rewrite if the format demands it.

**Cross-org cache cost is not modeled.** The migration policy does not optimize for cache stickiness. Cross-org migration can cost a prefix rewrite, and after Anthropic's Feb 5 2026 workspace-isolation change, even same-org cross-workspace migration may pay the rewrite. User feedback for Rev 5 accepts this cost because resume is rare and agent fan-out already makes cache continuity weak. Document but do not solve.

**Heuristic-coverage scope.** Initiative 04's exhausted flag is set only when stderr matches `quota`/`billing`/`usage limit`. If a CLI emits a different phrase on quota exhaustion, the flag isn't set, but the next resume still picks the least-loaded storage-backed provider. Defense-in-depth.

# 15. Unresolved

- **Codex migration deferred to a follow-up PR.** Verification at `research/05-codex-resume-verification.md` confirmed `codex-cli 0.125.0` does not expose a path-resume mechanism; `codex resume <UUID>` requires a state-DB row, and the internal `ThreadResumeParams.path` field is not surfaced via CLI flags. Cross-account file copy is therefore insufficient. The chain abstraction in v1 supports Codex chain identity (segment ledger, resume-by-id within the same provider) but blocks migration with `MigrationError::CodexMigrationDeferred`. Follow-up PR: either wait for Codex to expose a documented path-resume mechanism, OR design a state-DB-aware migration path for Codex (couples to Codex internals; lower priority).
- **Codex compaction format**: subsumed by the broader Codex migration deferral. `codex-turns` continues to ingest turns without `is_compaction_boundary`; chain identity works without it.
- **Claude Code compaction record format**: the `claude-code-turns` adapter update in §9.1.1 is implementation discovery — the exact record type Claude Code writes for compaction events must be confirmed against real JSONL samples before merge. If the format is unstable across Claude Code versions, the adapter needs a version detection step.
- **Multi-cwd Claude Code sessions**: a session JSONL is keyed by `cwd_hash`. If a user resumes from a DIFFERENT cwd than where the chain originated, Claude Code returns a fresh session — current behavior. Migration faces the same constraint: the target's `<projects_dir>/<cwd_hash>/<id>.jsonl` path uses the cwd at migration time. Document, do not solve. Users who change cwd lose chain continuity, same as today.
- **REPL mid-session migration**: a long-running `repl` session that depletes a provider mid-conversation cannot migrate without restarting. Out of scope. The user restarts via `agents repl --resume <chain_id>`; the resume-time evaluation picks the least-loaded provider cleanly.
- **Cross-CLI migration** (claude → codex): transcript formats differ; out of scope.
- **Chain pruning / archival**: chains accumulate forever. GC is a follow-up PR; for now `last_used_at` lets users grep stale chains via SQL.
- **`transcript_preview` adapter for ambiguity disambiguation**: v1 ships without snippet content. Follow-up PR adds a `[providers.transcript_preview]` adapter pattern parallel to the existing turn/quota/locator adapters.
- **Per-chain quota accounting**: the segment ledger could feed back into the balancer for "how much of the user's daily budget went to chain X." Out of scope for v1.
- **Frontend chain visibility**: PoolsView/StatusView don't surface chains in v1. A "Conversations" UI is a separate proposal.

**Phase 3 amendments applied:** §1.1, §1.2, §11.1, §13.1 added per `~/ai/workflows/implementation-pipeline.md` lines 87-93. Risk gates (audit/scope/shortcut) re-run against the amended proposal as Rev 3 in a follow-up dispatch; the supported-surface gate runs as the 4th Phase 4 report.

**Rev 4 (Codex migration deferral) applied:** §1, §6, §7, §9.1, §11, §13.1, §15 updated; `kind = "config"` resume strategy removed; `MigrationError::CodexMigrationDeferred` introduced; Codex chain identity preserved. Risk gates re-run as Rev 4 in a follow-up dispatch.

**Rev 4 nit cleanup applied:** §1.1 wording fixed; §11.1 orphan ref resolved; §11.2 added `compose_resume_args_rejects_config_kind` and `migration_does_not_emit_migrate_stderr_on_codex_deferred`; hookpoints doc cleaned of stale `zstd` / `ResumeKind::Config` notes; initiative file Rev 4 log entry added.
