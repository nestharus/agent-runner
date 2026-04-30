# Initiative 05 — Phase 4 Audit Risk Report (Rev 4)

**Verdict: LOW**

Rev 4 resolves both Rev 3 MEDIUM findings. The unverified Codex
`experimental_resume` path is no longer load-bearing: Codex cross-account
migration is deferred, `kind = "config"` is not introduced, and the proposal
adds a typed `MigrationError::CodexMigrationDeferred` guard. The incomplete
§13.1 supported-surface track is also repaired: all problem-map §4 paths are
enumerated and the new SQL observability queries are syntactically valid
against the proposed chain schema plus the current `invocations` schema.

The chain abstraction remains coherent. The Rev 2/3 carry-over checks still
hold: schema/index shape, resolver SQL, `RETURNING` race guard, projection
extraction, compaction-anchor lookup, migration trigger, provider-name keying,
and `default_model` fallback are unchanged or still internally consistent.

One new low-severity cleanup item surfaced outside the proposal body:
`research/05-session-migration-hookpoints.md` still contains pre-Rev-4
implementation notes for adding `zstd` and a `ResumeKind::Config` arm. The
final proposal and locked answers supersede those notes clearly, so this is
not a gate blocker, but it should be cleaned before Phase 5 implementation
handoff to avoid accidental reintroduction of the removed surface.

## Concern-by-concern findings

### A1. Rev 3 A7 Codex `experimental_resume` finding — RESOLVED

Rev 3 flagged A7 because the proposal depended on Codex
`experimental_resume="<absolute path>"`, while verification did not find a
documented or working CLI surface.

Rev 4 removes that dependency from the supported v1 surface:

- §1 Rev 4 changes state that Codex providers remain declarable but cannot be
  migration sources or targets in v1.
- A7 now says Codex file-copy migration is unverified and deferred.
- §6 step 1 returns `MigrationError::CodexMigrationDeferred { provider }`
  when the active provider's storage is `kind = "codex"`.
- §6 step 3 returns the same error if the target provider storage is
  `kind = "codex"`.
- §7 keeps only `ResumeStrategyKind::Flag` and
  `ResumeStrategyKind::Subcommand`.
- §9.1 keeps `SessionStorage::Codex { sessions_dir }` for chain identity but
  marks it identity-only for migration.
- §15 makes Codex migration deferral the explicit unresolved item.

Search results are consistent with the new design. The proposal contains
`experimental_resume` only in the Rev 4 change note, A7 evidence, and removed
surface statements. It contains `ConfigArgument` and
`ResumeStrategyKind::Config` only in the sentence saying they are NOT
introduced in v1. It contains `kind = "config"` only in "drop/no strategy"
statements and rollback cleanup.

The locked answer Q3 is also consistent. It preserves the old Codex answer
only under "Original answer, superseded by Rev 4", then states that
`experimental_resume` is not a real key and cross-account Codex migration is
deferred. Q7 says `kind = "codex"` is declarable for chain identity but does
not participate in migration, and no `kind = "config"` strategy ships.

The verification artifact supports the deferral. It records `NOT_FOUND`,
explains that `codex-cli 0.125.0` exposes no path-resume flag, and notes that
unknown `-c` keys are tolerated without proving the CLI reads them. That is
exactly the invalidator A7 now carries.

### A2. Rev 3 §13.1 supported-surface finding — RESOLVED

Problem-map §4 lists twelve currently supported or user-reachable paths:
the two REPL forms, the `resume -m ... --session-id ... -f ...` form, the
top-level prompt resume form, no-prompt resume through `run_repl`, trace,
direct user-terminal CLI ingestion, `session_scan`, `quota_check`,
Tauri/GUI `test_model_with_db_path`, ordinary balanced-execution ingestion,
and post-success capture through `ingest_and_emit_session_id`.

Rev 4 §13.1 enumerates all twelve paths explicitly and adds the frontend
non-surface note. The wording distinguishes "must keep working unchanged"
from surfaces that intentionally gain optional-model behavior or additive
`chain_id` output.

The cohort coverage check is now concrete. Existing configured users are
covered by the CLI, trace, diagnostics, GUI test-model, balanced execution,
and post-success capture paths. UI-only Claude Code / Codex users are covered
through `session_turns` ingestion and model fallback, with the correct
`sessions.toml` dependency called out.

The migration and rollback subsections now cover the Rev 4 deltas. The
migration path says Codex providers participate in chain identity but not
cross-account migration in v1. The rollback path says prior binaries ignore
the new tables/column and there is no `kind = "config"` schema/config drift
to undo.

### A3. §13.1 SQL observability queries — VERIFIED

The six SQL queries in §13.1 compile against an in-memory schema containing:

- proposed `session_chains`,
- proposed `session_chain_segments`,
- proposed `session_turns.is_compaction_boundary`,
- current `invocations` columns from `src-tauri/src/state/db.rs`.

`sqlite3` is not installed in this environment, so the syntax check used
Python's built-in SQLite engine against that schema. Q1 through Q6 all
prepared and executed successfully with empty result sets.

The column references are valid:

- Q1 uses `session_chain_segments.chain_id`, `session_id`, `started_at`,
  `provider_name`, and `ended_at`.
- Q2 uses `transition_reason`, `started_at`, and `last_turn_id`, all in
  `session_chain_segments`.
- Q3 uses `session_id` and `chain_id`.
- Q4 joins active segment rows to `session_turns` and uses
  `is_compaction_boundary`, `turn_id`, `timestamp`, `role`, and `source_file`.
- Q5 groups by `chain_id` and filters `transition_reason`.
- Q6 anti-joins `session_chain_segments` to current `invocations` on
  `provider_name`, `session_id`, and `created_at`.

Non-blocking note: Q2 compares RFC3339-style `started_at` text to
`datetime('now', '-1 day')`, whose default string uses a space separator. The
query is syntactically valid as requested, but using `strftime('%s', ...)`
as Q6 does would make the "past 24h" predicate more exact for operators.

### B1. `MigrationError::CodexMigrationDeferred` guard — VERIFIED

The new error is introduced in the right place: §6 step 1, before source-path
resolution can touch Codex rollout files. That is an early-return guard for a
Codex active provider.

The target side is also guarded. §6 step 3 returns
`MigrationError::CodexMigrationDeferred { provider }` if the computed target
storage is `kind = "codex"`. This prevents Codex from being a migration target
even when a migration decision was otherwise reached.

The wording is explicit that v1 supports Claude-Code migration only. Codex
segments are still recorded for ingestion-observed sessions and same-provider
resume-by-id remains delegated to Codex's native resume subcommand.

### B2. Codex storage remains parseable and identity-only — VERIFIED

§9.1 defines the new tagged storage union with `ClaudeCode { projects_dir }`
and `Codex { sessions_dir }`. The TOML examples include both
`kind = "claude_code"` and `kind = "codex"`. Validation deliberately does not
apply migration-target-pair uniqueness rules to Codex providers because they
do not participate in the migration mechanic.

The body states the intended split in multiple places:

- Codex storage is read by the chain layer.
- Codex participates in chain-id mint at ingestion.
- Codex participates in the segment ledger.
- Codex same-provider resume-by-id remains available.
- Codex cross-account migration is blocked with
  `MigrationError::CodexMigrationDeferred`.

That satisfies the "parseable but not migratable" requirement.

### B3. Decision/mechanic split for Codex — VERIFIED WITH NOTE

§5 says a migration-eligible target in v1 is a sibling with
`[providers.session_storage] kind = "claude_code"`. `kind = "codex"` is ignored
as a migration target.

For a Codex active provider, the decision layer may return `Migrate` if a
Claude-Code sibling is otherwise eligible. The migration mechanic then returns
`MigrationError::CodexMigrationDeferred`. If no Claude-Code sibling exists,
the decision layer returns `Stay` and logs that Codex migration is deferred.

That split is acceptable. It keeps threshold/manual policy observable without
pretending Codex file copy works.

Non-blocking naming note: the test
`decide_migration_returns_codex_deferred_for_codex_provider` is slightly
misnamed, because the described assertion is `Migrate` or `Stay` plus a log,
not a returned `MigrationError`. The mechanic test pins the actual error.

### C1. `kind = "config"` cleanup — VERIFIED IN PROPOSAL

The supported resume strategy section is clean. §7 is titled "Resume
strategies remain `flag` and `subcommand`" and the enum contains only:

- `Flag`,
- `Subcommand`.

The Codex provider example uses:

```toml
[providers.resume]
kind = "subcommand"
subcommand = ["resume"]
```

No proposal example introduces `kind = "config"`.

The README update list says resume strategies remain `kind = "flag"` and
`kind = "subcommand"` and no `kind = "config"` strategy ships in v1. The
rollback path explicitly says there is no `kind = "config"` drift to undo.

### C2. `ConfigArgument` and `ResumeStrategyKind::Config` cleanup — VERIFIED

The proposal does not introduce `ConfigArgument`. It names the enum only to
state that it is NOT introduced in v1.

The proposal does not introduce `ResumeStrategyKind::Config`. It names the
variant only to state that it is NOT introduced in v1.

`compose_resume_args()` still gains `target_jsonl_path: Option<&Path>`, but
§7 says the parameter is reserved for the deferred Codex follow-up and no new
strategy arm is added. The `flag` and `subcommand` tests explicitly assert
unchanged argv with and without a target path.

### C3. `zstd` and Codex `.zst` cleanup — VERIFIED IN PROPOSAL

The Rev 2 `.zst` paragraph remains in the historical change log only and is
immediately marked "Superseded by Rev 4". The Rev 4 change block says `zstd`
is not added in v1.

§6 step 5 now says "Plain JSONL only" and scopes the copy mechanic to
`kind = "claude_code"`. §6.6 says the compaction-aware target build is only
exercised for Claude-Code plaintext JSONL.

`src-tauri/Cargo.toml` and `src-tauri/Cargo.lock` do not contain `zstd`.

### C4. Removed Codex tests — VERIFIED

The old Rev 3 Codex tests are absent from §11:
`migration_zst_round_trip_preserves_post_offset_bytes`,
`migration_copies_codex_rollout_with_zst_extension`, and
`migration_composes_codex_experimental_resume_argv`.

The replacement tests are present: `chain_mint_works_for_codex_ingestion`,
`decide_migration_returns_codex_deferred_for_codex_provider`, and
`migration_mechanic_errors_codex_deferred_on_codex_active_provider`.

### D1. Schema and resolver carry-over checks — STILL VERIFIED

The schema design remains the same additive shape verified in Rev 2/3:

- `session_chains` is keyed by `chain_id`.
- `session_chain_segments` records provider/session segments and active
  status via `ended_at IS NULL`.
- `idx_segments_session` supports session-id lookup.
- `idx_segments_chain_active` supports active-segment lookup.
- `session_turns.is_compaction_boundary` is an additive defaulted column.

The resolver still looks up chains by `session_id` or `chain_id`, handles
ambiguity through the 24-hour rule and previews, resolves the active segment,
and resolves the model by override, latest invocation, chain model, provider
default, then typed failure.

No Rev 4 edit undermines those mechanics.

### D2. `RETURNING` and segment race guard — STILL VERIFIED

§3.2 still uses:

```sql
UPDATE session_chain_segments
SET ended_at = ?now, ...
WHERE id = ?id
  AND ended_at IS NULL
RETURNING id;
```

That is the same concurrency guard Rev 2/3 accepted. Bundled SQLite support
for `RETURNING` was previously verified and Rev 4 does not alter the database
dependency story.

The migration tests still include
`migration_returning_clause_aborts_on_concurrent_close`.

### D3. Projection extraction and migration trigger — STILL VERIFIED

§5.1 still frames `compute_projections` as a refactor, not a behavior change.
`score_by_density` remains the selection owner after calling the extracted
projection helper.

The migration trigger still runs after `resolve_resume`, reads the active
provider's `exhausted_at`, then evaluates projected usage against the threshold.
Rev 4 only narrows migration-eligible storage to Claude-Code providers.

The `compute_projections` equivalence test group remains in §11.1.

### D4. Compaction-aware Claude migration — STILL VERIFIED

The compaction-aware target build still:

- queries the latest `session_turns.is_compaction_boundary = 1` row,
- scans plaintext source JSONL for the matching turn line,
- returns `CompactionBoundaryNotInJsonl` if the boundary cannot be found,
- copies from the latest boundary offset,
- preserves pre-compaction rows in `session_turns`.

Rev 4 narrows this to Claude-Code plaintext JSONL and defers Codex compaction
as part of the broader Codex migration deferral. That is internally consistent.

### D5. Provider-name keying and `default_model` fallback — STILL VERIFIED

Provider quota state remains keyed by provider name, and chain segments store
`provider_name`. `exhausted_at` remains a provider-account signal, not a
session-local signal.

The UI-session model fallback path remains the same: latest invocation, chain
model, provider `default_model`, then `ModelInferenceImpossible`. Q4's Rev 4
update explicitly says Codex sessions still mint chain identity and can resume
by id within the same provider, while cross-account migration is deferred.

### E1. New Codex deferral tests — ADEQUATE

`chain_mint_works_for_codex_ingestion` pins the identity-only guarantee. It
asserts that ingesting a Codex turn creates `session_chains` and
`session_chain_segments` rows even though migration is deferred.

`decide_migration_returns_codex_deferred_for_codex_provider` pins the policy
boundary. With a Claude-Code sibling, it expects a migration decision to remain
observable. Without one, it expects `Stay` and a logged deferred reason.

`migration_mechanic_errors_codex_deferred_on_codex_active_provider` pins the
hard stop. It asserts `MigrationError::CodexMigrationDeferred { provider }`
and no target file or segment write.

Together these tests cover parseability, chain identity, decision behavior,
and the migration mechanic's early-return guard.

### F1. §1 Rev 4 change block — VERIFIED

Every claim in the Rev 4 change block is reflected in the body:

- §6 step 1 and step 3 block Codex source/target migration.
- §6 step 5 and §6.6 remove Codex `.zst` migration from v1.
- §7 drops `kind = "config"` and keeps only `flag` / `subcommand`.
- §9.1 keeps `kind = "codex"` for forward-compatible identity.
- §11 removes Codex `.zst` / `experimental_resume` tests and adds Codex
  deferred/identity coverage.
- §13.1 removes Codex migration from the supported migration surface.
- §15 replaces Codex path-resume uncertainty with a broader migration
  deferral entry.

### F2. §15 unresolved cleanup — VERIFIED

The old Codex compaction-format residual is now explicitly subsumed by the
broader Codex migration deferral. §15 still lists "Codex compaction format",
but the entry says it is subsumed and that `codex-turns` can continue to ingest
without `is_compaction_boundary`.

That is not a stale blocker. It is a useful pointer for the future Codex
migration follow-up.

### F3. §13.1 migration and rollback cleanup — VERIFIED

The migration-path subsection says Codex participates in identity but not
cross-account migration in v1.

The rollback-path subsection says the new tables/column are inert under the
prior binary and there is no `kind = "config"` config drift to unwind.

Those two statements close the specific Rev 4 cross-reference cleanup request.

## Low-severity cleanup finding

### L1. Stale hookpoints research still names removed Codex implementation work — LOW

`research/05-session-migration-hookpoints.md` is unchanged from the old design
in two places:

- It still says to add `zstd = "0.13"` and implement `.zst` handling in the
  migration helper.
- It still says `compose_resume_args()` should add a new
  `ResumeKind::Config` arm, `ConfigArgument`, `config_key`, and `argument`.

This conflicts with Rev 4's final proposal and locked Q7. It is not a
proposal-body defect, because §1, §6, §7, §9.1, §11, §13.1, §15, and
`research/05-session-migration-answers.md` all clearly supersede the removed
surface. It is also not a chain-abstraction defect.

Severity is LOW because an implementer following the final proposal will do
the right thing, and §11 contains tests that would catch accidental
`config`/Codex migration reintroduction. Still, the stale hookpoints should be
edited or annotated before Phase 5 implementation starts, because hookpoints
are commonly used as the mechanical code map.

## Verdict rationale

Rev 4 converts the Rev 3 MEDIUM risks into explicit, testable scope:

- Codex migration no longer depends on unverified `experimental_resume`.
- Codex storage remains parseable and identity-only.
- The migration mechanic has a typed early-return error for Codex.
- The supported-surface track now covers every problem-map §4 path.
- SQL observability is concrete and syntactically valid.
- Removed `config`/`zstd` surfaces are absent from the proposal body and test
  list except where explicitly marked as removed or superseded.

No new blocking or medium-severity proposal risk surfaced. The only new issue
is stale implementation-hookpoint research outside the proposal source of
truth. Treat that as a Phase 5 handoff cleanup, not a risk-gate failure.

Verdict: **LOW**.
