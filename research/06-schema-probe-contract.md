# Phase 6 Step 6a — Contract for `agents session schema-probe`

This contract bridges `proposals/06-schema-probe.md` (Rev 2) and Phase 6
implementation. Step 6b (test writer) and Step 6c (code writer) read
this contract; neither needs the proposal.

## 1. CLI surface

### 1.1 Subcommand

```text
agents session schema-probe
```

No flags in v1. No `--state-db` override. No top-level alias.

Clap shape — extend `SessionSubcommands` (introduced by 06-locate; if
schema-probe lands first, introduce the parent `Subcommands::Session`
group simultaneously):

```rust
enum SessionSubcommands {
    Locate { session_id: String, #[arg(long)] json: bool },  // 06-locate (may not be present yet)
    SchemaProbe,
}
```

Bare `agents session` exits with clap usage error code `2`.

### 1.2 Top-level dispatch

`Subcommands::Session` arm in `run(cli)` — same arm used by 06-locate.
Add a match on `SessionSubcommands::SchemaProbe` calling
`run_session_schema_probe()`.

## 2. Public types (extend `src-tauri/src/state/`)

### 2.1 `StateDb::open_read_only`

```rust
impl StateDb {
    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError>;
}
```

Behavior:
- Opens SQLite with `mode=ro` URI.
- Does NOT create parent directories.
- Does NOT enable WAL.
- Does NOT run schema-ensure helpers.
- Does NOT run `backfill_session_chains`.
- Returns `ReadOnlyOpenError::Missing` for nonexistent file.
- Returns `ReadOnlyOpenError::PermissionDenied` for unreadable file.
- Returns `ReadOnlyOpenError::NotADatabase` for invalid SQLite file.
- Returns `ReadOnlyOpenError::WalSidecarError` for WAL/shm read failure.
- Returns `ReadOnlyOpenError::Operational` for other unexpected open errors.

### 2.2 `ReadOnlyOpenError`

```rust
#[derive(Debug, Clone)]
pub enum ReadOnlyOpenError {
    Missing { path: PathBuf },
    NotADatabase { path: PathBuf, message: String },
    PermissionDenied { path: PathBuf },
    WalSidecarError { path: PathBuf, message: String },
    Operational { message: String },
}
```

CLI exit-code mapping:
- `Missing` → exit `0` (success; emit JSON with `state_db.exists: false`, `safe_for_import_replace: false`).
- `NotADatabase`, `PermissionDenied`, `WalSidecarError`, `Operational` → exit `1` (operational error).

### 2.3 Schema-probe report types

```rust
pub struct SchemaProbeReport {
    pub binary: BinaryInfo,
    pub state_db: StateDbReport,
    pub features: FeatureMap,
    pub supported_storage_types: Vec<String>,  // ["claude_code", "codex_session", "other"]
    pub safe_for_import_replace: bool,
}

pub struct BinaryInfo {
    pub name: String,        // env!("CARGO_PKG_NAME")
    pub version: String,     // env!("CARGO_PKG_VERSION")
    pub commit: String,      // build.rs-injected; "unknown" if absent
}

pub struct StateDbReport {
    pub path: PathBuf,
    pub exists: bool,
    pub schema_version: u32,
    pub user_version: u32,
    pub current_schema_version: u32,         // const CURRENT_SCHEMA_VERSION = 3
    pub minimum_supported_schema_version: u32,
    pub compatible: bool,
    pub tables: BTreeMap<String, bool>,                       // flat
    pub required_columns: BTreeMap<String, BTreeMap<String, bool>>,  // nested
    pub required_indexes: BTreeMap<String, BTreeMap<String, bool>>,  // nested
}

pub type FeatureMap = BTreeMap<String, bool>;
```

`BTreeMap` ensures deterministic JSON key order on serialization.

All structs serialize via `#[derive(serde::Serialize)]` with default snake_case field names.

## 3. JSON output schema

Single compact JSON line on stdout. Required success fields per
proposal §3 (line 117-132). The compatibility-map shape is
**load-bearing**:

- `state_db.tables`: flat boolean map, keys are table names.
- `state_db.required_columns`: nested table → column → bool.
- `state_db.required_indexes`: nested table → index → bool.
- No dotted keys (`"session_turns.parent_turn_id"`) anywhere.

Required tables (per proposal §3.4 / Rev 2):
- `invocations`
- `session_turns`
- `session_chains`
- `session_chain_segments`

Required columns (per proposal §3 example block):
- `invocations`: `session_id`, `session_capture_method`, `resume_acceptance_status`, `resume_acceptance_evidence`
- `session_turns`: `parent_turn_id`, `is_sidechain`, `is_compaction_boundary`
- `session_chains`: `chain_id`, `created_at`, `last_used_at`, `model_name`
- `session_chain_segments`: `chain_id`, `provider_name`, `session_id`, `started_at`, `ended_at`, `last_turn_id`, `transition_reason`

Required indexes (per proposal §3 example block):
- `invocations`: `idx_invocations_provider_session`
- `session_turns`: `idx_session_turns_session_lookup`
- `session_chain_segments`: `idx_segments_session`, `idx_segments_chain_active`

`features` map (Rev 1 values):
```text
session_locate: true (false if 06-locate not yet merged)
session_export: false
session_import_replace: false
session_pause_handshake: false
session_schema_probe: true
```

`supported_storage_types`: `["claude_code", "codex_session", "other"]`.

`safe_for_import_replace`: `true` only when ALL of:
- `state_db.exists`
- `state_db.compatible`
- `features.session_import_replace`

Otherwise `false`.

## 4. Resolution flow

1. Resolve state DB path via `dirs::data_dir().join("oulipoly-agent-runner/state.db")`.
2. Check if file exists with `Path::exists()`.
3. If missing: build `SchemaProbeReport` with `state_db.exists: false`, `state_db.schema_version: 0`, all required-table booleans `false`, etc. Emit JSON, exit `0`.
4. If present: call `StateDb::open_read_only(&path)`.
   - On `ReadOnlyOpenError::Missing`: same as step 3 (race condition handled).
   - On other errors: emit stderr JSON `{"error": {"code": "operational-error", ...}}`, exit `1`.
5. Read `PRAGMA user_version` → `state_db.user_version` and `state_db.schema_version` (same value).
6. Inspect schema: read `sqlite_schema` to enumerate tables and indexes; for each required table, check existence and required-column presence via `PRAGMA table_info(<table>)`.
7. Compute `state_db.compatible`: `schema_version >= MINIMUM_SUPPORTED_SCHEMA_VERSION` AND all required tables/columns/indexes present.
8. Build the rest of the report: binary info, features, storage types, `safe_for_import_replace`.
9. Emit compact JSON to stdout. Exit `0` for success; exit `14` if `state_db.compatible == false` AND the schema is fundamentally incompatible (e.g., schema_version too old).

## 5. Exit codes

| Exit | Trigger |
| --- | --- |
| `0` | Probe succeeded, including missing-DB case (emit JSON with structural booleans). |
| `1` | Operational error (open failure, unreadable file). |
| `2` | Clap usage error. |
| `14` | DB exists but schema is incompatible (older than `MINIMUM_SUPPORTED_SCHEMA_VERSION`). |

Stderr JSON error format: `{"error": {"code": "<error-code>", "message": "..."}}`.

## 6. Side-effect contract

`agents session schema-probe`:

**Permitted:**
- Read state DB metadata (PRAGMA user_version, sqlite_schema, PRAGMA table_info).
- Use `Path::exists()` to check file presence.

**Forbidden:**
- INSERT/UPDATE/DELETE on any table.
- `PRAGMA user_version = N` (no stamping).
- Schema migrations, table creation, index creation.
- Backfill (`backfill_session_chains` not called).
- WAL enable.
- Parent directory creation.
- Provider commands, quota refresh, auth flow, locator scripts.
- Config edits.
- Telemetry, invocation rows.

The implementation MUST use `StateDb::open_read_only`, never `StateDb::open` or `StateDb::open_default`.

## 7. Test-intent track

Per proposal §9.1 (Rev 2). T1–T8 (rough mapping; preserve every row from §9.1):

| ID | Risk | Level | Fixture | Observable signal |
| --- | --- | --- | --- | --- |
| T1 | Schema-probe success on current-schema DB returns full JSON with `compatible: true`, `safe_for_import_replace` per features | particular-integration | Temp state DB seeded with current schema (all required tables/columns/indexes); env XDG_DATA_HOME redirects open_default | exit 0; stdout JSON has all required fields; compatibility shapes nested correctly |
| T2 | Missing-DB case: probe returns exit `0` with JSON `state_db.exists: false`; structural booleans all `false`; `safe_for_import_replace: false` | particular-integration | Empty XDG_DATA_HOME (no oulipoly-agent-runner/state.db) | exit 0; stdout JSON; `state_db.exists` false |
| T3 | Incompatible-schema case: DB present, `user_version: 1` (below `MINIMUM_SUPPORTED_SCHEMA_VERSION`); probe returns exit 14 | particular-integration | Temp DB with explicit `PRAGMA user_version = 1` only; no schema | exit 14; stderr JSON code `schema-incompatible` |
| T4 | Operational error case: DB file unreadable (permissions); probe returns exit 1 | particular-integration | Temp DB with mode 000 | exit 1; stderr JSON code `operational-error` |
| T5 | `StateDb::open_read_only` does not mutate DB or create files | component | Snapshot DB mtime, parent dir state, no WAL/shm files; call open_read_only; re-snapshot | mtime unchanged; no new files |
| T6 | `ReadOnlyOpenError` variants raise per spec: Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational | component | Each fixture variant of the error condition | Each variant fires the right enum case |
| T7 | `safe_for_import_replace` predicate respects features map (false when feature gate is off, even on healthy DB) | unit | Construct SchemaProbeReport with controlled inputs | Boolean flips per condition |
| T8 | JSON shape stability: compatibility maps emit nested (not dotted) keys | unit | Serialize SchemaProbeReport with mock data | parse JSON; verify no dotted keys |

(Step 6b may add T-rows for additional D-decision branches; expand per
the proposal §9.1 contract.)

## 8. Fixture application points

- New `src-tauri/tests/initiative_06_schema_probe.rs` — CLI integration tests (binary spawn).
- New helpers in `src-tauri/tests/fixtures/initiative_06_schema_probe.rs` (or extend `fixtures/initiative_06.rs` from 06-locate if it ever lands on this branch — it doesn't on schema-probe's branch; create new fixture file).
- Component tests in `src-tauri/src/state/db.rs` `#[cfg(test)] mod tests` for `StateDb::open_read_only` direct calls.
- `cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_schema_probe` should pass after Step 6c.

## 9. Process-tree audit obligations

Step 6b and Step 6c must be **separate agent invocations** — same rule
as 06-locate. Step 6b writes
`.tmp/phase6/step6b-output-index.md` mapping every test-intent ID to:
- test file path
- test or test-group identifier
- named risk and selected level
- assumption-register link
- fixture source
- residual-risk entry path (if applicable)

Step 6c **must** write `.tmp/phase6/step6c-reads.md` BEFORE any
product-code change (this is the file-based firstness evidence
that 06-locate's repair pass established).

After Step 6c, run `process-tree-auditor` on the Phase 6 subtree.

## 10. References (test writer reads only as needed)

- Approved proposal: `proposals/06-schema-probe.md` (Rev 2).
- Hookpoints: `research/06-schema-probe-hookpoints.md`.
- Audit history: `risk/06-schema-probe-audit-history.md`.
- Initiative: `worktrees/06-locate/initiatives/06-session-override-contract.md`.
- 06-locate's contract for cross-feature consistency:
  `/home/nes/projects/agent-runner/worktrees/06-locate/research/06-locate-contract.md`.
