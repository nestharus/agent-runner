# Initiative 05 — Live QA Results (post-merge)

**Date:** 2026-04-30
**Binary:** `src-tauri/target/release/oulipoly-agent-runner` built from `2c56310`
**State DB:** `~/.local/share/oulipoly-agent-runner/state.db` (532 MB, 18,580 backfilled chains, 834,085 ingested turns)
**Backups:** `state.db.backup-pre-05`, `claude-opus.toml.pre-05`, `providers.toml.pre-05`

## QA matrix executed

| # | Test | Result |
|---|------|--------|
| 1 | Schema migration on production DB (532 MB, 834K turns) | ✓ no error |
| 2 | Backfill `session_chains` / `session_chain_segments` | ✓ 18,580/18,580 with `transition_reason='imported'` |
| 3 | `is_compaction_boundary` column added | ✓ |
| 4 | Create fresh agent session via `agents -m claude-opus "..."` | ✓ session_id `dd116a3c…`, provider `claude3` |
| 5 | Chain row minted with `model_name='claude-opus'` | ✓ |
| 6 | Resume without `-m` (`agents --resume <UUID> "..."`) | ✓ model inferred; routed to active segment; cache read 24,169 tokens |
| 7 | Force migration via `--migrate claude` | mechanic triggers; upstream Claude rejects (see Finding 5) |
| 8 | `[migrate] <src> -> <dst> reason=<reason>` stderr line | ✓ emitted exactly once |
| 9 | Segment ledger close+open transactional pair | ✓ source closed with `last_turn_id`; target opened |
| 10 | `claude-code-turns` adapter emits `is_compaction_boundary: true` for `isCompactSummary` records | ✓ 379 emissions on full re-scan |
| 11 | Runner ingest persists `is_compaction_boundary` for fresh rows | ✓ 3/4959 re-ingested turns flagged true |

## Findings

### Critical

**F5 — Migration broken: target JSONL bytes reference source session_id, upstream CLI rejects.**

`src-tauri/src/migration/mod.rs` mints a fresh `target_session_id` via `Uuid::new_v4()` (per proposal §6 step 2) and copies the source JSONL byte-for-byte. The JSONL records embed the source `sessionId` field. When Claude is launched with `--resume <new_target_id>`, it reads the file, sees `"sessionId":"dd116a3c…"`, but was asked to resume `e9c5344a…` — mismatch — emits "No conversation found with session ID: e9c5344a…".

Test reproduction:
- Create session: source provider `claude3`, session_id `dd116a3c-6819-42b1-b3d2-f512331eb5ec`.
- Force migrate: `agents --resume dd116a3c… --migrate claude "..."`.
- Target JSONL written at `/home/nes/.claude/projects/<cwd_hash>/e9c5344a-….jsonl` (10,088 bytes, byte-equal to source).
- Upstream Claude rejects.

Proposal Q2 first sentence said keeping the SAME UUID across HOMEs works:
> "copying ~/.claude/projects/<hash>/<UUID>.jsonl to ~/.claude2/projects/<hash>/<UUID>.jsonl and running `claude --resume <UUID>` under HOME=~/.claude2/... resumes the same conversation."

Q2's §6.2 "safer practice: mint a new UUID on target side" is what the implementation followed and is what's broken. The original Q2 first sentence is what works in practice.

**Fix options (preferred → least preferred):**

1. **Reuse source UUID** on target side. Don't mint. Each HOME is independent; cross-HOME UUID collisions don't happen because each HOME has its own JSONL space. Update §6 step 2 to: "target_session_id = source_session_id". Update §6 step 3 target path to use the source UUID.
2. Rewrite all `sessionId` fields in the copied JSONL to the new UUID. Per proposal §6 step 11, this is explicitly forbidden ("do not silently sed-edit JSONL bytes") — but doing it deliberately as part of the migration spec, with an audit log, is acceptable. Higher complexity.

Option 1 is the correct fix.

**F7 — Compaction backfill gap: `INSERT OR IGNORE` cannot retro-flag existing rows.**

`src-tauri/src/state/db.rs` ingest path uses `INSERT OR IGNORE INTO session_turns ...`. On conflict (existing row), the new `is_compaction_boundary` value is dropped. The schema migration adds the column with `DEFAULT 0`, so all 834,085 pre-existing turns are stuck at 0 even after the adapter is fixed.

This means migrations of compacted Claude sessions that existed before the upgrade will not see the compaction boundary; the source JSONL will be copied in full and the target Claude session will hit context overflow.

**Fix:** extend `agents migrate-db` to include a one-shot re-read pass: for every (provider, session_id) in `session_chain_segments`, re-read the source JSONL via the existing locator and `UPDATE session_turns SET is_compaction_boundary = 1 WHERE provider = ? AND session_id = ? AND turn_id = ?` for each compaction-summary record found. Idempotent. Run on demand.

### Major

**F1 — `agents resume --list <UUID>` not shipped.** Proposal §8.5 + §8.5.1 named the diagnostic chain-list subcommand. CLI surface has no `--list` flag on `resume` and no `ResumeList` subcommand. Help output confirms. Step 6c skipped this.

**F4 — `~` not expanded in `projects_dir`.** `src-tauri/src/migration/mod.rs::find_claude_source_from_storage` calls `std::fs::read_dir(projects_dir)` directly. If the user wrote `projects_dir = "~/.claude/projects"` (the natural form), `read_dir` literally tries `~/.claude/projects` as a relative path and fails. Workaround: use absolute paths. Fix: shellexpand or `dirs::home_dir`-based replacement at config load.

### Minor

**F2 — Agent-session chains tagged `'imported'` instead of `'initial'`.** The §3.1 mint hook in `emit_known_session_id` is bypassed by the §3.1.1 ingestion-path mint, which uses `'imported'` as the reason for backfilled-from-turns. Result: every fresh agent session starts as if it were imported. Functionally identical for migration logic (the chain still works), but cosmetically wrong and breaks `transition_reason` audit semantics.

**F6 — Failed migration leaves invocation row with source session_id, not target's.** The invocation row records the user's requested `--resume <UUID>` value, but after migration the actual session_id on the target spawn is the freshly-minted target id. `trace --json` therefore shows the source id. Fix is dependent on F5 — once we reuse source UUID, this finding goes away.

**F8 — New `claude-code-turns` adapter wasn't deployed.** README install instructions list `anthropic-usage`, `chatgpt-usage`, `zai-usage` for `~/.local/bin/`. `claude-code-turns` is in the same scripts dir but not in the install command. Users who upgrade won't get the new compaction-emitting adapter unless they also reinstall the script.

## Pieces verified working

- Schema migration on a real production DB (532 MB / 834K turns).
- Backfill mints chains for every existing (provider, session_id) pair without timing out.
- Fresh agent session creation records `model_name` on the chain.
- Resume without `-m` infers model from the chain and routes to the active segment provider correctly.
- Cache hits preserved when staying on same provider.
- Migration mechanic emits `[migrate]` stderr line and writes target JSONL.
- Segment ledger transitions correctly (close-source / open-target with `last_turn_id` and `transition_reason`).
- Adapter emits `is_compaction_boundary: true` for `isCompactSummary: true` records.
- Runner ingest path persists `is_compaction_boundary` end-to-end for fresh rows.
- `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt` all clean on `2c56310`.

## Remediation plan

Hotfix commit on `main` addressing F5, F7, F1, F4, F2, F8 (F6 closes via F5):

1. **F5**: in `src-tauri/src/migration/mod.rs`, set `target_session_id = resolved.active_session_id.clone()` instead of minting a fresh UUID. Update `compute_target_path` accordingly. Update segment INSERT to record the same session_id on the new segment (the UNIQUE(chain_id, provider_name, session_id) constraint still holds because target provider differs from source).
2. **F7**: extend `agents migrate-db` to re-read source JSONLs and UPDATE existing `session_turns` rows for compaction-summary records.
3. **F1**: add `Subcommands::ResumeList { uuid: String }` per hookpoints recommendation; dispatch to a new `run_resume_list` that uses the resolver's preview-builder.
4. **F4**: expand `~` in `projects_dir` at config load via `shellexpand` or `dirs::home_dir`.
5. **F2**: ensure §3.1 agent-path mint runs before / overwrites §3.1.1 ingestion-path mint when both fire on the same (provider, session_id). Use ON CONFLICT DO UPDATE SET transition_reason = excluded.transition_reason WHERE transition_reason = 'imported' (or similar resolution).
6. **F8**: extend README install command to include `claude-code-turns`.

After the hotfix:
- Re-run the full QA matrix.
- Confirm migration succeeds end-to-end with the upstream Claude CLI accepting the resumed session.
- Confirm `migrate-db` retro-flags compaction boundaries on pre-existing turns.
- Confirm `agents resume --list <UUID>` exists and prints chain previews.

## Hotfix verification (post-`21c67f7`)

After the hotfix commit, re-ran the live QA matrix:

- **F1 verified**: `agents resume --list <UUID>` works (also `agents resume-list <UUID>` direct subcommand). Output prints all chains matching the input session_id with `chain_id`, `last_used_at`, `active_provider`, `active_session_id`, `turn_count`, `recent_turns_count`.
- **F2 verified**: fresh agent session created; chain row has `transition_reason='initial'` (not `'imported'`), `model_name='claude-opus'`.
- **F4 verified**: `projects_dir = "~/.claude/projects"` (with `~`) loads cleanly; the binary expands the home prefix. Migration path resolution works under the `~` form.
- **F5 verified end-to-end**: `agents --resume <UUID> --migrate claude2` from a session originally on claude3 → migration runs → `[migrate] claude3 -> claude2 reason=manual` stderr → JSONL copied to `~/.claude2/projects/<cwd_hash>/<session_id>.jsonl` (same UUID reused) → spawned `claude2` accepts the resume → response: "gamma". `cache_read_input_tokens=16181`.
- **F7 verified end-to-end**: `agents migrate-db` re-read 3,931 source JSONLs and flagged 109 newly-discovered compaction-boundary turns; total flagged turns went from 3 → 112 (claude=55, claude2=44, claude3=13). Subsequent `migrate-db` runs are idempotent.
- **F8 verified**: README install command updated to include `claude-code-turns` and `codex-turns`. Adapter reinstalled to `~/.local/bin/`.

### Environment-specific caveat (not a code bug)

The user's shell has `CLAUDE_CONFIG_DIR=/home/nes/.claude2` set globally. This means the bare `claude` provider command (no wrapper) actually reads `~/.claude2/` config, not `~/.claude/`. Migration target=`claude` will copy to `~/.claude/projects/...` but the spawned subprocess looks at `~/.claude2/projects/...`. Fix: either (a) drop `CLAUDE_CONFIG_DIR` from shell env, (b) add a `claude` wrapper that explicitly sets `CLAUDE_CONFIG_DIR=~/.claude`, or (c) update the model TOML's `claude` provider command to explicitly set `CLAUDE_CONFIG_DIR=~/.claude` before invoking. This is a user-config concern, not a runner bug.

### F6 (closed by F5)

With same-UUID-reuse, the failed-migration invocation row records the same session_id as the source. No more discrepancy. Confirmed in the QA logs: invocation `e72c0f7f-…` records `session_id='76afe908-…'` matching the source.

## State after this QA pass

- Branch on `main` at commit `21c67f7`, 6 commits ahead of `origin/main`, NOT pushed.
- Production state DB modified: schema migrations applied, 18,580 chains backfilled, 1 fresh chain (`bd2a97ca…`) and 3 invocations from the QA test session.
- Production config modified: `[providers.session_storage]` blocks added to `claude-opus.toml` (with absolute paths after F4 workaround).
- Backups present: `state.db.backup-pre-05`, `claude-opus.toml.pre-05`, `providers.toml.pre-05`.
- `~/.local/bin/claude-code-turns` updated to the repo's new version (per F8 verification).
- Stuck chain segment from the failed migration: chain `bd2a97ca…` has an open segment on `claude` with target session_id `e9c5344a…` (segment id 18582). Resolver will pick this segment by max(started_at) on next resume, also failing. Manual cleanup needed: close segment 18582 with `ended_at = now`, OR delete it, OR ship F5 fix and re-attempt.
