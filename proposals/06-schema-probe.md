# 1. Scope statement (Rev 1)

06-schema-probe adds one read-only CLI surface:

```bash
agents session schema-probe
```

It emits one structured compatibility report for the CLI default
`state.db`, binary identity, compiled session features, and public
storage vocabulary needed by `agent-harness`. This is the second
Initiative 06 feature in technical order, after locate and before
export/pause/import-replace (`initiatives/06-session-override-contract.md:38-56`,
`initiatives/06-session-override-contract.md:75-89`).

This proposal defines the command, JSON, versioning, read-only open,
feature flags, import-replace safety predicate, exit mapping, tests,
supported surface, and anti-scope for Phase 6. It consumes
`research/06-schema-probe-problem-map.md`; §1.1 replaces that map's
draft register.

The change adds/extends the `session` subcommand group, introduces
`StateDb::open_read_only(path)` plus schema-inspection helpers, defines
schema-version constants, and documents the surface. Mutating
schema-ensure paths may stamp `PRAGMA user_version`; the probe only reads
it.

Unchanged surfaces: `agents trace`, `agents repl`, `agents resume`,
top-level `--resume`, hidden `resume-list`, `migrate-db`, and
`migrate-config`; the existing mutating `StateDb::open`; all sibling
Initiative 06 commands; provider spawn/resume/quota/config/transcript
work. No existing command is retrofitted to read-only open in this PR
(D7).

## 1.1 Assumption register

This is the approved register validated and narrowed from
`research/06-schema-probe-problem-map.md` §7. It replaces the draft
register there; do not maintain a competing register.

| ID | Assumption | Evidence | Invalidator | Used by |
| --- | --- | --- | --- | --- |
| A1 | A read-only `StateDb` open variant can inspect compatibility without running current open-time schema ensure or backfill. | `schema-probe` only needs path existence, `PRAGMA user_version`, and `sqlite_master` / PRAGMA table/index inspection; current resolver reads through `rusqlite` once a connection exists (`research/06-schema-probe-problem-map.md` §1 #9-13, §7 A1). | Correct session compatibility can only be established after mutating schema ensure or `backfill_session_chains`. | §4 flow; §6 open API; §8 side-effect contract; §9.1 D3 tests. |
| A2 | `PRAGMA user_version` is the right public schema-version source once introduced. | SQLite exposes an integer version slot and current code has no competing schema metadata table or schema constant surface (`research/06-schema-probe-problem-map.md` §5 #7-9). | Existing/future schema states cannot map to one integer without hiding meaningful compatibility differences. | D1; §3 `schema_version`; §5 exit `14`; §9.1 D1 tests. |
| A3 | Compiled feature support is binary-bound enough for harness gating. | Feature support is currently implied by compiled clap arms and modules, not by user config (`research/06-schema-probe-problem-map.md` §2 #13, §7 A3). | A claimed feature's safety depends on runtime provider config rather than binary support. | D2; §3 `features`; §4 flow; §9.1 D2 tests. |
| A4 | The CLI default DB path is the right v1 target for `schema-probe`. | CLI callers use `StateDb::open_default()` at `dirs::data_dir()/oulipoly-agent-runner/state.db`, and README documents that path (`research/06-schema-probe-problem-map.md` §1 #11-12, §7 A4). | GUI state DB divergence is declared part of the public harness surface. | §2 command surface; §3 `state_db.path`; §11 supported surface. |
| A5 | 06-locate will normally land first, but schema-probe must still be reviewable as a parallel PR. | Initiative sequencing puts locate before schema-probe, while this worktree does not contain locate's `session` group or storage enum yet (`research/06-schema-probe-problem-map.md` §1 #4-7, §7 A5). | The final base branch already exposes a shared storage enum/module that Phase 5 hookpoints require schema-probe to reuse. | D5; §2 command surface; §6 helpers; §12 residuals. |
| A6 | Schema incompatibility can be evaluated from structural inspection plus `user_version` without validating all data invariants. | Required tables, columns, and indexes for the public session surface are known today (`research/06-schema-probe-problem-map.md` §1 #13-22, §7 A6). | Compatibility depends on expensive, mutating, or full-data invariants such as complete chain backfill repair. | §3 schema fields; §4 flow; §5 exit `14`; §9.1 D4/D6 tests. |

## 1.2 Net-value statement

Yes: this reduces a concrete current-state risk on the supported CLI
surface. Today, a caller that only wants to know whether `state.db` is
compatible must either inspect SQLite manually or open the database
through paths that can create directories, enable WAL, create/alter/drop
schema objects, and run session-chain backfill (`research/06-schema-probe-problem-map.md`
§2 #1-8, §6 #1-5). There is no stable CLI JSON answer for "which schema
version is this DB, which session features are compiled into this binary,
and is the DB safe for future import-replace?"

The blast radius is bounded because the command is additive and
physically read-only. Migration burden is explicit: existing databases
without stamped `PRAGMA user_version` are refused for harness writes
until a normal mutating migration/open path stamps the current version.
Rollback is low cost because the probe writes no state. The value is
positive because `agent-harness` can stop reading private layouts
directly and can refuse before future write-capable session operations.

# 2. Subcommand surface

Add or extend the `Session` variant of `Subcommands`:

```text
agents session schema-probe
```

If 06-locate has landed, extend its existing `SessionSubcommands` enum:

```rust
enum SessionSubcommands {
    Locate { /* existing */ },
    SchemaProbe,
}
```

If schema-probe is applied independently before locate, introduce the
same parent group and only the `SchemaProbe` child. The final merged
surface must have one `session` group, not top-level aliases.

No flags are accepted in v1. There is no `--state-db` override: the
probe reports the CLI default DB path only (A4). Bare `agents session`
and unknown child commands retain clap usage behavior and code `2`.
Dispatch lives beside existing subcommand dispatch before top-level
prompt/resume routing, preserving `args_conflicts_with_subcommands =
true` (`research/06-schema-probe-problem-map.md` §1 #8).

# 3. JSON output schema

Success stdout is a single compact JSON object. Pretty printing is not a
contract. Existing fields must not change meaning; future additions must
be optional fields or additional object members.

Required success fields:

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `binary.name` | string | yes | Cargo package name, currently `oulipoly-agent-runner`. |
| `binary.version` | string | yes | Cargo package version. |
| `binary.commit` | string | yes | Build commit if embedded, otherwise `"unknown"`; the runtime command must not invoke git. |
| `state_db.path` | string path | yes | CLI default state DB path from the same resolver used by `open_default`. |
| `state_db.exists` | boolean | yes | Whether the DB file exists before any SQLite open attempt. |
| `state_db.schema_version` | integer | yes | Same value as `state_db.user_version`; source is `PRAGMA user_version` (D1a). Missing DB reports `0`. |
| `state_db.user_version` | integer | yes | Raw `PRAGMA user_version`, or `0` when the DB file is absent. |
| `state_db.current_schema_version` | integer | yes | Binary-declared current schema version. Rev 1 sets this to `3`. |
| `state_db.minimum_supported_schema_version` | integer | yes | Lowest schema version this binary accepts for the public session surface. Rev 1 sets this to `3`. |
| `state_db.compatible` | boolean | yes | True only when version and structural checks pass. |
| `state_db.tables` | object boolean map | yes | Presence of `invocations`, `session_turns`, `session_chains`, and `session_chain_segments`. |
| `state_db.required_columns` | object boolean map | yes | Presence of required public-session columns grouped by table. |
| `state_db.required_indexes` | object boolean map | yes | Presence of required public-session indexes. |
| `features` | object boolean map | yes | Compiled session feature claims (D2a). |
| `supported_storage_types` | string array | yes | Stable public vocabulary: `claude_code`, `codex_session`, `other` (D5). |
| `safe_for_import_replace` | boolean | yes | Predicate defined in §3.4 / D4. |

## 3.1 D1: `schema_version` source

Choose **D1a — `PRAGMA user_version`**.

`schema_version` and `user_version` report the same DB-owned integer. This
keeps compatibility inspectable through standard SQLite tooling. D1b is
rejected because a metadata table creates another bootstrap rule. D1c is
rejected because a binary constant cannot describe the opened DB.

Version assignments: `0` means missing or unversioned pre-probe DB; `1`
is the baseline before Initiative 04's schema change; `2` is Initiative
04 reactive routing (`provider_quotas.exhausted_at`, old
`quota_tight_routing` shape removed; `initiatives/04-reactive-routing.md:37-44`);
`3` is Initiative 05 session migration (`session_turns.is_compaction_boundary`,
`session_chains`, `session_chain_segments`, segment indexes;
`proposals/05-session-migration.md:62-103`).

Rev 1 declares `CURRENT_SCHEMA_VERSION = 3` and
`MINIMUM_SUPPORTED_SCHEMA_VERSION = 3`. Future schema-changing PRs must
bump the constant and assign the new integer in their proposal. Mutating
schema-ensure paths may stamp `PRAGMA user_version =
CURRENT_SCHEMA_VERSION`; the probe must never stamp it.

## 3.2 D2: Feature-flag enumeration

Choose **D2a — hardcoded list in binary**.

The probe reports compiled public support, not clap's accidental shape or
Cargo feature toggles. Cargo features are rejected because these are
ordinary product commands. Clap introspection is rejected because command
presence does not prove the harness contract or side-effect semantics.

Rev 1 feature map: `session_locate: true`,
`session_export: false`, `session_import_replace: false`,
`session_pause_handshake: false`, `session_schema_probe: true`. If
schema-probe lands before locate, `session_locate` is `false` until
locate's command and contract are present. Each future sibling PR updates
this list in the same PR that ships the feature.

## 3.3 D5: `storage_type` vocabulary

Choose the **parallel PR** path: schema-probe defines its own public
storage vocabulary for the probe report, matching 06-locate exactly:

```text
claude_code
codex_session
other
```

This enum is not the internal config enum (`SessionStorage::Codex`
currently serializes as `codex`). If Phase 5 finds that 06-locate has
landed a shared public `SessionStorageType`, implementation may reuse it,
but must not introduce a second JSON vocabulary or aliases.

## 3.4 D4: `safe_for_import_replace` predicate

`safe_for_import_replace` is `true` only when all conditions hold:

1. `state_db.exists == true`.
2. `state_db.compatible == true`.
3. `state_db.schema_version` is in this binary's supported range.
4. All required session tables, columns, and indexes are present.
5. `features.session_import_replace == true`.
6. `features.session_pause_handshake == true`, because import-replace is
   sequenced after lock support and must not advertise write safety
   without it.
7. `supported_storage_types` includes the public storage types required by
   import-replace's approved contract.

It is `false` for missing DBs, unversioned DBs, older/newer schemas,
structural incompatibility, and binaries without import-replace or
pause-handshake. In this PR it is expected to be `false`.

# 4. Resolution flow

1. Parse `agents session schema-probe`; usage errors are clap errors and
   exit `2`.
2. Resolve the CLI default DB path without creating directories.
3. Build `binary`, `features`, and `supported_storage_types` from
   compile-time metadata and hardcoded probe helpers. The command must not
   shell out to `git` at runtime; missing build commit emits `"unknown"`.
4. If the DB file does not exist, emit success JSON with
   `state_db.exists: false`, `schema_version: 0`, all required
   table/column/index booleans `false`, `compatible: false`, and
   `safe_for_import_replace: false`; exit `0`.
5. Open the DB through `StateDb::open_read_only(&path)` (D3). This step
   must not create parent directories, create the DB file, set
   `journal_mode`, ensure schema, or run backfill.
6. Read `PRAGMA user_version` and store it as both `user_version` and
   `schema_version`.
7. Inspect required tables, columns, and indexes through `sqlite_master`,
   `PRAGMA table_info`, and, where needed, `PRAGMA index_info`.
8. Compute compatibility: false when `schema_version` is below
   `MINIMUM_SUPPORTED_SCHEMA_VERSION`, above `CURRENT_SCHEMA_VERSION`, or
   any required table/column/index is missing; true otherwise.
9. If the DB exists but compatibility is false, emit a structured stderr
    JSON error with code `schema-incompatible` and exit `14`. Do not emit a
    success object on stdout.
10. If inspection fails for operational reasons (permission, invalid
    SQLite file, I/O, WAL/shm read failure), emit structured stderr JSON
    with code `state-open-failed` or `state-inspect-failed` and exit `1`.
11. If compatible, compute `safe_for_import_replace` from §3.4 and emit
    the success JSON object on stdout; exit `0`.

# 5. Exit codes table (D6)

| Exit | Error code | Scenario | Output |
| --- | --- | --- | --- |
| `0` | none | Probe succeeded against an existing compatible DB. | Success JSON on stdout. |
| `0` | none | Default DB file is missing. | Success JSON on stdout with `exists: false`, `schema_version: 0`, `compatible: false`, and `safe_for_import_replace: false`. |
| `1` | `state-open-failed` | DB file is present but cannot be opened read-only because of permissions, invalid SQLite header, WAL/shm access failure, or other SQLite open error. | JSON error object on stderr; no success stdout. |
| `1` | `state-inspect-failed` | Read-only open succeeded but PRAGMA or `sqlite_master` inspection fails operationally. | JSON error object on stderr; no success stdout. |
| `2` | clap usage | Structural CLI misuse such as unknown flag/child command. | Clap stderr. |
| `14` | `schema-incompatible` | DB exists with `user_version` lower than `MINIMUM_SUPPORTED_SCHEMA_VERSION`, including unversioned `0`. | JSON error object on stderr including the compatibility report; no success stdout. |
| `14` | `schema-incompatible` | DB exists with `user_version` higher than `CURRENT_SCHEMA_VERSION`. | JSON error object on stderr; no success stdout. |
| `14` | `schema-incompatible` | DB exists but required session tables, columns, or indexes are missing. | JSON error object on stderr naming failed booleans; no success stdout. |

Reserved Initiative 06 error codes `10`-`13` and `15`-`17` are not used
by schema-probe, but code `14` must remain in the shared
`schema-incompatible` slot (`initiatives/06-session-override-contract.md:106-122`).

# 6. Reusable `StateDb::open_read_only` API + helpers

Add a read-only open API in `src-tauri/src/state/db.rs`:

```rust
impl StateDb {
    pub fn default_path() -> Result<PathBuf, String>;
    pub fn open_read_only(path: &Path) -> Result<Self, ReadOnlyOpenError>;
}
```

Naming may become `open_ro` in implementation if that matches local style,
but the semantics are fixed.

## 6.1 D3: read-only open semantics

Open SQLite with read-only mode (`mode=ro` / read-only open flags), never
read-write-create. Do not create parent directories. Missing files return
a typed `Missing` result so the CLI can emit exit `0` with
`exists: false`. Do not call schema ensure helpers, `CREATE TABLE IF NOT
EXISTS`, `PRAGMA journal_mode=WAL`, or `backfill_session_chains`.

For WAL DBs, let SQLite read existing `-wal`/`-shm` state. If that cannot
be opened read-only, classify it as operational exit `1`, not schema
incompatibility. Do not set `immutable=1`, because that can ignore live
WAL content. Older/newer schemas may open successfully; compatibility
helpers map unsupported versions or structures to exit `14`.

## 6.2 Inspection helpers

Add helpers that do not mutate:

`StateDb::user_version(&self)` and
`StateDb::inspect_session_schema(&self)`. Exact struct names may be
adjusted in Phase 5, but `StateDb` owns SQLite inspection and the
CLI/probe module owns JSON plus exit-code mapping.

Required structural checks for Rev 1: tables `invocations`,
`session_turns`, `session_chains`, `session_chain_segments`; invocation
columns `session_id`, `session_capture_method`,
`resume_acceptance_status`, `resume_acceptance_evidence`; session-turn
columns `parent_turn_id`, `is_sidechain`, `is_compaction_boundary`; chain
columns `chain_id`, `created_at`, `last_used_at`, `model_name`; segment
columns `chain_id`, `provider_name`, `session_id`, `started_at`,
`ended_at`, `last_turn_id`, `transition_reason`; indexes
`idx_invocations_provider_session`, `idx_session_turns_session_lookup`,
`idx_segments_session`, `idx_segments_chain_active`.

# 7. Anti-scope

- **D7 decision:** do not retrofit `agents trace` or any other existing
  read-intent command to use `StateDb::open_read_only` in v1.
- No transcript locate/export/import/replace/normalization, no sibling
  Initiative 06 command implementation, no provider spawn/auto-resume/auth
  refresh/quota refresh/balancer/setup/discovery/scan/`turn_script`.
- No config edits, no `migrate-config` coupling, no DB repair/migration,
  no version stamping/backfill from the probe, no `--state-db` override,
  no GUI state DB support, and no compatibility promise for third-party
  direct writes to `state.db`.

# 8. Side-effect contract

`agents session schema-probe` must not create the state directory or DB
file; mutate SQLite schema/data/PRAGMAs; run backfill, migration,
provider, quota, setup, discovery, scan, or locator code; read or touch
provider transcripts; edit config; or emit invocation/telemetry/cache
state.

Permitted observations: resolve the default DB path, check DB file
existence, open SQLite read-only, read `PRAGMA user_version`,
`sqlite_master`, table/index metadata, and let SQLite read existing
WAL/shm sidecar state for a read-only snapshot.

# 9. Test-intent track

## 9.1 Test-intent track

| Change risk or verification risk | Intended behavior / acceptance condition | Level | Fixture source / application point | Assumption link | Expected observable signal | Residual risk |
| --- | --- | --- | --- | --- | --- | --- |
| D1 `PRAGMA user_version` is authoritative | DB with `user_version = 3` and required structures reports `schema_version: 3`; DB with `user_version = 0` exits `14` when file exists; mutating migration path stamps current version, probe does not. | particular-integration | Temp SQLite DB fixtures with explicit PRAGMA values; separate mutating-open fixture for stamping behavior. | A2 | Exit `0` or `14` as specified; stdout/stderr JSON values match PRAGMA; file mtime unchanged for probe. | Does not prove every future migration remembered to bump the constant. |
| D1 existing migration assignments | Fixtures representing version 2 (Initiative 04 shape) fail as older than min; fixtures representing version 3 pass when structures are present. | component | Hand-seeded temp DBs matching required table/column/index subsets. | A2, A6 | Version 2 exits `14`; version 3 exits `0`. | Structural fixtures may not cover all legacy data combinations. |
| D2 hardcoded feature enumeration | Feature map contains the exact Rev 1 keys and values; no clap-only command appears automatically. | unit | Pure function test for feature map. | A3 | `session_schema_probe: true`; future siblings false until implemented. | Does not prove future PRs update the map; review/test-intent must carry that forward. |
| D3 read-only open has no schema/backfill side effects | Probe against legacy DB does not create missing tables, add missing columns, set WAL mode, stamp `user_version`, or populate chains. | particular-integration | Temp DB seeded with old `session_turns` only; snapshot schema, `PRAGMA user_version`, row counts, and mtime before/after. | A1 | Exit `14`; before/after schema and row counts identical; no chain rows created. | Filesystem mtime granularity may need content-based checks. |
| D3 WAL read behavior | Existing WAL-mode DB can be read when SQLite can access sidecars; inaccessible WAL/shm state maps to operational exit `1`. | particular-integration | Temp WAL DB fixture plus permission-adjusted sidecar/directory case where supported by OS. | A1 | Success for readable WAL; exit `1` with `state-open-failed`/`state-inspect-failed` for inaccessible case. | Permission behavior can vary by platform; unsupported chmod cases become documented residuals. |
| D4 `safe_for_import_replace` predicate | Compatible DB with `session_import_replace: false` reports `safe_for_import_replace: false`; when fixture toggles import-replace and pause flags true, predicate follows schema/storage conditions. | unit + component | Pure predicate test plus report-builder fixture. | A3, A6 | Boolean is true only for all §3.4 conditions. | Does not test actual future import-replace implementation. |
| D5 storage vocabulary stability | Probe emits `["claude_code","codex_session","other"]` exactly; no internal `codex` config tag appears. | unit | Pure enum/serialization test. | A5 | JSON array matches stable public vocabulary. | Does not prove locate/export use the same Rust type if branches merge differently. |
| D6 exit mapping for missing DB | Missing default DB exits `0` and reports `exists: false`, version `0`, all required structures false, safe false. | end-to-end | Isolated `XDG_DATA_HOME` or data-dir fixture with no state file. | A4 | Exit `0`; stdout JSON; no directory or DB file created. | Platform-specific data-dir resolution may need fixture control. |
| D6 exit mapping for unreadable/present invalid DB | Present unreadable or non-SQLite file exits `1`, not `14`. | end-to-end | Temp data-dir fixture with unreadable file or invalid bytes. | A1 | Exit `1`; stderr JSON code `state-open-failed`; no stdout. | Permission simulation may differ on Windows. |
| D6 exit mapping for older/newer/missing structures | Older version, newer version, and required missing table/column/index all exit `14`. | particular-integration | Temp DB fixtures varying one incompatibility at a time. | A2, A6 | Exit `14`; stderr JSON code `schema-incompatible` with failed booleans. | Does not validate deep data integrity. |
| D7 no existing command retrofit | `agents trace` and other existing commands retain their current open paths and behavior in this PR. | unit/static | Grep/static assertion or review-gate checklist over call sites. | none | Diff only adds read-only usage to schema-probe path. | Static test can miss indirect refactors; Phase 8 review still checks diff. |
| Side-effect contract | Probe does not touch config, transcript files, adapter state dirs, quota/discovery tables, or invocation rows. | particular-integration | Temp config/data dirs with sentinel files and seeded DB rows; snapshot before/after. | A1, A4 | Sentinels unchanged; row counts unchanged; no new files except pre-existing SQLite sidecars. | Cannot prove absence of all possible OS metadata reads. |
| README examples remain truthful | Documented command, JSON fields, exit codes, and migration note match implementation. | unit/documentation check | README snippet or grep test if project convention supports it; otherwise Phase 6b residual entry. | none | README includes `session schema-probe`, schema fields, and exit `0/1/14`. | Documentation examples may not execute against real CLI. |

New fixture infrastructure is expected: a temp data-dir resolver harness,
SQLite schema fixture builders that can bypass `StateDb::open`, and
mtime/content snapshot helpers for no-side-effect assertions. These
belong in test support modules, not inline in test bodies.

# 10. README updates

Update `README.md` in the existing CLI style: add `session schema-probe`
to the subcommand list; add a "Session Schema Probe" section near
trace/resume/SQL inspection docs; document the §3 JSON fields, version
assignments `0`-`3`, exit codes `0`, `1`, `2`, `14`, missing-DB success
semantics, unversioned-DB refusal, and why `safe_for_import_replace` can
remain `false` until import-replace and pause-handshake are both compiled.
Keep SQL inspection framed as ad-hoc debugging, not the supported harness
compatibility path (`research/06-schema-probe-problem-map.md` §6 #1-2).

# 11. Supported-surface track

## 11.1 Supported-surface track

Deployment mode: local CLI binary only. No GUI/Tauri command, frontend
surface, daemon, or server endpoint.

Customer cohort: `agent-harness` is the primary consumer. Secondary
consumers are scripts that need a stable "is this binary/DB usable for
public session features?" answer without touching private SQLite details.

Adjacent public/user-reachable paths and blast-radius notes: `trace`,
`resume`, `repl --resume`, top-level `--resume`, `migrate-db`,
`migrate-config`, hidden `resume-list`, GUI/Tauri state commands, direct
CLI ingestion, and quota/discovery examples remain unchanged.
Schema-probe may tell users to run the migration path but never invokes
it, and v1 reports only CLI default state, not the GUI-path DB.

Migration path: existing DBs have `user_version = 0` until a new mutating
schema path stamps the current version after ensuring schema. Harnesses
treat exit `14` as refusal and either ask the user to run the documented
migration/open path or decline the write-capable operation.

Rollback path: uninstall/revert the binary or avoid the new subcommand.
The probe writes no durable state. A stamped `user_version` is ignored by
older binaries; no compatibility shim is added.

Observability: success stdout JSON and structured stderr JSON errors are
the entire surface. No telemetry, invocation rows, trace rows, quota
records, or transcript/cache files are emitted.

# 12. Implementation residuals

Known residuals Phase 4 should evaluate:

- Current-structure DBs with `user_version = 0` are refused until a
  mutating migration/open path stamps version `3`.
- Compatibility is structural plus versioned; it does not prove every
  `session_turns` row has complete chain membership or repair the
  Initiative 05 partial-chain skip condition.
- GUI state DB divergence remains out of scope in v1.
- `binary.commit` may be `"unknown"` when no build-time commit is
  embedded.
- Storage vocabulary is duplicated locally if 06-locate has not landed;
  reuse a shared public enum if Phase 5 finds one.
- Future sibling PRs must update the hardcoded feature map when their
  command ships; this proposal adds no stubs for them.

# 13. Cross-feature constraint compliance checklist

| Constraint | Compliance | Citation / note |
| --- | --- | --- |
| Shared error-code namespace uses `14` for schema-incompatible. | Yes | Namespace defined at `initiatives/06-session-override-contract.md:106-122`; mapping in §5. |
| Ownership resolution reuses `StateDb::resolve_resume`; no second ownership path. | Not applicable | Schema-probe does not resolve a session. It does not add an alternate resolver. |
| Lock observation applies to import-replace/migration/resume paths once pause-handshake lands. | Not applicable in v1 | Schema-probe is read-only. `safe_for_import_replace` requires pause-handshake before it can be true (§3.4). |
| Read-only `StateDb` open variant lands in 06-schema-probe. | Yes | Initiative assigns this to schema-probe (`initiatives/06-session-override-contract.md:118-120`); API in §6. |
| No auto-resume. | Yes | §7 and §8 forbid resume/provider execution. |
| No provider spawn. | Yes | §7 and §8 forbid provider commands. |
| No quota refresh. | Yes | §7 and §8 forbid quota refresh/scripts. |
| No config edits. | Yes | §7 and §8 forbid config writes. |
| No coupling to `migrate-config`. | Yes | §7 and §11 keep migration-config separate. |
| 06-schema-probe exposes feature flags so harness can gate later sibling adoption. | Yes | D2/§3.2 defines hardcoded compiled feature flags; future siblings update the map when they land. |
| 06-schema-probe provides the read-only compatibility surface before export/import-replace writes. | Yes | §1.2 and §11 describe harness refusal before future write-capable operations. |
