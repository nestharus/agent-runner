# WU-15-01 Phase 8 Test Audit

## Verdict

LOW

## Findings

No fix-pass test-audit findings.

Evidence supporting the verdict:

- Intent-first evidence is present and internally consistent. The Phase 6 process audit records a separate Step 6b test writer and Step 6c code writer, with Step 6b completing first and Step 6c consuming the Step 6b output paths before implementation (`risk/15-empty-bodies-ref-process-tree-audit-phase6.md:26`, `risk/15-empty-bodies-ref-process-tree-audit-phase6.md:27`, `risk/15-empty-bodies-ref-process-tree-audit-phase6.md:29`, `risk/15-empty-bodies-ref-process-tree-audit-phase6.md:49`).
- The Step 6b output index maps every T1-T12 item to a named risk, selected level, source, emitted test path, observable identifier, and fixture source (`tmp/scratch/wu-15-01/phase6/step6b-output-index.md:17` through `tmp/scratch/wu-15-01/phase6/step6b-output-index.md:28`). It also explicitly records that no named T1-T12 residual was left unverified (`tmp/scratch/wu-15-01/phase6/step6b-output-index.md:30`).
- The four RCA harnesses demonstrate RED before implementation and GREEN afterward. RC-1 RED/GREEN is recorded at `tmp/scratch/wu-15-01/phase6/step6b-output-index.md:95` and `tmp/scratch/wu-15-01/phase6/rc1-green-run.log:6`; RC-2 at `tmp/scratch/wu-15-01/phase6/step6b-output-index.md:130` and `tmp/scratch/wu-15-01/phase6/rc2-green-run.log:6`; RC-3 at `tmp/scratch/wu-15-01/phase6/step6b-output-index.md:165` and `tmp/scratch/wu-15-01/phase6/rc3-green-run.log:6`; RC-4 at `tmp/scratch/wu-15-01/phase6/step6b-output-index.md:201` and `tmp/scratch/wu-15-01/phase6/rc4-green-run.log:6`.
- The RCA tests themselves encode contract-visible signals rather than implementation self-comparison: schema type/nullability (`src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs:10` through `src-tauri/tests/empty_bodies_ref_rca/rc1_schema_contract.rs:38`), ingest DB retrieval (`src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs:14` through `src-tauri/tests/empty_bodies_ref_rca/rc2_ingest_body_payload.rs:45`), CLI export success plus DB-source sentinel (`src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs:11` through `src-tauri/tests/empty_bodies_ref_rca/rc3_export_db_source.rs:36`), and trace inline `body_state`/content (`src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs:13` through `src-tauri/tests/empty_bodies_ref_rca/rc4_trace_inline_transcript.rs:66`).
- T5-T12 are encoded with risk annotations and observable assertions: T5 legacy migration/quota coexistence (`src-tauri/src/state/db.rs:4653` through `src-tauri/src/state/db.rs:4714`), T6 encoding edge cases (`src-tauri/src/sessions/mod.rs:444` through `src-tauri/src/sessions/mod.rs:471`), T7 JSONL priority (`src-tauri/src/session_export/mod.rs:605` through `src-tauri/src/session_export/mod.rs:669`), T8 DB fallback plus `db://session_turns/` source path (`src-tauri/src/session_export/mod.rs:673` through `src-tauri/src/session_export/mod.rs:725`), T9 import-replace round trip (`src-tauri/src/session_replace/mod.rs:1289` through `src-tauri/src/session_replace/mod.rs:1412`), T10 mixed legacy/new trace and empty arrays (`src-tauri/src/trace/mod.rs:1124` through `src-tauri/src/trace/mod.rs:1199`, `src-tauri/tests/pr_b_trace_integration.rs:181` through `src-tauri/tests/pr_b_trace_integration.rs:204`), T11 Claude adapter bodies (`src-tauri/tests/scripts/claude_code_turns_body.rs:23` through `src-tauri/tests/scripts/claude_code_turns_body.rs:63`), and T12 Codex adapter bodies (`src-tauri/tests/scripts/codex_turns_body.rs:23` through `src-tauri/tests/scripts/codex_turns_body.rs:66`).
- The trace `null` placeholder deletion is justified by the AC-4 `body_state` replacement: the integration trace test now asserts empty arrays for zero-turn nodes (`src-tauri/tests/pr_b_trace_integration.rs:181` through `src-tauri/tests/pr_b_trace_integration.rs:204`), while the component trace test asserts per-row `missing` and `stored` body states (`src-tauri/src/trace/mod.rs:1178` through `src-tauri/src/trace/mod.rs:1199`).
- Fixtures are externalized at the Rust helper/module level where the suite needs reusable setup: the RCA harness uses `RcaFixture` for DB/config/script/CLI setup (`src-tauri/tests/empty_bodies_ref_rca/mod.rs:24` through `src-tauri/tests/empty_bodies_ref_rca/mod.rs:202`), and adapter tests execute the actual scripts through helper runners rather than stubbing parser logic (`src-tauri/tests/scripts/claude_code_turns_body.rs:13` through `src-tauri/tests/scripts/claude_code_turns_body.rs:20`, `src-tauri/tests/scripts/codex_turns_body.rs:13` through `src-tauri/tests/scripts/codex_turns_body.rs:20`).
- Non-blocking concurrency note: Round 6's `R6-N01` documents parallel Rust test interference from process-wide XDG env mutation, while the Phase 6 process audit records the single-threaded full Rust gate as passing and treats the caveat as documented, not hidden (`/home/nes/projects/agent-runner/trunk/tmp/scratch/wu-15-01/audit-history.md:39`, `risk/15-empty-bodies-ref-process-tree-audit-phase6.md:57`). The new CLI harness paths use per-command env isolation (`src-tauri/tests/empty_bodies_ref_rca/mod.rs:195` through `src-tauri/tests/empty_bodies_ref_rca/mod.rs:200`); inline unit helpers that must alter process env use the existing `env_lock` pattern (`src-tauri/src/session_export/mod.rs:554` through `src-tauri/src/session_export/mod.rs:566`, `src-tauri/src/session_replace/mod.rs:1198` through `src-tauri/src/session_replace/mod.rs:1219`).

Additional local verification during this audit:

- `RUST_TEST_THREADS=1 cargo test --test empty_bodies_ref_rca --test scripts --no-fail-fast`
- `RUST_TEST_THREADS=1 cargo test body --no-fail-fast`
- `RUST_TEST_THREADS=1 cargo test bodies --no-fail-fast`
- `RUST_TEST_THREADS=1 cargo test empty_arrays --no-fail-fast`

## LOW Justification

LOW because the actual diff carries separate firstness evidence, risk-annotated T1-T12 coverage, verified RCA RED-to-GREEN harnesses, observable contract assertions, and only a documented non-blocking parallel-test isolation caveat.
