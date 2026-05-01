# Phase 6 Step 6a — Contract for `agents session locate`

This contract bridges the approved proposal (`proposals/06-locate.md`
Rev 3) and Phase 6 implementation. It is **the** authoritative
specification for Step 6b (test writer) and Step 6c (code writer).
Either agent reads this contract; neither needs to consult the
proposal for its job.

The contract preserves every test-intent row from the approved §9.1
test-intent track, with explicit fixture-application points and
expected observable signals. Step 6b writes tests against this
contract. Step 6c writes code to make those tests pass while
respecting the contract's behavioral guarantees.

## 1. CLI surface

### 1.1 Subcommand

A new top-level subcommand `session` is added to the existing
`Subcommands` enum at `src-tauri/src/main.rs:77-166`. It carries one
child subcommand in v1: `locate`.

```text
agents session locate <session-id> [--json]
```

Clap shape:

```rust
// In Subcommands enum:
Session {
    #[command(subcommand)]
    command: SessionSubcommands,
},

// New enum:
enum SessionSubcommands {
    Locate {
        session_id: String,
        #[arg(long)]
        json: bool,
    },
}
```

Bare `agents session` exits with clap usage error (exit code `2`)
because `SessionSubcommands` is non-optional.

The `--json` flag is accepted for symmetry with `trace --json`. It
does NOT change formatting in v1: success output is always one
compact JSON object on stdout; error output is always a JSON object
on stderr.

### 1.2 Top-level dispatch

The `Subcommands::Session` arm is added to `run(cli)` in
`src-tauri/src/main.rs:287-338`, before `ResumeList`, `MigrateDb`,
and `MigrateConfig`. Top-level `args_conflicts_with_subcommands =
true` behavior is preserved (no new top-level flags introduced).

## 2. Public types (new module `src-tauri/src/session_metadata/`)

A new module is added: `src-tauri/src/session_metadata/mod.rs`,
exposed as `pub mod session_metadata;` from `src-tauri/src/lib.rs`.

### 2.1 `SessionMetadata`

```rust
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionMetadata {
    pub session_id: String,        // Active provider session UUID (lowercase hyphenated)
    pub chain_id: String,          // Logical chain UUID (lowercase hyphenated)
    pub provider_name: String,     // Active provider/account name
    pub storage_type: SessionStorageType,
    pub jsonl_path: std::path::PathBuf,    // Canonical absolute UTF-8 path
    pub workspace_root: std::path::PathBuf, // Canonical absolute UTF-8 path
    pub transcript_state: TranscriptState,
    pub mutable: bool,
}
```

Serde must emit field names in snake_case (default). All fields
required on success.

### 2.2 `SessionStorageType`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStorageType {
    ClaudeCode,    // Serializes as "claude_code"
    CodexSession,  // Serializes as "codex_session"
    Other,         // Serializes as "other"; v1 forward-compat only (see below)
}
```

The mapping from internal `config::model::SessionStorage` to
`SessionStorageType`:

- `SessionStorage::ClaudeCode { .. }` → `ClaudeCode`
- `SessionStorage::Codex { .. }` → `CodexSession`
- Provider has no `[providers.session_storage]` block → `Other`

**v1 reachability of `Other`**: the `Other` variant is exposed in
the type and reachable as the result of the storage-type mapping
function (unit-testable). However, the v1 `locate_session_metadata`
success path NEVER emits `storage_type == "other"` to stdout because
Step 8.C fails closed for `Other` (no v1 workspace_root derivation
is supported for storage types without `[providers.session_storage]`).
`Other` remains in the public type for forward-compat with future
locator/storage extensions and for downstream consumers (06-export,
06-import-replace) that may surface storage-type metadata for
sessions that locate cannot fully resolve.

When the CLI is invoked with a session whose provider has no
`[providers.session_storage]` block, the result is exit `12
unsupported-storage` (the type mapping internally produces
`Other`, then Step 8.C converts to `UnsupportedStorage`).

### 2.3 `TranscriptState`

Move the existing enum from `src-tauri/src/trace/mod.rs:73-80` into
`session_metadata/mod.rs`. Trace imports it (the enum's serde
representation must NOT change; current values are `unresolved`,
`no_locator`, `missing`, `available`).

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptState {
    Unresolved,
    NoLocator,
    Missing,
    Available,
}
```

The `as_str()` helper currently in trace moves with the enum.

For `agents session locate`, success requires `TranscriptState::Available`
and the actual `jsonl_path` to be canonical/absolute/exists/UTF-8.
All other states are exit `12 unsupported-storage`.

### 2.4 `MetadataError`

```rust
#[derive(Debug, Clone)]
pub enum MetadataError {
    InvalidSessionId { input: String },
    SessionNotFound { input: String },
    AmbiguousSession { input: String /* additional ambiguity payload optional */ },
    UnsupportedStorage { provider_name: String, reason: String },
    Operational { message: String },
}
```

Exit-code mapping is documented in §4 below. Each variant has at
least one concrete raise site in `locate_session_metadata`.

### 2.5 Public function

```rust
pub fn locate_session_metadata(
    state: &state::StateDb,
    models: &state::ModelStore,
    providers_cfg: &config::providers::ProvidersConfig,
    sessions_cfg: &config::sessions::SessionsConfig,
    input: &str,
) -> Result<SessionMetadata, MetadataError>
```

The CLI wrapper in `main.rs` owns:
- clap parsing
- `StateDb::open_default()`
- config loading via `unwrap_or_default` (matches `run_resume`
  semantics at `src-tauri/src/main.rs:1079-1084`)
- compact stdout JSON
- stderr JSON errors with `code` field
- exit-code mapping per §4

The reusable API owns steps 1, 4-9 of the resolution flow (§3 below).

## 3. Resolution flow (10 steps)

Step numbering matches `proposals/06-locate.md` §4.

### Step 1 — UUID parse

Validate `<session-id>` as a full UUID using `Uuid::parse_str`
(or equivalent). Invalid input returns
`MetadataError::InvalidSessionId { input }` and the CLI maps to
exit `2 invalid-session-id`. UUID parsing happens BEFORE state DB
open or config load.

### Step 2 — State DB open (CLI wrapper)

`StateDb::open_default()` at `src-tauri/src/state/db.rs:611-615`.
This uses `dirs::data_dir()/oulipoly-agent-runner/state.db`. No
`--state-db` override exists in v1.

DB open errors → `MetadataError::Operational` → exit `1`.

### Step 3 — Config load (CLI wrapper)

Mirror resume's exact load pattern:
```rust
let providers_cfg = ProvidersConfig::load(&providers_path).unwrap_or_default();
let sessions_cfg = SessionsConfig::load(&sessions_path).unwrap_or_default();
let models = load_models(&models_dir)?;
```

Cite: `src-tauri/src/main.rs:1079-1084`. Model load failure is the
only operational error at this step; provider/session load failures
silently degrade to default (matches resume).

### Step 4 — Resolve resume

```rust
state.resolve_resume(&models, input, None)
```

Cite: `src-tauri/src/state/db.rs:2577-2582`. Map errors:

| `ResumeError::*` | `MetadataError::*` |
| --- | --- |
| `InvalidUuid { input }` | `InvalidSessionId { input }` |
| `NoChainFound { input }` | `SessionNotFound { input }` |
| `Ambiguous { input, .. }` | `AmbiguousSession { input }` |
| `UnknownModel { .. }` | `Operational { message }` |
| `ProviderModelMismatch { .. }` | `Operational { message }` |
| `ProviderNotConfigured { .. }` | `UnsupportedStorage` (with reason) |
| `ActiveSegmentMissing { .. }` | `SessionNotFound { input }` |
| `ProviderMissingResume { .. }` | `UnsupportedStorage` (with reason) |
| `Db { message }` | `Operational { message }` |

D1a: `ResumeError::Ambiguous` is the ONLY producer of
`AmbiguousSession`; recency-collapsed multi-chain inputs that
return one chain do NOT trigger `AmbiguousSession`.

### Step 5 — Effective/runtime provider

```rust
let provider = if let Some(model) = &resolved.model {
    providers_cfg.effective_provider(/* ... */)
} else {
    providers_cfg.runtime_provider(&resolved.active_provider)
};
```

Cite: `src-tauri/src/config/providers.rs:116-134, 157-191`.
Missing runtime provider entry → `UnsupportedStorage`.

### Step 6 — Storage type

Read `provider.session_storage`:

- `Some(SessionStorage::ClaudeCode { .. })` → `ClaudeCode`
- `Some(SessionStorage::Codex { .. })` → `CodexSession`
- `None` → `Other` (provisional; later steps may convert to `UnsupportedStorage`)

### Step 7 — Locate JSONL

```rust
locate_transcript(sessions_cfg, &resolved.active_provider, &resolved.active_session_id)
```

Cite: `src-tauri/src/sessions/mod.rs:171-199`. Returns
`Result<Option<PathBuf>, String>`.

| Result | Behavior |
| --- | --- |
| `Ok(Some(path))` and path is absolute, canonicalizes, exists, is UTF-8 | success (`TranscriptState::Available`) |
| `Ok(Some(path))` and path is relative, missing, or non-UTF-8 | `UnsupportedStorage { reason: "missing" or "non_utf8" or "relative" }` |
| `Ok(None)` | `UnsupportedStorage { reason: "no_locator" }` |
| `Err(message)` | `UnsupportedStorage { reason: format!("locator_error: {message}") }` |

The locator may create the adapter `state_dir` directory as a
side effect of being invoked; this is the explicitly-permitted
mkdir from §8.

### Step 8 — Workspace root

#### Step 8.A — Claude branch (when `storage_type == ClaudeCode`)

When `jsonl_path` is `<projects_dir>/<project-dir>/<session-id>.jsonl`,
decode `<project-dir>` to an absolute path.

Algorithm:

1. Strip leading `-` from `<project-dir>` (Claude convention: leading
   `-` represents `/`).
2. Enumerate **all** candidate decompositions: each `-` in the
   remaining string is either a path separator or a literal `-` in
   a directory name.
3. For each candidate decomposition, generate the decoded absolute
   path (`/` + components joined by `/`).
4. Check filesystem existence for every candidate.
5. If exactly one decoded path exists, succeed with that path as
   `workspace_root` (canonical, UTF-8).
6. If zero or two-or-more decoded paths exist, return
   `UnsupportedStorage { reason: "ambiguous_path_hash" or "no_existing_path" }`.

Migration's encoding direction is at
`src-tauri/src/migration/mod.rs:155-188`. The decoder is new code in
`src-tauri/src/session_metadata/`.

#### Step 8.B — Codex branch (when `storage_type == CodexSession`)

Read the located rollout JSONL line-by-line until a line with
`type == "session_meta"` is found (one per file by Codex
convention; first-match per existing
`scripts/codex-locate-transcript` precedent).

Extract `session_meta.payload.cwd`:

```text
{
  "type": "session_meta",
  "payload": {
    "id": "...",
    "cwd": "/absolute/path/to/workspace",
    ...
  }
}
```

Validate: absolute, canonicalizes, exists, UTF-8. On any failure
(`session_meta` missing, malformed JSON, `payload.cwd` absent,
non-absolute, non-existing, non-UTF-8), return
`UnsupportedStorage { reason: "codex_<specific>" }`.

Phase 5 sampling evidence (25 real Codex rollout files, 0.46.0 and
0.58.0): see `research/06-locate-hookpoints.md` §I.WS1.

The Codex parser lives in `src-tauri/src/session_metadata/`. It
uses the same JSONL line-walk pattern as
`scripts/codex-locate-transcript`. Multi-record edge case:
first-match semantics. Document the choice in code.

#### Step 8.C — Other branch (when `storage_type == Other`)

In v1, no derivation is supported. Return
`UnsupportedStorage { reason: "no_workspace_root_for_other_storage" }`.

### Step 9 — Compute `mutable`

`mutable: true` if and only if ALL of:

1. `resolve_resume` returned an active segment for the chain (already
   guaranteed by Step 4 success).
2. `storage_type != SessionStorageType::Other` (storage is first-class).
3. `provider.resume.is_some()` — provider declares `[providers.resume]`.
4. `jsonl_path` is `Available` per Step 7 (already guaranteed if Step
   7 succeeded).
5. `workspace_root` is canonical/absolute/exists/UTF-8 per Step 8
   (already guaranteed if Step 8 succeeded).

Otherwise `mutable: false`.

`mutable` does NOT consult `provider_quotas.exhausted_at`. Quota
exhaustion is account-global and not session-scoped.

If only conditions 4 or 5 fail, the request returns exit `12
unsupported-storage` (no partial success JSON). If conditions 1, 2,
or 3 fail despite Steps 4–8 succeeding, the request still returns
exit `0` but with `mutable: false`.

### Step 10 — Emit success

Stdout: one compact JSON object (no pretty-printing), single line,
trailing newline. Field order: deterministic per `serde_json` derive
default (struct field declaration order). Exit `0`.

## 4. Exit codes

| Exit | Error code (stderr JSON) | Trigger |
| --- | --- | --- |
| `0` | none | Success: complete `SessionMetadata` JSON on stdout. |
| `1` | `operational-error` (or specific) | DB open/read failure, model-load failure, JSON serialization failure, unexpected I/O outside transcript/storage classification. |
| `2` | `invalid-session-id` (or clap usage error) | Non-UUID `<session-id>` (parse before DB open). Clap structural usage errors may use clap's default formatting. |
| `10` | `session-not-found` | `MetadataError::SessionNotFound`; partial-DB segmentless sessions also map here. |
| `11` | `ambiguous-session` | `MetadataError::AmbiguousSession`; only when resolver returns `ResumeError::Ambiguous`. |
| `12` | `unsupported-storage` | `MetadataError::UnsupportedStorage`; absent storage block, transcript not canonical/available, workspace_root not derivable, etc. No partial success JSON ever emitted. |

Reserved Initiative 06 codes `13`-`17` are not used by 06-locate.

### Stderr JSON error format

```json
{"error": {"code": "<error-code-string>", "message": "<human-readable-message>"}}
```

Single compact line, trailing newline.

## 5. Side-effect contract

`agents session locate`:

**Permitted:**
- Read state DB rows from `invocations`, `session_chains`,
  `session_chain_segments`, `session_turns`.
- Run configured `transcript_locator` script via
  `locate_transcript()` (existing trace/session contract).
- Create the locator adapter `state_dir` directory if absent
  (cited at `src-tauri/src/sessions/mod.rs:184-185`).
- Read configured Codex rollout JSONL when `storage_type ==
  CodexSession` (read-only, line-walk for `session_meta` first-match).
- `StateDb::open_default()`'s inherent open-time side effects
  (parent dir creation, WAL enable, schema ensure, chain backfill).
  These are documented in §6 as accepted.

**Forbidden:**
- INSERT/UPDATE/DELETE on any table.
- `start_invocation`, `update_session_capture`,
  `update_resume_acceptance`, `finalize_invocation`,
  `mint_chain_for_invocation_session`, `mint_imported_chain_if_absent`.
- Any provider command (`auth_refresh_command`, `quota_script`,
  `turn_script`, provider CLI invocation).
- Migration: `migrate_chain_segment` or related.
- File mutation: copy, create, rename, truncate, rewrite of any
  JSONL file.
- Config edits: `providers.toml`, `sessions.toml`, model TOML,
  auth state.
- Telemetry, durable trace cache, lock state.
- File mutation inside the locator's `state_dir`.

## 6. Test-intent track

This is the test obligation list. Step 6b emits tests covering each
row; Step 6c implements code that makes them pass.

| ID | Risk / acceptance condition | Level | Fixture | Assumption | Observable signal | Residual |
| --- | --- | --- | --- | --- | --- | --- |
| T1 | Resolver pass-through, single chain, single segment: known active segment returns one JSON object with required fields, `transcript_state == "available"`, exit `0` | particular-integration | New `src-tauri/tests/initiative_06_locate.rs`; seed `session_chains` + `session_chain_segments` with one row each; provider config with storage; sessions config with locator script; temp JSONL transcript | A1, A3 | exit `0`, parsed stdout has all fields | Does not validate provider-native transcript content |
| T2 | D1 ambiguity mirrors resolver: multi-chain input with multiple recent chains returns `AmbiguousSession` (exit `11`); recency-collapsed multi-chain returns success (exit `0`) | component | `StateDb` temp DB with controlled `last_used_at`; call `locate_session_metadata` directly | A2 | `MetadataError::AmbiguousSession` only when resolver returns `Ambiguous`; success otherwise | Time-window edges bounded by deterministic timestamps |
| T3 | D2 storage mapping (type level): `ClaudeCode` config → `SessionStorageType::ClaudeCode` (serializes `claude_code`); `Codex` config → `CodexSession` (serializes `codex_session`); `None` config → `Other` (serializes `other`). Test the mapping function in isolation. | unit | Unit tests over the storage-type mapping function (no DB or transcript fixtures needed) | A5 | Mapping function returns expected variant; serde renders expected lowercase string | Does not validate future variants |
| T4 | D2 unsupported no-storage case (CLI level): provider without `[providers.session_storage]` → exit `12 unsupported-storage` (mapping produces `Other`; Step 8.C fails closed). Holds regardless of locator state. | particular-integration | Temp DB + provider config without storage + sessions config with present-or-absent locator | A3, A5 | exit `12`, stderr JSON code `unsupported-storage`, no stdout success; `storage_type == "other"` never appears on stdout in v1 | Not all third-party locator failures classified ideally |
| T5 | D3 mutable truth conditions: matrix varying conditions 1, 3, 4, 5 from contract §3 Step 9. Condition 2 (`storage_type != Other`) is structurally unreachable in v1 success path because Step 8.C fails closed for `Other`; verify the invariant by including a no-storage fixture asserting the call returns `UnsupportedStorage` (not `Other` + `mutable: false`). | component | `locate_session_metadata` fixtures varying one of conditions 1, 3, 4, 5 at a time, plus a no-storage fixture | A8, A9 | Boolean flips only for specified condition; `provider_quotas.exhausted_at` does NOT affect result; no-storage case returns `UnsupportedStorage`, never `Other` + `mutable: false` | Does not prove future pause-handshake lock semantics; condition 2 is structurally unreachable in v1 |
| T6 | D4 partial DB invisible: segmentless `session_turns` row → `SessionNotFound` (exit `10`) | component | Temp DB with one `session_turns` row + one unrelated chain row (so `backfill_session_chains` skip condition holds) | A7 | `MetadataError::SessionNotFound`; CLI exit `10` | Open-time backfill side effects may need direct DB setup after open |
| T7 | D5 default DB only: clap rejects unknown `--state-db <path>` flag; locate uses `open_default()` only; no GUI state DB integration | unit | clap parser test in `src-tauri/src/main.rs` (no fixture) | A6 | Clap usage error / parse failure | GUI state DB integration out of scope |
| T8 | Missing UUID: well-formed unknown UUID → `SessionNotFound` (exit `10`) | particular-integration | Temp empty/chainless DB after open | A1 | exit `10`, stderr JSON code `session-not-found` | None |
| T9 | Invalid UUID: non-UUID input → exit `2 invalid-session-id` BEFORE DB open | end-to-end | CLI invocation; impossible DB path or no fixture | A1 | exit `2`, stderr JSON code `invalid-session-id`, no state directory created/touched | Clap structural usage errors may use clap formatting |
| T10 | D6 transcript state reconciliation: `no_locator`, `missing`, relative path, non-existing path, locator error → no partial success JSON; exit `12 unsupported-storage` | component + particular-integration | Sessions config variants + locator scripts under temp dir | A3 | exit `12`, optional `reason` includes internal transcript state | 600s timeout behavior covered by existing `locate_transcript` tests |
| T11 | D7 Claude path-hash inversion success: well-behaved project-dir → canonical absolute UTF-8 `workspace_root` | component | Temp `projects_dir/<project-dir>/<session>.jsonl` with one valid corresponding workspace tree on disk | A4 | Success root equals canonical temp workspace | Real upstream path encoding drift can invalidate A4 |
| T12 | D7 Claude path-hash ambiguity: zero / one / multiple existing decompositions → success only on exactly one; exit `12` on zero or multiple | component | Temp directory tree with `-` in components and fixtures where 0, 1, and multiple decompositions exist | A4 | One-existing decomposition succeeds; multiple existing return `UnsupportedStorage`/exit `12` | Specific tiebreaker only |
| T13 | D7 Codex `payload.cwd` success: located rollout JSONL with `type == "session_meta"` and valid absolute `payload.cwd` → canonical absolute UTF-8 `workspace_root` | component | Codex provider config with sessions_dir; rollout JSONL fixture with `session_meta` line containing `payload.cwd` pointing to a real directory | A4 | Success root equals canonical fixture workspace | Real Codex schema drift can invalidate A4 |
| T14 | D7 Codex failure modes: missing `session_meta`, absent `payload.cwd`, non-absolute `cwd`, non-existing `cwd`, non-UTF-8 `cwd` → exit `12 unsupported-storage` with specific reason | component | Codex rollout JSONL fixtures, one per failure mode | A4 | exit `12`, stderr JSON `code: unsupported-storage`, `reason` distinguishes the failure | Multi-record `session_meta` edge: first-match per `scripts/codex-locate-transcript` precedent |
| T15 | Read-only behavior after open: command does not mutate row counts in `invocations`, `session_turns`, `session_chains`, `session_chain_segments`, `provider_quotas`; transcript mtimes unchanged after metadata resolution | particular-integration | Snapshot DB row counts + transcript mtime; run CLI; snapshot again | A6 | All row counts and transcript mtimes unchanged (excluding open-time WAL/schema/backfill) | Physical read-only open deferred to 06-schema-probe |
| T16 | JSON shape stability: Unicode workspace path, long valid UUID strings, provider names with ordinary punctuation → UTF-8 JSON; required field set; paths round-trip | component | Temp dirs with Unicode path names; metadata API snapshot/assertion | A3, A4 | JSON parses; required field set present; paths round-trip as strings | Non-UTF-8 OS paths intentionally unsupported, not fuzzed |

T1–T16 cover the proposal §9.1 rows plus the path-hash ambiguity
row added in Rev 2 plus the Codex success/failure split in Rev 3.

## 7. Fixture application points

Step 6b is allowed (and expected) to introduce new shared fixture
infrastructure. Specifically:

### 7.1 New file `src-tauri/tests/initiative_06_locate.rs`

CLI integration tests live here, following the pattern of
`src-tauri/tests/pr_b_trace_integration.rs:107-125` and
`src-tauri/tests/pr_f_resume_integration.rs:360-384`. Use
`env!("CARGO_BIN_EXE_oulipoly-agent-runner")` to spawn the binary.

### 7.2 Component / unit tests

Place inside `src-tauri/src/session_metadata/` next to the API
under `#[cfg(test)] mod tests`. Use existing test helpers in
`src-tauri/src/state/db.rs:5348-5405` for `StateDb` resolver
fixtures.

### 7.3 Helpers expected to exist or be introduced

- A temp state-DB seeder that writes `session_chains` and
  `session_chain_segments` rows directly via SQL (existing
  `Initiative 05` migration tests already do this in
  `src-tauri/tests/initiative_05_migration.rs:23-145`).
- A temp config-root builder for `providers.toml`, `sessions.toml`,
  and locator scripts (existing patterns in
  `src-tauri/tests/pr_f_resume_integration.rs:11-88`).
- Temp Codex rollout JSONL fixture with
  `{"type": "session_meta", "payload": {"id": "...", "cwd": "..."}}`.
- Temp Claude project-dir layout for path-hash inversion fixtures.

If a single reusable shared fixture module is convenient, place it
in `src-tauri/tests/common/` or `src-tauri/src/session_metadata/test_fixtures.rs`. Step 6b decides the
exact factoring.

### 7.4 Codex rollout JSONL synthetic fixture format

Real Codex 0.46.0/0.58.0 rollout files have many records (turn,
session_meta, response, etc.). Fixtures need only the minimum:

```jsonl
{"type": "session_meta", "payload": {"id": "<uuid>", "cwd": "/absolute/workspace"}}
```

Fixtures for failure modes are minor variations (missing line,
absent `cwd`, non-absolute `cwd`, etc.).

## 8. Behavioral assumptions (carried from proposal §1.1)

These assumptions are embedded in the contract; tests verify the
inputs that satisfy them.

- **A1**: `resolve_resume` returns one owner for single-chain,
  single-segment case.
- **A2**: ambiguity == `ResumeError::Ambiguous` (D1a).
- **A3**: `transcript_locator` resolves canonical local JSONL
  without provider spawn.
- **A4**: `workspace_root` derivable for Claude (path-hash
  inversion) AND Codex (`session_meta.payload.cwd`).
- **A5**: `[providers.session_storage]` is the storage-type
  source; `codex_session` is output vocabulary only.
- **A6**: v1 logically read-only despite physical open side
  effects; physical read-only open deferred to 06-schema-probe.
- **A7**: post-Initiative-05 direct CLI sessions have chain
  membership.
- **A8**: `mutable` is composite, not stored.
- **A9**: `mutable` excludes `provider_quotas.exhausted_at`.

## 9. Process-tree audit obligations (Phase 6 firstness evidence)

After Step 6c, `process-tree-auditor` runs on the Phase 6 subtree.
Step 6b and Step 6c must be **separate agent invocations** with
separate `agents` CLI calls. Step 6b produces an output index at
`.tmp/phase6/step6b-output-index.md` that maps every test-intent
ID (T1–T16) to:

- the test file path
- the test or test-group identifier
- the named risk and selected level
- the fixture source / fixture application point
- assumption-register link
- residual-risk entry path (if applicable)

Step 6c must read `.tmp/phase6/step6b-output-index.md` AND the
test files before writing any product code, and its log must echo
those reads.

## 10. References (test writer reads only as needed; not for design)

- Approved proposal: `proposals/06-locate.md` (Rev 3)
- Phase 5 hookpoints: `research/06-locate-hookpoints.md`
- Problem map: `research/06-locate-problem-map.md`
- Initiative: `initiatives/06-session-override-contract.md`
- Audit history: `risk/06-locate-audit-history.md`
- Harness spec (read-only context): external `agent-harness` scratch file `01-session-locate.md`

## 11. Glossary

- **chain_id**: stable logical conversation UUID (Initiative 05).
- **active segment**: a `session_chain_segments` row with `ended_at IS NULL`.
- **canonical path**: `Path::canonicalize()` result.
- **first-match**: line-walk semantics matching the existing
  `scripts/codex-locate-transcript` pattern.
- **D-decision**: design decision number from the proposal §4
  (D1=ambiguity, D2=storage vocabulary, D3=mutable, D4=partial-DB,
  D5=state-DB-override, D6=transcript_state, D7=workspace_root).
