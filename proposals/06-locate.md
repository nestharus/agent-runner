# 1. Scope statement (Rev 1)

06-locate adds one read-only CLI surface:

```bash
agents session locate <session-id> [--json]
```

It returns stable JSON metadata for a session/chain that already exists in the Initiative 05 chain ledger. This is the first of five Initiative 06 PRs and ships independently. The initiative sequence is technical order, not harness-number order: `locate` first establishes the reusable `SessionMetadata` API consumed by 06-export and 06-import-replace; 06-schema-probe follows so the harness can pin feature flags; export, pause-handshake, and import-replace build on those surfaces (`initiatives/06-session-override-contract.md:38-56`, `initiatives/06-session-override-contract.md:75-89`).

This proposal does not implement code. It defines the command shape, JSON schema, resolver policy, storage vocabulary, mutability semantics, test-intent track, supported-surface track, and anti-scope for the later Phase 6 implementation. It consumes the approved current-state map at `research/06-locate-problem-map.md`; this proposal's §1.1 register replaces the draft register in that map.

What changes:

- Add the `session` subcommand group in `src-tauri/src/main.rs` and one child command, `locate`.
- Add reusable Rust metadata code under a new library module, proposed as `src-tauri/src/session_metadata/`.
- Factor transcript-state/location logic that currently exists around trace and `locate_transcript`.
- Document the public CLI surface and JSON shape in `README.md`.

What does not change:

- `agents resume`, `agents repl --resume`, top-level `--resume`, `trace --json`, `migrate-db`, and `migrate-config` behavior remain unchanged.
- Existing resolver semantics remain authoritative. `locate` does not invent a second ownership path.
- No provider is spawned, no resume is attempted, no quota is refreshed, no config is edited, no transcript is copied or rewritten.
- No GUI/Tauri command surface is added in v1.
- No sibling Initiative 06 subcommands (`export`, `import-replace`, `pause-handshake`, `schema-probe`) are added in this PR.

## 1.1 Assumption register

This is the approved register validated and narrowed from `research/06-locate-problem-map.md` §7. It replaces the draft register there; do not maintain a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | `StateDb::resolve_resume` returns exactly one owner for the common single-chain, single-active-segment case. | One candidate returns immediately; active segment reads `ended_at IS NULL` ordered by latest start/id (`src-tauri/src/state/db.rs:2713-2719`, `src-tauri/src/state/db.rs:2751-2764`). | Chain ledger state has no active segment, multiple recent candidate chains, or resolver behavior changes. | §4 resolution flow; §5 exit `0`, `10`, `11`; §9.1 resolver pass-through tests. |
| A2 | Ambiguity means "the resolver returned `ResumeError::Ambiguous`", not "more than one chain row exists". | `choose_resume_chain` collapses multiple chains when exactly one is recent or when none are recent (`src-tauri/src/state/db.rs:2721-2748`); Initiative 06 requires reuse of `StateDb::resolve_resume` with no second ownership path (`initiatives/06-session-override-contract.md:112-113`). | Harness refuses resolver-collapse cases and requires strict multi-row ambiguity despite the shared-ownership constraint. | D1 / §4 step 3; §5 exit `11`; §7 anti-scope; §9.1 ambiguity tests. |
| A3 | A configured `transcript_locator` is sufficient to resolve a canonical local JSONL path without provider spawn. | Trace calls `locate_transcript` lazily; reference locators print resolved paths and inspect local JSONL files (`src-tauri/src/sessions/mod.rs:171-199`, `scripts/claude-code-locate-transcript:33-45`, `scripts/codex-locate-transcript:27-39`). | A provider's canonical transcript requires network, provider execution, or DB mutation. | D6 / §3 `jsonl_path`; §4 step 5; §9.1 transcript-state tests. |
| A4 | `workspace_root` can be derived for supported file-backed transcripts without provider spawn, but not from the current DB invocation schema. | CLI accepts `-p/--project` only as runtime input (`src-tauri/src/main.rs:60-62`); `InvocationStart`/`InvocationRecord` do not store project/working directory (`src-tauri/src/state/db.rs:205-233`). Migration already treats Claude transcript parent as the workspace hash segment (`src-tauri/src/migration/mod.rs:155-188`). | Representative Claude/Codex transcripts cannot yield a stable local workspace root from path/metadata, or the harness requires roots for providers without such provenance. | D7 / §3 `workspace_root`; §4 step 6; §12 residuals. |
| A5 | `[providers.session_storage]` remains the source of first-class storage-type discrimination; `codex_session` is an output vocabulary choice, not an internal enum rename. | Current enum variants are `ClaudeCode` and `Codex` with serde tags `claude_code` and `codex` (`src-tauri/src/config/model.rs:195-229`); provider runtime config propagates storage into effective providers (`src-tauri/src/config/providers.rs:157-190`). | Internal config vocabulary is deliberately renamed as part of this PR, or a locator/script format conflicts with declared provider storage. | D2 / §3 `storage_type`; §6 `SessionMetadata`; §9.1 storage mapping tests. |
| A6 | v1 `locate` can be logically read-only even though the current open path is not physically read-only. | `StateDb::open` creates directories, sets WAL, ensures schema, and backfills chains (`src-tauri/src/state/db.rs:431-608`); Initiative 06 assigns the read-only open variant to 06-schema-probe (`initiatives/06-session-override-contract.md:118-120`). | Phase 4 decides physical read-only is mandatory for 06-locate rather than a 06-schema-probe dependency. | §4 step 2; §8 side-effect contract; §9.1 read-only intent tests; §12 residuals. |
| A7 | Direct CLI sessions ingested after Initiative 05 have chain membership and are resolver-visible. | `scan_provider` calls `mint_imported_chain_if_absent` after ingesting turns (`src-tauri/src/sessions/mod.rs:125-141`). | Scan errors, partial DBs, or pre-existing chain rows leave `session_turns` without segment membership (`src-tauri/src/state/db.rs:2256-2271`). | D4 / §4 step 3; §5 exit `10`; §9.1 partial-DB tests. |
| A8 | `mutable` can be conservatively approximated from active chain state, storage declaration, transcript availability, and provider resume config; it is not a stored session field. | Resolver requires an active segment (`src-tauri/src/state/db.rs:2609-2614`); providers may or may not declare `resume` and `session_storage` (`src-tauri/src/config/providers.rs:29-32`); no mutable column exists in session tables (`src-tauri/src/state/db.rs:559-592`). | Provider-specific locks or writable/read-only states are needed before a safe write-back decision can be made. | D3 / §3 `mutable`; §4 step 7; §9.1 mutable tests. |
| A9 | Provider quota exhaustion must not participate in `mutable`. | `exhausted_at` is provider-account global, not session-scoped, and current resume can still reach provider spawn until migration/selection logic decides otherwise (`research/06-locate-problem-map.md` §2 #17; `src-tauri/src/state/db.rs:455-463`). | A later pause/import contract explicitly defines quota exhaustion as a session write lock. | D3 / §8 side-effect contract; §13 checklist. |

## 1.2 Net-value statement

Yes: this reduces a concrete current-state risk on the supported CLI surface. Today, "where is this session and which provider owns it" requires ad-hoc SQL, `trace --json` with an invocation UUID, hidden text output from `resume-list`, or attempting `resume` and risking provider spawn plus invocation writes (`research/06-locate-problem-map.md` §6 #1-5). There is no stable CLI JSON object that combines chain id, provider, storage kind, canonical transcript path, workspace root, transcript state, and mutability (`research/06-locate-problem-map.md` §6 #6-11).

The blast radius is small because the command is additive and read-only in behavior. The main implementation risk is composing existing pieces without changing resolver semantics, and the main migration cost is none: existing chain/segment state remains the source of truth. Rollback is also low cost: remove the new binary/subcommand or avoid invoking it. The value is positive for the current supported surface because the harness can stop reading private SQLite/JSONL layouts directly while agent-runner keeps one owner-resolution implementation.

# 2. Subcommand surface

Add a new `Session` variant to `Subcommands` in the enum currently spanning `Trace`, `Repl`, `Resume`, hidden `ResumeList`, `MigrateDb`, and `MigrateConfig` (`src-tauri/src/main.rs:77-166`):

```text
session locate <session-id> [--json]
```

Clap shape:

```rust
Session {
    #[command(subcommand)]
    command: SessionSubcommands,
}

enum SessionSubcommands {
    Locate {
        session_id: String,
        #[arg(long)]
        json: bool,
    },
}
```

The parent `session` command is help-only when no child is supplied. There is no default action for bare `agents session`; clap usage failure exits with code `2`.

Wire the new match arm in `run(cli)` alongside existing subcommand dispatch before top-level `--resume` routing (`src-tauri/src/main.rs:287-338`). This keeps `args_conflicts_with_subcommands = true` behavior unchanged at the top level (`src-tauri/src/main.rs:20-23`).

`--json` is accepted for symmetry with `trace --json`; output is always one JSON object on stdout for success. The flag does not change formatting in v1. Error output is JSON on stderr for all `locate` failures, including usage-normalized `invalid-session-id`.

# 3. JSON output schema

Success stdout is a single compact one-line JSON object for script consumption. The schema is stable. Future additions must be backward-compatible optional fields; existing fields must not change meaning.

Required success fields:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `session_id` | string UUID | yes | Active provider session id returned by `ResolvedResume.active_session_id`, not necessarily the user's input if the input was a `chain_id`. |
| `chain_id` | string UUID | yes | Logical chain id returned by `ResolvedResume.chain_id`. |
| `provider_name` | string | yes | Active provider/account name from `ResolvedResume.active_provider`. |
| `storage_type` | string enum | yes | One of `claude_code`, `codex_session`, `other`. Internal `SessionStorage::Codex` is translated to `codex_session` at this boundary. |
| `jsonl_path` | string path | yes | Canonical absolute UTF-8 path to an existing local JSONL transcript. Non-UTF-8 paths are `unsupported-storage` rather than lossy output. |
| `workspace_root` | string path | yes | Canonical absolute UTF-8 path to the best-known local workspace root. Failure to derive this is `unsupported-storage`. |
| `transcript_state` | string enum | yes | `available` on success. Other internal states (`unresolved`, `no_locator`, `missing`) are failure states for this command. |
| `mutable` | boolean | yes | Conservative write-back eligibility signal, defined below. |

D2 decision: choose D2b. Keep internal `SessionStorage::Codex` and serde tag `codex` (`src-tauri/src/config/model.rs:195-229`), emit `codex_session` only in `locate` JSON. This avoids mixing a public harness vocabulary change with config-file migration. `other` is emitted only when a provider lacks `[providers.session_storage]` but has a configured locator that returns an absolute existing JSONL path and `workspace_root` can be derived. If no storage block and no canonical file-backed path exists, return exit `12 unsupported-storage`.

D3 decision: `mutable: true` means all of the following are true:

1. `resolve_resume` found an active segment, so `ended_at IS NULL` exists for the selected chain (`src-tauri/src/state/db.rs:2609-2614`, `src-tauri/src/state/db.rs:2751-2764`).
2. The provider declares `[providers.session_storage]`, so storage type is first-class and not `other`.
3. The provider declares `[providers.resume]`, so there is an existing resume strategy for the active provider (`src-tauri/src/config/providers.rs:29-32`, `src-tauri/src/main.rs:1154-1162`).
4. `jsonl_path` is available, absolute, canonical, exists, and is UTF-8.
5. `workspace_root` is available, absolute, canonical, exists, and is UTF-8.

`mutable` does not consult `provider_quotas.exhausted_at`. Quota exhaustion is account-global, not session-scoped, and reading it would blur locate metadata with routing policy. If the metadata can be located but one mutability condition fails because provider resume is missing or storage is `other`, exit `0` with `mutable: false`. If transcript or workspace location cannot be made canonical, exit `12` and do not emit partial success JSON.

# 4. Resolution flow

1. Parse `<session-id>` as a full UUID before opening state. Invalid parse returns exit `2` with stderr JSON error code `invalid-session-id`. This mirrors the full-UUID resolver precondition (`src-tauri/src/state/db.rs:2583-2585`) while giving the harness its requested code.
2. Open the CLI default state DB with `StateDb::open_default()` in v1. This uses `dirs::data_dir()/oulipoly-agent-runner/state.db` (`src-tauri/src/state/db.rs:611-615`). D5 decision: no `--state-db <path>` override in 06-locate; GUI-created sessions in the Tauri `models_dir.parent()/state.db` location are out of scope because the harness invokes the CLI, and GUI state divergence is already a known adjacent path (`src-tauri/src/lib.rs:525-533`).
3. Load models and providers the same way resume-adjacent code can resolve provider runtime shape: `load_models(default_models_dir())`, `ProvidersConfig::load`, and `SessionsConfig::load` from the CLI config root. Malformed config is an operational error unless the existing loader explicitly treats absence as empty (`src-tauri/src/main.rs:1072-1084`, `src-tauri/src/config/providers.rs:81-109`).
4. Call `StateDb::resolve_resume(&models, input, None)`. D1 decision: choose D1a, mirror resolver. `ambiguous-session` fires only when the resolver returns `ResumeError::Ambiguous`; recency collapse remains exactly as resume uses it (`src-tauri/src/state/db.rs:2577-2670`, `src-tauri/src/state/db.rs:2713-2749`). Strict multi-row ambiguity is anti-scope because Initiative 06 forbids a second ownership path (`initiatives/06-session-override-contract.md:112-113`).
5. Map the active provider to an effective/runtime provider. If `ResolvedResume.model` exists, use `ProvidersConfig::effective_provider`; otherwise use `runtime_provider`, matching the provider-only fallback shape exposed by resume (`src-tauri/src/config/providers.rs:116-134`, `src-tauri/src/config/providers.rs:157-190`, `README.md:471-485`).
6. Resolve `storage_type` from `ProviderConfig.session_storage`: `ClaudeCode` => `claude_code`, `Codex` => `codex_session`, `None` => `other` only if later file-backed transcript/workspace checks succeed (`src-tauri/src/config/model.rs:195-229`).
7. Resolve `jsonl_path` by calling existing `locate_transcript(sessions_cfg, provider_name, active_session_id)`. D6 decision: the reusable API preserves the trace four-state enum (`unresolved`, `no_locator`, `missing`, `available`) for later consumers, but `locate` success requires `available`. `Ok(None)` becomes `no_locator` then exit `12`; a returned non-existing path becomes `missing` then exit `12`; locator errors become `missing`/`unsupported-storage` with an error message (`src-tauri/src/sessions/mod.rs:171-199`, `src-tauri/src/trace/mod.rs:73-80`, `src-tauri/src/trace/mod.rs:318-380`). Paths must be absolute, canonicalizable, existing, and UTF-8.
8. Resolve `workspace_root`. D7 decision: use JSONL/path provenance, not invocation provenance. Current invocation rows do not store `-p/--project` (`src-tauri/src/state/db.rs:205-233`), so D7b and D7c's invocation-preferred branch are rejected for v1. Supported derivations:
   - Claude Code: when `jsonl_path` is under `projects_dir/<project-dir>/<session>.jsonl`, decode the `<project-dir>` convention to an absolute path and require that path to exist. Migration already treats this parent directory as `cwd_hash` (`src-tauri/src/migration/mod.rs:155-188`).
   - Codex: scan the located JSONL for a `session_meta` object and use an absolute `payload.cwd`/`payload.workspace_root` if present; this extends the same local JSON inspection precedent used by the Codex locator for `payload.id` (`scripts/codex-locate-transcript:45-60`).
   - Other: use a future explicit locator-provided workspace only if the API is extended later; in v1, failure to derive is exit `12`.
9. Compute `mutable` using the §3 D3 conditions.
10. Emit JSON to stdout and exit `0`.

D4 decision: choose D4a. If a pre-Initiative-05 or partial-DB session exists in `session_turns` but has no chain/segment membership, `resolve_resume` returns `NoChainFound` because candidate selection reads `session_chain_segments` only (`src-tauri/src/state/db.rs:2696-2711`). `locate` maps that to exit `10 session-not-found`. Falling back to `session_turns` directly is rejected because it creates a second ownership path.

# 5. Exit codes

| Exit | Error code | Producing condition | Notes |
| --- | --- | --- | --- |
| `0` | none | Exactly one resolver-selected session produced complete metadata. | Success JSON on stdout. `mutable` may be `false`. |
| `1` | `operational-error` or specific internal code | DB open/read failure, malformed config, JSON serialization failure, or unexpected I/O outside the transcript/storage classification path. | JSON error object on stderr; no success JSON. |
| `2` | `invalid-session-id` or clap usage | `<session-id>` is not a full UUID, or command usage is invalid. | Parse failure is normalized to stderr JSON with `invalid-session-id`; clap may print usage for structural errors. |
| `10` | `session-not-found` | `StateDb::resolve_resume` returns `NoChainFound`, including D4 partial-DB segmentless sessions. | No fallback to `session_turns`. |
| `11` | `ambiguous-session` | `StateDb::resolve_resume` returns `Ambiguous`. | D1a: strict multi-chain-but-resolver-collapsed cases are not exit `11`. |
| `12` | `unsupported-storage` | Provider/session exists but `locate` cannot emit required canonical file-backed metadata: no locator, non-existing path, relative/non-UTF-8 path, unsupported workspace-root derivation, no storage block plus no canonical transcript, or no supported workspace provenance. | D2/D6/D7 failure bucket; no partial success JSON. |

Reserved Initiative 06 error codes `13`-`17` are not used by 06-locate (`initiatives/06-session-override-contract.md:108-111`).

# 6. Reusable `SessionMetadata` API

Create a new library module, proposed path:

```text
src-tauri/src/session_metadata/
```

Public types:

```rust
pub struct SessionMetadata {
    pub session_id: String,
    pub chain_id: String,
    pub provider_name: String,
    pub storage_type: SessionStorageType,
    pub jsonl_path: PathBuf,
    pub workspace_root: PathBuf,
    pub transcript_state: TranscriptState,
    pub mutable: bool,
}

pub enum SessionStorageType {
    ClaudeCode,
    CodexSession,
    Other,
}

pub enum MetadataError {
    InvalidSessionId { input: String },
    SessionNotFound { input: String },
    AmbiguousSession { input: String },
    UnsupportedStorage { provider_name: String, reason: String },
    Operational { message: String },
}
```

Move `TranscriptState` out of `trace` into the reusable module and have trace import the shared enum. The shared enum keeps the existing serde snake-case values (`src-tauri/src/trace/mod.rs:73-80`). If Phase 5 hookpoint research proves that move would materially change trace behavior, stop and revise this proposal rather than duplicating a second transcript-state type silently.

Public function shape:

```rust
pub fn locate_session_metadata(
    state: &StateDb,
    models: &ModelStore,
    providers_cfg: &ProvidersConfig,
    sessions_cfg: &SessionsConfig,
    input: &str,
) -> Result<SessionMetadata, MetadataError>
```

The reusable API owns steps 1, 4-9 from §4 except state/config loading. The CLI wrapper owns clap parsing, state/config load, stdout/stderr formatting, and exit-code mapping. 06-export and 06-import-replace should consume this API so they inherit the same owner resolution, storage vocabulary, transcript state, and workspace-root checks.

The API must not call `migrate_chain_segment`, `scan_provider`, quota refresh, provider spawn, or config writers. It may call `locate_transcript`, because that is already part of the trace/session contract (`src-tauri/src/sessions/mod.rs:171-199`, `README.md:374-386`).

# 7. Anti-scope

- No transcript export, import, replace, append, truncate, normalization, or canonical transcript transformation.
- No `agents session export`, `agents session import-replace`, `agents session pause-handshake`, `agents session resume-handshake`, or `agents session schema-probe` in this PR.
- No auto-resume, provider spawn, provider login/auth refresh, quota refresh, provider selection, or migration.
- No config edits and no coupling to `agents migrate-config`.
- No DB schema migration beyond whatever current `StateDb::open` already performs; 06-locate does not add a read-only open variant because that is assigned to 06-schema-probe (`initiatives/06-session-override-contract.md:118-120`).
- No strict multi-row ambiguity query outside `StateDb::resolve_resume` (rejected D1b).
- No fallback to `session_turns` outside the resolver (rejected D4b).
- No `--state-db <path>` override and no GUI state DB support in v1 (D5).
- No public exposure of credentials, quota windows, auth state, raw provider config, or raw transcript contents.
- No attempt to make `mutable` a hard import/replace safety lock; 06-pause-handshake owns locks later.

# 8. Side-effect contract

`agents session locate` does not:

- Insert, update, or delete `invocations`, `session_turns`, `session_chains`, `session_chain_segments`, `provider_quotas`, adapter cursor files, transcript files, provider config, model config, or `sessions.toml`.
- Start an invocation row, update `session_capture_method`, update `resume_acceptance`, or touch chain `last_used_at`.
- Run provider commands, `auth_refresh_command`, `quota_script`, `turn_script`, `scan_provider`, `migrate_chain_segment`, or diagnostics.
- Copy, create, rename, truncate, or rewrite JSONL files.
- Resolve or observe future pause-handshake locks in v1.
- Emit telemetry or durable trace/cache state.

Known caveat from A6: current `StateDb::open_default()` is physically side-effecting because it ensures schema, enables WAL, and runs backfill (`src-tauri/src/state/db.rs:431-608`). This PR's contract is behaviorally read-only after open. Physical read-only open is deliberately left to 06-schema-probe per initiative sequencing (`initiatives/06-session-override-contract.md:118-120`).

# 9. Test-intent track

## 9.1 Test-intent track

| Change risk or verification risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| Resolver pass-through: one chain / one segment | Known active segment returns one JSON object with `session_id`, `chain_id`, `provider_name`, `transcript_state: "available"`. | particular-integration | New Rust CLI integration fixture seeded with `session_chains`, `session_chain_segments`, provider config, sessions locator script, and temp JSONL. | A1, A3 | Command exits `0`; stdout parses; no stderr error. | Does not prove provider-native transcript content validity. |
| D1 ambiguity mirrors resolver | Multi-chain input with multiple recent chains exits `11`; multi-chain input collapsed by resolver exits `0`. | component | `StateDb` temp DB fixture with controlled `last_used_at` rows; call metadata API directly. | A2 | Error enum `AmbiguousSession` only when resolver returns ambiguous. | Time-window edge around exact 24h cutoff remains bounded by deterministic timestamps. |
| D2 storage mapping | `ClaudeCode` emits `claude_code`; `Codex` emits `codex_session`; no storage with valid locator emits `other` and `mutable: false`. | unit + component | Unit mapping test plus metadata fixture using providers config entries. | A5 | JSON/storage enum values match harness vocabulary. | Does not validate future storage variants. |
| D2 unsupported no-storage case | Provider with no `[providers.session_storage]` and no usable canonical locator exits `12 unsupported-storage`. | particular-integration | Temp DB active segment; providers config without storage; sessions config missing locator or locator returning missing path. | A3, A5 | Exit `12`; stderr JSON error code `unsupported-storage`; no stdout success. | Does not prove every third-party locator failure is classified ideally. |
| D3 mutable truth conditions | `mutable` true only when active segment, first-class storage, resume block, available JSONL, and workspace root all exist; missing resume/storage yields `mutable: false` when location still succeeds. | component | Metadata API fixtures varying one condition at a time. | A8, A9 | Boolean flips only for specified condition; quota rows do not affect result. | Does not prove future pause-handshake lock semantics. |
| D4 partial DB invisible to resolver | Segmentless `session_turns` row exits `10 session-not-found` even when raw turn exists. | component | Temp DB with one `session_turns` row and one unrelated chain row so backfill skip condition is represented. | A7 | Metadata API returns `SessionNotFound`; CLI maps to exit `10`. | Current `StateDb::open` backfill side effects may require direct DB setup after open. |
| Missing UUID | Unknown well-formed UUID exits `10`. | particular-integration | Temp empty/chainless DB fixture after open. | A1 | Exit `10`; stderr JSON error code `session-not-found`. | None beyond resolver correctness. |
| Invalid UUID | Non-UUID input exits `2 invalid-session-id` before DB open. | end-to-end | CLI invocation with impossible DB path or no fixture required. | A1 | Exit `2`; stderr JSON error; no state directory dependency. | Clap structural usage errors may still use clap's default formatting. |
| D6 transcript state reconciliation | `no_locator`, `missing`, relative path, non-existing path, and locator error do not produce partial success JSON. | component + particular-integration | Sessions config variants and locator scripts under temp dir. | A3 | Exit/error `12 unsupported-storage`; optional error details include internal transcript state. | Does not prove 600s timeout behavior except existing `locate_transcript` tests. |
| D7 workspace root derivation | Claude path convention and Codex `session_meta` cwd produce canonical UTF-8 `workspace_root`; absent/relative/non-existing root exits `12`. | component | Temp JSONL files and provider storage dirs; new fixture transcripts for Claude and Codex. | A4 | Success root equals canonical temp workspace; failures map to unsupported storage. | Real upstream path encoding drift can still invalidate A4. |
| Read-only behavior after open | Command does not mutate DB row counts or transcript mtimes after metadata resolution. | particular-integration | Snapshot row counts for `invocations`, `session_turns`, `session_chains`, `session_chain_segments`, `provider_quotas`; snapshot transcript mtime. | A6 | Counts/mtimes unchanged after command, excluding WAL/open side effects. | Does not prove physical read-only open until 06-schema-probe. |
| JSON shape stability under unusual inputs | Unicode workspace path, long but valid UUID strings, and provider names with ordinary punctuation serialize as UTF-8 JSON with required fields only. | component | Temp dirs with Unicode path names; metadata API snapshot/assertion. | A3, A4 | JSON parses; required field set present; paths round-trip as strings. | Non-UTF-8 OS paths are intentionally unsupported, not fuzzed. |
| README examples remain truthful | Documented command synopsis and JSON fields match implementation behavior. | unit/documentation check | Grep/snapshot test over README snippets if project convention supports it; otherwise Phase 6b index maps to manual doc review residual. | none | Synopsis includes `session locate`; fields match schema. | Documentation tests may not execute examples against real CLI. |

New fixture infrastructure is expected for CLI-level Rust integration tests: a temp state DB seeder, temp config-root builder for `providers.toml`/`sessions.toml`, and tiny locator scripts. These do not exist today as a single reusable fixture module and should be flagged in Phase 6b's output index.

# 10. README updates

Update `README.md` in the same style as the current CLI synopsis and trace/resume sections:

- Add `session locate <session-id> [--json]` under Subcommands near `trace`, `repl`, and `resume` (`README.md:127-140`).
- Add a short "Locating a Session" section near "Inspecting a Run" / "Resuming a session" explaining that output is always JSON, success requires a canonical file-backed transcript, and `--json` is accepted for symmetry.
- Document success JSON fields exactly as §3.
- Document exit codes `0`, `1`, `2`, `10`, `11`, and `12`.
- Clarify that `trace --json` still degrades to `no_locator`/`missing`, while `session locate` refuses partial locations with `unsupported-storage` (`README.md:374-386`, `README.md:414-418`).
- Update the "Inspecting via SQL" paragraph to position SQL as ad-hoc debugging, not the supported harness path (`README.md:500-512`).

# 11. Supported-surface track

## 11.1 Supported-surface track

Deployment mode: local CLI binary only. No GUI command, no Tauri frontend surface, no daemon, and no server.

Customer cohort: `agent-harness` is the primary consumer, replacing its v1 direct `state.db`/JSONL locator. Secondary consumers are downstream scripts that need stable session metadata without attempting resume.

Adjacent public/user-reachable paths and blast-radius notes:

- `agents resume`, `agents repl --resume`, and top-level `--resume` keep using `StateDb::resolve_resume`; locate shares the resolver but does not change spawn/resume behavior (`src-tauri/src/main.rs:341-389`, `src-tauri/src/main.rs:1056-1200`).
- `trace --json` remains invocation-tree scoped and keeps its graceful transcript-state degradation; locate is arbitrary-session scoped and refuses partial location (`src-tauri/src/trace/mod.rs:59-80`, `src-tauri/src/trace/mod.rs:318-380`).
- `migrate-config` remains a config rewrite command and is not invoked or coupled (`src-tauri/src/main.rs:160-165`).
- `migrate-db` remains the explicit chain/compaction backfill path; locate does not create a new migration mode (`src-tauri/src/main.rs:158-159`, `src-tauri/src/main.rs:1909-1966`).
- Hidden `resume-list` remains human text and unchanged (`src-tauri/src/main.rs:1887-1900`).
- Direct CLI ingestion remains adapter-script based; locate only reads chain membership produced by ingestion (`src-tauri/src/sessions/mod.rs:55-141`).

Migration path: no user state one-shot is required for read-only locate. Existing partial DBs remain partial; users can run existing `agents migrate-db` when they need backfill repair.

Rollback path: uninstall/revert the binary or avoid the new subcommand. The command is additive and does not write new durable state.

Observability: locate emits no telemetry, no invocation rows, no trace records, and no quota reads beyond ordinary config/state access. Success JSON and stderr JSON errors are the entire observable surface.

# 12. Implementation residuals

Known residuals Phase 4 should evaluate rather than treat as accidental omissions:

- Physical read-only DB open is not in 06-locate. Current `StateDb::open` side effects remain until 06-schema-probe introduces the read-only variant (`src-tauri/src/state/db.rs:431-608`, `initiatives/06-session-override-contract.md:118-120`).
- GUI state DB divergence is out of scope in v1. CLI locate reads `open_default`; GUI commands use `models_dir.parent()/state.db` (`src-tauri/src/state/db.rs:611-615`, `src-tauri/src/lib.rs:525-533`).
- Strict multi-row ambiguity is not implemented; resolver parity is intentionally chosen.
- Workspace-root derivation may reject valid sessions whose provider transcript does not expose an invertible path or metadata root. This is preferable to returning a guessed root under a stable harness contract.
- `other` storage is success-capable only when a file-backed transcript and workspace root can still be proven. Otherwise it is `unsupported-storage`.

# 13. Cross-feature constraint compliance checklist

| Constraint | Compliance | Citation / note |
| --- | --- | --- |
| Shared error-code namespace uses `10` session-not-found, `11` ambiguous-session, `12` unsupported-storage, reserved siblings. | Yes | Namespace defined at `initiatives/06-session-override-contract.md:106-111`; mapping in §5. |
| Ownership resolution reuses `StateDb::resolve_resume`; no second ownership path. | Yes | Initiative constraint at `initiatives/06-session-override-contract.md:112-113`; D1a and D4a in §4 reject strict enumeration and `session_turns` fallback. |
| Lock observation is for import-replace/migration/resume paths once pause-handshake lands. | Not applicable to locate v1 | Locate is read-only and does not write/import; constraint listed at `initiatives/06-session-override-contract.md:114-117`. |
| Read-only `StateDb` open variant lands in 06-schema-probe. | Yes / deferred by initiative sequencing | Constraint at `initiatives/06-session-override-contract.md:118-120`; residual in §12. |
| No auto-resume. | Yes | Initiative anti-scope at `initiatives/06-session-override-contract.md:121-122`; §7 and §8. |
| No provider spawn. | Yes | Initiative anti-scope at `initiatives/06-session-override-contract.md:121-122`; §8 forbids provider commands. |
| No quota refresh. | Yes | Initiative anti-scope at `initiatives/06-session-override-contract.md:121-122`; §3 excludes `exhausted_at` from mutability and §8 forbids quota scripts. |
| No config edits. | Yes | Initiative anti-scope at `initiatives/06-session-override-contract.md:121-122`; §7 and §8. |
| No coupling to `migrate-config`. | Yes | Initiative anti-scope at `initiatives/06-session-override-contract.md:121-122`; §7 and §11.1. |
| 06-locate factors reusable `SessionMetadata` API for later export/import-replace consumers. | Yes | Initiative scope states this at `initiatives/06-session-override-contract.md:41-43`; proposed API in §6. |
