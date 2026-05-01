# 06-locate — Test Audit Gate
Verdict: **PASS-WITH-FINDINGS**

Scope: re-run against tip `2605b37` (`docs(06-locate): README — agents session locate`).
Comparison basis: `git diff main..06-locate -- src-tauri/`.

## Inputs Read
- `~/ai/workflows/pr-review.md` — Test Audit rules and R2 firstness routing.
- `research/06-locate-contract.md` — canonical Step 6a contract, T1-T16.
- `proposals/06-locate.md` Rev 3.
- `research/06-locate-problem-map.md`.
- `risk/06-locate-supported-surface.md` Rev 3.
- `risk/06-locate-audit-history.md`.
- `risk/06-locate-process-tree-audit-r2.md`.
- Actual test diff and files under `src-tauri/tests/`.

`risk/06-locate-test-residuals.md` does not exist. That matches the handoff:
T1-T16 are fully verifiable and no residual-risk artifact is required.

## Current Verification
Focused test run at `2605b37`:
```text
cargo test --test initiative_06_locate --test session_metadata_component
```
Result: **PASS** — `initiative_06_locate` 10/10 passed;
`session_metadata_component` 26/26 passed.

The initial `-p agent-runner` command used a nonexistent package id and failed
before running tests; the corrected command above is the recorded evidence.

## Diff / Test-Edit Check
- Test files in the PR diff are additive only:
  `initiative_06_locate.rs`, `session_metadata_component.rs`,
  `fixtures/initiative_06.rs`, and `fixtures/mod.rs`.
- `git diff 9d8cfe3..HEAD -- src-tauri/tests src-tauri/tests/fixtures`
  is empty; post-test-write commits did not edit tests or fixtures.
- Current tip `2605b37` changes only `README.md` and
  `risk/06-locate-audit-history.md`.
- No assertion relaxation, baseline regeneration, coverage deletion, input
  narrowing, or risk-annotation removal is present.

## Phase 6 Firstness
Process-tree audit status: **PASS-WITH-ADVISORY**.

`risk/06-locate-process-tree-audit-r2.md` verifies Step 6a contract presence,
separate Step 6b test-writer invocation, Step 6b T1-T16 output index, Step 6c
REDO, and file-based Step 6c read evidence before product-code mtimes.

The prior Step 6c firstness violation `PTA-06-P6-001` is
`REPAIRED-VERIFIED`. R2 routing therefore lands in the complete-cell path:
ordinary Test Audit checks continue. The remaining advisory is prompt-output
precision only, not a blocking firstness route.

## T1-T16 Mapping
| ID | Status | Test evidence |
| --- | --- | --- |
| T1 | covered | CLI success test checks exit 0, empty stderr, all 8 fields, provider/session/chain/storage/transcript/mutable. |
| T2 | covered | Component ambiguity tests cover recent multi-chain `AmbiguousSession` and recency-collapse success. |
| T3 | covered | Unit mapping test covers Claude/Codex/None and serde names. |
| T4 | covered | CLI no-storage fixtures with present and absent locator both exit 12 and avoid `storage_type:"other"` success output. |
| T5 | covered | Mutable matrix covers all reachable success conditions, missing resume, quota ignored, missing transcript/workspace, no-storage, and missing active segment. |
| T6 | covered | Segmentless `session_turns` fixture returns `SessionNotFound`. |
| T7 | covered with finding | CLI rejects `--state-db` with exit 2; see F2. |
| T8 | covered | Unknown well-formed UUID exits 10 with `session-not-found`. |
| T9 | covered | Invalid UUID exits 2 with JSON error before default state dir creation. |
| T10 | covered | Component covers no locator, missing, relative, locator error; CLI covers locator error and `--json` error shape. |
| T11 | covered | Claude path-hash success returns canonical workspace and JSONL paths. |
| T12 | covered | Claude zero / one / multiple decomposition cases enforce exactly-one success. |
| T13 | covered | Codex `session_meta.payload.cwd` success returns canonical workspace and `CodexSession`. |
| T14 | covered | Codex missing meta, absent cwd, non-absolute cwd, missing cwd, and non-UTF-8 canonical cwd fail closed. |
| T15 | covered with finding | Row counts and transcript mtime unchanged after locate; see F1. |
| T16 | covered | CLI compact single-line JSON and component UTF-8 path/provider punctuation round trip. |

No T row requires non-applicability routing. No T row is left to a missing
residual artifact.

## Fixture Externality
Fixture state is externalized in `src-tauri/tests/fixtures/initiative_06.rs`.
The test bodies call named scenarios and do not inline temp-DB setup blocks.

The fixture module owns temp dirs, state DB open/connection helpers, config
writers, locator scripts, SQL seeders, transcript staging, Codex JSONL staging,
and read-only snapshots. That matches the dedicated fixture-file pattern for
this slice.

## Risk Annotations
Every test/group carries the required fields: T-id risk, explicit level,
contract/assumption source, observable signal, and residual. The fixture module
does not need repeated annotations because it is shared test infrastructure.

## Validator Level
Most selected levels are the cheapest reliable validators:
type mapping is unit-level; resolver/mutable/transcript/path/Codex/parser cases
are component-level; CLI stdout/stderr/exit-code and default-DB behavior are
particular-integration or end-to-end where process/environment behavior is the
observable.

One level-selection issue remains as F2.

## Findings
### F1 — LOW — T15 proves the named row but not every forbidden side effect
`locate_does_not_mutate_state_rows_or_transcript_file_after_open` snapshots
the contract row's named observables: row counts in `invocations`,
`session_turns`, `session_chains`, `session_chain_segments`,
`provider_quotas`, plus transcript mtime. That covers T15 as written.

The broader side-effect contract also forbids provider/auth/quota/turn commands
and config edits. The fixtures include strings such as
`provider-command-that-must-not-run`, but no marker assertion proves those
commands or scripts were not invoked. This is non-blocking because T15's
canonical observable is row counts plus transcript mtime, but it leaves a weak
spot for command-execution regressions that do not mutate those rows/files.

### F2 — LOW — T7 uses a heavier validator level than the contract selected
Contract T7 selected `unit` with a clap parser test. The emitted test
`locate_rejects_state_db_override_flag` is annotated as
`particular-integration` and spawns the compiled CLI. The observable is strong
and user-realistic, so coverage is acceptable, but it is not the cheapest
reliable validator named by the contract.

## Over-Assertion / Coupling
No blocking over-assertion found. JSON-shape assertions target the stable
required field set and compact-line formatting required by the contract.

Weaknesses are limited to F1 and F2. Fixture coupling is acceptable: tests use
the public metadata API or CLI and seed public state/config concepts; direct
SQL setup is contained in the fixture module.

## Supported-Surface Interaction
No Test Audit finding collapses the approved supported-surface net-value case.
There is no Supported-Surface Verification finding to forward.

Final determination: **PASS-WITH-FINDINGS**.
