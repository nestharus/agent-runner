# 06-schema-probe — Phase 8 Supported-Surface PR Verification

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Phase 4 Round 2 cleared this proposal at LOW. Phase 8 verifies the
implementation against §1.1 assumptions, §11.1 supported-surface claims,
and the read-only side-effect contract using the actual diff at
`src-tauri/`. All six assumptions still HOLD; §11.1 claims remain
accurate; the read-only contract is verifiable in code. One
implementation/proposal drift exists on the stderr error-code names
(§5 vs. main.rs) — it does not violate §11.1 (which promises only
"structured stderr JSON errors"), but is carried forward as a contract
finding for the audit-track review.

## Concern 1 — Assumption walk on §1.1 (A1-A6)

| ID | Verdict | PR-state evidence |
| --- | --- | --- |
| A1 read-only open feasible | **HOLDS** | `StateDb::open_read_only` (`src-tauri/src/state/db.rs:676-706`) opens with `OpenFlags::SQLITE_OPEN_READ_ONLY` only; calls no `ensure_*_schema`, no `backfill_session_chains`, no `journal_mode` set, no `create_dir_all`. `inspect_schema` (`src-tauri/src/schema_probe/mod.rs:139-181`) reads only `PRAGMA user_version`, `sqlite_master`, `PRAGMA table_info`, `PRAGMA index_info`. Invalidator (compatibility requires mutation) does not fire. |
| A2 `PRAGMA user_version` source | **HOLDS** | `schema_version` is sourced from `PRAGMA user_version` and mirrored into `user_version` (`mod.rs:140-142, 192-194`). Constants `CURRENT_SCHEMA_VERSION = 3`, `MINIMUM_SUPPORTED_SCHEMA_VERSION = 3` (`mod.rs:7-8`) match §3.1. |
| A3 compiled features binary-bound | **HOLDS** | `feature_map()` (`mod.rs:217-225`) is a hardcoded `BTreeMap`; matches the §3.2 Rev 1 list exactly (`session_locate:false`, `session_export:false`, `session_import_replace:false`, `session_pause_handshake:false`, `session_schema_probe:true`). No clap introspection. |
| A4 CLI default DB v1 target | **HOLDS** | `StateDb::default_path()` (`db.rs:713-717`) returns `data_dir/oulipoly-agent-runner/state.db`, identical to the prior `open_default` path. No `--state-db` flag is accepted (`main.rs` `SessionSubcommands` is unit-arm only). |
| A5 reviewable parallel to 06-locate | **HOLDS** | `supported_storage_types()` (`mod.rs:227-232`) is locally defined as `["claude_code","codex_session","other"]`. It does not import a shared `SessionStorageType` from a locate module that has not landed. |
| A6 structural+version sufficient | **HOLDS** | `inspect_schema` checks tables, columns, and required-index column-order via `required_index_matches`/`index_definition` (`mod.rs:355-396`); the §3.4 predicate is gated on schema range plus structural booleans. No data-invariant or chain-completeness check is invoked. |

**Termination signal #1 (`invalidated-assumption`) — DOES NOT FIRE.**

## Concern 2 — Net value vs. problem-map §6

The probe answers all of: schema version (`PRAGMA user_version`),
binary identity (`binary.{name,version,commit}`), compiled feature
support (`features` map), and import-replace safety
(`safe_for_import_replace` is `false` because `session_import_replace`
and `session_pause_handshake` are both `false` — exactly what §3.4
predicts in v1). Missing-DB success path
(`schema_probe::missing_report`, `mod.rs:103-119`) emits canonical
keys with `false` and never creates the parent directory; verified by
`schema_probe_missing_db_emits_non_mutating_success_report`
(`tests/initiative_06_schema_probe.rs`) asserting the data-app dir is
absent post-probe. WS1 / WS2 closure from Rev 2 is realized in code:
`tables` is flat, `required_columns` and `required_indexes` are
nested, and `ReadOnlyOpenError` ships with all five variants
(`db.rs:52-58`).

Net value relative to baseline: **positive**. No problem-map gap is
re-opened.

**Termination signal #2 (`non-positive-value`) — DOES NOT FIRE.**

## Concern 3 — Adjacent path preservation (D7)

Diff inspection of `src-tauri/src/main.rs`: only
`Subcommands::Session { command }`, `SessionSubcommands::SchemaProbe`,
`run_session_schema_probe`, `probe_error_message`, and
`write_json_error` are added. `run_trace_command`, `run_repl_command`,
`run_resume_command`, `run_resume_list`, `run_migrate_db`,
`run_migrate_config`, and the top-level `--resume` routing remain
untouched. `StateDb::open_default` is refactored to delegate to the
new `default_path()` helper but resolves to the same path string and
still calls the mutating `Self::open(&db_path)`; no existing read-intent
command is retrofitted to `open_read_only`. Hidden `resume-list` is
unchanged. **PRESERVED.**

## Concern 4 — §11.1 claims accuracy

| §11.1 claim | PR state |
| --- | --- |
| Deployment mode: local CLI binary only; no GUI/Tauri command | **Accurate.** No new `tauri::command` is added; no frontend file changes; only the CLI dispatcher in `main.rs` gains a `Session` arm. |
| Adjacent paths (`trace`, `resume`, `repl --resume`, top-level `--resume`, `migrate-db`, `migrate-config`, hidden `resume-list`, GUI/Tauri state commands, direct CLI ingestion) unchanged | **Accurate.** Diff confirms (Concern 3). |
| v1 reports CLI default state, not GUI-path DB | **Accurate.** `default_path()` resolves the CLI default; no GUI path resolver is introduced. |
| Migration path: existing DBs report `user_version = 0` until a future mutating-open PR stamps the current version | **Accurate.** Probe never stamps; `open_read_only` does not call any `ensure_*_schema` helper. Existing pre-Rev-1 DBs hit exit `14`. |
| Rollback: probe writes no durable state | **Accurate.** `open_read_only` opens with `SQLITE_OPEN_READ_ONLY`; verified by `open_read_only_preserves_existing_db_physical_snapshot` (mtime/size/sidecar snapshot before/after). |
| Observability: success stdout JSON and structured stderr JSON errors are the entire surface | **Accurate at §11.1 granularity.** Only `println!`/`eprintln!` with serialized JSON; no telemetry, invocation rows, trace rows, quota records, or transcript/cache files. |

## Concern 5 — Read-only contract verifiable in code

- **Open flags:** `Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)` (`db.rs:698-700`). No `SQLITE_OPEN_CREATE` or `SQLITE_OPEN_READ_WRITE`.
- **No directory creation:** `open_read_only` exits early with `Missing` when the file is absent; no `create_dir_all` exists in the read-only path or in `default_path`.
- **No schema-ensure:** `open_read_only` does not call any of `ensure_invocations_schema`, `ensure_session_turns_schema`, `ensure_session_chains_schema`, or `backfill_session_chains` (those remain on the mutating `open` path).
- **No PRAGMA mutation:** the only PRAGMAs read are `user_version`, `table_info`, `index_info`. No `PRAGMA journal_mode = WAL`, no `PRAGMA user_version = N` from the probe.
- **CLI dispatch:** `Subcommands::Session { command: SessionSubcommands::SchemaProbe }` routes only to `run_session_schema_probe`, which calls `schema_probe::run_schema_probe()` (`main.rs:462-481`). No fallthrough to a write path.
- **Side-effect tests:** `open_read_only_preserves_existing_db_physical_snapshot` and `open_read_only_missing_path_does_not_create_parent_directory` enforce physical no-op behavior end-to-end.

**Verifiable.** Contract holds in code.

## Findings carried forward

| Source | Status | Note |
| --- | --- | --- |
| Rev 1 #1 stamping-PR coordination | **Carried.** No mutating-open PR has stamped yet; harness sees exit `14` for unstamped current-structure DBs. Phase 5/6 hookpoint, not a Phase 8 blocker. |
| Rev 1 #2 `safe_for_import_replace` permanently `false` in v1 | **Carried.** `feature_map()` confirms; behavior matches §3.4 expectation. README check-in is the audit track's concern. |
| Rev 1 #3 D7 leaves locate's A6 caveat documentary | **Carried.** No locate retrofit was attempted, as required. |
| Rev 1 #4 storage-vocabulary duplication risk | **Carried.** Locally defined; reuse-if-present clause unexercised because locate has not landed. |
| Rev 1 #5 WAL/permission read variability | **Carried.** Tests gated `#![cfg(unix)]`; `WalSidecarError` exercised under chmod-0 sidecars. Platform variance is residualized. |
| **New (PR-only)** stderr error-code drift | **New finding, low severity.** §5 names two distinct codes (`state-open-failed`, `state-inspect-failed`) for exit `1`; `main.rs:466-471` collapses both into a single `operational-error` code. §11.1 promises only "structured stderr JSON errors" (no specific codes), so the supported-surface posture holds, but the harness contract in §5 is narrower than what shipped. **Hand off to audit/contract review** to decide between widening §5 or splitting the code in implementation. |

## Verdict

**LOW. No termination signal fires.** All six §1.1 assumptions hold
under the implemented diff; problem-map §6 net value is preserved;
adjacent CLI/GUI paths are untouched; §11.1 claims are accurate at the
granularity §11.1 makes them; and the read-only contract is mechanically
verifiable in `open_read_only` plus the probe call graph. The single
new finding (stderr error-code drift between §5 and `main.rs`) is a
contract-track concern, not a supported-surface block. Cleared from the
supported-surface track for Phase 8.
