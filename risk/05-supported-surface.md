# Initiative 05 — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Rev 1's `none / LOW` carries to Rev 2 under Rev 4's Codex deferral. The
old A7 (`experimental_resume` bypasses the picker) is INVALIDATED by
`research/05-codex-resume-verification.md`; Rev 4 retires it into an
honest, evidence-backed assumption (Codex cross-account file-copy
migration is deferred; Codex chain identity still works through
ingestion and same-provider resume-by-id), and the proposal no longer
depends on the original A7 anywhere. Migration mechanic, resume
strategy enum, and storage validation all sequester Codex migration
behind `MigrationError::CodexMigrationDeferred`. Net value remains
positive: Claude migration, UI chain mint, projection refactor, model
inference, ambiguity disambiguation, and compaction-aware copy are all
unchanged; only Codex cross-account migration is removed from the
value column, while Codex chain identity is added. Two of Rev 1's
largest blast-radius items (`.zst` round-trip; `experimental_resume`
argv liveness) are removed because the corresponding code paths are
removed. Phase 5 (hookpoints already complete) and Phase 6
(implementation) may proceed.

## Concern 1 — Assumption invalidation check (Rev 2)

Re-evaluated against current evidence as of 2026-04-30.

**A1-A6 and A8 are unchanged from Rev 1 and still hold.** A1 (Claude
Code resume purely local), A2 (Anthropic cache scoping), A3
(`session_turns` ingestion), A4 (`compute_projections` refactor), A5
(backfill + `agents migrate-db`), A6 (Claude compaction marker
identifiable), and A8 (UI default_model fallback) carry verbatim from
Rev 1. Rev 4 does not touch any of these assumptions; the §11 test
pins, hookpoints, and evidence sources all remain intact.

### A7 — Codex migration deferred; chain identity preserved — HOLDS (REPHRASED)

**Original Rev 1 A7** ("Codex `experimental_resume` bypasses the
picker") is INVALIDATED by `research/05-codex-resume-verification.md`:
`experimental_resume` is not documented and not present in codex-cli
0.125.0 source; the live CLI accepts unknown `-c` keys without
validating that any code reads them. The original Rev 1 A7
invalidator ("Codex CLI removes or changes `experimental_resume`")
materialized as "the key was never real."

**Rev 4 §1.1 row 7 rewrites A7** to: "Codex cross-account file-copy
migration is not verified for v1 because the CLI has no documented
path-resume surface. Codex chain identity still works through
ingestion and same-provider resume-by-id." The evidence column cites
the verification doc directly. Greppable verification that A7 is not
re-invoked elsewhere in Rev 4:

- `experimental_resume`: zero matches in `proposals/05-session-migration.md`.
- `kind = "config"`: present only as negations in §7 ("are NOT
  introduced in v1") and §12 README.
- `ConfigArgument`: zero matches.
- `ResumeStrategyKind::Config`: only as a §7 negation.

The `Used by` column points only to "§6 Codex deferred guard, §9.1
`codex` storage identity-only note, §11 Codex deferral/identity tests,
§15 Codex migration deferred residual" — every one of those treats
Codex as identity-only and depends on the deferral, not on
`experimental_resume`. A7's new invalidator is a positive future event
(Codex exposing path-resume) that would unlock follow-up work, not a
fault that breaks v1.

**Verdict: A7 is properly retired in its original form and replaced
with a weaker, honest assumption that holds against verification
evidence.** No internal inconsistency.

### Termination signal #1 — DOES NOT FIRE

A1-A6 and A8 hold unchanged. A7 is correctly rephrased and
sequestered: the original is invalidated but the proposal no longer
depends on it; the new A7 holds. No `assumption_invalidated`.

## Concern 2 — Net-positive value on the current supported surface (Rev 2)

### Risk reduced (problem-map §2 entries Rev 4 still demonstrably retires)

| §2 entry | Retired by | Rev 4 status |
| --- | --- | --- |
| §2.1 Resume `matches[0]`-wins ambiguity | §4 resolver disambiguation | Unchanged |
| §2.2 Resolver answers "which provider", not "which conversation" | Chain table + segment ledger | Unchanged — chains still mint for Codex |
| §2.4 `--resume` cliff requiring `-m` | §8.1-8.3 + §4.5 fallback | Unchanged |
| §2.6 Provider missing `[providers.resume]` blocks resume | §5 + §9.1 | Unchanged for Claude; Codex storage decl now identity-only |
| §2.18 UI sessions un-resumable through agent-runner | §3.1.1 + §9.2 `default_model` | Unchanged |
| §2.21 `score_by_density` projection inaccessible | §5.1 `compute_projections` | Unchanged |

§2.7 ("No `Config` resume strategy for Codex") is no longer retired
by Rev 4 — that's the surface Rev 4 explicitly drops. Net retired:
six distinct §2 entries, down from seven in Rev 1. Codex risks
unique to cross-account migration remain unaddressed (deferred), but
no Codex behavior is regressed.

### Blast radius added (Rev 2 — substantially reduced from Rev 1)

| New failure mode | Rev 4 status | Guard |
| --- | --- | --- |
| Backfill stalls on first open | Unchanged | `agents migrate-db` foreground retry |
| Migration copies bad bytes to target | Unchanged for Claude | `.tmp`+rename atomicity; typed errors |
| Compaction boundary recorded but missing in JSONL | Unchanged for Claude | `MigrationError::CompactionBoundaryNotInJsonl` |
| Two concurrent migrations on same chain | Unchanged | `RETURNING` guard |
| `compute_projections` refactor changes selection | Unchanged | Bit-for-bit pin via existing 20-test balancer suite |
| Cross-org / cross-workspace cache cost | Unchanged | Documented |
| ~~Codex `.zst` decompress/recompress correctness~~ | **REMOVED** | `zstd` crate not added in v1 |
| ~~Codex `experimental_resume` argv unverified~~ | **REMOVED** | `kind = "config"` strategy not in v1 |
| Codex migration trigger fires on misconfigured cross-CLI pool | NEW (small) | `MigrationError::CodexMigrationDeferred`; pinned by `migration_mechanic_errors_codex_deferred_*` |

Rev 4 strictly reduces blast radius vs Rev 1. Two large items dropped;
one small new defensive item added with typed error and pin.

### Migration / rollback burden

Same idempotent `CREATE TABLE IF NOT EXISTS` + `ALTER TABLE ADD COLUMN
DEFAULT 0`. **No `zstd` dependency added** — Cargo.toml diff is smaller
than Rev 1 anticipated. Rollback unchanged: uninstall binary; new
tables/column inert under prior binary (Grep of `src-tauri/src/`
confirms zero matches for `session_chain`, `chain_id`,
`is_compaction_boundary`).

### Net-value verdict — POSITIVE

Risk-reduced count (six §2 entries) significantly exceeds blast-radius
added (six net items, down from seven in Rev 1). Codex chain identity
is added value not present in Rev 1's framing. Termination signal #2
does NOT fire. Net value is positive and qualitatively cleaner than
Rev 1.

## Concern 3 — Supported-path continuity (Rev 4)

| Path | Verdict | Rev 4 evidence |
| --- | --- | --- |
| `agents repl <model>` (no resume) | PRESERVED | §8.3 |
| `agents repl <model> --resume <UUID>` | PRESERVED + EXTENDED | `-m` optional |
| `agents resume -m <model> --session-id <UUID> -f <file>` | PRESERVED + EXTENDED | §8.2 — `model` becomes `Option<String>` |
| `agents -m <model> --resume <UUID> "prompt"` | PRESERVED + EXTENDED | §8.1 deletes `ok_or_else` at `main.rs:318-321` |
| `agents trace <invocation_uuid>` and `--json` | PRESERVED + ADDITIVE | §10 — `chain_id` field added; existing fields unchanged |
| Direct CLI `claude` / `codex` | PRESERVED | Runner does not intercept |
| Tauri `test_model_with_db_path` | PRESERVED | `src-tauri/src/lib.rs` does not reference chain or resolver paths |
| Frontend PoolsView/StatusView | PRESERVED | §13: "no frontend changes" |
| `agents quota_check` example | PRESERVED | §13.1: "do not add chain/session-density fields to this diagnostic surface" |

No path is BROKEN or DEGRADED. All extensions are strict supersets of
the prior contract — identical to Rev 1 findings.

## Concern 4 — Adjacent-surface blast radius under Codex deferral

### `session_chains` for Codex sessions

- **Written by ingestion** (§3.1.1): yes — `chain_mint_works_for_codex_ingestion` pins.
- **Written by migration**: never — §6 Steps 1 and 3 return
  `MigrationError::CodexMigrationDeferred` before any chain row writes
  for Codex sources or targets.
- **Acceptable**: yes — single-segment Codex chains with stable
  identity from first ingestion is the design intent.

### `session_chain_segments` for Codex sessions

- **transition_reason values**: only `'initial'` (from session_capture
  mint) or `'imported'` (from ingestion mint) — never `'manual'`,
  `'quota_threshold'`, or `'exhausted'` because the migration write
  path is blocked at §6.
- **CHECK constraint**: `('initial', 'manual', 'quota_threshold',
  'exhausted', 'imported')` — both Codex-reachable values are in the
  set; absence of the migration values is correct, not a violation.

### `decide_migration` for Codex active provider

Per §5: when active is Codex,
- Step 3 (exhaustion): if Codex is exhausted AND a migration-eligible
  Claude-Code sibling exists, returns `Migrate`; else `Stay`.
- Step 6 (threshold): same — `Migrate` if eligible Claude-Code
  sibling AND strictly better score; else `Stay`.
- Step 1 (manual): `Migrate { reason = Manual }` if user passes
  `--migrate <claude-code-provider>`.

In all `Migrate` cases, §6 Step 1 returns
`MigrationError::CodexMigrationDeferred` because the source is
`kind = "codex"`. The migration is observable (planned but failed) in
stderr and resolver state. This is the "observability of the threshold
being crossed" rationale §5 cites.

In practice the `Migrate → §6 deferred` branch only fires under
cross-CLI misconfiguration (Claude-Code provider in a Codex model's
pool — out of scope per §15). Typical Codex-only pools fall through to
`Stay` at every step. The defensive path is friendly for diagnosing
misconfigurations and is pinned by
`decide_migration_returns_codex_deferred_for_codex_provider`.

**Verdict: ADEQUATE.** Behavior is consistent, tested, and observable.

## Concern 5 — Migration path concreteness (Rev 4)

§13.1 amendment ("Codex providers participate in chain identity but
not cross-account migration in v1") is mechanized:

- **§6 Step 1** (locate source): `kind = "codex"` returns
  `MigrationError::CodexMigrationDeferred { provider }`.
- **§6 Step 3** (compute target): same.
- **§11 test pins**:
  - `migration_mechanic_errors_codex_deferred_on_codex_active_provider`
    (source guard).
  - `decide_migration_returns_codex_deferred_for_codex_provider`
    (policy layer).
  - `chain_mint_works_for_codex_ingestion` (Codex chain identity
    preserved).

The four other Rev 1 migration concreteness claims (idempotent schema
DDL, first-open backfill, `agents migrate-db` unconditional,
no-data-loss/no-double-write) are unchanged in Rev 4 and remain
verified.

**Migration concreteness: VERIFIED.**

## Concern 6 — Rollback path concreteness (Rev 4)

§13.1 Rev 4 amendment: "Because Rev 4 removes the `kind = "config"`
resume strategy, there is no v1 schema or config drift to undo for
Codex."

- **`kind = "config"` config on user disks**: never shipped. Rev 1 of
  the proposal was reviewed under risk gates pre-merge; no released
  binary parses that strategy variant. No drift to undo.
- **New tables / column inert under prior binary**: same as Rev 1 —
  Grep confirms zero matches for `session_chain`, `chain_id`,
  `is_compaction_boundary` in `src-tauri/src/`. New column on
  `session_turns` carries `DEFAULT 0` and is invisible to fixed-
  column-list INSERTs at `state/db.rs:1962-1974` and `:1998-2014`.
- **Prior binary's `find_provider_for_session()`** still works against
  unmodified `session_turns` and `invocations` shape.

**Rollback concreteness: VERIFIED.** No schema downgrade required; no
config drift to undo because `kind = "config"` never reached users.

## Concern 7 — Observability adequacy (Rev 4)

§13.1 added six SQL queries Q1-Q6 plus the `[migrate]` stderr line
mechanization. Each query verified syntactically against §2 schema.

| Query | Use | Validity |
| --- | --- | --- |
| Q1 — Active chains on a given provider | Live segment audit per provider | All cited columns (`chain_id`, `session_id`, `started_at`, `provider_name`, `ended_at`) exist in §2 schema. **VALID.** |
| Q2 — Migrations in past 24h | Migration audit trail | All three `transition_reason` values are in CHECK set. **VALID.** Codex chains never appear (correct — they don't migrate). |
| Q3 — Chains sharing session_id | Ambiguity diagnostic — finds session_ids that would trigger `ResumeError::Ambiguous` | Standard GROUP BY + HAVING. **VALID.** Works for Codex chains too. |
| Q4 — Live-state turns after latest compaction | Compaction-aware view; full history when no compaction (COALESCE fallback) | Uses `is_compaction_boundary`, `timestamp`, `provider_name`, `session_id`, `chain_id`, `ended_at`, `role`, `source_file`, `turn_id` — all present. **VALID.** |
| Q5 — Quota-threshold migrations per chain | Identifies chains repeatedly bouncing on threshold | Standard aggregate. **VALID.** |
| Q6 — Open segments with no recent invocation | Orphan-segment detection | Uses `invocations.provider_name`, `session_id`, `created_at` — all present per problem map §1.19. **VALID.** |

### Failure-mode coverage check

| Rev-1-identified failure mode | Covered by |
| --- | --- |
| Orphan segments | Q6 |
| Chain ambiguity | Q3 |
| Migration audit trail | Q2, Q5 |
| Active chains per provider | Q1 |
| Live-state vs full history | Q4 |
| Compaction-boundary missing | `MigrationError::CompactionBoundaryNotInJsonl` typed error on stderr; Q4 for inspection |

### `[migrate]` stderr line anchor

Rev 4 §13.1 mechanizes the line at "§6 step 6, after the segment row
is opened and before §6 step 7 composes target argv." Verified — that's
the segment-open INSERT transaction. Mechanizable at that exact line.
Mirrors `[resume] -> <provider>`. Always emitted, regardless of TTY,
exactly once per migration event.

### Codex chain observability

Q1, Q3, Q6 work for Codex chains because they're segments too. Q2 and
Q5 won't return Codex rows because Codex chains don't migrate — that
is correct, not a gap. Q4 returns full history for Codex chains
because `codex-turns` doesn't emit `is_compaction_boundary` per §15.
All consistent.

The original Rev 1 advisory observability gaps (no `[migrate-failed]`
line; no non-power-user chain walker beyond `agents resume --list`)
remain non-blocking.

**Verdict: ADEQUATE.**

## Concern 8 — Cohort-specific concerns (Rev 4)

§13.1 names existing agent-runner users plus UI-only Claude/Codex users.

### Cohort A: existing agent-runner users with providers configured

- **Drop-in for missing `[providers.session_storage]`?** Yes for chain
  mint, resolver, model inference. NO for migration (storage decl
  required to be a target). Fail-closed: without storage decl,
  `decide_migration` returns `Stay` (Step 6 short-circuit).
- **Resume continues to work without storage decl?** Yes — only the
  migration mechanic requires it.
- **Backfill** runs synchronously regardless of storage decl.
- **Net**: drop-in for chain identity / model inference / resume;
  opt-in for migration.

### Cohort B: UI-only Claude/Codex users

- **Reachable post-PR?** Yes when `sessions.toml` is configured.
  Strict superset of prior behavior.
- **UI-only Claude users**: chain identity via §3.1.1; resume by id;
  can configure storage to gain migration. Same as Rev 1.
- **UI-only Codex users**: chain identity via §3.1.1
  (`chain_mint_works_for_codex_ingestion` pins). Resume by id works
  through Codex's native `resume <UUID>` if both providers share the
  same `state_5.sqlite` (i.e. same `CODEX_HOME`). Migration is
  unavailable in v1 — but it never was prior to Rev 4 either, since
  the original Rev 1 `experimental_resume` mechanism was unverified.
  **No regression from prior Codex behavior.**
- **Codex chain mint with no `default_model`**: §3.1.1 mints with
  `model_name = '<unknown>'`; resolver falls back to `-m` or returns
  `ResumeError::ModelInferenceImpossible`. Fail-closed with hint.

### No cohort regressed

Pre-Rev-4 Codex migration was a *proposed* feature that hadn't shipped
— its absence in Rev 4 is a deferral, not a regression. Codex users
gain chain identity; no other supported behavior changes.

## Verdict rationale

**Termination signal #1** does not fire — A1-A6, A8 unchanged from
Rev 1; A7 correctly retired in its original form and replaced with an
honest, weaker assumption that holds against
`research/05-codex-resume-verification.md`. No proposal section
depends on the invalidated original A7.

**Termination signal #2** does not fire — six §2 risky/brittle
behaviors still retired (down from seven). Two of Rev 1's largest
blast-radius items (`.zst` round-trip; `experimental_resume` argv
liveness) are removed. One small defensive failure mode added with
typed error and pin. Net value positive and cleaner than Rev 1.

**Standard verdict: LOW.** Supported-surface continuity preserved
across all nine adjacent paths (concern 3); Codex blast-radius is
bounded — `session_chains` writes only via ingestion, segments only
carry valid CHECK values, `decide_migration` behavior is consistent
and tested (concern 4); migration deferral mechanized at §6 Steps 1
and 3 with typed errors and §11 pins (concern 5); rollback safe with
no `kind = "config"` config drift to undo (concern 6); observability
adequate via Q1-Q6 plus mechanized `[migrate]` line, with Codex
chains observable through Q1/Q3/Q4/Q6 (concern 7); both cohorts have
fail-closed, opt-in paths with no regression — Codex users gain chain
identity without losing anything (concern 8).

**Recommendation:** Rev 1's `none / LOW` carries to Rev 2. Phase 5
(hookpoints already complete) and Phase 6 (implementation) may
proceed.

## Advisory items for the implementer (non-blocking)

1. Add a `[migrate-failed] <source> -> <target> reason=<reason>
   error=<class>` stderr line parallel to the success line, so failed
   migrations (including `CodexMigrationDeferred` if a misconfigured
   pool triggers one) surface without requiring `trace` re-run.
2. Backfill performance budget ("<2s on 100K rows") is asserted, not
   measured. Run a benchmark on a representative DB during Phase 6.
3. If `compute_projections` exposes new public types, add a focused
   projection-shape test alongside the equivalence test.
4. **New (Rev 2)**: when implementing the `[migrate]` stderr line at
   §6 Step 6, also consider a parallel `[migrate-deferred]
   <source>=codex reason=...` line at the §6 Step 1 / Step 3
   deferred-error sites so the misconfiguration path is visible
   without inspecting the failed invocation row.
