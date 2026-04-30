# Initiative 05 — Session Migration

**Status:** awaiting-decision
**Depends on:** Initiative 04 (reactive routing — uses `provider_quotas.exhausted_at` and `score_by_density` projection)
**Blocks:** —

## Problem (user framing, verbatim)

Captured from the conversation that opened this initiative on 2026-04-30:

> We have all of the turns at this point in the database, right? So I'm
> wondering if there's a way for us to move sessions between providers.
> Like move a claude session to a different claude provider by copying
> it over.
>
> What I am getting at is that as a provider starts to get high, we
> don't want to use it as a "UI" for the user anymore. We want to swap
> to a different provider.
>
> In general we need to stick with a provider for a period of turns or
> until 98% in order to utilize cache hits. Hmm.. actually.. cache hits
> if we run other agents, that won't hit the cache right? it will miss?
> it's only for consecutive turns? So why wouldn't we, whenever we
> resume a session, migrate it to the provider with the most amount of
> usage remaining?
>
> Also, I'm not sure if we have to pass in a model when we a resume a
> session right now but the resume session should just be `--resume
> session-id`. Find the most recently used session id that matches. If
> two session ids are equal to each other and within the last 24 hours
> of usage, have the user use. We also need to modify the database if
> we are going to be migrating session ids because we will get
> duplications. We need to show a session id as "migrated" or
> something or perhaps just change the provider that it is on? So we
> just edit the session id to be on a different provider when we
> migrate it? I don't think that's good though. I'd rather have an
> appendum where we can see the log of providers and which turn we
> ended on + when we switched and use that to determine which provider
> a session is currently on. Two sessions can still share the same id,
> but they won't be part of the same chain. Like we'd resolve to two
> different chains. Those chains can't really be uniquely identified.
> The user would just have to choose between them and we could maybe
> should information within them, like the last few turns, to help the
> user figure out which is which.

Follow-up the same day, on model inference at resume time:

> For resume, if it is a UI session it won't have gone through agents,
> meaning we won't have a model assigned to the session. For UI
> sessions it just uses whatever the default is. For agent sessions,
> we know the model assigned to the session. We aren't tracking it
> right now though. We can associate that model with the session and
> when we resume bring that model back up. This means that agents
> won't have to pass the model arg in anymore. They can just resume
> and get the correct model back that they were previously using.
>
> This also allows us to port "agent" sessions AND ui sessions between
> different providers on resume.

Follow-up on compaction:

> Another tricky thing CLIs will compact sessions, rewriting turns. If
> we restore "sessions" without any of the compaction we just get a
> transcript that is too long. We'd need to detect compactions and
> then create a "new" session state with the compacted form. This
> allows us to also look at the original form before the compaction
> if we want to retrieve details pre-compaction. It allows us to more
> accurately search logs.

## Scope

**In scope:**

- Two new SQLite tables: `session_chains` (stable chain identity) and
  `session_chain_segments` (append-only ledger of provider/session_id
  occupancy).
- Resolver `resolve_resume(state, config, input, model_override) →
  ResolvedResume`, replacing today's `find_provider_for_session()`.
- Sticky-then-migrate policy at resume time, default 95% projected-usage
  threshold (configurable per-model via `[migration] threshold = 0.95`),
  hard trigger when `exhausted_at` is set.
- Migration mechanic: copy source JSONL to target HOME, mint new
  target-side session_id, append segment ledger entry, compose target
  argv per provider's resume strategy.
- Codex chain identity remains in scope through ingestion, segment ledger,
  and same-provider resume-by-id; Codex cross-account migration is deferred
  to a follow-up PR per `research/05-codex-resume-verification.md` and
  proposal §15.
- New per-provider `[providers.session_storage]` block in model TOML
  (kinds: `claude_code`, `codex`).
- New per-provider `default_model` field in `providers.toml` for UI
  session model fallback.
- Lift `--resume requires --model` enforcement: model inferred from
  chain's invocation history → chain's recorded model → provider's
  `default_model` → fail.
- Compaction-aware target build: new `session_turns.is_compaction_boundary`
  column; truncate target JSONL at latest compaction boundary; retain
  pre-compaction turns in source and DB for search/audit.
- Updated `claude-code-turns` reference adapter to emit
  `is_compaction_boundary`.
- New CLI affordances: `--migrate <provider>` (force migration) and
  `agents resume --list <UUID>` (diagnostic chain dump).
- `chain_id` field added to `trace --json` output.

**Out of scope:**

- Mid-process migration during a single `repl` session.
- Cross-org cache prophylaxis (the runner has no way to detect orgs).
- Cross-CLI migration (claude → codex etc.).
- Codex cross-account migration and compaction adapter update — no
  documented Codex path-resume surface is available in v1; deferred to a
  follow-up PR.
- `transcript_preview` adapter for ambiguous-chain disambiguation —
  deferred; v1 ships chain_id + provider + turn count without snippet
  text.
- Garbage collection / archival of stale segments and copied JSONLs.
- Frontend chain visibility (PoolsView/StatusView unchanged).
- Per-chain quota accounting in the balancer.
- Retroactive merging of orphaned `session_turns` rows beyond first-read
  backfill.

## Reference framework

- Cache scoping and pricing — Anthropic prompt caching docs
  (org-scoped today, workspace-scoped Feb 5 2026; cache write 1.25×,
  read 0.1×). Codex caching is org-scoped, free writes.
- Local-replay model — Claude Code can resume from copied local JSONL.
  Codex same-provider resume depends on the target HOME's state DB; Codex
  cross-account migration is deferred until a documented path-resume or
  state-DB-aware migration path exists.
- `~/ai/conventions/no-backwards-compatibility.md` — replace
  `find_provider_for_session()`, do not deprecate.
- `~/ai/initiatives/01-risk-and-value-axes.md` — value computation:
  this initiative reduces integration risk (quota walls blocking
  conversations) and increases value (cache continuity, agent
  ergonomics) at cost of one cold-cache cost per migration.

## Open questions answered

See `research/05-session-migration-answers.md` for locked answers.
Summary:

- Q1: Anthropic cache is org-scoped; cross-org migration costs ~1.25×
  one prefix rewrite, break-even after one cache read.
- Q2: Claude Code `--resume` is purely local; copying JSONL across
  HOMEs works; mint new session_id on target side.
- Q3: Rev 4 verification found no documented working Codex path-resume
  surface; Codex cross-account migration is deferred, while Codex chain
  identity remains supported.
- Q4: Model inference fallback chain — invocation history → chain
  model → providers.toml default → fail.
- Q5: Two chains sharing session_id disambiguate by 24h window then
  user choice with previews.
- Q6: 95% projection threshold default for migration trigger;
  configurable per-model.
- Q7: `[providers.session_storage]` declared per-provider with kind
  discriminator; both source and target must declare for migration.
- Q8: Compaction handled via `is_compaction_boundary` flag on
  `session_turns`; target JSONL truncated to start from latest
  boundary.

## Artifacts

| Phase | Files |
|-------|-------|
| Research — problem | `research/05-session-migration-problem.md` |
| Research — answers | `research/05-session-migration-answers.md` |
| Proposal | `proposals/05-session-migration.md` |
| Risk | `risk/05-audit.md` (Rev 4: LOW), `risk/05-scope.md` (Rev 4: LOW), `risk/05-shortcut.md` (Rev 4: LOW), `risk/05-supported-surface.md` (Rev 4: LOW) |
| Hookpoints | `research/05-session-migration-hookpoints.md` |
| Review | (pending — post-implementation) `review/05-justification.md`, `review/05-multi-concern.md`, `review/05-test-audit.md` |
| Implementation | (pending Phase 5 user gate) |

## Decision gate

User reads the risk-cleared proposal and picks: accept → implementation phase; reframe; follow-up research.

The proposal ships as one PR per `proposals/05-session-migration.md §1` because schema, resolver, executor, and CLI are mutually dependent. Three split candidates were evaluated by `risk/05-scope.md` (schema-only prereq, resolver+CLI / migration-mechanic split, compaction carve-out); each was rejected as producing dead intermediate state. The single-PR scope will be re-validated by the multi-concern PR review gate post-implementation.

## Log

- **2026-04-30** — Initiative opened. User raised migration question;
  parallel research (code survey + cache/import semantics) dispatched
  and returned. Problem doc + answers doc + proposal v1 drafted.
- **2026-04-30** — User added two follow-ups: (a) UI vs agent sessions
  + model inference from history, (b) compaction-aware target build.
  Proposal updated to v1.1; answers extended with Q8 (compaction).
- **2026-04-30** — User flagged that the work skipped the upstream
  product-strategy / initiative-package step. Initiative file
  backfilled to capture verbatim framing.
- **2026-04-30** — Risk gates Rev 1: audit MEDIUM (3 FLAGs: cwd_hash
  encoder, .zst truncation deps, is_compaction_boundary ingest
  plumbing; 2 UNVERIFIED: score_by_density projection access,
  backfill perf), scope MEDIUM (missing test, README scope undersized,
  cross-ref renumbering), shortcut MEDIUM (F1 legacy fallback in §14,
  F2 silent fallback at §6.6 step 3).
- **2026-04-30** — Proposal Rev 2 written addressing all 11 distinct
  findings: §1 Rev 2 changes block; §3.4 ingest plumbing enumerated;
  §5.1 `compute_projections` refactor committed; §6.1 cwd_hash
  derived from source path (no encoder); §6.5 zstd dep + atomicity
  story; §6.6 step 3 hard error; §6 cross-refs fixed; §11 added
  three tests; §12 README scope extended; §14 no-runtime-fallback
  decision locked; §8.5.1 `agents migrate-db` shipped unconditionally;
  §11 added three more tests for migrate-db, malformed path, and
  refusal-to-start.
- **2026-04-30** — Risk gates Rev 2: audit LOW, scope LOW, shortcut
  LOW. All Rev 1 findings resolved; minor non-blocking nits patched
  in a final cleanup pass.
- **2026-04-30** — Phase 5 hookpoint research complete:
  `research/05-session-migration-hookpoints.md`. Maps every Rev 2
  proposal action to file:line code sites. Surfaces 10 implementer
  notes including: only 2 production callers of
  `find_provider_for_session` (clean delete); `WindowProjection`
  type doesn't exist (refactor must introduce it); single-row
  vs batch `INSERT OR IGNORE INTO session_turns` divergence at
  `state/db.rs:1962-1974` vs `1998-2014` (implementer choice on
  whether to align them); `claude-code-turns:68-70` filters by
  record `type` — compaction records are a different type that
  requires real JSONL sample inspection before adapter update;
  `agents resume --list` needs a new Subcommands variant; the pre-Rev-4
  Codex compressed migration hookpoint was removed by the Codex deferral.
  Status: awaiting-decision (Phase 5 human gate before Phase 6
  implementation).
- **2026-04-30** — Proposal Rev 4 deferred Codex cross-account
  migration after `research/05-codex-resume-verification.md` found no
  documented working path-resume surface. `kind = "config"` /
  `experimental_resume` and the Codex compressed-copy path were removed;
  Codex chain identity remains in v1 through ingestion, segment ledger,
  and same-provider resume-by-id. Rev 4 risk gates returned LOW across
  audit, scope, shortcut, and supported-surface.
