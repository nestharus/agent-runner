# Expected Process Manifest — 06-locate Phase 6

Workflow: workflow library `workflows/implementation-pipeline.md` Phase 6
(test/code separation).

Subtree under audit: Phase 6 work for feature 06-locate (Step 6a
contract → Step 6b tests → Step 6c code).

## Expected nodes

### Node `step6a-contract`

- id: `step6a-contract`
- required: `true`
- operator_or_role: orchestrator-owned contract drafting
- model: `n/a` (orchestrator wrote the contract directly; no agent invocation expected)
- parent: `root`
- prompt: `n/a` (orchestrator)
- log: `n/a` (orchestrator)
- expected_outputs:
  - `research/06-locate-contract.md` (committed at `c85744d`)
- questions_allowed: `false`
- question_artifacts: `none`
- answer_artifacts: `none`
- continuation_evidence: `n/a`
- blocking_if_missing: `true`
- notes: orchestrator owns Step 6a per workflow library `workflows/implementation-pipeline.md` Phase 6 rules. The contract is committed at git ref `c85744d` and updated at `737063b` (clarification of `SessionStorageType::Other` v1 reachability after Step 6b NEEDS_INPUT).

### Node `step6b-test-writer`

- id: `step6b-test-writer`
- required: `true`
- operator_or_role: test writer
- model: `gpt-high`
- parent: `root`
- prompt: `.tmp/06-locate-step6b-tests.md`
- log: `.tmp/06-locate-step6b-tests.log`
- expected_outputs:
  - `src-tauri/tests/initiative_06_locate.rs`
  - `src-tauri/tests/session_metadata_component.rs`
  - `src-tauri/tests/fixtures/initiative_06.rs`
  - `src-tauri/tests/fixtures/mod.rs`
  - `.tmp/phase6/step6b-output-index.md`
- questions_allowed: `true`
- question_artifacts: `.tmp/phase6/step6b-needs-input.md`
- answer_artifacts: `.tmp/phase6/step6b-input-answer.md`
- continuation_evidence: agent session resumed via `agents resume --session-id 019de19a-ee67-7ce2-8a02-2b60af79dec7`; resumed-invocation UUID `55a9b892-2791-4274-8551-82f32a75ce6b` (saved trace at `.tmp/phase6/trace-step6b-resumed.json`). Initial-invocation UUID `9fa160ce-bcdb-4c8a-a459-1114acfdaa6b` (saved trace at `.tmp/phase6/trace-step6b-initial.json`).
- blocking_if_missing: `true`
- notes: First Step 6b invocation returned `NEEDS_INPUT` after detecting a real contract inconsistency (`SessionStorageType::Other` was specified as reachable in §2.2 but unreachable in §3 Step 8.C / T3 / T5). Orchestrator clarified the contract at `737063b` and answered via `step6b-input-answer.md`; agent resumed and emitted tests + output index.

### Node `step6c-code-writer` (REDO after firstness-evidence repair)

- id: `step6c-code-writer`
- required: `true`
- operator_or_role: code writer
- model: `gpt-high`
- parent: `root`
- prompt: `.tmp/06-locate-step6c-code-redo.md` (the redo prompt with file-based read-evidence requirement)
- log: `.tmp/06-locate-step6c-code-redo.log`
- expected_outputs:
  - `.tmp/phase6/step6c-reads.md` (firstness-evidence file written BEFORE product code)
  - `src-tauri/src/session_metadata/mod.rs`
  - modified `src-tauri/src/lib.rs` (adds `pub mod session_metadata;`)
  - modified `src-tauri/src/main.rs` (extends `Subcommands` and dispatch)
  - modified `src-tauri/src/trace/mod.rs` (imports moved `TranscriptState`)
  - all four test files committed at `2c1416b` pass (`cargo test` exit 0)
- questions_allowed: `true`
- question_artifacts: `none`
- answer_artifacts: `none`
- continuation_evidence: `n/a`
- blocking_if_missing: `true`
- notes: **REDO invocation** UUID `815d9cf3-310a-4572-a923-d53dbfd09888` (saved trace at `.tmp/phase6/trace-step6c-redo.json`). Prior FAILED Step 6c invocation UUID `657148d4-0e44-492a-9ba7-43bb21d449ac` (trace at `.tmp/phase6/trace-step6c.json`) was discarded via `git reset --hard HEAD~1` per process-tree-auditor recommendation Repair option B. **Firstness evidence**: `.tmp/phase6/step6c-reads.md` was written at 2026-04-30 21:06:19 (timestamp inside the file: `2026-04-30T21:06:19-07:00`); first product-code file (`src-tauri/src/session_metadata/mod.rs`) mtime is 2026-04-30 21:09:28 — read-evidence exists 3+ minutes before product-code changes. The redo agent's stdout closing summary contains the `=== Step 6c product code complete ===` block per the redo prompt's final-summary obligation. **Producer relationship**: Step 6c product code is a downstream consumer of `step6b-test-writer`'s output index and four test files; the redo's read-evidence file lists those paths explicitly.

## Timing evidence (orchestrator-collected) — REDO

| Artifact | mtime | Source |
| --- | --- | --- |
| Step 6b tests (`tests/initiative_06_locate.rs`) | 2026-04-30 20:46:43 | filesystem stat |
| Step 6b output index (`.tmp/phase6/step6b-output-index.md`) | 2026-04-30 20:49:35 | filesystem stat |
| Step 6c REDO read-evidence file (`.tmp/phase6/step6c-reads.md`) | 2026-04-30 21:06:19 | filesystem stat |
| Step 6c REDO product code (`src-tauri/src/session_metadata/mod.rs`) | 2026-04-30 21:09:28 | filesystem stat |
| Step 6c REDO invocation timestamps | see `trace-step6c-redo.json` | trace JSON |

**REDO firstness ordering**: Step 6b output index existed → Step 6c
REDO read-evidence file existed → Step 6c REDO product code existed.
Specifically the read-evidence file at `21:06:19` is 3+ minutes
older than the product code at `21:09:28`, satisfying the "before
product-code changes" requirement.

The prior FAILED Step 6c invocation (`657148d4-...`, trace at
`trace-step6c.json`) is preserved as evidence of the audit-
repair cycle but its product code was discarded by `git reset
--hard HEAD~1`.

## Audit obligations

1. **Independence**: Step 6b and Step 6c must be separate agent
   invocations with separate session IDs. Confirmed by
   `019de19a-ee67-7ce2-8a02-2b60af79dec7` (6b) vs
   `019de1aa-e924-70a1-9278-58945c06b20f` (6c).
2. **Timing order**: Step 6b output index existed before Step 6c
   started writing product code. Confirmed by mtime evidence
   above.
3. **Step 6b output index presence**: at
   `.tmp/phase6/step6b-output-index.md`; contains required fields
   (test-intent ID, named risk, level, source, file path, test
   identifier, fixture source, etc.). Confirmed by file inspection.
4. **Step 6b output paths**: four test files at
   `src-tauri/tests/initiative_06_locate.rs`,
   `src-tauri/tests/session_metadata_component.rs`,
   `src-tauri/tests/fixtures/initiative_06.rs`,
   `src-tauri/tests/fixtures/mod.rs`. All present.
5. **Step 6c consumption of Step 6b outputs** — **REPAIRED via
   Repair option B**: prior failed Step 6c (`657148d4-...`) was
   discarded; redo Step 6c (`815d9cf3-...`) wrote
   `.tmp/phase6/step6c-reads.md` at 2026-04-30 21:06:19 listing
   the contract, output index, four test files, and hookpoints —
   3 minutes before any product-code file mtime. Read-evidence is
   in original position (before product-code changes).

## Repair record

The first Step 6c invocation (`657148d4-0e44-492a-9ba7-43bb21d449ac`,
log at `.tmp/06-locate-step6c-code.log`, trace at
`.tmp/phase6/trace-step6c.json`) was correctly classified as
`blocking` by the prior process-tree audit at
`.tmp/phase6/process-tree-audit.report.md` due to missing read-echo.

Per Repair option B from that report:
1. The prior Step 6c product-code commit (`58cb2eb`) was discarded
   via `git reset --hard HEAD~1`.
2. Step 6c was re-dispatched with a stricter prompt
   (`.tmp/06-locate-step6c-code-redo.md`) requiring a file-based
   read-evidence artifact AND a stdout echo BEFORE product-code
   changes.
3. The redo invocation (`815d9cf3-...`) wrote
   `.tmp/phase6/step6c-reads.md` at 21:06:19 (verifiable
   filesystem mtime), which is 3+ minutes earlier than the first
   product-code file's mtime (21:09:28).
4. The redo's product code is functionally equivalent to the
   discarded code (same contract + tests reproduce the same
   implementation) and committed at `b681ba8`.
5. All 418 tests pass against the redo's product code.
