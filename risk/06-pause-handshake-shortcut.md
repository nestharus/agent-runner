# 06-pause-handshake — Phase 4 Shortcut Risk Assessment (Rev 2)

## Verdict: LOW

Rev 2 closes audit findings R1-F01..R1-F04 in proposal text, not in
Phase-5 discretion. None of the closures introduces a shortcut, and
the Round 1 LOW shortcut posture is preserved. Specifically: the
release-marker shape is now concrete (R1-F01 replaces "Phase 5
chooses one of two shapes" with the named sibling marker
`session-<uuid>.released`); the deferred-observers framing (R1-F02)
is restated honestly with explicit "advisory in v1" language and
named sibling-PR retrofit targets; the §8 `StateDb::open` exception
is now pinned to the same accepted-side-effect contract as
06-locate / 06-export rather than left as an open carve-out
(R1-F03); and §9.1 gains `assumption_link` and `residual_risk`
columns (R1-F04). All five named design decisions (D1a, D2, D3,
D4b, D5) carry over unchanged in substance, and no Rev 2 wording
introduces a new shortcut indicator beyond the negation-by-naming
hits already disposed of in Round 1.

## R1 closure check (audit findings, audit-only)

- **R1-F01 — release-marker storage choice deferred to Phase 5.**
  Closed in proposal text. §6 now defines the concrete sibling
  marker `<lock_dir>/session-<session_id>.released` with versioned
  JSON schema (`version`, `session_id`, `chain_id`, `provider_name`,
  `token_hash`, `released_at`); §3.2 surfaces `release_marker_path`
  in the resume receipt; §4 steps 11/13/14/15/17 thread the marker
  through acquire (removes stale marker), resume (reads marker for
  same-token replay), expired-with-marker (idempotent), expired
  without marker (`17 lock-expired`), and successful release
  (writes marker, removes lockfile). §12 explicitly retires the
  prior deferral wording ("Release idempotency uses the concrete
  sibling marker `<lock_dir>/session-<uuid>.released`; there is no
  future marker-shape deferral"). No shortcut: the chosen shape is
  bounded (next acquire removes the marker before writing fresh
  lock metadata, §4 step 11), permission-pinned (§8 owner-private),
  and compatible with §4 step 13's same-token replay rule. Round 1
  L2 is retired.

- **R1-F02 — writer-path observers deferred without explicit
  narrowing.** Closed by explicit narrowing in §1, §1.2, §10, §11,
  §12, and §13. The proposal now states in five places that v1
  ships the primitive only and that the harness should treat the
  lock as advisory until 06-import-replace (and migration / repl /
  resume / balanced one-shot) wire observation in their own PRs.
  This is not a new shortcut: the narrower acceptance surface is
  the natural consequence of D4b, which Round 1 already evaluated
  as purpose-fit. The "advisory in v1" framing hardens the no-
  symptom-masking posture by making the partial coverage
  unmistakable to any reader of the proposal, README, or §13
  compliance table.

- **R1-F03 — `StateDb::open` mutation exception unpinned.** Closed
  by §8's new explicit clause: "`StateDb::open` side effects
  (parent dir creation, WAL enable, schema-ensure, chain backfill)
  are accepted, matching 06-locate and 06-export's §8 contracts.
  No DDL, no row mutation, no
  `session_turns`/`session_chains`/`session_chain_segments`
  writes." This pins the exception to a peer-feature contract
  rather than leaving it as an open carve-out, and §12 commits to
  switching to read-only open once 06-schema-probe's
  `StateDb::open_read_only` is mergeable. No shortcut: the
  inheritance is a real cross-feature design decision (the
  read-only open belongs to schema-probe per the cross-feature
  constraint), not a workaround.

- **R1-F04 — §9.1 missing assumption_link + residual_risk
  columns.** Closed. §9.1 now has both columns populated for every
  test row, including a new `Writer-path advisory scope` row that
  ties A6 to the v1-vs-eventual-coverage doc + integration test.
  Pure documentation strengthening; no shortcut implication.

## Fresh assessment of Rev 2 changes

**Sibling release marker (§3.2, §6, §4 steps 11/13/14/15/17, §8,
§12).** Purpose-fit. The marker is the minimum artifact needed to
distinguish same-token retry from arbitrary missing-lock release
without resurrecting the lockfile after release. It is the right
inversion of the "missing-lock release should not silently
succeed" symptom-masking class. Step 11 ("remove any previous
sibling release marker under the same critical section") plugs
Round 1 L2's only unbounded-growth concern: each acquire reaps the
prior marker for the same session.

**Advisory-in-v1 framing (§1, §1.2, §10, §11, §12, §13).** Not a
shortcut. The proposal refuses to pretend the v1 lease is end-to-
end mutual exclusion, names every sibling that must retrofit, and
sequences the retrofit through the cross-feature constraint
(`/home/nes/projects/agent-runner/worktrees/06-locate/initiatives/06-session-override-contract.md:114-117`).
That is the inverse of symptom-masking.

**§8 `StateDb::open` clause + read-only follow-up.** Not a
shortcut. The accepted side effects match 06-locate and 06-export
verbatim; the read-only switch is gated on 06-schema-probe, the
designated owner of that surface per the cross-feature constraint.

**§9.1 assumption_link + residual_risk columns.** Not a shortcut.
Residual_risk cells are honest about what each test does not
verify (e.g., "Does not prove sibling writer paths observe the
lock in v1"). Disclosure, not deferral.

## Findings (severity >= MEDIUM)

None.

## Shortcut-indicator grep (Rev 2)

Re-ran the canonical flag list (`compat`, `shim`, `backward`,
`legacy`, `transitional`, `dual-write`, `feature flag`, `for now`,
`in the future`, `TODO`, `FIXME`, `workaround`, `temporary`,
`graceful`, `self-heal`, `placeholder`, `hardcode`, `magic`,
`symptom`, `hack`, `fallback`, `defer`, `partial`, `followup`,
`follow-up`, `advisory`).

- **`advisory`** (lines 4, 32, 62, 456, 457, 475, 513, 526, 546).
  All occurrences are negation-by-naming: each one names the
  partial surface (writer-path observation deferred to sibling
  PRs), the compensating mechanism (observer API surface in §6
  + cross-feature constraint), and the eventual closure (sibling
  PR retrofits). One occurrence (line 513) refers to POSIX
  *advisory* locking, which is the correct technical term for
  `flock` semantics, not a shortcut indicator.
- **`deferred` / `follow-up`** (lines 24, 28–29, 426, 521–522).
  All point to the named sibling-PR retrofit work and are
  bounded by the cross-feature constraint commitment.
- **`fallback`** (lines 204, 377). Both negations, unchanged from
  Rev 1.
- **`partial`** (line 546, §13 D4b row). Negation-by-naming,
  unchanged from Rev 1.
- **`TODO` / `FIXME` / `compat` / `shim` / `backward` /
  `transitional` / `dual-write` / `feature flag` / `for now` /
  `in the future` / `temporary` / `workaround` / `hack` /
  `magic` / `placeholder` / `hardcode` / `self-heal` /
  `graceful` / `symptom` / `legacy`** — zero hits.

## Regression check vs Rev 1

No regressions. Round 1 LOW observations L1, L3, L4, L5 (exit-13
second-clause framing nit, malformed-metadata branch, "fsync when
practical" softness, stacked-vs-unstacked resolver parity test)
all carry forward as Phase 5 implementer notes; Rev 2 did not
target them and they remain non-blocking. Round 1 L2 (marker shape
two-options-pick-one) is retired by R1-F01 closure. The five named
design decisions (D1a, D2, D3, D4b, D5) are unchanged in substance.
