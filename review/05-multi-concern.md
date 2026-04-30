# Multi-Concern Check: Initiative 05 — session migration

## Verdict: single-concern

Initiative 05 is one coherent unit of work: *introduce the
session-chain abstraction so a single conversation can move across
provider accounts (e.g. `claude` → `claude2`) without renaming or
losing history*. 45 files touched (commits `15c121a` + `a344bd0` +
`91403a0`), but the code-only surface is 18 implementation files /
~5,000 net inserted lines; the remaining 27 files are planning
artifacts (initiatives/, proposals/, research/, risk/, review/) and
test fixtures. Every implementation hunk traces back to one of the
nine concerns the prompt enumerates, every concern has at least one
mechanical coupling to another, and the dependency graph forms a
closed cycle rooted at `state/db.rs` and the new `migration/mod.rs`.
`risk/05-scope.md` rev 4 already validated the single-PR framing
against three split candidates and rejected all three; the shipped
diff confirms — no candidate split would produce a strictly better
reviewer experience, and proposal §1's "single PR because schema,
resolver, executor, and CLI are mutually dependent" framing is
correct on inspection.

## Concerns → primary hunks

| Concern | Primary surface |
| --- | --- |
| 1. Chain identity | `state/db.rs:578-606` (schema), `:715-725` (column add), `:2247-2329` (backfill), `:2401-2484` (`resolve_resume`), `:1214-1278` (`mint_chain_for_invocation_session`); `state/mod.rs:7` re-exports; mint hooks at `main.rs:646-648` and `sessions/mod.rs:130-145` |
| 2. Sticky-then-migrate | `balancer/mod.rs:295-384` (`compute_projections`), `:386-489` (`decide_migration`), `:48-72` (`TransitionReason`) |
| 3. Migration mechanic | `migration/mod.rs` (new, 262 lines: `MigrationError`, `MigratedSegment`, `migrate_chain_segment`); reads `latest_compaction_boundary`, writes via `close_active_segment_returning` / `open_chain_segment` |
| 4. CLI affordances | `main.rs:44-47` (`--migrate`), `:102-128` (optional `--model`), `:155-158` (hidden `resume-list`, `migrate-db`), `:339-376` (top-level `--resume` no `-m`), `:1024-1051` (decide+migrate wiring), `:1337-1402` (`run_migrate_db`, `run_resume_list`, arg normalizer) |
| 5. Provider config | `config/model.rs:168-191` (`SessionStorage`), `:249-254` + `:592-602` (`migration_threshold`); `config/providers.rs:18-19` (`default_model`); `config/mod.rs:9` re-export |
| 6. Adapter contract | `scripts/claude-code-turns:69-84`, `state/db.rs:578` + `:2186-2207` (`is_compaction_boundary`), `sessions/mod.rs:43,122`, two adapter fixtures |
| 7. Trace integration | `trace/mod.rs:62` (`chain_id` field), `:296-298` (lookup), eight construction-site updates |
| 8. Codex deferral | `migration/mod.rs:11-13` (typed error), `:107-113` + `:145-149` (early-return guards), three pinning tests |
| 9. Documentation | `initiatives/05-*`, `proposals/05-*`, `research/05-*` (5 files), `risk/05-*` (5 gates); `15c121a` retroactively backfills 03/04 packages |

Forced follow-through hunks (no Default; mechanical sweeps): every
`ModelConfig` / `ProviderConfig` literal in `executor/cli.rs`,
`lib.rs`, `quota/mod.rs`, and `pr_f_resume_integration.rs` gains
`migration_threshold: 0.95`, `session_storage: None`,
`default_model: None`, or `is_compaction_boundary: false`. The
WAL-mode error message at `state/db.rs:447` gains
`"; run agents migrate-db first"` (forced by concern 4).

## Re-evaluation of the three scope-rev-4 splits against the actual diff

### Split A — schema-only prereq PR — rejected (confirms scope rev 4)

Schema is additive but inert. The new tables have no readers
outside `resolve_resume`, `chain_id_for_segment`, `chain_previews`,
segment-ops, and the migration ledger txn — every one added in this
same PR. The `is_compaction_boundary` column has no writer outside
`claude-code-turns` (concern 6) and no reader outside
`latest_compaction_boundary` (consumed only by `migrate_chain_segment`,
concern 3). The backfill is wired into both `StateDb::open` and
`run_migrate_db`; a schema-only PR would land tables that nothing
reads, plus a backfill that produces data nothing consumes.

### Split B — resolver+CLI vs migration mechanic — rejected (confirms scope rev 4)

The resolver and the migration mechanic share `ResolvedResume`
and `session_chain_segments`. A "resolver+CLI without migration"
PR would ship `--migrate <provider>` as a dormant flag whose only
effect is a "not implemented" error — a UX regression vs. landing
it never. A "migration mechanic without resolver" PR has no
caller: `run_resume` is the sole consumer and it ingests
`ResolvedResume` (concern 1's product). Same dead-intermediate
shape as initiative 04's seam B.

### Split C — Codex-deferred carve-out — already taken

Rev 4 *is* this carve-out: cross-account Codex migration is in
§15; only Codex chain identity remains in v1. The shipped diff
matches: `MigrationError::CodexMigrationDeferred` is the single
typed-error surface that replaces the unworkable
`-c experimental_resume` mechanism, and Codex chain identity is
preserved through `mint_imported_chain_if_absent` in
`scan_provider`. No further split available without dead code.

## Per-concern separability

- **Concern 1 (chain identity)** — five in-PR consumers: own
  resolver, `--migrate` CLI (4), `decide_migration` reads
  `exhausted_at` and produces a target index that
  `migrate_chain_segment` (3) then operates on, trace (7),
  ingest mint (sessions/mod.rs). No outside-PR consumer.
- **Concern 2 (sticky-then-migrate)** — `decide_migration` is
  consumed only by `run_resume`. Its signature
  `(&StateDb, &ModelConfig, &ResolvedResume, f64, Option<&str>)
  -> Result<MigrationDecision, MigrationError>` mechanically
  forces concerns 1, 3, 5, and 8 to co-exist.
- **Concern 3 (migration mechanic)** — `migrate_chain_segment`
  reads `provider.session_storage` (5),
  `state.latest_compaction_boundary` (1+6), `resolved.chain_id`
  (1), and writes through segment ops (1). Splitting any seam
  produces a build break at the function signature.
- **Concern 4 (CLI affordances)** — each of the four surfaces
  is a thin wrapper over a producer in 1, 2, 3, or the backfill.
  Without the producers the flags are dead arguments. The
  model-inference fallback that lets `--resume` work without
  `-m` lives in `resolve_resume` (1).
- **Concern 5 (provider config)** — `migration_threshold` is
  consumed only by `decide_migration` (2); `session_storage`
  by `migrate_chain_segment` (3) and `decide_migration`'s
  storage filter (2); `default_model` by `resolve_resume`'s
  model-inference fallback (1, `state/db.rs:2473-2480`).
- **Concern 6 (adapter)** — `is_compaction_boundary` has
  exactly one reader: `latest_compaction_boundary`, consumed
  only by `migrate_chain_segment`. Landing the adapter ahead
  writes a column that nothing reads; landing the migration
  ahead would silently fall through to offset=0 on every
  migration (proposal §6.6 step 3 forbids that explicitly via
  `MigrationError::CompactionBoundaryNotInJsonl`, which can
  only fire if the adapter actually marks boundaries).
- **Concern 7 (trace)** — additive `Option<String>` field, so
  *cosmetically* could ship later, but its DB read
  (`chain_id_for_segment`) is introduced here. A follow-up
  trace PR would have one reviewable line plus a dormant-
  helper. Not worth splitting.
- **Concern 8 (Codex deferral)** — without the early-return
  guard, a Codex provider routed into `migrate_chain_segment`
  attempts to read `.jsonl` from a `sessions_dir` (possibly
  `.zst`-compressed) and produces a corrupted target. The
  typed error is the *only* thing keeping Codex providers
  safe; ships with the mechanism it gates.
- **Concern 9 (documentation)** — separable in principle but
  ships together by convention so the §11.2 test-list and
  §13.1 supported-surface tracks can be cross-checked against
  a single tree state. `15c121a`'s 03/04 backfill is non-
  load-bearing housekeeping but justified by its commit
  message.

## Things flagged

### Balancer math (`score_by_density`, `best_binding_score`, `bootstrap_burn_rate`, `project_used_percent`) — untouched

`score_by_density`'s body (`balancer/mod.rs:243-292`) is byte-
for-byte identical to its pre-PR form. Other balancer hunks
are: new imports, new types, `select_provider` /
`scan_provider` gain a `providers_cfg` argument (forced
follow-through for concern 1's mint-on-ingest path), and test-
fixture `migration_threshold: 0.95`. No initiative-04 math
modified.

### `compute_projections` is an additive duplicate, not an extraction — observation, not a smuggled concern

Proposal §5.1 (and risk/05-scope.md A4) committed to "factor
out `compute_projections` that **both** `score_by_density` and
`decide_migration` call." The shipped form is parallel
duplication: `compute_projections` is new; `score_by_density`
is unchanged. This produces strictly stronger bit-for-bit
equivalence (no edit risk on the pinned function) but leaves
duplicated window-fold math that drift could split. For the
multi-concern question this *strengthens* the verdict — an
extracted `compute_projections` could in theory ship as a
prereq refactor PR; the shipped duplicate cannot, because its
only consumer is `decide_migration`. Code-quality finding for
the simplify pass / `review/05-justification.md`; does not
change the multi-concern verdict.

### `find_provider_for_session` — kept, not deleted

Pre-PR `find_provider_for_session` is preserved at
`state/db.rs:2245-2295` and still called from
`ingest_and_emit_session_id` and the resume-detail diagnostic
in `run_repl` / `run_resume`. `resolve_resume` is additive
alongside. No surface deletion smuggled.

### `discover_fixture_turn_session` (main.rs:603-628) — test-fixture infrastructure in production code

Reads `XDG_CONFIG_HOME` → parent directory → `turns.jsonl` and
ingests rows when no session is found through normal mechanism.
In production this file does not exist (returns `None`
immediately); it exists so `tests/initiative_05_migration.rs`
can inject session turns into the runner via env. Load-bearing
for the integration test that exercises
`emit_known_session_id` → `mint_chain_for_invocation_session`
(concern 1). Not a smuggled concern but a code-quality finding
(move to `#[cfg(test)]` or rename+document) — flag for
`review/05-justification.md` or simplify pass.

### `interactive_args` validation drop (`config/model.rs:659-665`) — forced follow-through

The pre-PR check rejected resume strategies without
`interactive_args`. Rev 4 dropped `kind = "config"`, and the
new flag/subcommand resume paths don't need
`interactive_args`. Mechanically pairs with the
`compose_resume_args_rejects_config_kind` negative test added
at `config/model.rs:1840-1854`. Sits inside concern 5.

### No hunk fails to map to a concern

Every hunk in the diff traces to one of the nine concerns or
is forced follow-through (test fixture sweeps, error-message
tweak, re-export plumbing). No unrelated bug fix or refactor
smuggled in.

## Cross-checks against the AGENTS.md split rules

- **"Large deletion is its own PR."** Not applicable — the
  PR is net +5,000 code lines. No load-bearing surface is
  removed; `find_provider_for_session` is preserved.
- **"Additive changes go before behavioral changes."**
  Satisfied at the commit level: `15c121a` (init package
  backfill, docs only) → `a344bd0` (proposal + risk gates,
  docs only) → `91403a0` (single feat commit with tests).
  No in-PR test-before-impl split because the integration
  tests ship in the same commit as the code; the earlier
  two commits are pure-docs.
- **"Dependency order matters."** Initiative 05 depends on
  initiative 04 (`provider_quotas.exhausted_at`,
  `score_by_density`'s past-reset skip). 04 is on `main`
  (`58aa68d` is the head before this PR). No intra-
  initiative split signal beyond rev 4's analysis.

## Summary

Ship as one PR. 18 code files / 27 doc files / ~5,000 net
inserted code lines, three commits (docs-backfill, docs-05,
code-05). Every code file load-bearing for at least one
other; every concern mechanically fused with at least two
others by virtue of shared types (`ResolvedResume`,
`TransitionReason`, `MigrationError`, `SessionStorage`,
`migration_threshold`) or shared tables (`session_chains`,
`session_chain_segments`, `session_turns.is_compaction_boundary`).
Risk-scope rev 4 already rejected three splits for the right
reasons; the shipped diff confirms.

Two non-blocking observations for downstream gates:

- `compute_projections` is an additive duplicate of
  `score_by_density`, not a shared extraction as proposal
  §5.1 stated. Code-quality finding for the simplify pass.
- `discover_fixture_turn_session` (main.rs:603-628) is
  test-fixture infrastructure in the production binary.
  Code-quality finding; should be moved under `#[cfg(test)]`
  or renamed + documented.

Neither argues for a split. Verdict: **single-concern**.
