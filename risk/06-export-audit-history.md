# Audit history — 06-export (`agents session export`)

## Purpose

Multi-round revise/review loop. Round 1 audit returned MEDIUM
(R1-F01: STATE_DIR mkdir contract conditional); Rev 2 closes by
pinning the clause in §8.

## Round 1

- Verdicts: audit MEDIUM, scope LOW, shortcut LOW, supported-surface LOW (termination none).
- Findings:
  - R1-F01 (MEDIUM, audit): §8 deferred read-only locator clause to Phase 5; pin in proposal contract.
- Decision: continue. Dispatch Rev 2 with §8 STATE_DIR mkdir clause (matching 06-locate).

## CodeRabbit Pass 1

- Findings: 6 total; 5 applied, 1 skipped.
- Applied:
  - R2-F01: computed source byte offsets from fixture bytes instead of hardcoding them in the T3 parser component test.
  - R2-F02: documented why missing `session_id`/`native_type` records are skipped while mismatched `metadata.session_id` is malformed.
  - R2-F04: computed expected source hashes from fixture constants instead of storing stale-prone literals.
  - R2-F05: replaced the handwritten SHA-256 implementation with a direct `sha2` dependency.
  - R2-F06: removed unused compaction DB seeding from the component fixture.
- Skipped:
  - R2-F03: malformed `sessions.toml` should not fail export in this branch because `research/06-export-contract.md` §4 explicitly specifies `SessionsConfig::load(...).unwrap_or_default()` matching resume.
- Tests: PASS — `cargo test --manifest-path src-tauri/Cargo.toml`.
- Determination: continue.

## CodeRabbit Pass 2

- Findings: 4 total; 0 applied, 4 skipped.
- Skipped:
  - R3-F01: `sha256_hex` preallocation suggestion is a micro-optimization after the direct `sha2` fix, not a material risk.
  - R3-F02: fixture `conn()` comment request is explanatory churn for local test helper setup.
  - R3-F03: avoiding a clone in `parse_stdout_jsonl` is negligible test-helper churn.
  - R3-F04: `table_count` identifier interpolation is private test code fed by hardcoded table names from the snapshot helper, not user input.
- Tests: not rerun; no edits applied in pass 2 after the pass 1 test run.
- Determination: converge (`ALL_CHURN`).
