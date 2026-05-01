# Initiative 06 / 06-schema-probe - Existing-State Risk Profile

## 1. Touched Surface Inventory

1. The Phase 2.5 rule for this artifact is current-state only: capture the existing touched surface, existing brittleness, adjacent blast radius, and supported paths before proposal work. (`/home/nes/ai/workflows/implementation-pipeline.md:73-83`)
2. The harness asks for `agents session schema-probe`, one stdout JSON object, exit `0`/`1`/`14`, and no DB/config/transcript/quota side effects. (`/home/nes/projects/agent-harness/tmp/scratch/agent-runner-feature-requests/05-session-schema-probe.md:9-62`)
3. Initiative 06 reserves shared session-feature exit codes, including `14` for `schema-incompatible`, and assigns the read-only `StateDb` open variant to 06-schema-probe. (`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:106-122`)
4. The local `06-schema-probe` worktree does not yet have a `session` subcommand group. `Subcommands` contains `trace`, `repl`, `resume`, hidden `resume-list`, `migrate-db`, and `migrate-config`. (`src-tauri/src/main.rs:77-166`)
5. The stacked `06-locate` worktree has already added `Subcommands::Session { command: SessionSubcommands }` and `SessionSubcommands::Locate`. If 06-locate lands first, schema-probe extends that group; if not, schema-probe introduces the group. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`)
6. The local top-level dispatch matches concrete subcommands directly and has no `Session` arm today. (`src-tauri/src/main.rs:287-338`)
7. The stacked `06-locate` dispatch routes `Subcommands::Session` to `run_session_locate`; there is no schema-probe arm in that branch. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:354-358`)
8. `Cli` uses `args_conflicts_with_subcommands = true`, so top-level prompt/model/resume flags are clap-conflicting with subcommand forms. (`src-tauri/src/main.rs:18-23`)
9. `StateDb` is a thin wrapper around one `rusqlite::Connection`; there is no mode flag or read-only state type today. (`src-tauri/src/state/db.rs:48-50`)
10. `StateDb::open` currently creates the parent directory, opens SQLite read-write, sets `PRAGMA journal_mode=WAL`, ensures schemas, and then runs session-chain backfill before returning. (`src-tauri/src/state/db.rs:431-608`)
11. `StateDb::open_default` resolves the CLI default DB as `dirs::data_dir()/oulipoly-agent-runner/state.db` and then calls the mutating `open`. (`src-tauri/src/state/db.rs:611-615`)
12. README documents the same persistent CLI state location as `~/.local/share/oulipoly-agent-runner/state.db` on typical Linux systems. (`README.md:222-224`)
13. The schema bootstrap batch creates providers, quota tables, setup/memory/discovery tables, `session_turns`, `session_chains`, `session_chain_segments`, and chain indexes. (`src-tauri/src/state/db.rs:443-598`)
14. `session_turns` stores provider name, session id, turn id, timestamp, role, parent turn, sidechain flag, compaction-boundary flag, source file, ingested timestamp, and a unique `(provider_name, session_id, turn_id)` constraint. (`src-tauri/src/state/db.rs:559-572`)
15. `session_chains` stores one logical chain id with created/last-used timestamps and `model_name`. (`src-tauri/src/state/db.rs:574-579`)
16. `session_chain_segments` stores provider/session segments with nullable `ended_at`, nullable `last_turn_id`, and transition reasons constrained to `initial`, `manual`, `quota_threshold`, `exhausted`, or `imported`. (`src-tauri/src/state/db.rs:581-592`)
17. Segment indexes exist for lookup by `session_id` and active-segment lookup by `(chain_id, ended_at)`. (`src-tauri/src/state/db.rs:594-597`)
18. `ensure_invocations_schema` creates a fresh invocation table when absent, mutates older invocation tables by adding session/resume columns, drops an obsolete quota column when present, and then ensures invocation indexes. (`src-tauri/src/state/db.rs:618-676`)
19. `ensure_session_turns_schema` mutates existing `session_turns` tables by adding parent, sidechain, and compaction columns when absent, then ensures session-turn indexes. (`src-tauri/src/state/db.rs:693-721`)
20. `ensure_provider_quotas_schema` and `ensure_provider_quota_windows_schema` add/drop quota columns in place when opened. (`src-tauri/src/state/db.rs:739-791`)
21. Invocation schema includes `session_id`, `session_capture_method`, `resume_acceptance_status`, and `resume_acceptance_evidence`; the provider/session partial index is `idx_invocations_provider_session`. (`src-tauri/src/state/db.rs:826-866`)
22. Session-turn indexes include provider/timestamp, provider/session/timestamp, session/timestamp, and parent-turn lookup indexes. (`src-tauri/src/state/db.rs:869-877`)
23. `backfill_session_chains` runs on every `StateDb::open`, skips only when any `session_chains` row exists, otherwise groups all `session_turns` by `(provider_name, session_id)` and inserts imported chains/segments. (`src-tauri/src/state/db.rs:2256-2363`)
24. `run_migrate_db` also opens the default DB, calls `backfill_session_chains` explicitly, then runs compaction backfill and prints human summary lines. (`src-tauri/src/main.rs:1450-1462`)
25. `migrate-config` is a separate config rewrite command. It does not open `StateDb` in this worktree; it reads/writes model TOMLs and `providers.toml`. (`src-tauri/src/main.rs:1472-1597`)
26. `trace --json` is the closest existing stdout JSON precedent: it opens the default state DB, loads `sessions.toml`, builds a trace report, and prints one pretty JSON object when `--json` is set. (`src-tauri/src/main.rs:447-478`)
27. `TraceSession` JSON already serializes session id, chain id, capture method, transcript path/state, turn counts, and resume-acceptance fields. It does not expose schema compatibility or feature support. (`src-tauri/src/trace/mod.rs:59-80`)
28. The stderr `OULIPOLY_SESSION={...}` line is another JSON precedent, but it is embedded in stderr text and is emitted after session capture writes. (`src-tauri/src/main.rs:594-615`)
29. The binary package name/version are currently static Cargo/Tauri metadata: `oulipoly-agent-runner` and `0.1.0`. There is no commit embedding in `build.rs`. (`src-tauri/Cargo.toml:1-4`, `src-tauri/build.rs:1-3`, `src-tauri/tauri.conf.json:1-4`)
30. Current code has no stable public DB `schema_version`; searches find no `user_version` or `schema_version` implementation, and schema evolution is embedded in create/ensure helpers. (`src-tauri/src/state/db.rs:618-877`)
31. Provider storage declarations exist as config enum variants `claude_code` and `codex`; there is no current public `supported_storage_types` probe field. (`src-tauri/src/config/model.rs:195-228`)
32. `providers.toml` carries optional `session_storage`, and effective provider resolution copies it into the runtime `ProviderConfig`. (`src-tauri/src/config/providers.rs:10-32`, `src-tauri/src/config/providers.rs:157-190`)
33. `06-locate` adds a JSON-facing storage vocabulary `claude_code`, `codex_session`, and `other`, but that is on the stacked locate branch, not this local worktree. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-38`)
34. `StateDb::resolve_resume` is the current ownership read path: it validates UUID shape, reads candidate chains, chooses one, reads an active segment, infers model, and validates provider/model membership when a model is known. (`src-tauri/src/state/db.rs:2577-2670`)
35. `candidate_chain_ids` reads `session_chain_segments` only; raw `session_turns` rows are not current resolver candidates unless chain/segment rows already exist. (`src-tauri/src/state/db.rs:2696-2711`)
36. `active_segment_for_chain` treats the active segment as the latest `ended_at IS NULL` row ordered by started time and id. It is a read convention over existing rows, not a schema-compatibility check. (`src-tauri/src/state/db.rs:2751-2764`)
37. The hidden `resume-list` subcommand is a current chain-inspection surface that opens the default DB and prints human-oriented previews, not JSON schema metadata. (`src-tauri/src/main.rs:155-157`, `src-tauri/src/main.rs:1889-1900`)
38. On the 06-locate branch, `run_session_locate` already emits structured errors with exit `10`, `11`, and `12` for not-found, ambiguous, and unsupported storage; schema-probe's `14` would share that namespace. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:561-568`)
39. `SessionsConfig`/adapter scanning is the current bridge for direct external CLI sessions: adapter stdout becomes normalized `ScriptTurn`s, then batch DB rows, then imported chains. (`src-tauri/src/sessions/mod.rs:32-45`, `src-tauri/src/sessions/mod.rs:87-141`)

## 2. Currently Risky or Brittle Behavior

1. `StateDb::open` is always mutating. A caller that only wants to inspect schema still creates directories, flips/sets WAL, creates tables, alters columns, drops obsolete columns, creates indexes, and runs backfill. (`src-tauri/src/state/db.rs:431-608`)
2. WAL enablement is itself an open-time side effect, and failures are reported with a `run agents migrate-db` hint even for callers that conceptually only read. (`src-tauri/src/state/db.rs:437-440`)
3. Schema ensure helpers are idempotent in the operational sense, but there is no version pin. Adding a table, column, index, or drop rule silently changes any DB opened by a newer binary. (`src-tauri/src/state/db.rs:618-791`)
4. Legacy invocation migration can rebuild `invocations` during open and depends on config-derived provider-name lookup; corrupt/missing model config degrades rows to `status='legacy'`. (`src-tauri/src/state/db.rs:880-1034`)
5. `backfill_session_chains` runs unconditionally after schema ensure and writes imported chains/segments unless any chain row already exists. (`src-tauri/src/state/db.rs:604-606`, `src-tauri/src/state/db.rs:2256-2363`)
6. Backfill's skip condition is coarse: if any `session_chains` row exists, segmentless older `session_turns` rows remain segmentless. This is already called out by the 06-locate map and should not be rediscovered here. (`/home/nes/projects/agent-runner/worktrees/06-locate/research/06-locate-problem-map.md:47-50`)
7. There is no public schema compatibility surface. Users can inspect `sqlite_master` or PRAGMAs manually, but the binary has no CLI answer for "is this DB compatible with this binary?" (`README.md:500-512`)
8. Fresh and post-open state differ by construction. A DB seeded with only old `session_turns` and `invocations` gets `session_chains` created and populated just by `StateDb::open`. (`src-tauri/tests/initiative_05_migration.rs:1450-1507`)
9. The explicit `migrate-db` command and normal open path overlap: both can run the same chain backfill, so "migration command" is not the only migration trigger. (`src-tauri/src/main.rs:1450-1457`, `src-tauri/src/state/db.rs:604-606`)
10. `session_scan` and balancer scans can ingest direct CLI turns and mint imported chain state as a side effect of operational paths, not only explicit migration paths. (`src-tauri/src/sessions/mod.rs:125-141`)
11. Batch ingestion records `source_file = ''`, so schema/state inspection cannot reliably derive raw transcript provenance from DB rows alone. (`src-tauri/src/state/db.rs:2188-2201`)
12. The single-turn and batch-turn insert paths persist different optional fields: the single-turn path does not bind parent/sidechain fields, while batch does. (`src-tauri/src/state/db.rs:2134-2164`, `src-tauri/src/state/db.rs:2171-2228`)
13. Current feature support is implicit in compiled clap arms and modules. There is no stable feature enumeration for `session_locate`, `session_export`, `session_import_replace`, or `session_pause_handshake`. (`src-tauri/src/main.rs:77-166`)
14. The local worktree and stacked locate branch differ on command shape: local has no `session` group; locate has `session locate`. This is a current stacked-branch dependency risk. (`src-tauri/src/main.rs:77-166`, `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`)
15. README's resume text still says lookup is "via the `session_turns` ingest table" even though current resolver reads chain/segment tables; docs lag the Initiative 05 chain implementation. (`README.md:458-496`, `src-tauri/src/state/db.rs:2577-2711`)
16. Storage type naming is split: config serde names use `codex`, while the harness and 06-locate public metadata use `codex_session`. (`src-tauri/src/config/model.rs:195-200`, `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-38`)
17. The no-backwards-compatibility convention forbids checked-in compatibility shims and old/new dual implementations; any current/future split between `codex` and `codex_session` is a naming risk, not an existing compatibility layer. (`/home/nes/ai/conventions/no-backwards-compatibility.md:1-35`)
18. Missing DB files are not observable as "missing" through current `StateDb::open_default`; the parent directory and DB file are created as part of open. (`src-tauri/src/state/db.rs:431-437`, `src-tauri/src/state/db.rs:611-615`)
19. Current open can mask incompatible absence by creating missing tables with `CREATE TABLE IF NOT EXISTS`; absence becomes a repaired fresh shape before callers can inspect it. (`src-tauri/src/state/db.rs:443-598`)
20. Direct one-shot execution masks default-state open failures by falling back to an in-memory DB, so ordinary execution can succeed even when persistent state is unavailable. (`src-tauri/src/main.rs:1272-1275`)
21. Config load behavior differs across supported paths: trace treats malformed `sessions.toml` as an error, while resume paths use default config on providers/sessions load errors. (`src-tauri/src/main.rs:447-458`, `src-tauri/src/main.rs:1076-1084`)

## 3. Adjacent Surfaces in Blast Radius

1. `agents trace <invocation_uuid>` opens the default DB read-write today and emits the existing JSON precedent. (`src-tauri/src/main.rs:447-478`)
2. `agents repl [model] --resume <uuid>` opens the default DB, loads configs, resolves resume state, may migrate an active segment, writes invocation/session rows, and may ingest sessions after the child exits. (`src-tauri/src/main.rs:809-1054`)
3. `agents resume -m <model> --session-id <uuid>` validates UUID, opens the default DB, resolves ownership, may migrate, writes invocation/session/resume-acceptance rows, spawns the provider, and may ingest sessions after success. (`src-tauri/src/main.rs:1056-1263`)
4. Top-level `--resume` routes into `run_resume` or `run_repl` based on prompt/file/stdin presence, so it shares the same DB open and write surfaces. (`src-tauri/src/main.rs:341-389`)
5. Direct balanced one-shot execution opens the default DB, but falls back to in-memory state if opening fails; that fallback is current execution behavior, not an existing schema compatibility signal. (`src-tauri/src/main.rs:1265-1275`)
6. `agents migrate-db` is in blast radius because it intentionally exercises schema/backfill mechanics that schema-probe must inspect without running. (`src-tauri/src/main.rs:1450-1462`)
7. `agents migrate-config` is adjacent as a public command and anti-scope boundary. It rewrites config files and moves `session_storage`, but it is not a state DB opener today. (`src-tauri/src/main.rs:1472-1597`)
8. `06-locate`'s `agents session locate` is the sibling command in the same clap group. It currently still opens `StateDb::open_default`, and its proposal explicitly defers physical read-only open to 06-schema-probe. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:505-519`, `/home/nes/projects/agent-runner/worktrees/06-locate/proposals/06-locate.md:245-265`)
9. GUI/Tauri commands open `state.db` beside `models_dir`, not through CLI `dirs::data_dir()`, so DB-location divergence already exists. (`src-tauri/src/lib.rs:525-533`)
10. GUI quota refresh and discovery commands open the GUI-path state DB and then mutate quota/discovery rows. (`src-tauri/src/lib.rs:329-350`, `src-tauri/src/lib.rs:652-671`)
11. `session_scan` and `quota_check` examples are adjacent read/inspection precedents, but both open the real DB read-write and can mutate state through scanning or quota refresh. (`src-tauri/examples/session_scan.rs:16-74`, `src-tauri/examples/quota_check.rs:15-68`)
12. Provider/session config loading is adjacent because feature and storage claims may be confused with config presence. `ProvidersConfig::load` tolerates absent file as empty config and validates declared storage only when present. (`src-tauri/src/config/providers.rs:81-109`, `src-tauri/src/config/providers.rs:137-154`)
13. Transcript locator behavior is adjacent because existing JSON inspection (`trace --json`) can run scripts at read time, create adapter state dirs, and classify transcript state. Schema-probe has no current equivalent. (`README.md:374-386`, `src-tauri/src/sessions/mod.rs:156-199`)
14. Compaction backfill is adjacent to DB compatibility because `migrate-db` reads session chain segments and transcript paths, then writes compaction flags. (`src-tauri/src/main.rs:1450-1462`, `src-tauri/src/state/db.rs:2510-2575`)
15. `StateDb` tests already encode the current expectation that opening legacy shapes mutates them into the current schema; those tests are adjacent when changing open semantics. (`src-tauri/src/state/db.rs:3412-3485`, `src-tauri/src/state/db.rs:5183-5244`)

## 4. Currently Supported / User-Reachable Paths

1. `agents trace <invocation_uuid>` remains a supported inspection command with optional JSON output. (`README.md:405-426`, `src-tauri/src/main.rs:447-478`)
2. `agents repl <model>` opens state, selects a provider, starts/finalizes an invocation, and performs post-success session ingestion/emission when possible. (`src-tauri/src/main.rs:809-1054`)
3. `agents repl <model> --resume <uuid>` resolves the owner, emits `[resume] -> <provider>`, requires interactive args and resume config, writes the attempted resume target, and launches the provider REPL. (`src-tauri/src/main.rs:830-1007`)
4. `agents resume -m <model> --session-id <uuid>` reads answer payload, resolves ownership, launches one-shot resume, records acceptance, and writes stdout on success. (`src-tauri/src/main.rs:1056-1263`)
5. `agents -m <model> --resume <uuid> "prompt"` and stdin/file variants route through top-level dispatch into non-interactive resume; no-prompt top-level resume routes to REPL. (`src-tauri/src/main.rs:341-389`)
6. `agents migrate-db` is user-reachable and runs chain plus compaction backfills. (`src-tauri/src/main.rs:1450-1462`, `src-tauri/tests/initiative_05_migration.rs:1494-1528`)
7. `agents migrate-config` is documented and user-reachable; it rewrites runtime provider config and remains separate from schema-probe. (`README.md:127-140`, `src-tauri/src/main.rs:1472-1597`)
8. Direct CLI usage outside agent-runner is user-reachable through configured `turn_script`s; the runner ingests those turns into `session_turns` and mints imported chains during scans. (`README.md:330-372`, `src-tauri/src/sessions/mod.rs:55-141`)
9. `cargo run --release --example session_scan` is a supported diagnostic that runs turn scripts, ingests new turns, and prints per-provider counts. (`README.md:515-523`, `src-tauri/examples/session_scan.rs:1-74`)
10. `cargo run --release --example quota_check` is a supported diagnostic that opens the real state DB, refreshes quota, and prints balancer picks. (`README.md:515-523`, `src-tauri/examples/quota_check.rs:1-149`)
11. Hidden `agents resume-list <uuid>` remains reachable from the binary and opens the default DB to print chain previews. (`src-tauri/src/main.rs:155-157`, `src-tauri/src/main.rs:1889-1900`)
12. GUI `test_model` is user-reachable through Tauri and opens the GUI-path DB to select a provider and possibly mark exhaustion. (`src-tauri/src/lib.rs:490-507`)
13. GUI provider/account/discovery commands are user-reachable through Tauri and share `StateDb::open` with a different path root. (`src-tauri/src/lib.rs:525-538`, `src-tauri/src/lib.rs:647-671`)

## 5. Migration / Pre-Existing-State Implications

1. Fresh installs after current open get the full table set from `StateDb::open`: invocations, providers/quota windows, setup/discovery tables, `session_turns`, `session_chains`, and `session_chain_segments`. (`src-tauri/src/state/db.rs:443-598`)
2. Post-Initiative-04 era DBs may have provider quota tables without the latest `last_empty_refresh_at`, `exhausted_at`, or per-window delta columns; current open mutates those into place. (`src-tauri/src/state/db.rs:739-791`)
3. Post-Initiative-05 state includes `session_turns.is_compaction_boundary`, `session_chains`, `session_chain_segments`, and imported chain backfill. These are required for current `resolve_resume` lookup. (`src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:2577-2711`)
4. Pre-chain DBs can consist of `session_turns` plus `invocations` only; tests seed that shape and prove `migrate-db`/open can create `session_chains`. (`src-tauri/tests/initiative_05_migration.rs:1450-1507`)
5. Partially migrated DBs can contain chain rows and also segmentless old turn rows because open-path backfill skips when any chain exists. This is a current-state risk inherited from Initiative 05/06-locate. (`src-tauri/src/state/db.rs:2256-2271`, `/home/nes/projects/agent-runner/worktrees/06-locate/research/06-locate-problem-map.md:47-50`)
6. Existing `invocations` rows may be legacy-rebuilt during open; unmappable old rows become `provider_name = NULL` and `status = 'legacy'`, reducing their usefulness as session/model provenance. (`src-tauri/src/state/db.rs:891-1034`, `src-tauri/src/state/db.rs:3488-3541`)
7. Current `PRAGMA user_version` is not used by code. SQLite makes it inspectable in principle, but no current open path reads or writes it. (`src-tauri/src/state/db.rs:431-877`)
8. There is no dedicated metadata table for schema version or feature support. Existing metadata-like tables are setup/discovery/account tables with product-specific payloads, not DB compatibility state. (`src-tauri/src/state/db.rs:482-557`)
9. Schema-probe's requested `schema_version` field has no current source of truth. The only existing signals are table/column/index presence, Cargo/Tauri binary version, and absence/presence of stacked feature command arms. (`src-tauri/Cargo.toml:1-4`, `src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:826-877`)
10. Future-migration states are currently indistinguishable from "unknown extra schema" unless an incompatible missing/changed table or column breaks query preparation; no current probe refuses before corruption or mutation. (`src-tauri/src/state/db.rs:618-791`)
11. Pre-multi-window quota state is folded into `provider_quota_windows` during open with `INSERT OR IGNORE`; that is another example of schema/data migration on ordinary open. (`src-tauri/src/state/db.rs:455-480`)
12. Pre-parent/sidechain session-turn state is upgraded by `ensure_session_turns_schema`; tests assert the columns and lookup index are added on open. (`src-tauri/src/state/db.rs:693-721`, `src-tauri/src/state/db.rs:3412-3485`)
13. Post-some-future DBs with extra tables or columns are not rejected by current code. The ensure helpers inspect only named tables/columns they know how to create, add, or drop. (`src-tauri/src/state/db.rs:618-824`)

## 6. Observability Gaps That Exist Today

1. There is no CLI-level "state DB is compatible with this binary" check. The README sends users to ad-hoc SQL for questions outside trace. (`README.md:500-512`)
2. There is no way to discover `schema_version` or `user_version` from the `agents` binary; manual SQLite inspection is the only route, and `user_version` is currently unused. (`src-tauri/src/state/db.rs:431-877`)
3. There is no binary feature list for `session locate`, `session export`, `session import-replace`, or pause-handshake support. Feature support is inferred from command help or failed execution. (`src-tauri/src/main.rs:77-166`)
4. There is no CLI output that names supported storage types. Storage type lives in provider config and, on the locate branch, in locate metadata for one session. (`src-tauri/src/config/model.rs:195-200`, `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-38`)
5. There is no structured refusal report for missing session schema tables/columns/indexes. Current open either repairs them, mutates them, or fails with a string error. (`src-tauri/src/state/db.rs:618-791`)
6. There is no no-side-effect path to ask where the default state DB is. `open_default` computes the path and immediately calls mutating `open`. (`src-tauri/src/state/db.rs:611-615`)
7. GUI and CLI state DB path divergence is not surfaced by any probe; a compatible CLI DB does not imply the GUI-path DB is compatible, or vice versa. (`src-tauri/src/lib.rs:525-533`, `src-tauri/src/state/db.rs:611-615`)
8. `trace --json` observes one invocation tree, not the binary/schema surface; it cannot answer whether future session import/replace would be safe. (`src-tauri/src/trace/mod.rs:23-80`)
9. There is no current command that reports the exact `state.db` path without also opening it or running a path-specific workflow. (`src-tauri/src/state/db.rs:611-615`, `src-tauri/examples/session_scan.rs:16-27`)
10. There is no current structured distinction between "DB missing", "DB old but repairable", "DB incompatible", and "DB operationally inaccessible"; current callers return command-specific string errors or repair in place. (`src-tauri/src/state/db.rs:431-608`, `src-tauri/src/main.rs:447-478`)

## 7. Assumption Register Draft

1. A1: a read-only `StateDb` open variant can be added. Evidence: the read-only probe only needs table/column/index inspection, and current resolver/read callers use `rusqlite` queries after open. Invalidator: current WAL/schema/backfill side effects remain required before any correct session read. (`src-tauri/src/state/db.rs:431-608`, `src-tauri/src/state/db.rs:2577-2711`)
2. A2: a stable schema-version field is feasible. Evidence: current schema shape is already centralized enough to enumerate key tables, columns, and indexes; SQLite also exposes `PRAGMA user_version`. Invalidator: schema remains too fluid or path-dependent, with fresh/post-open/post-migrate states unable to map to one integer without hiding important differences. (`src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:826-877`)
3. A3: feature-flag enumeration is binary-version-bound enough for harness gating. Evidence: command support is compiled into clap arms and modules, not loaded from user config. Invalidator: a claimed feature's actual safety depends on config presence or provider storage rather than binary support. (`src-tauri/src/main.rs:77-166`, `src-tauri/src/config/providers.rs:81-154`)
4. A4: the CLI default state-DB path is a stable enough probe target. Evidence: CLI callers consistently use `StateDb::open_default()` and README documents the data-dir path. Invalidator: GUI-state-DB divergence is considered part of the public session surface. (`src-tauri/src/state/db.rs:611-615`, `README.md:222-224`, `src-tauri/src/lib.rs:525-533`)
5. A5: 06-locate lands first and supplies the `session` clap group. Evidence: the locate worktree already contains the group and dispatch, and Initiative 06 sequences locate before schema-probe. Invalidator: schema-probe is merged independently before locate. (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`, `/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:75-86`)
6. A6: `schema-incompatible` can be evaluated from structural inspection without running migrations. Evidence: required probe tables/columns/indexes are known SQL structures today. Invalidator: compatibility depends on data invariants that are expensive, mutating, or impossible to validate read-only. (`src-tauri/src/state/db.rs:559-597`, `src-tauri/src/state/db.rs:826-877`, `src-tauri/src/state/db.rs:2256-2363`)

## What This Map Deliberately Does NOT Cover

1. `06-locate` / `agents session locate <session-id>` behavior is outside this map except for the shared `session` clap group and read-only-open dependency.
2. `06-export` / `agents session export <session-id>` and canonical transcript JSONL rules are outside this map.
3. `06-pause-handshake` / `agents session pause-handshake` and `resume-handshake` locking semantics are outside this map except as a future feature flag name.
4. `06-import-replace` / `agents session import-replace <session-id>` atomic replace semantics are outside this map except for the requested `safe_for_import_replace` probe field.
5. General provider spawn, auto-resume behavior, quota refresh policy, auth refresh, and balancer scoring are outside this map except where existing commands open or mutate the same state DB.
6. Config migration design is outside this map; `migrate-config` is included only as a public anti-scope boundary.
7. Frontend/Tauri UI design, Ollie/design-system work, HomeView/StatusView, and desktop visualization are outside this map except for GUI DB path divergence.
8. Raw transcript normalization, provider-native JSONL schemas, cross-CLI migration policy, and alternate export formats are outside this map.
