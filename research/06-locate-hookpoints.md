# Phase 5 Hookpoints — 06-locate (`agents session locate`)

> **Note (pre-change evidence):** This hookpoint map describes the current
> `06-locate` worktree before any Phase 6 implementation. The risk gate cleared
> Rev 2 and forwarded WS1, WS4, and WS5 (`risk/06-locate-audit-history.md:100-116`).
> The approved proposal is `proposals/06-locate.md` Rev 2; its committed changes
> are the `session locate` CLI surface, a reusable `SessionMetadata` module,
> `TranscriptState` extraction, README updates, and no behavior change to
> `resume`, `repl --resume`, `trace --json`, `migrate-db`, or `migrate-config`
> (`proposals/06-locate.md:13-26`).

## A. `session` subcommand surface hookpoints (proposal §2)

- **Extend:** `Subcommands` lives in `src-tauri/src/main.rs:77-166`. Add a
  `Session { #[command(subcommand)] command: SessionSubcommands }` variant near
  the existing user-facing subcommands, before hidden `ResumeList` is the least
  surprising placement (`src-tauri/src/main.rs:77-157`).
- **New:** define `SessionSubcommands` in `src-tauri/src/main.rs` near
  `Subcommands`. It only needs `Locate { session_id: String, #[arg(long)] json:
  bool }` for v1, matching the proposal shape (`proposals/06-locate.md:70-85`).
- **Extend:** top-level dispatch is the `match command` in
  `run(cli)` at `src-tauri/src/main.rs:287-338`. Add the `Subcommands::Session`
  arm before `ResumeList`, `MigrateDb`, and `MigrateConfig`; this keeps the
  subcommand path before top-level `--resume` routing at `src-tauri/src/main.rs:341-389`.
- **Help-only bare parent:** current `Cli` uses `#[command(subcommand)] command:
  Option<Subcommands>` (`src-tauri/src/main.rs:24-26`). For the nested parent,
  make the child required by using a non-optional `SessionSubcommands` field as
  in the proposal. Clap will reject bare `agents session` with a usage error
  rather than running a default action; no custom handler is needed.
- **Preserve top-level conflicts:** `args_conflicts_with_subcommands = true`
  is on the root parser (`src-tauri/src/main.rs:18-23`). Adding a nested
  subcommand does not change top-level conflict behavior because `session` is
  still one root subcommand.
- **No hookpoint — net new:** there is no existing `session` command group
  today (`research/06-locate-problem-map.md:5-7`). Do not reuse hidden
  `resume-list`; it is a different command shape and output contract.

## B. `SessionMetadata` reusable API hookpoints (proposal §6)

- **New module:** create `src-tauri/src/session_metadata/` with `mod.rs`.
  Expose it from the library crate by adding `pub mod session_metadata;` next to
  existing modules in `src-tauri/src/lib.rs:1-11`. The module must be public
  because 06-export and 06-import-replace are expected downstream consumers
  (`initiatives/06-session-override-contract.md:41-43`).
- **Public types:** define `pub struct SessionMetadata`, `pub enum
  SessionStorageType`, `pub enum MetadataError`, and moved `pub enum
  TranscriptState` in `src-tauri/src/session_metadata/mod.rs`. `SessionMetadata`,
  `SessionStorageType`, and `TranscriptState` need `serde::Serialize`; use
  `#[serde(rename_all = "snake_case")]` on both enums so `CodexSession`
  serializes as `codex_session` and `TranscriptState` preserves trace's current
  values.
- **Crate-internal helpers:** workspace-root derivation, path canonicalization,
  storage mapping, and CLI error formatting should remain `pub(crate)` or
  private in the new module / main wrapper. Only the proposal's types and
  `locate_session_metadata` should be public.
- **Move:** `TranscriptState` is currently defined in `trace` at
  `src-tauri/src/trace/mod.rs:73-80`. It is stored on `TraceSession` at
  `src-tauri/src/trace/mod.rs:59-70`.
- **Current writers:** `build_trace_session` writes every state:
  `Unresolved` for no session id or missing provider at
  `src-tauri/src/trace/mod.rs:242-276`; `NoLocator` for missing sessions config
  or missing locator at `src-tauri/src/trace/mod.rs:300-333`; `Available` for an
  existing located path at `src-tauri/src/trace/mod.rs:334-348`; `Missing` for a
  non-existing located path or locator error at `src-tauri/src/trace/mod.rs:349-381`.
- **Current readers:** ASCII trace calls `TranscriptState::as_str()` at
  `src-tauri/src/trace/mod.rs:425-438`. Tests compare enum values directly at
  `src-tauri/src/trace/mod.rs:1337-1343` and `src-tauri/src/trace/mod.rs:1429-1433`,
  and JSON tests assert string values at `src-tauri/src/trace/mod.rs:1017-1035`,
  `src-tauri/src/trace/mod.rs:1125-1151`,
  `src-tauri/src/trace/mod.rs:1164-1186`, and
  `src-tauri/src/trace/mod.rs:1230-1252`.
- **Serde byte-shape check:** trace JSON uses `serde_json::to_string_pretty` in
  `run_trace_command` (`src-tauri/src/main.rs:470-473`). Moving the enum does
  not alter JSON if the moved enum keeps `#[derive(Serialize)]` and
  `#[serde(rename_all = "snake_case")]` exactly as now
  (`src-tauri/src/trace/mod.rs:73-80`). The `as_str()` helper can move with the
  enum and become `pub(crate)`, or trace can render with a local match. Either
  route preserves JSON output.
- **Stop-trigger status:** no evidence that the `TranscriptState` move would
  materially change trace behavior. Do not duplicate a parallel enum.
- **Public function signature:** `locate_session_metadata` should accept
  `state: &StateDb`, `models: &ModelStore`, `providers_cfg: &ProvidersConfig`,
  `sessions_cfg: &SessionsConfig`, and `input: &str`, matching existing types:
  `StateDb`/`ModelStore` are re-exported from `state` at
  `src-tauri/src/state/mod.rs:3-10`, `ModelStore` is defined at
  `src-tauri/src/state/db.rs:128`, `ProvidersConfig` is defined at
  `src-tauri/src/config/providers.rs:52-55`, and `SessionsConfig` is defined at
  `src-tauri/src/config/sessions.rs:27-30`.
- **Reuse:** the API should own proposal §4 steps 1 and 4-9. The CLI wrapper in
  `main.rs` should own clap parsing, `StateDb::open_default`, config loading,
  compact stdout/stderr JSON, and exit-code mapping (`proposals/06-locate.md:192-204`).

## C. Resolution flow hookpoints (proposal §4)

- **Step 1, UUID parse:** current resume checks `Uuid::parse_str(session_id)` in
  `run_resume` before opening state (`src-tauri/src/main.rs:1065-1068`).
  `StateDb::resolve_resume` itself uses `Uuid::try_parse(input)` and returns
  `ResumeError::InvalidUuid` (`src-tauri/src/state/db.rs:2577-2585`). Locate
  should parse up front in the reusable API so invalid UUID maps to exit `2`
  before DB/config work.
- **Step 2, state DB open:** use `StateDb::open_default()` at
  `src-tauri/src/state/db.rs:611-615`. Existing CLI callers include trace
  (`src-tauri/src/main.rs:447-448`), repl (`src-tauri/src/main.rs:809-817`),
  resume (`src-tauri/src/main.rs:1056-1072`), migrate-db
  (`src-tauri/src/main.rs:1450-1451`), and resume-list
  (`src-tauri/src/main.rs:1887-1889`).
- **Step 3, config load parity:** mirror resume's exact load pattern:
  models from `models_dir_override.unwrap_or_else(default_models_dir)` then
  `load_models(&models_dir)?`, config root from `dirs::config_dir`, then
  `ProvidersConfig::load(&providers_path).unwrap_or_default()` and
  `SessionsConfig::load(&sessions_path).unwrap_or_default()`
  (`src-tauri/src/main.rs:1071-1084`). Repl has the same unwrap-or-default
  shape at `src-tauri/src/main.rs:816-829`.
- **WS5 inherited limitation:** malformed `providers.toml` or `sessions.toml`
  in resume degrade to empty config because of `unwrap_or_default`
  (`src-tauri/src/main.rs:1081-1084`). Locate inherits that intentionally, so
  malformed config can become `unsupported-storage` instead of an operational
  config error. Trace differs: malformed `sessions.toml` is an error
  (`src-tauri/src/main.rs:447-458`).
- **Step 4, resolver reuse:** `StateDb::resolve_resume` signature is
  `(&self, models: &ModelStore, input: &str, model_override: Option<&str>) ->
  Result<ResolvedResume, ResumeError>` at `src-tauri/src/state/db.rs:2577-2582`.
  Current production callers are `run_repl` (`src-tauri/src/main.rs:830-846`)
  and `run_resume` (`src-tauri/src/main.rs:1087-1107`). Locate becomes the
  third production caller and should pass `None` for model override.
- **Resolver error mapping:** `ResolvedResume` fields are `chain_id`,
  `model_name`, optional `model`, `active_provider`, and `active_session_id`
  (`src-tauri/src/state/db.rs:131-138`). `ResumeError` variants live at
  `src-tauri/src/state/db.rs:140-170`; locate maps `InvalidUuid` to
  `InvalidSessionId`, `NoChainFound` to `SessionNotFound`, `Ambiguous` to
  `AmbiguousSession`, provider/config/model failures to operational or
  unsupported-storage per proposal §5 (`proposals/06-locate.md:140-151`).
- **Step 5, effective/runtime provider:** reuse `resume_execution_target`.
  It uses `ProvidersConfig::effective_provider(&model.providers[index])` when
  `resolved.model` exists (`src-tauri/src/main.rs:722-742`), otherwise
  `ProvidersConfig::runtime_provider(&resolved.active_provider)`
  (`src-tauri/src/main.rs:743-755`). Those APIs are defined at
  `src-tauri/src/config/providers.rs:116-134` and build final
  `ProviderConfig` at `src-tauri/src/config/providers.rs:157-191`.
- **Step 6, storage type:** internal storage is `ProviderConfig.session_storage`
  (`src-tauri/src/config/model.rs:6-25`), with enum variants `ClaudeCode` and
  `Codex` (`src-tauri/src/config/model.rs:195-229`). Existing match precedent:
  migration rejects `SessionStorage::Codex` at
  `src-tauri/src/migration/mod.rs:112-118` and requires target
  `SessionStorage::ClaudeCode` at `src-tauri/src/migration/mod.rs:142-154`.
- **Step 7, transcript location:** reuse `locate_transcript(sessions_cfg,
  provider_name, session_id)` at `src-tauri/src/sessions/mod.rs:171-199`.
  Current callers are trace (`src-tauri/src/trace/mod.rs:318-381`), migration
  (`src-tauri/src/migration/mod.rs:120-128`), and compaction backfill
  (`src-tauri/src/main.rs:1952`). The helper may create the adapter state dir
  at `src-tauri/src/sessions/mod.rs:183-185`; this is the allowed §8 mkdir.
- **Step 8, Claude workspace root:** migration takes the JSONL parent directory
  name as `cwd_hash` (`src-tauri/src/migration/mod.rs:155-161`) and writes
  target Claude transcripts under `projects_dir.join(cwd_hash)` at
  `src-tauri/src/migration/mod.rs:188-195`. Locate needs the inverse mapping:
  `<projects_dir>/<project-dir>/<session>.jsonl` to an existing absolute
  workspace root. No current code inverts this; `find_claude_source_from_storage`
  only scans project dirs by session filename (`src-tauri/src/migration/mod.rs:256-270`).
- **Step 8, Claude tiebreaker fixture:** the component test should build temp
  directories where a project dir like `-tmp-a-b` can decode as both `/tmp/a-b`
  and `/tmp/a/b`. The rule from §9.1 is implementable: zero existing decoded
  paths is exit `12`, exactly one succeeds, multiple existing decoded paths is
  exit `12` (`proposals/06-locate.md:252-253`). See WS4 below.
- **Step 8, Codex WS1 empirical sample:** real local rollout files exist under
  `/home/nes/.codex/sessions/2025/11/13/rollout-*.jsonl`; there are 5739 files
  under `/home/nes/.codex/sessions`. A 25-file sample showed
  `session_meta.payload.cwd` present in every sampled file and
  `session_meta.payload.workspace_root` absent; versions included `0.46.0` and
  `0.58.0`. Example sampled files:
  `/home/nes/.codex/sessions/2025/11/13/rollout-2025-11-13T14-54-48-019a7f6d-baa6-7212-8e61-6f500d9c742f.jsonl`
  and
  `/home/nes/.codex/sessions/2025/11/13/rollout-2025-11-13T23-49-44-019a8157-7678-7500-b817-5a486f11d413.jsonl`.
  The repo's bundled Codex locator only checks `payload.id`
  (`scripts/codex-locate-transcript`, cited by problem map at
  `research/06-locate-problem-map.md:44-45`), so product code has no current
  Codex workspace-root parser.
- **Step 9, mutable condition 1:** active segment existence comes from
  `resolve_resume` requiring `active_segment_for_chain` to return a segment
  (`src-tauri/src/state/db.rs:2609-2614`). The SQL chooses latest active segment
  at `src-tauri/src/state/db.rs:2751-2764`.
- **Step 9, mutable condition 2:** first-class storage comes from
  `ProviderConfig.session_storage` (`src-tauri/src/config/model.rs:21-24`) after
  runtime-provider expansion (`src-tauri/src/config/providers.rs:180-189`).
- **Step 9, mutable condition 3:** resume support is `provider.resume.is_some()`.
  Existing resume checks and refuses spawn when absent at
  `src-tauri/src/main.rs:1154-1162`; locate mirrors the predicate without
  invoking spawn.
- **Step 9, mutable condition 4:** `jsonl_path` is available only after
  `locate_transcript` returns `Some(path)`, the path exists, is absolute,
  canonicalizes, and is UTF-8. Trace only checks `path.exists()`
  (`src-tauri/src/trace/mod.rs:334-340`), so canonicality is locate-specific.
- **Step 9, mutable condition 5:** `workspace_root` is available only after
  Claude inversion succeeds in v1. Codex and `other` without explicit future
  provenance fail closed to `unsupported-storage`.
- **Step 10, JSON emission:** trace JSON is pretty multi-line via
  `serde_json::to_string_pretty` (`src-tauri/src/main.rs:470-473`). Locate
  should intentionally diverge and use compact single-line JSON on stdout and
  JSON errors on stderr. There is compact JSON precedent for embedded stderr
  env values via `serde_json::to_string` at `src-tauri/src/main.rs:979-980` and
  `src-tauri/src/main.rs:1183-1184`, but not for a stdout subcommand.

## D. Storage discrimination hookpoints (proposal §3, D2b)

- **Internal enum stays:** `SessionStorage` lives at
  `src-tauri/src/config/model.rs:195-229`, tagged with
  `#[serde(tag = "kind", rename_all = "snake_case")]`; its current serialized
  tags are `claude_code` and `codex`.
- **Config reader boundary:** `ProvidersConfig::load` reads
  `session_storage` into `ProviderEntry` and expands tildes at
  `src-tauri/src/config/providers.rs:81-105`. `ProviderEntry::effective_provider`
  copies it into runtime `ProviderConfig` at
  `src-tauri/src/config/providers.rs:157-191`.
- **Config writer boundary:** `migrate-config` treats `session_storage` as an
  internal provider block name and moves it from model TOML to providers TOML
  (`src-tauri/src/main.rs:1607`, `src-tauri/src/main.rs:1635`,
  `src-tauri/src/main.rs:1704`, `src-tauri/src/main.rs:1754`). No config reader
  or writer should learn `codex_session`.
- **One-way external translation:** `SessionStorage::Codex` maps to
  `SessionStorageType::CodexSession` only when `SessionMetadata` is serialized
  for locate JSON. This avoids a config migration and satisfies A5
  (`proposals/06-locate.md:50`, `proposals/06-locate.md:110`).
- **`other` is not internal config:** providers without
  `[providers.session_storage]` are valid today (`src-tauri/src/config/providers.rs:35-48`).
  `other` is a locate output enum value, not a new `SessionStorage` variant.

## E. Read-only behavior hookpoints (proposal §8)

- **State open side effects:** `StateDb::open` creates the DB parent directory
  (`src-tauri/src/state/db.rs:431-435`), opens SQLite read/write
  (`src-tauri/src/state/db.rs:437`), enables WAL
  (`src-tauri/src/state/db.rs:439-440`), ensures invocation/schema tables
  (`src-tauri/src/state/db.rs:441-600`), runs ensure helpers
  (`src-tauri/src/state/db.rs:601-603`), and runs chain backfill
  (`src-tauri/src/state/db.rs:604-606`). Locate v1 accepts these inherited
  open-path side effects.
- **Default DB location:** `open_default` uses `dirs::data_dir()` plus
  `oulipoly-agent-runner/state.db` (`src-tauri/src/state/db.rs:611-615`).
  No `--state-db` override should be introduced in 06-locate.
- **Locator mkdir side effect:** `locate_transcript` creates adapter
  `state_dir` before running the locator (`src-tauri/src/sessions/mod.rs:183-187`).
  This is allowed by proposal §8 and already part of trace/session behavior
  (`proposals/06-locate.md:221-234`).
- **Avoid write APIs:** the metadata path must not call invocation writers
  (`start_invocation`, `update_session_capture`, `finalize_invocation` as used
  in `src-tauri/src/main.rs:971-1013` and `src-tauri/src/main.rs:1173-1205`),
  migration (`migrate_chain_segment` at `src-tauri/src/migration/mod.rs:79-254`),
  scan ingestion (`scan_provider` at `src-tauri/src/sessions/mod.rs:60-141`), or
  config rewrite (`run_migrate_config`, `src-tauri/src/main.rs:1472-1488`).
- **Test hook:** row-count/mtime read-only assertions should snapshot after DB
  open to exclude allowed WAL/schema/backfill side effects, then call locate.

## F. Test-intent track hookpoints (proposal §9.1)

- **General test home:** API/component tests belong in the new
  `session_metadata` module under `src-tauri/src/session_metadata/mod.rs` or in
  a sibling `tests` module. CLI integration tests belong in a new
  `src-tauri/tests/initiative_06_locate.rs`, following `pr_b_trace_integration`
  and `pr_f_resume_integration` patterns that run
  `env!("CARGO_BIN_EXE_oulipoly-agent-runner")`
  (`src-tauri/tests/pr_b_trace_integration.rs:107-125`,
  `src-tauri/tests/pr_f_resume_integration.rs:360-384`).
- **Fixture infrastructure exists partially:** integration fixtures already
  create temp `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, model dirs, scripts, and
  default state DB paths (`src-tauri/tests/pr_f_resume_integration.rs:11-88`,
  `src-tauri/tests/initiative_05_migration.rs:23-70`). There is no single
  reusable temp state/config builder module; proposal §9.1's new fixture claim
  is accurate (`proposals/06-locate.md:258`).
- **Resolver pass-through:** new particular-integration test in
  `initiative_06_locate.rs`; seed `session_chains` / `session_chain_segments`
  using existing `StateDb` helpers or direct SQL. Existing fixture style can
  write temp `providers.toml`, `sessions.toml`, locator scripts, and JSONL
  (`src-tauri/tests/initiative_05_migration.rs:88-145`).
- **D1 ambiguity mirrors resolver:** component test can reuse existing resolver
  fixture patterns in `state/db.rs` tests around
  `resolve_resume_returns_active_segment_for_single_chain`,
  `resolve_resume_filters_by_24h_when_two_chains_share_session_id`, and
  `resolve_resume_errors_ambiguous_when_both_recent`
  (`src-tauri/src/state/db.rs:5348-5405`). Prefer metadata API tests that call
  `locate_session_metadata` so the mapping is verified without a process spawn.
- **D2 storage mapping:** unit test for `SessionStorage` to
  `SessionStorageType` mapping in `session_metadata`; component fixtures with
  provider config entries. Runtime config parser already has storage tests at
  `src-tauri/src/config/providers.rs:257-309`.
- **D2 unsupported no-storage case:** CLI integration test with active segment
  and providers config missing storage plus missing/malformed locator. Existing
  `locate_transcript` unit tests cover no locator and locator errors
  (`src-tauri/src/sessions/mod.rs:457-513`) but not locate's exit-code mapping.
- **D3 mutable truth conditions:** component matrix in `session_metadata` that
  varies storage, resume block, JSONL, workspace root, and quota rows. Existing
  resume block check is at `src-tauri/src/main.rs:1154-1162`; quota rows must
  not affect metadata.
- **D4 partial DB invisible:** component test can open temp DB, insert
  `session_turns` after open, insert an unrelated chain to represent the
  backfill skip condition (`src-tauri/src/state/db.rs:2256-2271`), then assert
  `SessionNotFound`.
- **D5 default DB only:** unit clap test in `src-tauri/src/main.rs` near
  existing parser tests (`src-tauri/src/main.rs:2157-2490`) should reject
  unknown `--state-db`. CLI integration can verify `XDG_DATA_HOME` default DB
  use, but no alternate DB flag should exist.
- **Missing UUID:** particular-integration test in `initiative_06_locate.rs`
  with empty/chainless default DB and well-formed unknown UUID; expect exit 10
  and stderr JSON.
- **Invalid UUID:** end-to-end CLI test in `initiative_06_locate.rs` with no
  DB fixture required; because UUID parse is before DB open, this can run with
  impossible or temp XDG locations and expect exit 2 / stderr JSON.
- **D6 transcript state reconciliation:** component tests in
  `session_metadata`; integration variants can reuse tiny locator scripts.
  Existing `sessions::locate_transcript` tests at `src-tauri/src/sessions/mod.rs:457-513`
  cover no locator, path stdout, nonzero exit, and empty stdout, but not
  relative path, canonicality, UTF-8, or locate's no-partial-success rule.
- **D7 workspace root derivation:** component tests in `session_metadata` with
  temp Claude `projects_dir`, JSONL parent project dirs, and Codex provider
  fixtures. Existing migration tests stage Claude JSONL under a fake cwd hash
  (`src-tauri/tests/pr_f_resume_integration.rs:222-225`) but do not invert it.
- **D7 Claude path-hash ambiguity:** component test in `session_metadata` with
  zero/one/multiple decoded existing workspaces, including path components with
  `-`. This is new fixture logic.
- **Read-only behavior after open:** particular-integration test in
  `initiative_06_locate.rs`; snapshot row counts for the proposal's five table
  groups and transcript mtime after `StateDb::open`, call CLI locate, assert no
  row-count or transcript mtime changes.
- **JSON shape stability:** component serialization test in
  `session_metadata`, plus a CLI integration assertion that stdout is one
  compact object. Use `serde_json::from_slice` as existing integration tests do
  (`src-tauri/tests/pr_b_trace_integration.rs:159-165`).
- **README examples remain truthful:** either a unit/documentation grep test or
  a Phase 6b residual/manual doc review. There is no existing README snapshot
  test; current parser tests live in `main.rs`, not docs.
- **Collision risk:** adding a new CLI surface may require updating parser tests
  in `src-tauri/src/main.rs:2157-2490`. There is no insta/snapshot help-output
  test; `Cargo.toml` has no `insta` or `assert_cmd` dependency
  (`src-tauri/Cargo.toml:10-24`).

## G. Deletion candidates

- **Keep hidden `resume-list`:** `ResumeList` is hidden at
  `src-tauri/src/main.rs:155-157`, normalized from `resume --list` at
  `src-tauri/src/main.rs:2018-2035`, and dispatched at
  `src-tauri/src/main.rs:335`. Its output is chain preview text from
  `run_resume_list` (`src-tauri/src/main.rs:1887-1900`) and
  `resume_previews` / `chain_previews` (`src-tauri/src/state/db.rs:2672-2675`,
  `src-tauri/src/state/db.rs:2794-2856`). It is not superseded by
  single-session locate JSON.
- **No TODO closure found:** `rg` over the touched source found no TODO or
  placeholder for "expose session metadata" to delete. Locate adds a missing
  public surface rather than closing an existing stub.
- **Keep `compose_resume_args` `target_jsonl_path`:** `ResumePayload` carries
  `target_jsonl_path` (`src-tauri/src/executor/cli.rs:276-280`) and
  `compose_resume_args` currently ignores it (`src-tauri/src/executor/cli.rs:282-290`).
  Initiative 05 added tests pinning that ignored behavior
  (`src-tauri/src/executor/cli.rs:1786-1810`). Locate does not use it and should
  not delete it in this PR; it belongs to the migration/resume surface, not
  metadata lookup.
- **No duplicate `TranscriptState`:** move the existing enum; do not create a
  parallel type in `session_metadata`.

## H. Conflict and collision check

- **Legacy branch conflict:** local branch `init-06/pr-a-resume-with-answer`
  has only `Trace`, `Repl`, and `Resume` in `Subcommands` and no `session`
  group (`git show init-06/pr-a-resume-with-answer:src-tauri/src/main.rs`,
  lines 72-141 in that branch). It predates current hidden `ResumeList`,
  `MigrateDb`, and `MigrateConfig`; no competing `session` subcommand exists.
- **Struct name collision:** no existing `SessionMetadata` struct or
  `session_metadata` module exists. `rg` only found the future references in
  this research/proposal.
- **Error name collision:** no existing `MetadataError` enum exists in
  `src-tauri/src` or tests. `ResumeError` remains in state
  (`src-tauri/src/state/db.rs:140-170`) and should not be renamed.
- **README anchor collision:** there is no current "Locating a Session" section
  or `session locate` mention in README. The subcommand synopsis is at
  `README.md:127-140`; trace/resume sections are at `README.md:405-419` and
  `README.md:458-496`; SQL debugging starts at `README.md:500-512`.
- **Internal module exports:** adding `pub mod session_metadata` in
  `src-tauri/src/lib.rs:1-11` is additive. Do not re-export moved
  `TranscriptState` from `trace`; no-backwards-compatibility doctrine forbids
  compatibility aliases.

## I. Watch signal closures

- **WS1 (Codex rollout sample): empirical evidence found.** Local Codex rollout
  JSONL exists under `/home/nes/.codex/sessions`; sampled `session_meta.payload`
  objects include `cwd` and do not include `workspace_root`. `turn_context`
  payloads in sampled files also include `cwd`. This suggests Codex workspace
  root may be derivable from `payload.cwd`, but product code has no parser and
  the proposal deliberately does not commit to the field.
- **WS1 disposition:** acceptable Phase 5 hand-off if the human gate keeps Rev 2
  v1 scope: Codex locate continues fail-closed with exit 12, and a follow-up
  folds in `payload.cwd` after contract/risk review. If the human gate wants
  Codex support in v1, return to Phase 3 to revise A4/§4/§9.1 before Phase 6.
- **WS4 (path-hash prose ambiguity): implementable.** Although §4 prose says
  "pick the first interpretation" (`proposals/06-locate.md:131-133`), the test
  row is explicit that multiple existing decompositions return
  `UnsupportedStorage` / exit 12 (`proposals/06-locate.md:252-253`). Phase 6
  can implement the correct rule from §9.1 alone: enumerate candidates in
  longest-prefix-existing order, succeed only if exactly one decoded existing
  path remains.
- **WS5 (resume parity malformed config): mapped.** Resume loads provider and
  session config with `unwrap_or_default` (`src-tauri/src/main.rs:1081-1084`);
  repl does the same (`src-tauri/src/main.rs:826-829`). Locate mirrors that path
  and therefore inherits indistinguishability between absent config and
  malformed config. This is not a locate-specific code change.

## J. Implementation surface summary

| Proposal action | Hookpoint | Reuse / extend / new |
| --- | --- | --- |
| `session` subcommand parent | `src-tauri/src/main.rs:77-166` (`Subcommands`) | extend |
| `locate` child command | new `SessionSubcommands` near `Subcommands` | new |
| Bare `agents session` help/usage behavior | nested non-optional `#[command(subcommand)]` child | extend clap shape |
| Top-level dispatch | `src-tauri/src/main.rs:287-338` | extend |
| Preserve top-level conflict behavior | `src-tauri/src/main.rs:18-23` | reuse |
| `SessionMetadata` module | `src-tauri/src/session_metadata/mod.rs`; export from `src-tauri/src/lib.rs:1-11` | new |
| `SessionMetadata` / `SessionStorageType` / `MetadataError` | `src-tauri/src/session_metadata/mod.rs` | new |
| Move `TranscriptState` | `src-tauri/src/trace/mod.rs:73-80` -> `src-tauri/src/session_metadata/mod.rs` | move |
| Trace import of shared state enum | `src-tauri/src/trace/mod.rs:59-70`, `:223-381`, `:425-438` | extend |
| UUID parse | `src-tauri/src/main.rs:1065-1068`; `src-tauri/src/state/db.rs:2583-2585` | reuse |
| CLI state DB open | `src-tauri/src/state/db.rs:611-615` | reuse |
| Resume-parity config load | `src-tauri/src/main.rs:1071-1084` | reuse |
| Owner resolution | `src-tauri/src/state/db.rs:2577-2670` | reuse |
| Provider runtime mapping | `src-tauri/src/main.rs:718-755`; `src-tauri/src/config/providers.rs:116-134`, `:157-191` | reuse |
| Storage mapping | `src-tauri/src/config/model.rs:195-229` | reuse + boundary translation |
| Transcript location | `src-tauri/src/sessions/mod.rs:171-199` | reuse |
| Locator state dir mkdir classification | `src-tauri/src/sessions/mod.rs:183-185` | reuse / allowed side effect |
| Claude workspace-root derivation | inverse of `src-tauri/src/migration/mod.rs:155-195` | new helper |
| Codex workspace-root derivation | sampled `/home/nes/.codex/sessions/.../rollout-*.jsonl` has `payload.cwd` | follow-up or Phase 3 revision |
| Mutable active segment | `src-tauri/src/state/db.rs:2609-2614`, `:2751-2764` | reuse |
| Mutable storage/resume checks | `src-tauri/src/config/model.rs:21-24`; `src-tauri/src/main.rs:1154-1162` | reuse predicate |
| Compact stdout JSON | new locate CLI wrapper; contrast `src-tauri/src/main.rs:470-473` | new |
| Stderr JSON errors / exit mapping | new locate CLI wrapper; proposal §5 | new |
| README synopsis | `README.md:127-140` | extend |
| README trace/resume/SQL sections | `README.md:374-386`, `:405-419`, `:458-512` | extend |
| CLI integration tests | new `src-tauri/tests/initiative_06_locate.rs`; patterns in `src-tauri/tests/pr_b_trace_integration.rs:107-125` | new + reuse fixture style |
| API component/unit tests | new tests in `src-tauri/src/session_metadata/mod.rs`; resolver precedents in `src-tauri/src/state/db.rs:5348-5405` | new + reuse fixtures |
| Keep `resume-list` | `src-tauri/src/main.rs:155-157`, `:1887-1900` | retain |
| Keep `compose_resume_args target_jsonl_path` | `src-tauri/src/executor/cli.rs:276-290`, tests at `:1786-1810` | retain |

## What this hookpoint research deliberately does NOT cover

1. `06-schema-probe` / `agents session schema-probe`, except where read-only
   DB-open sequencing explains why 06-locate uses current `StateDb::open`.
2. `06-export` / `agents session export <session-id>`, except that it should
   consume the new `SessionMetadata` API later.
3. `06-pause-handshake` / session lease locking, except as a future additional
   `mutable` condition.
4. `06-import-replace` / atomic transcript replacement, except that it should
   consume the new `SessionMetadata` API later.
5. Frontend, Tauri UI commands, HomeView/StatusView/PoolsView, and Ollie/design
   work.
6. Cross-CLI migration policy, Claude-to-Codex copy semantics, and Codex
   migration implementation.
7. Transcript export formats, canonical JSONL normalization, and raw provider
   transcript schemas beyond the single Codex `session_meta.payload.cwd` sample
   obligation.
8. Provider quota correctness, quota-window math, and provider selection policy,
   except that quota state must not affect `mutable`.
9. General setup, discovery, account management, diagnostics quality, and
   design-system work.
