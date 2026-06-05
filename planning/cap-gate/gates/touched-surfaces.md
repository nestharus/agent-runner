# Touched surfaces — capture-time session persistence + pin-scrub sweep (42200fb..9ba1275)

Incremental gate over two commits. Prior surfaces gated LOW at 42200fb.

## Commit 9e00408 — production substance (capture-time backfill)
- `crates/oulipoly-runtime/src/executor/cli/supervision/mod.rs` — None→Some streamed-capture transition now
  backfills the sidecar session_id + marks the session running (single-fire; non-fatal on failure).
- `crates/oulipoly-runtime/src/executor/cli/spawn_identity.rs` — mark-running seam factored so a captured (late)
  session id flows into the same session_runtime upsert used when the session is known at spawn.
- `crates/oulipoly-runtime/src/executor/cli/interactive.rs`, `pty_broker.rs` — adjusted call sites for the
  factored seam.
- Tests: `crates/oulipoly-runtime/tests/age_pid_sidecar_spawn.rs` (backfill + negative),
  `src-tauri/tests/wu_d_proactive_wake_integration.rs` (mid-turn notify live-race reproduction).

## Commit 9ba1275 — test-only isolation sweep (51 files)
Every XDG-isolating test/fixture env now scrubs `OULIPOLY_DATA_DIR` (the pin would otherwise leak production
state into "isolated" runs — reproduced live via age100_one_shot_quota_migration failing under a real pin).
Immunity proof: full workspace suite green under a poison pin AND unpinned (2550 passed / 0 failed both).
No production changes in this commit.
