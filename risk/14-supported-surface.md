# WU-14-01 — Phase 4 Supported-Surface Risk Gate

## 1. Termination signal

`NONE`.

Assumption-register check (`proposals/14-session-migration-cwd.md` §4
A1–A7) against the problem map and RCA:

- A1 (both call sites know spawn cwd before migration) — **upheld**.
  Problem map confirms `run_repl` and `run_resume` already receive
  `working_dir` and forward it via `cmd.current_dir(dir)` at
  `src-tauri/src/executor/cli.rs:348` (`research/14-problem-map.md:18`,
  `research/14-problem-map.md:22`, `research/14-problem-map.md:65`).
- A2 (Unix project dir = absolute path with `/` → `-`) — **upheld**
  by the live RCA observation
  `/home/nes/.claude3/projects/-home-nes-projects-server-manager-worktrees-init-142i-cleanup/...`
  at `research/14-session-migration-rca.md:21` and the production
  inversion helper at `src-tauri/src/session_metadata/mod.rs:338-364`
  (`research/14-problem-map.md:54`, `research/14-problem-map.md:71`).
- A3 (absolutize but do not canonicalize relative `working_dir`) —
  **upheld**. `build_command` forwards `working_dir` directly without
  canonicalization (`research/14-problem-map.md:65`,
  `research/14-problem-map.md:86`) and Claude's own canonicalization
  behavior is explicitly an open question, not a contradicted claim
  (`research/14-problem-map.md:87`).
- A4 (removing `ResumePayload.target_jsonl_path` is safe) — **upheld**.
  Both supported handoffs already pass `None`
  (`research/14-problem-map.md:17`, `research/14-problem-map.md:23`),
  `compose_resume_args` ignores the field at
  `src-tauri/src/executor/cli.rs:285`
  (`research/14-problem-map.md:25`,
  `research/14-problem-map.md:40`), and the RCA harness shows passing
  `Some` does not change child behavior at
  `src-tauri/tests/session_migration_rca/rc1_cwd_project_dir_mismatch.rs:54`
  (`research/14-session-migration-rca.md:81`).
- A5 (Windows hashing not answerable here, deferred) — **upheld**.
  Problem map found no production Windows encoder
  (`research/14-problem-map.md:70-73`); the proposal correctly
  defers to `WU-14-02-windows-claude-path-hash`
  (`proposals/14-session-migration-cwd.md:171-175`).
- A6 (source discovery may stay broader than target lookup) —
  **upheld**. Failure is target placement, not source discovery
  (`research/14-problem-map.md:42`,
  `research/14-session-migration-rca.md:31-36`).
- A7 (session graph independent of target dir name) — **upheld**.
  Migration records provider/session/reason in DB; chain resolution
  uses provider/session identity, not the JSONL directory name
  (`research/14-problem-map.md:11`,
  `research/14-session-migration-rca.md:67`).

No assumption is invalidated → no `RETURN_TO_RESEARCH`.

Net value (proposal §6, evaluated below in §3) is clearly positive →
no `TERMINATE`.

## 2. Verdict

`LOW`.

## 3. Findings

### Blast radius is bounded and explicitly excludes hot adjacencies

The proposal changes only:

- `src-tauri/src/migration/mod.rs` target-dir computation
  (`proposals/14-session-migration-cwd.md:13-48`),
- the two main migration call sites in `run_repl` / `run_resume`
  (`proposals/14-session-migration-cwd.md:50-72`),
- a dead executor field
  (`proposals/14-session-migration-cwd.md:74-83`),
- migration tests + RCA harness expected target
  (`proposals/14-session-migration-cwd.md:85-117`),
- one README paragraph
  (`proposals/14-session-migration-cwd.md:124-127`).

It explicitly **does not** touch the most dangerous adjacent surfaces
flagged by the problem map:

- balancer / `decide_migration` policy
  (`proposals/14-session-migration-cwd.md:148-149`,
  `research/14-problem-map.md:49`),
- `src-tauri/src/state/db.rs` and session graph schema
  (`proposals/14-session-migration-cwd.md:158`),
- `locate_transcript` semantics and `find_claude_source_from_storage`
  fallback ordering
  (`proposals/14-session-migration-cwd.md:166-167`),
- frontend (`proposals/14-session-migration-cwd.md:159`),
- routing / fanout / cross-platform lock tests
  (`proposals/14-session-migration-cwd.md:174-175`).

The migration path *is* a hot path, but the proposal narrows the
change to "where the bytes get written" rather than "when migration
fires" or "who owns the chain." That is the minimum surface needed
to flip the RC-1 harness GREEN
(`research/14-session-migration-rca.md:104-125`).

### Net value is clearly positive

Current behavior: migration emits `[migrate]` and reports success,
but the child Claude process rejects `--resume` with
`No conversation found with session ID: ...`
(`research/14-session-migration-rca.md:7-13`). This is a
user-visible product break on the supported resume-migration path.

Post-fix: target JSONL is written under the cwd-derived project dir
that `claude --resume` actually inspects
(`research/14-session-migration-rca.md:31-36` independently confirms
this resolves the discovery error). The fix also eliminates the dead
`ResumePayload.target_jsonl_path` field, removing executor signature
debt.

Costs: no DB migration, no schema change, no bulk rewrite of
already-migrated JSONLs. Old already-misplaced JSONLs simply get
re-migrated on next resume attempt
(`proposals/14-session-migration-cwd.md:225-227`). Rollback is
`git revert`; orphaned cwd-derived JSONLs are harmless local
transcript files (`proposals/14-session-migration-cwd.md:230-235`).

Reduction clearly outweighs blast radius and migration/rollback cost.

### Migration mode is honestly described

Code-only migration; no schema migration; no in-place rewrite of
historical JSONLs (`proposals/14-session-migration-cwd.md:220-227`,
§6:357-358). This matches the RCA's framing of the defect as
content-transfer placement, not stored schema shape.

### Rollback is honestly described

`git revert` returns the old (broken) source-derived placement for
future migrations only; no DB cleanup; cwd-derived JSONLs already on
disk are inert (`proposals/14-session-migration-cwd.md:230-235`).
The proposal does not claim rollback is loss-free for users mid-flow,
but for a local-desktop CLI/Tauri product that is the right shape.

### Observability is preserved, not extended

Reuses existing `[migrate]` stderr line, `MigratedSegment.target_jsonl_path`
(now correctly meaning the cwd-derived path), and the RC-1 harness
exit-code/stderr signal
(`proposals/14-session-migration-cwd.md:238-245`). No new metric or
IPC contract — appropriate for a placement fix.

### Adjacent user-reachable paths are unchanged

`agents ... --resume <id>` argv unchanged; `[resume]` / `[migrate]`
/ `OULIPOLY_INVOCATION=...` lines unchanged; locator script
unchanged; provider config unchanged
(`proposals/14-session-migration-cwd.md:194-217`,
`proposals/14-session-migration-cwd.md:120-122`). Only the file
location changes, and that is the precise change required by the
RCA.

### Cross-platform residuals are named and deferred, not papered over

Windows path hashing is correctly not implemented in this WU
(no production encoder exists per
`research/14-problem-map.md:70-73`); it is deferred to
`WU-14-02-windows-claude-path-hash` with a named future harness
(`proposals/14-session-migration-cwd.md:171-173`,
`proposals/14-session-migration-cwd.md:286-295`). The new helper
returns `MigrationError::SpawnCwdUnsupported` for non-Unix absolute
paths rather than silently producing a wrong hash
(`proposals/14-session-migration-cwd.md:21-26`), which keeps Windows
release builds (restored by WU-13-01) from regressing into a worse
state — they go from "migration silently writes to a Unix-shaped
path" to "migration fails fast with a typed error." That is a strict
improvement on the Windows surface even though Windows hashing is
not solved here.

Symlink/canonicalization is similarly deferred as an acknowledged
unknown rather than guessed (proposal §4 A3, §7).

### Minor risk noted, not blocking

- The new helper rejects non-absolute / non-Unix paths with a typed
  error, but the proposal accepts the existing
  `eprintln!("migration failed: {err:?}")` call-site handling
  (`proposals/14-session-migration-cwd.md:65-68`). This is fine for
  LOW — `Debug` includes both `provider` and `cwd`, and the error is
  a fail-stop for migration rather than a silent corruption.
- The proposal correctly flags that Phase 5 must verify
  `working_dir = None` coverage exists, otherwise add helper
  coverage (`proposals/14-session-migration-cwd.md:323-324`,
  §7:374-375). Acceptable as a Phase 5 obligation.
- A2 is empirical: validated by the in-repo decoder and a real
  Claude run on Linux (`research/14-session-migration-rca.md:17-36`),
  but not by an authoritative spec. Test row in
  `proposals/14-session-migration-cwd.md:317` correctly names the
  residual ("does not verify real Claude binary behavior").

None of these widen blast radius beyond what the proposal claims.

## 4. LOW + NONE justification

The change is precisely scoped to the broken target-path computation
plus a dead-parameter cleanup, with balancer policy, DB schema,
locator semantics, and frontend explicitly out of scope, rollback as
plain `git revert`, and Windows/symlink unknowns deferred to named
future WUs rather than guessed.
