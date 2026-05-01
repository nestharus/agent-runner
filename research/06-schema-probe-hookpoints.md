# Phase 5 Hookpoints — 06-schema-probe (`agents session schema-probe`)

> **Note (pre-change evidence):** This hookpoint map describes the current
> `06-schema-probe` worktree before any Phase 6 implementation. Rev 2 is the
> primary action map; it explicitly adds/extends the `session` command group,
> adds `StateDb::open_read_only`, chooses `PRAGMA user_version`, defines a
> hardcoded feature map, and leaves existing commands unchanged
> (`proposals/06-schema-probe.md:22-33`). Round 1 risk findings were limited
> to JSON map-shape ambiguity and `ReadOnlyOpenError` variant discipline, both
> addressed in Rev 2 (`risk/06-schema-probe-audit-history.md:22-34`).

## A. `session schema-probe` Subcommand Surface Hookpoints

- **Extend or introduce:** local `Subcommands` currently has no `Session`
  group; it contains `Trace`, `Repl`, `Resume`, hidden `ResumeList`,
  `MigrateDb`, and `MigrateConfig` (`src-tauri/src/main.rs:77-166`). Add
  `Session { #[command(subcommand)] command: SessionSubcommands }` if
  schema-probe is applied directly to this worktree.
- **Stacked-base path:** 06-locate already adds `Subcommands::Session` and a
  `SessionSubcommands::Locate` child at
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`.
  If that lands first, extend that existing enum with `SchemaProbe`; do not add
  a second parent group or a top-level alias (`proposals/06-schema-probe.md:87-98`).
- **New child shape:** `SchemaProbe` takes no flags in v1. The proposal
  explicitly forbids `--state-db`; it must report only the CLI default DB path
  (`proposals/06-schema-probe.md:100-105`).
- **Bare parent behavior:** the root CLI uses optional root subcommands
  (`src-tauri/src/main.rs:24-26`), but the nested `Session` child should be
  non-optional like 06-locate's shape
  (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:157-160`).
- **Preserve root conflicts:** `args_conflicts_with_subcommands = true` is on
  the root parser (`src-tauri/src/main.rs:18-23`). Adding `session` preserves
  the existing top-level conflict model for `--resume`, `--model`, prompt args,
  `--file`, and related root options.
- **Dispatch hookpoint:** local dispatch is the `match command` in `run(cli)`
  before top-level `--resume` routing (`src-tauri/src/main.rs:287-338`,
  `src-tauri/src/main.rs:341-389`). Add a `Subcommands::Session { command }`
  arm there. On the locate branch, extend the existing nested match at
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:354-358`.
- **Exit `2` source:** do not hand-roll usage errors; unknown flags/children
  and bare parent misuse remain clap errors (`proposals/06-schema-probe.md:241-242`,
  `proposals/06-schema-probe.md:284`). Hidden `resume-list` is not reusable:
  it opens state and prints previews (`src-tauri/src/main.rs:155-157`,
  `src-tauri/src/main.rs:1887-1900`).

## B. `StateDb::open_read_only` + `ReadOnlyOpenError` API Hookpoints

- **Primary module:** implement `StateDb::default_path()` and
  `StateDb::open_read_only(path)` in `src-tauri/src/state/db.rs`, next to the
  current mutating open path (`src-tauri/src/state/db.rs:431-615`).
- **Current wrapper shape:** `StateDb` is a thin wrapper over one private
  `rusqlite::Connection` (`src-tauri/src/state/db.rs:48-50`). Returning `Self`
  from `open_read_only` fits the current API, but all write methods remain
  callable at type level; schema-probe must keep the returned value inside
  read-only inspection helpers.
- **Default path extraction:** `open_default()` currently computes
  `dirs::data_dir()/oulipoly-agent-runner/state.db` and immediately calls
  mutating `open` (`src-tauri/src/state/db.rs:611-615`). Factor that path
  computation into `default_path()` so schema-probe can report/check the path
  without creating directories.
- **Public error type:** define `pub enum ReadOnlyOpenError` in `db.rs` with
  exactly the Rev 2 variants: `Missing`, `NotADatabase`, `PermissionDenied`,
  `WalSidecarError`, and `Operational` (`proposals/06-schema-probe.md:303-309`).
  Re-export it from `src-tauri/src/state/mod.rs` beside `StateDb`
  (`src-tauri/src/state/mod.rs:3-10`) if CLI/tests import it through
  `agent_runner_lib::state`.
- **Open/classification behavior:** detect missing files before SQLite open
  (`proposals/06-schema-probe.md:247-250`, `proposals/06-schema-probe.md:319-320`).
  Use read-only SQLite flags instead of `Connection::open`
  (`src-tauri/src/state/db.rs:437`), and do not set `immutable=1` because Rev 2
  requires live WAL visibility (`proposals/06-schema-probe.md:333-337`).
- **Do not share mutating open body:** `StateDb::open` creates the parent dir,
  opens read-write, sets WAL, ensures schemas, and backfills chains
  (`src-tauri/src/state/db.rs:431-608`). `open_read_only` must bypass all of
  those operations (`proposals/06-schema-probe.md:327-331`).
- **Error mapping boundary:** keep SQLite/IO classification in `state/db.rs`;
  keep stderr JSON and exit mapping in the CLI wrapper. `WalSidecarError`
  covers read-only WAL/shm access failures (`proposals/06-schema-probe.md:276-291`,
  `proposals/06-schema-probe.md:321-322`).

## C. Resolution Flow Hookpoints (Proposal §4 Numbered Steps)

- **Step 1, parse:** parsing is rooted in `Cli::parse()`/`run(cli)` and the
  `Subcommands` enum (`src-tauri/src/main.rs:18-26`,
  `src-tauri/src/main.rs:287-338`). `schema-probe` should have no extra
  pre-dispatch parsing beyond clap.
- **Step 2, default DB path without mkdir:** factor the computation from
  `StateDb::open_default()` (`src-tauri/src/state/db.rs:611-615`) into
  `StateDb::default_path()`. Do not call `open_default()` because it mutates.
- **Step 3, binary identity:** package name/version are available from Cargo
  metadata (`src-tauri/Cargo.toml:1-4`). There is no commit embedding today:
  `build.rs` only calls `tauri_build::build()` (`src-tauri/build.rs:1-3`).
  Use compile-time env values and emit `"unknown"` for missing commit, as
  required by Rev 2 (`proposals/06-schema-probe.md:117-120`,
  `proposals/06-schema-probe.md:244-246`).
- **Step 3, feature/storage helpers:** add pure helpers for `features` and
  `supported_storage_types`; they do not need config, state DB, provider
  detection, or clap introspection (`proposals/06-schema-probe.md:188-202`,
  `proposals/06-schema-probe.md:204-218`).
- **Step 4, missing DB fast path:** after resolving the default path, check
  file existence before opening SQLite. Missing DB returns a full success
  report with every required table/column/index key present and `false`
  (`proposals/06-schema-probe.md:247-250`).
- **Step 5, read-only DB open:** call `StateDb::open_read_only(&path)`, never
  `StateDb::open_default()` or `StateDb::open`. Current callers of mutating
  open include trace (`src-tauri/src/main.rs:447-448`), repl
  (`src-tauri/src/main.rs:809-817`), resume
  (`src-tauri/src/main.rs:1056-1072`), migrate-db
  (`src-tauri/src/main.rs:1450-1451`), and resume-list
  (`src-tauri/src/main.rs:1887-1889`); schema-probe is the first read-only
  caller.
- **Step 6, user version:** add `StateDb::user_version(&self) -> Result<i64,
  String>` or similar in `db.rs`. Current code has no `user_version` or
  `schema_version` references outside the input-schema docs search noise; the
  problem map correctly says this source does not exist yet.
- **Step 7, structural inspection:** add `StateDb::inspect_session_schema()` or
  equivalent in `db.rs`; use only `sqlite_master`, `PRAGMA table_info`, and
  index metadata, and initialize every canonical Rev 2 key before reading
  (`proposals/06-schema-probe.md:134-163`, `proposals/06-schema-probe.md:256-263`).
- **Step 8, compatibility:** compute compatibility in the report layer from
  `schema_version`, `CURRENT_SCHEMA_VERSION`, `MINIMUM_SUPPORTED_SCHEMA_VERSION`,
  and the structural maps (`proposals/06-schema-probe.md:264-266`). Keep this
  separate from operational inspection errors.
- **Steps 9-11, exits:** incompatible present DB emits stderr JSON code
  `schema-incompatible` and exit `14`; operational open/inspection failures are
  exit `1`; compatible DB emits compact stdout JSON and exit `0`
  (`proposals/06-schema-probe.md:267-274`, `proposals/06-schema-probe.md:285-287`).
  Compact output intentionally differs from trace's pretty JSON
  (`src-tauri/src/main.rs:470-473`).

## D. Schema-Version Source: `PRAGMA user_version` vs Metadata Table

- **Confirmed choice:** Rev 2 chooses D1a, `PRAGMA user_version`, and rejects a
  metadata table because it creates another bootstrap rule
  (`proposals/06-schema-probe.md:165-172`). Phase 5 found no evidence
  invalidating that choice.
- **Current absence:** searches in `src-tauri/src`, `src-tauri/tests`, and
  `README.md` find no existing `user_version`, `schema_version`,
  `CURRENT_SCHEMA_VERSION`, or metadata-table implementation. Existing schema
  evolution is embedded in create/ensure helpers (`src-tauri/src/state/db.rs:618-877`).
- **Version constants:** add `CURRENT_SCHEMA_VERSION: i64 = 3` and
  `MINIMUM_SUPPORTED_SCHEMA_VERSION: i64 = 3` in `state/db.rs` or a new
  schema-probe module. Prefer central placement near the DB inspection helpers
  if mutating `StateDb::open` will stamp the same constant
  (`proposals/06-schema-probe.md:182-186`).
- **Migration path hookpoint:** mutating open is the migration path that can
  stamp `PRAGMA user_version = CURRENT_SCHEMA_VERSION` after it has ensured the
  current schema. The natural insertion point is after
  `ensure_invocations_schema`, the table bootstrap batch, quota/session-turn
  ensure helpers, and before returning the DB (`src-tauri/src/state/db.rs:441-608`).
- **`migrate-db` implication:** `run_migrate_db` calls `StateDb::open_default()`
  and then calls `backfill_session_chains()` again (`src-tauri/src/main.rs:1450-1457`).
  If stamping lives inside `StateDb::open`, `migrate-db` inherits it without a
  second special case.
- **Missing/unversioned semantics:** missing DB reports version `0` with exit
  `0`; existing DB with `user_version = 0` is schema-incompatible exit `14`
  (`proposals/06-schema-probe.md:174-186`, `proposals/06-schema-probe.md:438-441`).
- **No metadata table:** existing metadata-like tables are product tables
  created in the bootstrap batch (`memory_nodes`, `setup_sessions`,
  `cli_providers`, `discovered_models`, `model_parameters`) rather than DB
  compatibility state (`src-tauri/src/state/db.rs:482-557`). Do not add a new
  metadata table for this feature.

## E. Feature-Flag Enumeration Hookpoints

- **New pure surface:** there is no current binary feature list for
  `session_locate`, `session_export`, `session_import_replace`,
  `session_pause_handshake`, or `session_schema_probe`; support is implicit in
  compiled clap arms (`src-tauri/src/main.rs:77-166`).
- **Hardcoded list:** implement a static/pure feature map with exactly the Rev
  2 keys: `session_locate`, `session_export`, `session_import_replace`,
  `session_pause_handshake`, and `session_schema_probe`
  (`proposals/06-schema-probe.md:197-202`).
- **Locate value depends on base:** local worktree has no `session locate`; if
  schema-probe lands independently, `session_locate` is `false`. If the base
  includes 06-locate's `SessionSubcommands::Locate`, `session_locate` is
  `true` (`proposals/06-schema-probe.md:197-201`,
  `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:174-184`).
- **Future siblings:** leave export/import-replace/pause-handshake false in
  this PR. Future sibling PRs update the same static helper when their contracts
  land (`proposals/06-schema-probe.md:200-202`).
- **Do not use Cargo features:** `Cargo.toml` has ordinary dependencies and no
  product feature switches for these commands (`src-tauri/Cargo.toml:10-27`);
  Rev 2 explicitly rejects Cargo features for this map
  (`proposals/06-schema-probe.md:190-195`).
- **Do not introspect clap:** command presence alone does not prove harness
  contract or side-effect semantics, so feature enumeration should be an
  explicit reviewed list, not generated from `Subcommands`
  (`proposals/06-schema-probe.md:192-195`).

## F. `safe_for_import_replace` Predicate Hookpoints

- **Pure predicate:** implement as a small helper over the already-built report
  inputs: DB existence, compatibility, version range, structural maps, feature
  map, and supported storage vocabulary (`proposals/06-schema-probe.md:222-233`).
- **Expected current value:** in this PR the predicate should normally be
  `false` because `session_import_replace` and `session_pause_handshake` are
  false (`proposals/06-schema-probe.md:235-237`,
  `proposals/06-schema-probe.md:395-396`).
- **No runtime provider checks:** do not load config, run locators, or examine
  transcripts. Schema-probe reports binary/schema readiness, not one session's
  replaceability (`proposals/06-schema-probe.md:359-382`).
- **Storage input:** use the public `supported_storage_types` list, not the
  internal config enum. The predicate should fail if the public list does not
  contain storage types required by import-replace's approved future contract
  (`proposals/06-schema-probe.md:232-233`).

## G. Storage-Type Vocabulary Hookpoints

- **Internal enum boundary:** current config storage is
  `SessionStorage::{ClaudeCode, Codex}` with serde tags `claude_code` and
  `codex` (`src-tauri/src/config/model.rs:195-229`). This is not the public
  probe vocabulary because the public Codex value is `codex_session`
  (`proposals/06-schema-probe.md:204-218`).
- **06-locate public enum:** the stacked locate branch defines
  `SessionStorageType::{ClaudeCode, CodexSession, Other}` with
  `#[serde(rename_all = "snake_case")]`
  (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-39`).
- **Import or duplicate:** if 06-locate has landed, import/reuse its public
  `SessionStorageType`; if schema-probe lands first, define a local enum with
  the same serialized values and collapse duplicates when branches merge
  (`proposals/06-schema-probe.md:206-218`).
- **No config migration:** do not rename `SessionStorage::Codex` or accept
  `codex_session` in TOML. `ProvidersConfig` currently reads optional
  `session_storage` and validates the internal enum
  (`src-tauri/src/config/providers.rs:81-109`,
  `src-tauri/src/config/providers.rs:149-154`).
- **Supported list source:** emit `["claude_code","codex_session","other"]`
  statically; it is public capability vocabulary, not configured providers.

## H. Read-Only Behavior Hookpoints (§8 Side-Effect Contract Enforcement)

- **Disallowed open path:** current `StateDb::open` creates parent dirs
  (`src-tauri/src/state/db.rs:431-435`), opens read-write/create
  (`src-tauri/src/state/db.rs:437`), sets WAL (`src-tauri/src/state/db.rs:439-440`),
  ensures schema (`src-tauri/src/state/db.rs:441-603`), and runs chain backfill
  (`src-tauri/src/state/db.rs:604-606`). Schema-probe must use none of this.
- **No schema repair:** ensure helpers mutate by creating/rebuilding/altering
  invocations (`src-tauri/src/state/db.rs:618-676`), adding session-turn
  columns/indexes (`src-tauri/src/state/db.rs:693-721`), and quota table
  changes (`src-tauri/src/state/db.rs:739-824`). Probe inspection must only
  observe missing structures.
- **No backfill:** `backfill_session_chains` inserts chains and active segments
  from `session_turns` (`src-tauri/src/state/db.rs:2256-2363`). D3 tests must
  prove a legacy DB remains legacy after probe (`proposals/06-schema-probe.md:393-394`).
- **No config/transcript touch:** unlike trace, schema-probe should not load
  `sessions.toml` (`src-tauri/src/main.rs:447-458`) or call transcript locator
  code that may create adapter state dirs (`src-tauri/src/sessions/mod.rs:171-199`,
  `src-tauri/src/sessions/mod.rs:183-185`). Rev 2 forbids
  transcript/config/provider side effects (`proposals/06-schema-probe.md:371-382`).
- **Existing command preservation:** trace, repl, resume, top-level resume,
  hidden resume-list, migrate-db, and migrate-config are explicitly unchanged
  (`proposals/06-schema-probe.md:28-33`). Do not retrofit trace to read-only
  open in this PR (`proposals/06-schema-probe.md:361-362`).
- **GUI DB boundary:** GUI/Tauri commands open a DB next to `models_dir`
  (`src-tauri/src/lib.rs:525-533`). Schema-probe reports only CLI default
  state (`proposals/06-schema-probe.md:56`, `proposals/06-schema-probe.md:424-436`).
- **Permitted observations:** path resolution, `exists`, read-only SQLite open,
  `PRAGMA user_version`, `sqlite_master`, table/index metadata, and SQLite's
  read-only WAL/shm reads are allowed (`proposals/06-schema-probe.md:379-382`).

## I. Test-Intent Track Hookpoints (Proposal §9.1 Rows)

- **Unit tests, pure report helpers:** feature map, storage vocabulary
  serialization, compatibility calculation, and `safe_for_import_replace`
  predicate can live near the new schema-probe/report helper module. Parser
  tests already live in `src-tauri/src/main.rs` around
  `src-tauri/src/main.rs:2157-2250`.
- **State DB component tests:** `StateDb::open_read_only`, `user_version`, and
  `inspect_session_schema` tests belong in `src-tauri/src/state/db.rs` because
  they need access to the private `conn` and local fixture helpers. Existing DB
  schema tests already query `sqlite_master` and PRAGMAs directly
  (`src-tauri/src/state/db.rs:3333-3485`).
- **CLI integration tests:** add a new `src-tauri/tests/initiative_06_schema_probe.rs`
  following the existing process-spawn pattern with
  `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`
  (`src-tauri/tests/pr_b_trace_integration.rs:107-125`,
  `src-tauri/tests/pr_f_resume_integration.rs:360-384`).
- **Temp data-dir fixture:** reuse the fixture convention of isolated
  `XDG_CONFIG_HOME` and `XDG_DATA_HOME` plus a deterministic default DB path
  (`src-tauri/tests/pr_f_resume_integration.rs:18-39`,
  `src-tauri/tests/initiative_05_migration.rs:30-49`).
- **D1 tests:** hand-seed `PRAGMA user_version` fixtures for `0`, `2`, and `3`;
  assert `0`/`2` exit `14`, `3` succeeds with required structures, and mutating
  open stamps current version once the stamp hook is chosen
  (`proposals/06-schema-probe.md:390-392`).
- **D2 tests:** pure feature-map test must assert exact keys and values, with
  `session_locate` conditioned on whether the final base contains locate
  (`proposals/06-schema-probe.md:392`).
- **D3 no-side-effect tests:** seed old `session_turns` only, snapshot schema,
  `PRAGMA user_version`, row counts, mtime/content, run probe, then assert no
  added tables/columns/indexes and no chain rows. Existing tests prove mutating
  open currently adds session-turn columns/indexes (`src-tauri/src/state/db.rs:3412-3485`)
  and backfills chains (`src-tauri/src/state/db.rs:5183-5244`).
- **D4 predicate tests:** pure predicate plus report-builder fixture should
  prove `safe_for_import_replace` remains false with current feature flags and
  only becomes true when import-replace and pause flags are toggled in the test
  fixture (`proposals/06-schema-probe.md:395-396`).
- **D5 vocabulary tests:** serialize the storage list and assert exactly
  `["claude_code","codex_session","other"]`; also assert internal `codex`
  never appears in probe JSON (`proposals/06-schema-probe.md:396`).
- **D6 tests:** cover missing DB exit `0`, invalid/unreadable DB exit `1`, and
  older/newer/missing-structure exit `14`; assert canonical map shape and
  stdout/stderr split (`proposals/06-schema-probe.md:397-400`).
- **D7 static check:** review/static test should ensure only the schema-probe
  path calls `open_read_only`; trace/repl/resume/migrate-db/resume-list retain
  current `StateDb::open_default()` behavior (`proposals/06-schema-probe.md:400`).
- **Side-effect contract integration:** place sentinel config/transcript files
  and seeded DB rows; run probe; assert sentinels and row counts unchanged
  (`proposals/06-schema-probe.md:401-402`).
- **README check:** no README snapshot test exists today; update/check the
  subcommand list, trace/resume area, and SQL inspection area
  (`README.md:127-140`, `README.md:405-426`, `README.md:458-512`).

## J. Deletion Candidates / Conflict Check

- **No deletion candidate:** there is no current `schema-probe` stub, no
  `StateDb::open_read_only`, no schema-version constant, and no feature-list
  helper. This PR adds missing surfaces rather than deleting placeholders.
- **Keep mutating `StateDb::open`:** many supported paths depend on its current
  repair/backfill behavior (`src-tauri/src/main.rs:447-448`,
  `src-tauri/src/main.rs:809-817`, `src-tauri/src/main.rs:1056-1072`,
  `src-tauri/src/main.rs:1450-1451`). Rev 2 explicitly leaves it unchanged
  (`proposals/06-schema-probe.md:28-33`).
- **Keep `open_default`:** existing callers use `open_default()` for the
  mutating operational path (`src-tauri/src/state/db.rs:611-615`). Add
  `default_path()` underneath it; do not change all callers.
- **Keep hidden `resume-list`:** it remains a human chain-inspection surface,
  not a schema compatibility report (`src-tauri/src/main.rs:155-157`,
  `src-tauri/src/main.rs:1887-1900`).
- **Do not rename internal storage:** `SessionStorage::Codex` must continue to
  serialize as `codex` in config (`src-tauri/src/config/model.rs:195-229`).
  The public `codex_session` value belongs only to schema-probe/locate JSON.
- **Stacked-branch merge conflict:** local `Subcommands` has no `Session`, but
  06-locate's branch does (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-185`).
  Resolve by extending the locate enum if present, not by duplicating.
- **Shared enum conflict:** local worktree has no `session_metadata` module in
  `src-tauri/src/lib.rs:1-11`; 06-locate adds one with `SessionStorageType`
  (`/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/session_metadata/mod.rs:23-39`).
  Reuse when present, duplicate only in independent schema-probe implementation.
- **No problem-map invalidation found:** Phase 5 evidence matches the approved
  assumption register. No `NEEDS_INPUT` stop condition was triggered.

## K. Implementation Surface Summary Table

| Proposal action | Hookpoint | Reuse / extend / new |
| --- | --- | --- |
| `session` command parent | `src-tauri/src/main.rs:77-166`; or 06-locate `Subcommands::Session` at `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:156-160` | extend or new |
| `schema-probe` child | new `SessionSubcommands::SchemaProbe`; extend locate enum at `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:174-185` if present | new / extend |
| Top-level dispatch | `src-tauri/src/main.rs:287-338`; locate branch nested dispatch at `/home/nes/projects/agent-runner/worktrees/06-locate/src-tauri/src/main.rs:354-358` | extend |
| CLI default DB path | factor from `src-tauri/src/state/db.rs:611-615` into `StateDb::default_path()` | extend |
| Read-only open | `src-tauri/src/state/db.rs:431-608` adjacency; new `open_read_only` bypasses this body | new |
| `ReadOnlyOpenError` | `src-tauri/src/state/db.rs`; re-export from `src-tauri/src/state/mod.rs:3-10` | new |
| User-version helper | `StateDb::user_version(&self)` near DB inspection helpers | new |
| Session schema inspection | `sqlite_master` / PRAGMA helpers near `invocations_columns` and `session_turns_columns` (`src-tauri/src/state/db.rs:678-735`) | new |
| Required structures | invocation/session/chain SQL at `src-tauri/src/state/db.rs:559-597`, `:826-877` | reuse as source of truth |
| Version constants | new constants, preferably in `state/db.rs` or schema-probe report module | new |
| Mutating stamp path | `StateDb::open` after schema ensure/backfill decision (`src-tauri/src/state/db.rs:441-608`) | extend |
| Binary identity | `src-tauri/Cargo.toml:1-4`; `src-tauri/build.rs:1-3` has no commit today | reuse/extend |
| Feature map | pure helper; no current feature enum (`src-tauri/src/main.rs:77-166`) | new |
| Supported storage types | import 06-locate `SessionStorageType` if present; else local enum | reuse or new |
| Internal storage boundary | `src-tauri/src/config/model.rs:195-229` | do not change |
| `safe_for_import_replace` | pure predicate over report fields | new |
| JSON / exit mapping | schema-probe CLI wrapper; contrast pretty trace JSON at `src-tauri/src/main.rs:470-473`; exits in `proposals/06-schema-probe.md:276-291` | new |
| Preserve trace/repl/resume | current `StateDb::open_default()` call sites (`src-tauri/src/main.rs:447-448`, `:809-817`, `:1056-1072`) | retain |
| Preserve migrate-db/resume-list | `src-tauri/src/main.rs:1450-1462`, `:1887-1900` | retain |
| Unit / DB tests | parser tests at `src-tauri/src/main.rs:2157-2250`; schema tests at `src-tauri/src/state/db.rs:3333-3485` | extend |
| CLI integration tests | new `src-tauri/tests/initiative_06_schema_probe.rs`; fixture patterns in `src-tauri/tests/pr_b_trace_integration.rs:107-125` and `src-tauri/tests/pr_f_resume_integration.rs:18-39` | new + reuse |
| README synopsis/docs | `README.md:127-140`, `README.md:405-426`, `README.md:458-512` | extend |

## What this hookpoint research deliberately does NOT cover

1. `06-locate` behavior except for shared `session` command shape and optional
   reuse of its public `SessionStorageType`.
2. `06-export` canonical transcript reading or export JSONL format.
3. `06-pause-handshake` lock acquisition/renewal/release semantics, except as
   a future feature flag required by `safe_for_import_replace`.
4. `06-import-replace` atomic replacement, crash recovery, or preimage checks,
   except for the static safety predicate field.
5. Provider spawn, resume argv composition, auto-resume, quota refresh,
   diagnostics, setup, discovery, scan ingestion, and transcript locator
   behavior.
6. GUI/Tauri state DB compatibility, frontend UI, HomeView/StatusView/PoolsView,
   and Ollie/design-system work.
7. Config migration design beyond preserving `migrate-config` as an anti-scope
   boundary.
8. Deep data-integrity validation for session chain completeness; schema-probe
   is structural plus versioned and does not repair Initiative 05 partial-chain
   states.
