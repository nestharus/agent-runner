# Audit history — 06-pause-handshake

## Round 1

- audit HIGH (F1, F2 HIGH; F3, F4 MEDIUM); scope/shortcut/supported-surface LOW.
- F1: idempotent release marker storage choice deferred to Phase 5.
- F2: writer-path observers deferred without explicit narrowing.
- F3: StateDb::open mutation exception unpinned.
- F4: §9.1 missing assumption_link + residual_risk columns.
- Decision: continue. Rev 2 closes all 4 R1 findings.

## CodeRabbit Pass 1

- Findings: 16 total.
- Applied real/value findings: `R1-F05`, `R1-F07`, `R1-F08`, `R1-F09`, `R1-F11`, `R1-F14`, `R1-F15`, `R1-F16`.
- Skipped churn/false-positive findings: `R1-F01`, `R1-F02`, `R1-F03`, `R1-F04`, `R1-F06`, `R1-F10`, `R1-F12`, `R1-F13`.
- `R1-F06` skip rationale: adding a minimum TTL contradicts the Step 6a contract's default/max-only TTL policy and would invalidate existing 1 ms TTL coverage.
- Watch signal: raw token material must remain stdout-only; lockfile and release marker persistence must use `token_hash`.
- Determination: continue after applying real findings and rerun CodeRabbit.

## CodeRabbit Pass 2

- Findings: 8 total.
- Applied documentation-only finding: `R2-F01`.
- Skipped churn/design-preference findings: `R2-F02`, `R2-F03`, `R2-F04`, `R2-F05`, `R2-F06`, `R2-F07`, `R2-F08`.
- `R2-F03` / `R2-F06` skip rationale: 1 ms TTL coverage aligns with the contract's max-only TTL bound and avoids implying a minimum TTL.
- `R2-F08` skip rationale: switching to the `Flock` RAII wrapper is a style/API preference; Rev 4's sentinel `flock(2)` design is already implemented directly and contained behind `with_flock`.
- Determination: all churn; convergence candidate.

## CodeRabbit Pass 3

- Findings: 13 total.
- Applied real/consistency findings: `R3-F03`, `R3-F09`, `R3-F11`.
- Skipped churn/design-preference findings: `R3-F01`, `R3-F02`, `R3-F04`, `R3-F05`, `R3-F06`, `R3-F07`, `R3-F08`, `R3-F10`, `R3-F12`, `R3-F13`.
- `R3-F09` fix: pause success JSON now includes `chain_id` from `ResolvedResume`, matching the Rev 4 receipt schema.
- `R3-F11` fix: Step 6a contract now matches the security fix from pass 1; busy errors expose only `expires_at`, not token material.
- TTL skip rationale: Phase 6 implementation and tests follow `research/06-pause-handshake-contract.md` defaults (`60_000` / `600_000`) rather than older proposal/hookpoint TTL prose.
- Determination: continue after applying receipt/contract fixes.

## CodeRabbit Pass 4

- Findings: 8 total.
- Applied findings: none.
- Skipped churn/design-preference findings: `R4-F01`, `R4-F02`, `R4-F03`, `R4-F04`, `R4-F05`, `R4-F06`, `R4-F07`, `R4-F08`.
- Repeat skips: TTL proposal/contract discrepancy (`R4-F02`, `R4-F03`), `flock` API preference (`R4-F07`), and resume-handshake DB-open comment (`R4-F06`) were previously classified and remain non-blocking.
- `R4-F05` skip rationale: `research/06-pause-handshake-hookpoints.md` is intentionally a Phase 5 hookpoints artifact consumed by Phase 6.
- `R4-F08` skip rationale: dependency version bumping is outside this scoped CodeRabbit pass and not required for the applied lock behavior.
- Determination: `ALL_CHURN`; converge.
