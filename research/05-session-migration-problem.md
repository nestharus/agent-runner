# Initiative 05 — Session Migration: Problem Statement

## Motivation

Today a session lives on exactly one provider account for its entire life. The runner's `--resume <UUID>` looks up the owning provider via `find_provider_for_session()` (`src-tauri/src/state/db.rs:2062`) and rejects any model whose pool excludes that provider (`src-tauri/src/main.rs:599-628`). When the owning account approaches its 5-hour or weekly quota wall, the user has two bad choices: hit the wall and fail, or copy/paste history into a fresh session manually.

Initiative 04 lands reactive exhausted routing — once a provider hits a quota error, the balancer routes new invocations elsewhere — but that does not move existing sessions. A user who is mid-conversation on `claude` and has weekly headroom on `claude2` cannot keep going.

## What "migration" means here

Anthropic's Claude Code and OpenAI's Codex CLI both store the canonical conversation transcript as a local JSONL file under each CLI's HOME (`~/.claude/projects/<cwd-hash>/<UUID>.jsonl`, `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<UUID>.jsonl(.zst)`). `--resume` reads that JSONL and replays it as conversation history; the upstream API is stateless. Therefore migrating a session from `claude` to `claude2` reduces to: copy the JSONL into the target HOME's tree, append a chain-segment row, and spawn the next turn on the target provider.

## Why stickiness matters for cache hits

Anthropic's prompt cache is org-scoped today and workspace-scoped after Feb 5 2026. Within a single conversation, every turn extends the same prefix; the assistant's prior turn writes the cached prefix and the next user turn hits it at 0.1× input pricing. **Migration costs one cache write at 1.25× input pricing on the first post-migration turn** (or zero, if both providers share an Anthropic org/workspace).

Two implications:

1. Different agents on the same provider account do **not** share cache with each other — caches are per-prefix, and each agent has its own system prompt + history. Stickiness across agents on one account does not buy cache hits.
2. Stickiness **within** a single conversation does buy cache hits. Migrating mid-conversation costs ~1.25× one prefix rewrite. Break-even after one subsequent turn.

Rev 5 policy update: resume is rare and happens between invocations, so the policy should pick the best-scored storage-backed provider at every resume. Cache stickiness is not a useful gate because agents fan out and often miss cache anyway.

## What needs to change

1. **Resume without `-m`.** `agents --resume <UUID>` requires `--model` today (`main.rs:318-321`). Lift that — infer the model from the latest invocation row carrying that session_id; fall back to the model the chain was created under.

2. **Chain identity decoupled from session_id.** A migrated session lives on two providers; reusing the same UUID is risky (Claude Code's behavior on duplicate session-id is undocumented; Codex's local sqlite picker indexes by UUID). Migration mints a new UUID on the target side. "Session id" is no longer the stable identity for the conversation. Introduce `chain_id` as the stable identity; let session_id vary per segment.

3. **Append-only migration ledger.** Per the user's framing: "an appendum where we can see the log of providers and which turn we ended on + when we switched and use that to determine which provider a session is currently on." `session_chain_segments` records each (chain, provider, session_id, started_at, ended_at, last_turn_id, transition_reason).

4. **Two chains can share a session_id.** This isn't pathological — a chain that was migrated mid-conversation, where the original side then continued independently before the user noticed, can fork the upstream UUID across two now-independent chains. The resolver must surface both with previews and let the user pick.

5. **Best-on-resume policy.** At resume entry, pick the highest-scored storage-backed provider and migrate when it differs from the active segment's provider. If the active provider is marked exhausted, pick the best-scored storage-backed sibling.

## Open questions (resolved in answers doc)

- Q1: Does Anthropic prompt cache transfer across OAuth accounts?
- Q2: Is `claude --resume` purely local, and does copying the JSONL across HOMEs work?
- Q3: Same questions for Codex, given its sqlite picker index.
- Q4: When can we infer model from history with confidence vs. require user input?
- Q5: How to disambiguate when two chains share a session_id?
- Q6: What threshold should trigger migration? Per-window or aggregate?
- Q7: Where in the model TOML do we declare the per-CLI session-storage layout?

## Non-goals

- Mid-process migration during a single `repl` session (would require hooking provider switches into the running Claude Code process; not exposed).
- Cross-org cache prophylaxis — the runner has no way to detect orgs from OAuth state, and the cost (≈1.25× one prefix rewrite) is bounded.
- Retroactive merging of orphaned `session_turns` rows into chains beyond first-read backfill.
- Garbage collection of stale segments or copied JSONLs.
- Cross-CLI migration (e.g. claude → codex). The transcript formats differ; out of scope.

## Dependencies

Initiative 04 must land first — the migration trigger reads `provider_quotas.exhausted_at` (added by 04) and reuses `score_by_density`'s projection (kept by 04). 05 adds the chain layer on top of an unchanged balancer.
