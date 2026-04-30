# Initiative 05 — Locked Answers

## Q1: Anthropic prompt cache scoping

**Cache is org-scoped today, becomes workspace-scoped Feb 5, 2026.** Two OAuth accounts in the same Anthropic org share cache; two accounts in different orgs miss on first turn after migration.

Pricing on Anthropic:

- Uncached input: 1.0× base
- 5-minute cache write: 1.25×
- 1-hour cache write: 2.0×
- Cache read: 0.1×

Cross-org migration cost = one prefix rewrite at 1.25× (5-min cache) on the first post-migration turn; break-even after one subsequent re-read at 0.1×. Same-org migration is free.

OpenAI / Codex caching is also per-org, automatic, and **free for writes** (no surcharge); reads are discounted up to ~90% for prefixes ≥ 1024 tokens.

Sources:
- https://platform.claude.com/docs/en/build-with-claude/prompt-caching ("Caches are isolated between organizations… Starting February 5, 2026, prompt caching will use workspace-level isolation")
- https://platform.claude.com/docs/en/about-claude/pricing
- https://developers.openai.com/api/docs/guides/prompt-caching

After Feb 5 2026 the runner cannot guarantee shared cache across "claude" and "claude2" accounts within the same org if they sit in different workspaces. Acceptable: cost is bounded; user can colocate workspaces.

## Q2: Cross-account session import — Claude Code

**`claude --resume <UUID>` is purely local.** It reads `~/.claude/projects/<cwd-hash>/<UUID>.jsonl`, replays the recorded turns into the API, and writes new turns back to the same JSONL. The Anthropic API is stateless — the JSONL is the source of truth.

Therefore copying `~/.claude/projects/<hash>/<UUID>.jsonl` to `~/.claude2/projects/<hash>/<UUID>.jsonl` and running `claude --resume <UUID>` under HOME=`~/.claude2/...` resumes the same conversation. **Confirmed by direct fs inspection** (`/home/nes/.claude2/projects/...` is a parallel tree).

`--session-id <UUID>` forces the UUID for new sessions; behavior on dup UUID across HOMEs is undocumented but each HOME is independent so collisions are intra-HOME, not cross-HOME. **Safer practice**: mint a new UUID on the target side at migration time and record the old↔new mapping in `session_chain_segments`.

Sources:
- https://code.claude.com/docs/en/agent-sdk/sessions
- `claude --help` (run during research): `--session-id` "Use a specific session ID"; `--resume` "Resume a conversation by session ID"

## Q3: Cross-account session import — Codex

**Original answer, superseded by Rev 4**: Codex sessions live at `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<UUID>.jsonl(.zst)`. The original proposal assumed `codex resume <UUID>` and `codex -c experimental_resume="<absolute path>"` both replayed the local rollout.

Codex maintains a per-HOME sqlite picker index (`state_5.sqlite`, `logs_2.sqlite`). The original proposal assumed a pasted-in rollout might not appear in `codex resume`'s picker until the index sees it, but that `-c experimental_resume="<abs path>"` could bypass the picker. Rev 4 verification invalidates that assumption.

**Rev 4 update**: Verification at `research/05-codex-resume-verification.md` shows `experimental_resume` is not a real Codex config key; cross-account Codex migration is deferred to a follow-up PR. The original Q3 framing about path-aware resume is preserved for the eventual follow-up but is not load-bearing for v1.

Sources:
- https://github.com/openai/codex/discussions/3827 (Session/Rollout Files spec)
- https://developers.openai.com/codex/cli/reference (checked during Rev 4 verification; no working `experimental_resume` key found)
- Direct fs inspection: `/home/nes/.codex2/sessions/...` is a parallel tree.

## Q4: Model inference at resume time

There are two classes of session, distinguished by how they were started:

**Agent sessions** were spawned by agent-runner with a known `-m <model>`. The runner already knows the model and records it in the invocation row. The fix is to also write it into `session_chains.model_name` at chain mint, so future resumes pick it up without the caller passing `-m`.

**UI sessions** were started by the user running the bare CLI (`claude`, `codex`) outside agent-runner. They surface here only via `session_turns` ingestion. The runner has no per-session model — the upstream CLI used whatever its own default is. For these, fall back to a per-provider `default_model` declared in `providers.toml`.

Resolution fall-back order:

1. `invocations.model_name` of the latest invocation tied to any segment of the resolved chain. (Always populated for agent sessions; populated for UI sessions only after the first agent-runner-mediated resume.)
2. `session_chains.model_name` (recorded at chain mint).
3. `providers.<active_provider>.default_model` from `providers.toml`. (UI fallback.)
4. Fail: `--resume requires --model — no default_model configured for provider <name>`.

A user override via `-m <model>` always wins, but is validated against the chain's owning-provider pool (existing logic at `main.rs:599-628`).

Implication for the migration mechanic: both agent and UI sessions migrate identically for Claude-Code providers — the chain layer abstracts over how the session was started. Codex sessions still mint chain identity through ingestion and can resume by id within the same provider, but cross-account file-copy migration is deferred in v1.

## Q5: Two chains, same session_id

When `SELECT DISTINCT chain_id FROM session_chain_segments WHERE session_id = ?` returns >1 row:

- Filter to chains with `last_used_at >= now − 24h` (tunable).
- If 1 chain remains: pick it.
- If 0 remain: pick max(last_used_at).
- If >1 remain: surface previews and exit 1; user retries with `--resume <chain_id>` (chain_id is also accepted by the resolver).

Last-3-turns preview source: `session_turns` joined to the active segment, ordered by `timestamp` desc, limit 3, role-tagged. Snippet content (first 120 chars) requires a `transcript_preview` adapter — **defer adapter implementation to a follow-up PR**; v1 prints chain_id, last_used_at, active provider, and turn count without snippet text.

## Q6: Migration trigger policy

Pick the best-scored sibling at every resume. The decision uses the existing balancer projection (`score_by_density` produces per-window projected_used_percent at `src-tauri/src/balancer/mod.rs:162-196`) and compares provider binding scores, with ties broken by lower provider index.

Reasoning: resume is rare and happens between invocations, not per turn, so thrashing is not a concern. Cache stickiness rarely buys much because agents fan out and often miss cache anyway.

There is no `migration_threshold` config field and no `[migration]` block. Removed policy config is deleted, not retained as ignored compatibility surface.

A second, hard trigger: if the current provider has `provider_quotas.exhausted_at IS NOT NULL` (set by initiative 04's reactive flag), migrate to the highest-scored storage-backed sibling regardless of the active provider's score.

A short-circuit: if the model's pool has only one provider with `[providers.session_storage]` declared, no migration target exists — stay.

## Q7: Provider session-storage declaration

Each provider in a model TOML gains an optional `[providers.session_storage]` block:

```toml
[[providers]]
name = "claude2"
command = "env -u CLAUDECODE claude2 ..."

[providers.session_storage]
kind         = "claude_code"
projects_dir = "~/.claude2/projects"
```

```toml
[[providers]]
name = "codex2"
command = "..."

[providers.session_storage]
kind         = "codex"
sessions_dir = "~/.codex2/sessions"
```

Without a `session_storage` block, that provider opts out of being a migration target. A Claude-Code provider can still be the source of a migration if the runner can locate its existing JSONL via convention — but for v1, **require both source and target Claude-Code providers to declare storage**. Simplifies path resolution; users who want migration declare it.

**Rev 4 note**: `kind = "codex"` is declarable for forward-compatibility and chain identity (chain_id mint at ingestion, segment ledger, resume-by-id within the same provider), but it does not participate in migration in v1. If a migration trigger reaches a Codex chain, the mechanic returns `MigrationError::CodexMigrationDeferred`.

The `kind` discriminator drives:

- Source path resolution: where the existing JSONL lives.
- Target path layout: where to place the copy.
- Resume composition: Claude uses the existing `[providers.resume]` flag/subcommand; Codex keeps the existing `kind = "subcommand"` shape for same-provider resume. No `kind = "config"` strategy ships in v1.

## Code map (proposal hookpoints)

- `find_provider_for_session()` at `src-tauri/src/state/db.rs:2062-2107` — replaced by `resolve_resume()` (no compatibility shim per `~/ai/conventions/no-backwards-compatibility.md`).
- `resume_model_pool_mismatch_message()` at `src-tauri/src/main.rs:599-628` — operates on the resolved owning provider of the active segment.
- `compose_resume_args()` at `src-tauri/src/executor/cli.rs:246-274` — gains optional `target_jsonl_path: Option<PathBuf>` parameter reserved for the deferred Codex migration follow-up; `Config` strategy kind is not added in v1.
- Top-level `--resume requires --model` enforcement at `src-tauri/src/main.rs:318-321` — deleted; replaced with model inference (Q4).
- `session_capture` write paths — same as today; chain mint hooked in after session_id write succeeds.
- `executor::execute_resume()` (`src-tauri/src/executor/cli.rs:410`) and `execute_interactive()` (`:528`) — extended with optional pre-spawn migration step (copy + segment append).

## Q8: Compaction-aware migration

CLIs compact long conversations — when a session approaches the model's context window, the CLI rewrites earlier turns into a single summary turn so the live API request stays under the limit. The on-disk JSONL keeps growing, but the LIVE conversation state is `[summary, post-compaction turns...]`, not the full uncompacted history.

If migration copies the source JSONL byte-for-byte to the target HOME, the target CLI replays the **full** history into the API and gets a "context window exceeded" rejection. The migration must reproduce the compacted state, not the raw byte stream.

**Strategy:**

1. Extend the turn-script adapter contract with an optional `is_compaction_boundary: bool` flag (parallel to existing `parent_turn_id` and `is_sidechain` optional fields). Adapters that don't track compaction omit the field; the runner treats omitted as `false`.
2. Update `claude-code-turns` to emit the flag when it sees a compaction event in Claude Code's JSONL. (The exact record type Claude Code emits is implementation discovery — Claude Code's open-source adapter scripts and JSONL samples in `~/.claude/projects/...` are the reference.)
3. Add `session_turns.is_compaction_boundary INTEGER DEFAULT 0` and ingest accordingly.
4. At migration time, query the latest compaction-boundary turn for the source `(provider, session_id)`. If one exists: locate that turn's line in the source JSONL by matching turn_id, and copy the JSONL **from that line onward** to the target path. Target CLI replays a session that begins with the compaction summary — same live state as the source CLI had, no overflow.
5. If no compaction boundary exists: copy the full JSONL (current §6 behavior).
6. The original uncompacted JSONL remains in the source HOME unchanged; agent-runner does not delete it. Pre-compaction turns remain in `session_turns` (already the case — ingestion stores everything). Search and audit queries see the full history; only the runtime target sees the compacted slice.

**Bonus from this design**: search across pre-compaction turns is more accurate than searching the upstream CLI's view, because `session_turns` retains every turn ever observed. The compaction-boundary flag lets queries scope to "live state only" (`WHERE timestamp >= latest compaction`) or "full history including compacted-away turns" (no filter).

**Codex compaction**: subsumed by Rev 4's broader Codex migration deferral. `codex-turns` can continue to ingest turns without `is_compaction_boundary`; chain identity works without it. If a later follow-up adds Codex cross-account migration, that PR can revisit compaction-boundary detection.

**Confidence**: medium. The strategy is correct in principle (compaction state is in the JSONL; replay-from-compaction-summary matches what the CLI itself does at runtime). The byte-level details — record types, summary turn structure, edge cases like multi-step compaction — are deferred to the adapter implementation.

## What this proposal explicitly does NOT change

- The balancer scoring math (`score_by_density`, bootstrap cascade, refresh logic).
- Initiative 04's exhausted-flag schema and clear path.
- Existing `session_turns` ingestion (chains read from it; nothing writes back).
- `trace` output existing fields (chain_id is added; existing fields unchanged).
- Within-process REPL provider stickiness (out of scope — migration is between-invocation).
- Cross-CLI migration paths (claude → codex etc. — out of scope).
