# 06-schema-probe — Phase 8 Justification Review

**Verdict: LOW_CONCERN**

Scope: `git diff main..06-schema-probe -- src-tauri/` (1,653 lines, 8 files
in `src-tauri/`). Reviewed against `proposals/06-schema-probe.md` (Rev 2),
`research/06-schema-probe-contract.md`, and the accepted residuals in
`risk/06-schema-probe-audit-history.md`.

## Element-by-element justification

| Diff element | Why it's required | Source |
| --- | --- | --- |
| `src-tauri/build.rs`: `BUILD_COMMIT` env + `.git/HEAD`/active-ref rerun directives | Feeds the required `binary.commit` field; the runtime command must not invoke `git`. Rerun directives are directly the R1-F03 CodeRabbit fix so `BUILD_COMMIT` refreshes on amend/checkout. | proposal §3 (`binary.commit`), §4 step 3, audit-history Pass 1 R1-F03 |
| `src-tauri/src/lib.rs`: `pub mod schema_probe` | Required to expose the new module. | contract §2 |
| `src-tauri/src/main.rs`: `Subcommands::Session { command: SessionSubcommands }` + `SchemaProbe` arm + `run_session_schema_probe` + `probe_error_message` + `write_json_error` | Implements the `agents session schema-probe` CLI surface, the §5 exit-code mapping (`0`/`1`/`14`), and the stderr JSON error envelope. `probe_error_message` exhaustively covers each `ReadOnlyOpenError` variant — exactly the variant-mapping contract added in Rev 2 (R1-F02). | proposal §2, §4, §5, §6 variant mapping |
| `src-tauri/src/schema_probe/mod.rs` (new, 409 lines): `SchemaProbeReport`, `BinaryInfo`, `StateDbReport`, `FeatureMap`, `ProbeError`, `RequiredIndex`/`IndexDefinition`, `feature_map`, `supported_storage_types`, `safe_for_import_replace`, `inspect_schema`, `report_from_state_db`, `missing_report`, `required_index_matches`, `index_definition`, `quote_identifier` | One-to-one realization of the §3 JSON schema, §3.1–§3.4 D-decisions, §4 resolution flow, and §6.2 inspection helpers. `RequiredIndex { name, columns }` + `IndexDefinition` exist because Pass 1 R1-F05 required ordered column-list validation; this is not speculative. `quote_identifier` is the minimum sanitization required when interpolating table/index names into PRAGMA. The future-version branch (`> CURRENT_SCHEMA_VERSION` ⇒ incompatible) is the R1-F06 fix. | proposal §3, §3.1, §3.2, §3.3, §3.4, §4, §6.2, audit-history R1-F05/R1-F06 |
| `src-tauri/src/state/db.rs`: `ReadOnlyOpenError` enum, `classify_read_only_open_error`, `wal_path`/`shm_path`, `path_is_unreadable` (unix + non-unix), `StateDb::open_read_only`, `StateDb::default_path`, `pub(crate) fn connection` | Implements the §6 reusable read-only API. The five enum variants are the explicit R1-F02 closure. `path_is_unreadable` + sidecar pre-check classify `PermissionDenied` and `WalSidecarError` before SQLite collapses them into a generic open error — needed for the §9.1 D6 test obligations. `default_path` is a focused extraction so the probe can resolve the path without opening; `open_default` is rewired through it (no behavior change for callers). `connection()` is `pub(crate)` and only consumed by `inspect_schema`. | proposal §6 (variants table), §9.1 D3/D6, contract §2.1–§2.2 |
| `src-tauri/src/state/mod.rs`: re-export `BinaryInfo`, `FeatureMap`, `SchemaProbeReport`, `StateDbReport`, `ReadOnlyOpenError` | Public state-surface types per audit-history Pass 2 R2-F03 (intentionally part of `state` surface, kept as state-shaped output). Not drive-by. | audit-history Pass 2 R2-F03 |
| `src-tauri/tests/fixtures/initiative_06_schema_probe.rs` (518 lines) + `tests/fixtures/mod.rs` | Test-support module required by proposal §9.1 ("New fixture infrastructure is expected: a temp data-dir resolver harness, SQLite schema fixture builders that can bypass `StateDb::open`, and mtime/content snapshot helpers"). Each helper maps to a §9.1 / contract §7 row. `#![cfg(unix)]` is consistent with the WAL/permission tests' platform notes. | proposal §9.1 trailer; contract §7, §8 |
| `src-tauri/tests/initiative_06_schema_probe.rs` (379 lines) | Houses T1–T8 plus the D6 future-version and wrong-index regressions (R1-F05, R1-F06). Each `#[test]` carries a doc comment mapping to its risk row, level, source, and residual. | contract §7; audit-history Pass 1 |

## Drift / drive-by cleanup

None observed. The only refactor outside new code is `open_default` →
`open(default_path()?)`; that extraction is required for the probe to
resolve the path without opening the DB and has no behavior change. No
unrelated renames, formatting churn, or non-probe call-site edits.

## Speculative abstractions

None. `ReadOnlyOpenError`'s five variants are mandated by §6 (R1-F02
closure) — every variant is exercised by a T6 test. `RequiredIndex` /
`IndexDefinition` are mandated by R1-F05 (column-order validation). The
`Option<&Connection>` pattern in `required_column_map` /
`required_index_map` is the minimum reuse needed for the missing-DB
report; it is not generalized further.

## Behavior changes not required by the proposal

None. The implementation's `safe_for_import_replace` predicate enforces
all seven §3.4 conditions (including `session_pause_handshake` and the
exact storage-types vocabulary). The contract §3 lists a shorter set,
but the proposal is the authoritative source and §3.4 is what shipped.
T7 covers each condition's flip.

## Cleanup that should ship separately

None. The diff is additive against `main` and stays inside the §7
anti-scope: no retrofit of `agents trace`, no provider/quota/locator
work, no config edits, no migration code, no `--state-db` flag, no GUI
state DB.

## §10 README updates

**Flagged.** Proposal §10 requires README updates ("`session
schema-probe` to the subcommand list … document the §3 JSON fields,
version assignments `0`-`3`, exit codes `0`/`1`/`2`/`14`, missing-DB
success semantics, unversioned-DB refusal …"). `git diff main..06-schema-probe
-- README.md` is empty. Proposal §9.1's "README examples remain
truthful" row carries the residual escape hatch ("Phase 6b residual
entry"), and audit-history records no Phase 6 README work, so this is a
known accepted residual rather than missed scope. Recommend a follow-up
docs PR before harness consumers are pointed at the command.

## Summary

Every src-tauri change traces to a proposal section, contract row, or
acknowledged audit/CodeRabbit finding. No drive-by cleanup, no
speculative abstractions, no out-of-scope behavior changes. The single
gap is README §10, which is already a documented residual.
