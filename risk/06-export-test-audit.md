# 06-export — Test Audit Gate
Verdict: **PASS-WITH-FINDINGS**

Scope: run against tip `fc59558` (`risk(06-export): Phase 6 process-tree audit PASS-WITH-ADVISORY`).
Comparison basis: `git diff main..06-export`.

## Inputs Read

- `~/ai/workflows/pr-review.md` — Test Audit rules and R2 firstness routing.
- `research/06-export-contract.md` — canonical Step 6a contract, T1-T9.
- `proposals/06-export.md` Rev 2.
- `research/06-export-problem-map.md`.
- `risk/06-export-supported-surface.md`.
- `risk/06-export-audit-history.md`.
- `risk/06-export-process-tree-audit.md`.
- Actual diff and files under `src-tauri/tests/`, `src-tauri/src/session_export/`, and `src-tauri/src/main.rs`.

`risk/06-export-test-residuals.md` does not exist. That is acceptable for the completed T1-T9 cells, but several proposal §9 rows are neither directly tested nor documented as formal residuals; see findings.

No `reports/report-index.md` bundle is present or required for this non-UI CLI/Rust test surface.

## Current Verification

Focused test run at `fc59558`:

```text
cargo test --manifest-path src-tauri/Cargo.toml --test initiative_06_export
```

Result: **PASS** — `initiative_06_export` 11/11 passed.

Audit history also records a prior full `cargo test --manifest-path src-tauri/Cargo.toml` pass after CodeRabbit pass 1. I did not rerun the full crate suite for this gate.

## Diff / Test-Edit Check

- Test files in the PR diff are additive relative to `main`: `src-tauri/tests/initiative_06_export.rs`, `src-tauri/tests/fixtures/initiative_06_export.rs`, and `src-tauri/tests/fixtures/mod.rs`.
- Tests were edited after the Step 6b test-writer commit: `git diff 4eae35c..06-export -- src-tauri/tests src-tauri/tests/fixtures` is non-empty.
- The post-Step 6b edits are documented in `risk/06-export-audit-history.md` as CodeRabbit pass 1 repairs: computed source offsets/hashes from fixture bytes and removed unused compaction DB seeding.
- I did not find assertion weakening in those edits. The offset/hash changes reduce stale literals, and the compaction fixture cleanup removes unused fixture state rather than broadening expected output.

## Phase 6 Firstness

Process-tree audit status: **PASS-WITH-ADVISORY**.

`risk/06-export-process-tree-audit.md` verifies separate Step 6b and Step 6c invocations, Step 6b test outputs, Step 6c read evidence before product-code mtimes, and passing Step 6c test evidence. The remaining advisory is that the Step 6b output-index provenance fields were repaired after Step 6c completion. That does not create a blocking firstness route for the current tests.

The transient `.tmp/phase6` companion files are not present in this review worktree, so this gate consumes the checked-in process-tree audit report as the provenance authority rather than revalidating those temporary artifacts directly.

## T1-T9 Mapping

| ID | Status | Test evidence |
| --- | --- | --- |
| T1 | covered | `export_claude_session_emits_canonical_jsonl_records` spawns the CLI, uses `--format canonical-jsonl`, checks exit 0, compact JSONL, all 8 fields, provider/session/source metadata, and Claude user/assistant roles. |
| T2 | covered with finding | `export_codex_session_emits_canonical_jsonl_records` covers Codex success shape and storage metadata. It does not cover non-text content variants; see F1. |
| T3 | covered | `canonical_reader_source_metadata_matches_jsonl_preimage` checks LF, CRLF, whitespace line skipping, final unterminated line, byte offsets, and SHA-256. |
| T4 | covered with finding | `canonical_reader_preserves_provider_jsonl_order` checks source-file order. Proposal D5's timestamp-regression failure case is missing; see F2. |
| T5 | covered with finding | `canonical_reader_emits_unsupported_record_placeholders` covers a safe unsupported system-like Claude record. Tool/tool-result and unsafe unsupported policy coverage is incomplete; see F1. |
| T6 | covered | `export_malformed_transcript_exits_15_without_partial_stdout` covers malformed mid-stream JSONL, exit 15, stderr JSON, and empty stdout. |
| T7 | covered with finding | `export_does_not_mutate_state_rows_transcript_or_config` snapshots table counts, transcript bytes/mtime, and config bytes. Broader forbidden side effects are not asserted; see F4. |
| T8 | covered | `canonical_reader_emits_live_transcript_from_latest_compaction_boundary` covers latest Claude `isCompactSummary` cutoff and boundary inclusion. T1/T2 also cover no-boundary full export for simple Claude/Codex fixtures. |
| T9 | covered | Missing, ambiguous, and unsupported-storage resolver errors map to exits 10, 11, and 12 with stderr JSON and empty stdout. |

## Fixture Externality

Fixture state is externalized in `src-tauri/tests/fixtures/initiative_06_export.rs`. Test bodies call named fixture builders and do not inline temp DB/config/transcript setup. That matches the dedicated fixture-file pattern for this slice.

The fixture module owns temp homes, model/provider/session config writers, locator scripts, SQL chain/segment seeders, transcript staging, read-only snapshots, and JSONL/error parsing helpers.

## Risk Annotations

Every current test carries risk, level, source, observable, and residual comments. The comments are useful and mostly map cleanly to T1-T9.

Two comments mark untested behavior as a local residual (`timestamp regression` and invalid UUID being outside T9), but those gaps are not backed by `risk/06-export-test-residuals.md` or a proposal non-applicability decision. They remain ordinary fix-pass findings below.

## Findings

### F1 — MEDIUM — Proposal D2 content variants are not covered

Proposal Rev 2 requires typed content coverage for text, system, tool call, and tool result records (`proposals/06-export.md:124`, `proposals/06-export.md:131`, `proposals/06-export.md:132`) and its test-intent row requires representative Claude/Codex fixtures for those shapes (`proposals/06-export.md:363`). The current tests only exercise simple text user/assistant content and one unsupported system placeholder:

```rust
// src-tauri/tests/initiative_06_export.rs:41
assert_eq!(record["unsupported_record"], false);
assert!(record["content"].as_array().unwrap().len() >= 1);
```

```rust
// src-tauri/tests/initiative_06_export.rs:168
assert_eq!(records.len(), 2);
let unsupported = records
    .iter()
    .find(|record| record.turn_id == "claude-system-1")
    .expect("unsupported system record should be present");
```

No test fixture contains `tool_call`, `tool_result`, or equivalent native provider tool events. A regression that flattens or drops non-text content would not be caught. Add component fixtures for at least one supported provider-native tool call/result shape, or record a deliberate residual/non-applicability decision if v1 is narrower than proposal D2.

### F2 — MEDIUM — Timestamp-regression failure is named in the proposal but untested

Proposal D5 says parsers validate timestamps and exit `15` on regression (`proposals/06-export.md:209`, `proposals/06-export.md:212`), and the test-intent row requires increasing/equal timestamps plus a regressing-timestamp malformed result (`proposals/06-export.md:368`). The current T4 test only verifies order for an increasing fixture:

```rust
// src-tauri/tests/initiative_06_export.rs:148
let records = read_canonical_transcript(&fixture.metadata).unwrap();

let turn_ids = records
    .iter()
    .map(|record| record.turn_id.as_str())
    .collect::<Vec<_>>();
assert_eq!(turn_ids, vec!["claude-turn-1", "claude-turn-2"]);
```

The nearby comment explicitly says regression is not separately fuzzed (`src-tauri/tests/initiative_06_export.rs:143`), but there is no residual artifact. Add a component test that a regressing timestamp returns `MalformedTranscript` and a CLI test if the observable exit-code mapping is considered part of D5.

### F3 — LOW — CLI negative surface and documentation-check rows are incomplete

The contract and proposal require the negative CLI surface: unsupported `--format`, missing/bare `agents session`, missing `session export` id, and invalid UUID exit `2` (`research/06-export-contract.md:13`, `research/06-export-contract.md:14`, `proposals/06-export.md:99`, `proposals/06-export.md:100`, `proposals/06-export.md:178`, `proposals/06-export.md:180`). Current tests cover only the valid explicit format in T1 and default format in T2; T9 explicitly leaves invalid UUID outside that group:

```rust
// src-tauri/tests/initiative_06_export.rs:19
let output = prepared
    .fixture
    .run_export(&prepared.session_id, &["--format", "canonical-jsonl"]);
```

```rust
// src-tauri/tests/initiative_06_export.rs:240
/// Risk: T9: resolver missing-session errors are remapped to a generic operational failure.
/// Level: particular-integration.
/// Source: contract section 8 row T9; contract sections 4 and 5.
/// Observable: unknown well-formed UUID exits 10 with stderr code session-not-found and empty stdout.
/// Residual: invalid UUID exit 2 is part of the CLI parse path, not this resolver mapping group.
```

Proposal §9 also includes a README documentation check row (`proposals/06-export.md:371`), and §10 requires README updates for synopsis, JSONL schema, source hash semantics, compaction behavior, and exit codes (`proposals/06-export.md:381`). The actual diff has no `README.md` change and no residual explaining why docs are deferred.

Add focused CLI negative tests and either update README or record an explicit non-applicability/residual for the documentation check.

### F4 — LOW — Read-only test covers the named rows/files but not every forbidden side effect

T7 snapshots key state rows, transcript bytes/mtime, and config bytes:

```rust
// src-tauri/tests/fixtures/initiative_06_export.rs:289
pub fn snapshot_read_only_state(&self, transcript_path: &Path) -> ReadOnlySnapshot {
    let conn = self.conn();
    let mut table_counts = BTreeMap::new();
    for table in [
        "invocations",
        "session_turns",
        "session_chains",
```

The contract also forbids provider commands, quota refresh, auth flow, migration, scans, telemetry/invocation rows, and transcript mutation (`research/06-export-contract.md:158`). The fixture contains a provider command string that should not run, but it is not marker-backed; `quota_script_marker` support exists but the read-only fixture passes `None`:

```rust
// src-tauri/tests/fixtures/initiative_06_export.rs:153
let quota_script = quota_script_marker
    .map(|path| format!("quota_script = \"printf touched > {}\"\n", path.display()))
    .unwrap_or_default();
```

```rust
// src-tauri/tests/fixtures/initiative_06_export.rs:411
pub fn cli_read_only_fixture() -> PreparedExport {
    let prepared = cli_claude_export_fixture();
    prepared
        .fixture
        .seed_provider_quota_exhausted(CLAUDE_PROVIDER);
```

This is non-blocking because the row/file snapshot covers the primary read-only observable, but it leaves a weak spot for regressions that invoke provider/quota/turn scripts without changing those rows. Add marker files for forbidden scripts and assert they remain absent, while preserving the allowed transcript-locator `STATE_DIR` carve-out.

## Over-Assertion / Coupling

No blocking over-assertion found. The JSON field-set checks target the contract's stable 8-field surface, source preimage tests derive offsets from fixture bytes, and CLI tests exercise the binary rather than internal command functions.

Fixture coupling is acceptable for a CLI/config/state feature: direct SQL setup is contained in the fixture module and seeds public state concepts needed for resolver ownership.

## Supported-Surface Interaction

No Test Audit finding collapses the approved supported-surface net-value case. There is no Supported-Surface Verification finding to forward.

Final determination: **PASS-WITH-FINDINGS**. The findings are ordinary fix-pass items, not firstness blockers.
