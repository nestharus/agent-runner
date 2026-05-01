# Initiative 06 / 06-pause-handshake - Existing-State Risk Profile

## 1. Touched Surface Inventory

1. The Phase 2.5 rule for this artifact is current-state only: capture the existing touched surface, existing brittleness, adjacent blast radius, and supported paths before proposal work. (`~/ai/workflows/implementation-pipeline.md` Phase 2.5)
2. The harness asks for `agents session pause-handshake <session-id> [--ttl-ms <ms>]` and `agents session resume-handshake <session-id> --token <token>`. Pause returns JSON with `session_id`, `provider_name`, `token`, `expires_at`, and `lock_path`; resume releases only the matching token and is idempotent for an already released same-token release. (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`)
3. The requested exit namespace is `0`, `1`, `2`, `10`, `11`, `13`, `16`, and `17`. `13` is `session-busy`, `16` is `lock-token-invalid`, and `17` is `lock-expired`. (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`)
4. The requested side effect is lock state only. The handshake must not mutate transcript content unless paired with a separate import command. (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`)
5. Initiative 06 sequences pause-handshake after locate, schema-probe, and export, and before import-replace. Pause-handshake is the shared session-scoped exclusive lease lock for import-replace, migration, resume/repl, and balanced one-shot write paths. (`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:48-56`, `:106-122`)
6. The local `06-pause-handshake` worktree has no `session` command group yet. `Subcommands` currently contains `trace`, `repl`, `resume`, hidden `resume-list`, `migrate-db`, and `migrate-config`. (`src-tauri/src/main.rs:77-166`)
7. The stacked `06-locate` worktree has introduced `Subcommands::Session` and `SessionSubcommands::Locate`; the stacked `06-schema-probe` worktree extends that group with `schema-probe`. Pause-handshake will be adjacent to that command group if the initiative sequence lands as planned. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:157-185`, `/home/nes/projects/agent-runner/worktrees/06-schema-probe/src-tauri/src/main.rs:162-177`)
8. Top-level dispatch currently has no `Session` arm. It routes `trace`, `repl`, `resume`, `resume-list`, `migrate-db`, and `migrate-config` directly before top-level execution. (`src-tauri/src/main.rs:287-338`)
9. Top-level `--resume <UUID>` is a separate supported dispatch path: prompt/file/stdin routes to `run_resume`; no prompt routes to `run_repl`. (`src-tauri/src/main.rs:341-389`)
10. **Session-scoped lock primitive: none exists today.** Searches of product code find no session lock table, no session lockfile, no pause/resume-handshake command, no persisted lock token, no lock expiry field, and no session-busy/token-invalid/lock-expired error mapping. (`src-tauri/src`, `README.md`)
11. **Lock state storage: absent today.** `StateDb::open` creates invocation, quota, setup, account, discovery, `session_turns`, `session_chains`, and `session_chain_segments` tables; it does not create `session_locks` or any lock metadata table. (`src-tauri/src/state/db.rs:431-608`)
12. There is no documented lockfile directory under the CLI data directory. README documents persistent state as SQLite at `~/.local/share/oulipoly-agent-runner/state.db`; it does not document `locks/` or session lock paths. (`README.md:224`)
13. There is no durable in-memory session lock registry. The only nearby in-process lock primitive is quota refresh `InFlight`, a `Mutex<HashSet<String>>` scoped to one process and one provider refresh operation. (`src-tauri/src/quota/mod.rs:28-65`)
14. Existing cross-process concurrency control is SQLite WAL only. README explicitly says there is no daemon/background process and state is shared via filesystem-level SQLite WAL locking. (`README.md:224`)
15. `StateDb` wraps one `rusqlite::Connection`; there is no mode flag carrying a lock owner, active lease, or read/write guard. (`src-tauri/src/state/db.rs:48-50`)
16. `StateDb::open_default` resolves the CLI state DB to `dirs::data_dir()/oulipoly-agent-runner/state.db` and then calls the mutating `open`. (`src-tauri/src/state/db.rs:611-615`)
17. `StateDb::open` is not read-only in this worktree: it creates the parent directory, opens read/write, sets WAL, ensures schemas, and runs chain backfill before returning. (`src-tauri/src/state/db.rs:431-608`)
18. The stacked schema-probe branch adds a read-only open path and schema probe reporting, but the local pause-handshake worktree does not contain that API. (`/home/nes/projects/agent-runner/worktrees/06-schema-probe/src-tauri/src/state/db.rs:676-703`)
19. **Session ownership state today is chain/segment based.** `ResolvedResume` carries `chain_id`, optional `model_name`, optional `ModelConfig`, `active_provider`, and `active_session_id`. It does not carry lock state, lock token, transcript path, storage type, workspace root, pid, or lease expiry. (`src-tauri/src/state/db.rs:131-138`)
20. `session_chains` stores `chain_id`, created/last-used timestamps, and `model_name`; `session_chain_segments` stores provider/session segments with nullable `ended_at`, `last_turn_id`, and transition reason. (`src-tauri/src/state/db.rs:574-592`)
21. The active owner convention is the latest segment for a chain with `ended_at IS NULL`, ordered by `started_at DESC, id DESC`. Multiple active rows are tolerated by selecting one. (`src-tauri/src/state/db.rs:2751-2764`)
22. `StateDb::resolve_resume` is the existing ownership path. It validates UUID shape, finds candidate chains where input matches `session_id` or `chain_id`, chooses one chain, reads the active segment, infers model, validates provider/model membership when possible, and returns the active provider/session. (`src-tauri/src/state/db.rs:2577-2670`)
23. Candidate lookup reads only `session_chain_segments`; raw `session_turns` rows are not direct ownership candidates unless chain/segment rows already exist. (`src-tauri/src/state/db.rs:2696-2711`)
24. Ambiguity is current resume semantics: one candidate returns directly; multiple candidates are reduced by a 24-hour recency rule; multiple recent candidates return ambiguity. (`src-tauri/src/state/db.rs:2713-2749`)
25. The stacked locate branch has a higher-level `SessionMetadata` shape with `session_id`, `chain_id`, `provider_name`, `storage_type`, `jsonl_path`, `workspace_root`, `transcript_state`, and `mutable`, but the local pause-handshake branch does not have that module. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:12-21`)
26. Existing invocation lifecycle writes `invocations` rows with status `running`, then finalizes to `succeeded` or `failed`. Running rows are not session locks and do not carry TTL or token semantics. (`src-tauri/src/state/db.rs:826-854`, `src-tauri/src/state/db.rs:1036-1157`)
27. `FinalizerGuard` finalizes a running invocation as failed on normal Rust drop/unwind when not explicitly finalized; it does not run after process kill or machine crash. (`src-tauri/src/main.rs:502-535`)
28. Provider process handles live only inside executor functions. `execute_provider_with_args` and `execute_interactive` spawn a child and wait; no pid, process group, or session ownership lease is persisted. (`src-tauri/src/executor/cli.rs:424-450`, `:589-603`)

## 2. Current Write Paths That Would Need Lock Awareness

1. `run_repl` opens the default DB, loads configs, optionally resolves resume ownership, optionally migrates the active chain segment, records an invocation, may mark `session_capture_method = "resumed"`, spawns the provider interactively, finalizes, and ingests/emits session state on success. (`src-tauri/src/main.rs:809-1054`)
2. `run_repl --resume` writes before provider spawn by calling `start_invocation` and then `update_session_capture(..., Some(session_id), "resumed")` using the user-supplied resume id. (`src-tauri/src/main.rs:971-984`)
3. `run_repl --resume` passes the resolved active provider session id into `execute_interactive` through `ResumePayload`. The child process can then write to provider-native session storage outside agent-runner's DB. (`src-tauri/src/main.rs:990-1007`, `src-tauri/src/executor/cli.rs:566-603`)
4. After a successful interactive run, `run_repl` scans provider session sources and may mint or update chain state through `ingest_and_emit_session_id` / `emit_known_session_id`. (`src-tauri/src/main.rs:1014-1037`, `:541-615`)
5. `run_resume` validates the session UUID, reads the answer payload, opens state, resolves ownership, optionally migrates, records an invocation, writes `session_capture_method = "resumed"`, spawns the provider non-interactively, records resume acceptance, finalizes, scans/emits session state, and writes provider stdout. (`src-tauri/src/main.rs:1056-1263`)
6. `run_resume` starts the invocation and writes the requested session id before provider spawn; the actual provider target can be `resolved.active_session_id` after chain resolution or migration. (`src-tauri/src/main.rs:1173-1198`)
7. `run_resume` updates `resume_acceptance_status` and `resume_acceptance_evidence` after child completion. Those are invocation writes adjacent to session mutation but not transcript mutation. (`src-tauri/src/main.rs:1214-1220`)
8. Top-level `agents --resume <UUID>` enters the same `run_resume` / `run_repl` write paths, depending on whether prompt content is present. (`src-tauri/src/main.rs:341-389`)
9. Balanced one-shot execution (`run_with_balancing`) opens state, may refresh quota during provider selection, starts an invocation, spawns the provider, updates session capture, marks quota exhaustion, finalizes, ingests/emits session state, and increments quota calls. (`src-tauri/src/main.rs:1265-1411`)
10. Balanced one-shot execution may not know a target session id before spawn unless the provider capture mode requested one; current session writes can still occur after success via capture result or turn-script scan. (`src-tauri/src/executor/cli.rs:618-679`, `src-tauri/src/main.rs:1344-1393`)
11. `ingest_and_emit_session_id` runs `scan_provider`, writes normalized turns, finds a session in the invocation time window, writes invocation session capture, and mints/promotes chain state. (`src-tauri/src/main.rs:541-615`, `src-tauri/src/sessions/mod.rs:60-141`)
12. `scan_provider` writes `session_turns` in a batch transaction and then calls `mint_imported_chain_if_absent` once per parsed turn. It captures errors but continues on many failure modes. (`src-tauri/src/sessions/mod.rs:87-141`)
13. `mint_chain_for_invocation_session` reads an invocation session id and either promotes an existing imported segment to `initial` or inserts a new chain and active segment. (`src-tauri/src/state/db.rs:1205-1285`)
14. `open_chain_segment` inserts or reopens a segment for a `(chain_id, provider_name, session_id)` tuple and sets `ended_at = NULL` on conflict. (`src-tauri/src/state/db.rs:2365-2402`)
15. `close_active_segment_returning` updates the active segment for a chain, sets `ended_at`, and snapshots the latest turn id. (`src-tauri/src/state/db.rs:2474-2498`)
16. `run_migrate_db` opens the default DB, runs chain backfill explicitly, then runs compaction backfill. Both can mutate session tables. (`src-tauri/src/main.rs:1450-1462`)
17. `run_compaction_backfill` iterates every distinct chain segment, locates transcript sources, parses JSONL lines for compaction summaries, and flags matching `session_turns` rows. (`src-tauri/src/main.rs:1909-2001`)
18. `migrate_chain_segment` reads source transcript bytes, may slice from a compaction boundary, writes a target temp JSONL file, renames it into place, closes the old active segment, and opens a new target segment. (`src-tauri/src/migration/mod.rs:79-254`)
19. `migrate_chain_segment` checks for a conflicting target active segment in another chain before writing target JSONL and changing segment state. It does not check any lock primitive because none exists. (`src-tauri/src/migration/mod.rs:196-231`)
20. `locate_transcript` is read-like but currently creates the adapter `STATE_DIR` before running the locator script. That is adjacent because pause-handshake itself is required to create/remove only lock state. (`src-tauri/src/sessions/mod.rs:171-199`)

## 3. Currently Risky or Brittle Behavior

1. There is no current way to block a second writer for a resolved session. Any process that can open the DB and pass resolver/config checks can resume, migrate, ingest, or backfill while another process is preparing a transcript override.
2. SQLite WAL serializes individual database writes, but it does not protect provider-native transcript files, external locator/script state dirs, or a multi-step sequence spanning file copy/rename plus DB segment updates. (`README.md:224`, `src-tauri/src/migration/mod.rs:206-231`)
3. There is no token identity to distinguish the lock holder from a wrong releaser, and no error path for `lock-token-invalid`. Existing resume errors cover UUID, not-found, ambiguity, provider/model, provider config, active segment, and DB failures only. (`src-tauri/src/state/db.rs:140-170`)
4. There is no `expires_at` or TTL field anywhere in session state. Quota has a TTL concept for refresh staleness, but that is provider-quota freshness, not a session lease. (`src-tauri/src/quota/mod.rs:13-23`, `src-tauri/src/state/db.rs:574-592`)
5. There is no crash-recovery path for lock state because no lock state exists. Running invocation rows can survive a process crash as `running`, but they are not cleaned by TTL and are not treated as session locks. (`src-tauri/src/state/db.rs:1036-1157`)
6. The provider child process is opaque to state. Agent-runner does not persist child pids or a "provider is actively writing session X" row, so a pause attempt cannot currently prove whether an active provider process is safe to pause. (`src-tauri/src/executor/cli.rs:424-450`, `:589-603`)
7. `run_repl --resume` and `run_resume` mark the invocation with the caller-supplied session id before provider spawn, while the provider target can be the resolved active session id. This split makes current DB state an imperfect proxy for the actual provider write target. (`src-tauri/src/main.rs:982-999`, `:1181-1198`)
8. Balanced one-shot execution can write session state after success even though its target session may be unknown before provider spawn. Current lock-free execution cannot preflight a session-scoped lock for post-hoc discovered sessions. (`src-tauri/src/main.rs:1317-1393`)
9. Migration writes provider JSONL before closing/opening chain segments. A crash between file rename and DB segment updates can leave storage and chain state out of sync; current code reports errors but has no lease/lock artifact to help another process decide ownership. (`src-tauri/src/migration/mod.rs:206-231`)
10. `migrate_chain_segment` uses `close_active_segment_returning` to detect a concurrently closed active segment, but it does not guard the whole source-read/target-write/segment-update sequence. (`src-tauri/src/migration/mod.rs:216-231`, `src-tauri/src/state/db.rs:2474-2498`)
11. `scan_provider` degrades on script, parse, and mint errors; callers can continue with partial or stale session state. A future lock observer would need to account for writes that happen after a provider child exits, not just writes during child execution. (`src-tauri/src/sessions/mod.rs:55-141`)
12. Open-path backfill runs during ordinary `StateDb::open` and can create imported chains/segments without an explicit migration command. This is a session-state write that is not tied to a named user operation. (`src-tauri/src/state/db.rs:604-606`, `:2256-2363`)
13. Backfill skips if any `session_chains` row exists, so partially migrated DBs can retain segmentless turns. A pause-handshake built on resolver ownership inherits that current not-found surface. (`src-tauri/src/state/db.rs:2256-2271`, `:2696-2711`)
14. Multiple active segments for one chain are not treated as invalid. A session lock keyed to the resolved owner would inherit the latest-active-row convention. (`src-tauri/src/state/db.rs:2751-2764`)
15. Ambiguity is time-window dependent. A session id matching multiple chains may still resolve to one chain when only one is recent or none are recent. (`src-tauri/src/state/db.rs:2713-2749`)
16. There is no structured session-busy signal today. Busy provider accounts are handled indirectly by quota/exhaustion selection, not session ownership. (`src-tauri/src/balancer`, `src-tauri/src/quota/mod.rs:28-65`)
17. There is no stable `lock_path` value today. The only stable path-like state surface is the default DB path; transcript paths are provider/session-locator dependent. (`src-tauri/src/state/db.rs:611-615`, `src-tauri/src/sessions/mod.rs:171-199`)

## 4. Adjacent Surfaces in Blast Radius

1. `agents session locate` is adjacent because the harness requires pause-handshake to resolve `<session-id>` through the same ownership path as locate. In current lower-level code that path is `StateDb::resolve_resume`; in the stacked locate branch it is wrapped by `locate_session_metadata`. (`src-tauri/src/state/db.rs:2577-2670`, `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:81-132`)
2. `agents session schema-probe` is adjacent because it establishes read-only state inspection and feature flags in the initiative sequence. The local pause worktree does not yet include that read-only surface. (`/home/nes/projects/agent-runner/worktrees/06-schema-probe/src-tauri/src/schema_probe/mod.rs:61-160`)
3. `agents session export` is adjacent as the canonical transcript reader that precedes pause-handshake in the initiative sequence, but export is read-only and not itself a lock observer in the current harness request. (`/home/nes/projects/agent-runner/worktrees/06-export/research/06-export-problem-map.md`)
4. Future `agents session import-replace` is directly adjacent because the harness names it as a writer that pause-handshake must block. The local worktree has no import-replace command today.
5. `agents resume`, `agents repl --resume`, and top-level `--resume` are adjacent because they resolve the same session owner and then cause provider or DB writes. (`src-tauri/src/main.rs:341-389`, `:809-1263`)
6. Balanced one-shot execution is adjacent because it can mint or update session state after provider execution and is named by the initiative as a lock observer. (`src-tauri/src/main.rs:1265-1411`)
7. Migration is adjacent because it already mutates provider transcript files and chain segment ownership. (`src-tauri/src/migration/mod.rs:79-254`)
8. `migrate-db` is adjacent because it runs session-chain and compaction backfills over all known sessions. (`src-tauri/src/main.rs:1450-1462`, `:1909-2001`)
9. `StateDb::open` is adjacent because ordinary open has migration/backfill side effects before any command-specific lock check could run in the current code shape. (`src-tauri/src/state/db.rs:431-608`)
10. `sessions.toml` adapter scripts are adjacent because scan and locator scripts can read/write adapter state dirs and discover sessions outside the DB. (`src-tauri/src/sessions/mod.rs:1-30`, `:55-199`)
11. Provider config is adjacent because resume and migration need `[providers.resume]` and `session_storage` to know how to target the owner. (`src-tauri/src/config/providers.rs`, `src-tauri/src/config/model.rs:195-229`)
12. GUI/Tauri state access is adjacent only as DB-location divergence. GUI commands open `state.db` beside `models_dir`, while CLI `open_default` uses the OS data directory. (`src-tauri/src/lib.rs:525-533`, `src-tauri/src/state/db.rs:611-615`)

## 5. Currently Supported / User-Reachable Paths

1. `agents repl <model>` launches an interactive provider, records invocation state, and may ingest/emits session state after success. (`src-tauri/src/main.rs:809-1054`)
2. `agents repl <model> --resume <UUID>` resolves session ownership, may migrate the active segment, launches provider interactive resume args, and writes invocation/session state. (`src-tauri/src/main.rs:830-1007`)
3. `agents resume -m <model> --session-id <UUID> --prompt ...` and `--file ...` resolve ownership, may migrate, launch one-shot provider resume, record resume acceptance, and emit stdout. (`src-tauri/src/main.rs:1056-1263`)
4. `agents -m <model> --resume <UUID> ...` and stdin/file variants route through the same non-interactive resume path; no-prompt top-level resume routes to REPL. (`src-tauri/src/main.rs:341-389`)
5. `agents -m <model> <prompt>` and agent-file based execution route through balanced one-shot execution. (`src-tauri/src/main.rs:390-445`, `:1265-1411`)
6. `agents migrate-db` is user-reachable and mutates chain/compaction state. (`src-tauri/src/main.rs:1450-1462`)
7. `agents migrate-config` is user-reachable but rewrites config, not session lock or transcript state. (`src-tauri/src/main.rs:1472-1597`)
8. Hidden `agents resume-list <UUID>` is reachable and reads chain previews; it does not check or display locks. (`src-tauri/src/main.rs:155-157`, `:1887-1900`)
9. `agents trace <invocation_uuid> --json` is a structured inspection surface but starts from invocation id, not a lock token or session lease. (`src-tauri/src/main.rs:447-478`, `src-tauri/src/trace/mod.rs:23-80`)
10. Direct provider CLI usage outside agent-runner enters state through configured `turn_script`s and `session_scan`, which ingest turns and mint imported chains. (`README.md:330-372`, `src-tauri/src/sessions/mod.rs:55-141`)
11. Ad-hoc SQL against `state.db` is documented for questions outside trace's shape; there is no public SQL-backed lock contract today. (`README.md:500-512`)

## 6. Migration / Process Lifecycle / TTL / Observability Gaps

1. There is no current lock migration path because there is no lock storage. Existing schema bootstrap creates session owner tables but not a lock table. (`src-tauri/src/state/db.rs:559-597`)
2. If lock state were DB-backed, current `StateDb::open` is the place where new tables are commonly ensured; today that open also backfills chains and is not side-effect-free. This is current-state context, not a design choice. (`src-tauri/src/state/db.rs:431-608`)
3. If lock state were file-backed, current code has no lock directory, file naming convention, token file schema, or cleanup command. README documents only the DB path for persistent state. (`README.md:224`)
4. If lock state were only in-memory, current in-process patterns would not survive process exit and would not coordinate multiple `agents` processes. The quota `InFlight` set is exactly process-local. (`src-tauri/src/quota/mod.rs:28-65`)
5. Current provider lifecycle has no durable "active writer" lease. Invocation rows can show `running`, but they are per invocation and may remain running after a hard crash; no code treats old running rows as expired or session-busy. (`src-tauri/src/state/db.rs:826-854`, `src-tauri/src/main.rs:502-535`)
6. Current process-exit cleanup is best-effort through Rust destructors for invocation finalization, not a durable lock release. Hard process death bypasses `Drop`. (`src-tauri/src/main.rs:522-535`)
7. Current migration lifecycle has no recovery marker around target temp file, target rename, segment close, and segment open. The only durable results are the filesystem target and DB segment rows. (`src-tauri/src/migration/mod.rs:206-231`)
8. Current TTL/expiry cleanup exists for quota freshness only. It does not remove rows/files, does not expose `lock-expired`, and is provider-scoped rather than session-scoped. (`src-tauri/src/quota/mod.rs:13-23`)
9. There is no existing command or background process that periodically cleans expired state. The product is CLI/no-daemon by design. (`README.md:224`)
10. Existing open-path migrations can run in any command that opens state, including read-like commands such as trace. Any future lock expiry cleanup would share a surface with commands that currently expect `StateDb::open` to be enough to prepare state. (`src-tauri/src/state/db.rs:431-608`, `src-tauri/src/main.rs:447-478`)
11. Current chain/segment state can predate the lock feature. Pre-existing sessions have no lock owner rows or files and therefore no persisted paused/unpaused distinction.
12. Current resolver errors map not-found and ambiguity but not busy/expired/token-invalid. Any current CLI path seeing these conditions would fall back to generic operational errors because the typed states do not exist. (`src-tauri/src/state/db.rs:140-170`, `src-tauri/src/main.rs:658-707`)

### Observability Gaps That Exist Today

1. There is no CLI command that reports whether a session is paused, who owns a lock, when it expires, or which path stores the lock.
2. There is no current stdout JSON precedent for a lease receipt. Existing `trace --json` emits one pretty JSON object; `OULIPOLY_SESSION={...}` is embedded in stderr after session capture writes. (`src-tauri/src/main.rs:470-473`, `:610-615`)
3. There is no structured stderr JSON mapping for `session-busy`, `lock-token-invalid`, or `lock-expired`. Resume errors are human text today. (`src-tauri/src/main.rs:658-707`)
4. There is no durable audit trail for lock acquisition, release, wrong-token attempts, or expiry cleanup.
5. There is no observable process ownership for an active provider child. Users can see invocation rows and provider subprocess behavior, but not a persisted pid/session writer relation. (`src-tauri/src/state/db.rs:826-854`, `src-tauri/src/executor/cli.rs:424-450`)
6. `resume-list` can show candidate chains and active provider/session ids, but it is hidden, text-only, and lock-blind. (`src-tauri/src/main.rs:1887-1900`, `:2004-2014`)
7. Trace can report transcript state for invocation sessions, but it does not expose a paused/unpaused state or lease owner. (`src-tauri/src/trace/mod.rs:59-80`)
8. SQL inspection can reveal `session_chains`, `session_chain_segments`, and `invocations`, but no current table contains token, expiry, or lock path fields. (`src-tauri/src/state/db.rs:559-597`, `:826-854`)

## 7. Assumption Register Draft

1. A1: Pause-handshake resolution should inherit the current ownership semantics of `StateDb::resolve_resume`. Evidence: the harness and initiative both say to use the same ownership path as locate; current shared lower-level resolver is `resolve_resume`. Invalidator: locate's final merged metadata API changes ownership semantics rather than wrapping `resolve_resume`. (`src-tauri/src/state/db.rs:2577-2670`, `/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:112-113`)
2. A2: A lock can only be session-scoped in today's vocabulary by keying off resolved `chain_id` plus active provider/session state. Evidence: no persisted transcript mutability or lock field exists; `ResolvedResume` exposes only chain and active segment ownership. Invalidator: preceding merged features add a distinct public session identity or mutable target key that supersedes chain/segment state. (`src-tauri/src/state/db.rs:131-138`, `:574-592`)
3. A3: Existing `running` invocation rows are insufficient as a session-busy signal. Evidence: they do not require a session id, can persist after hard crash, and have no TTL/token semantics. Invalidator: a prior feature changes invocation lifecycle to persist active session writer leases with expiry. (`src-tauri/src/state/db.rs:826-854`, `src-tauri/src/main.rs:502-535`)
4. A4: Provider-native transcript writes can occur outside SQLite transaction boundaries. Evidence: migration writes/renames JSONL files before segment updates; provider children write their own storage outside runner DB control. Invalidator: supported providers move transcript mutation fully behind an agent-runner-managed transactional writer. (`src-tauri/src/migration/mod.rs:206-231`, `src-tauri/src/executor/cli.rs:424-450`)
5. A5: Crash survivability is a real requirement for pause-handshake, not already covered by process-local guards. Evidence: harness requires crash-safe TTL cleanup, while current guards are process-local/destructor-based. Invalidator: the harness drops crash recovery as an acceptance criterion. (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/04-session-pause-handshake.md`, `src-tauri/src/main.rs:522-535`)
6. A6: The local pause worktree will eventually consume the earlier Initiative 06 session command and read-only DB surfaces. Evidence: initiative sequence places locate and schema-probe before pause-handshake; local branch currently does not contain those surfaces. Invalidator: pause-handshake is rebased directly on main without the preceding features. (`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:41-56`)

## What This Map Deliberately Does NOT Cover

1. It does not design whether lock state should be a lockfile, DB table, hybrid, or in-memory structure.
2. It does not define token format, token entropy, token persistence, or release receipt JSON.
3. It does not define the lock acquisition algorithm, wait/drain behavior, polling interval, or fairness policy.
4. It does not define how an active provider process should be detected, paused, refused, or waited on.
5. It does not design crash recovery or TTL cleanup mechanics beyond documenting their absence today.
6. It does not design `agents session import-replace` or transcript replacement atomicity.
7. It does not design canonical transcript export or provider-native transcript parsing.
8. It does not redesign `StateDb::open`, read-only DB access, or schema-probe feature flags.
9. It does not change the current resume ambiguity, active-segment, or model/provider ownership semantics.
10. It does not cover frontend/Tauri UI visibility, HomeView/StatusView, or design-system work.

deliberately does NOT cover
