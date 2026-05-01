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
