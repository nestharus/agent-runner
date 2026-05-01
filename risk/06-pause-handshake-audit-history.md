# Audit history — 06-pause-handshake

## Round 1
- audit HIGH (F1, F2 HIGH; F3, F4 MEDIUM); scope/shortcut/supported-surface LOW.
- F1: idempotent release marker storage choice deferred to Phase 5.
- F2: writer-path observers deferred without explicit narrowing.
- F3: StateDb::open mutation exception unpinned.
- F4: §9.1 missing assumption_link + residual_risk columns.
- Decision: continue. Rev 2 closes all 4 R1 findings.
