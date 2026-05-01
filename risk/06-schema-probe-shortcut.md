# 06-schema-probe — Phase 4 Shortcut Risk Gate (Rev 2)

**Verdict: LOW**

Rev 2 only tightens §3 (compat-map JSON shape) and §6
(`ReadOnlyOpenError` enumeration). Both are clarifications of
contract that already existed in Rev 1, not new design surface.
Each Rev 2 change strengthens the "refuse rather than corrupt"
posture rather than weakening it. Per-D verdicts from Round 1
are unchanged; D1–D7 remain LOW. No Rev 2 change introduces a
deferred stub, backwards-compat shim, or anti-scope problem
shift.

## Rev 2 deltas

### R1-F01 — compat-map shape pinning (LOW)

Purpose-fit. §3 fixes serialized shape: `state_db.tables` flat
table → boolean; `required_columns` nested
table → column → boolean; `required_indexes` nested
table → index → boolean. Dotted keys such as
`"session_turns.parent_turn_id"` are explicitly forbidden.

This clarifies an existing field surface, not new design.
Pinning the shape closes a corruption-shaped degeneracy: a
caller parsing dotted keys versus one walking nested objects
would otherwise disagree about column presence without either
being wrong by Rev 1's text. §4 step 7 reinforces this by
requiring every required key to be initialized to `false` even
when its parent table is absent — canonical keys are
contractual, not optional. §9.1 D6 rows name the shape as a
test obligation on both success and §14 incompatibility paths,
so the contract is verified, not advisory. No anti-scope shift:
rigor lands inside the existing surface, not on a residual.

### R1-F02 — `ReadOnlyOpenError` variant enumeration (LOW)

Purpose-fit. §6 enumerates five variants — `Missing`,
`NotADatabase`, `PermissionDenied`, `WalSidecarError`,
`Operational` — and maps each to triggering condition, CLI
exit, and §9.1 test row.

This is API discipline, not new behavior. Rev 1 flow steps 4,
5, 9, 10 already required distinguishing missing files
(exit `0`), schema incompatibility (exit `14`), and operational
failures (exit `1`); Rev 2 names the Rust types Rev 1 implied.
The enum is the boundary preventing an implementation from
collapsing missing-DB and permission-denied into the same code
path and emitting the wrong exit.

No catch-all hidden under opaque prose. `Operational` is named
and carries an exit `1` mapping plus a test obligation.
`WalSidecarError` separates sidecar-access failure from
`NotADatabase` so §9.1 D3 WAL row has a typed target. `Missing`
anchors the exit `0` path so missing-DB success cannot silently
regress to exit `1`. The enum stays inside the read-only open
surface; mutating `StateDb::open` is untouched and §7 D7 still
forbids retrofit. No backwards-compat shim — the type is new in
this PR.

## Watchpoint coverage

- **WS1 (compat-map shape stability):** closed by §3 prose
  pinning flat vs nested, the canonical example block, the
  dotted-key prohibition, and §9.1 D6 rows asserting the shape
  on both success and incompatibility paths. Watchpoint does
  not sneak forward into a shortcut.
- **WS2 (error-variant discipline):** closed by §6 enum
  declaration plus the variant → exit → test mapping table.
  Each variant has a §9.1 anchor; none routes to an unstated
  catch-all. Watchpoint does not sneak forward into a shortcut.

## Cross-cutting checks (re-verified for Rev 2)

- **Deferred stubs:** still none. R1-F02 adds named error
  variants for behavior that already had to exist; the
  enumeration is not a stub for future error sources.
- **Backwards-compat shims:** still none. The Rev 2 type
  surface is new alongside the new read-only open path; the
  mutating `StateDb::open` is unchanged.
- **Anti-scope problem-shifting:** unchanged from Round 1. §7
  exclusions remain genuine boundaries; Rev 2 does not move
  any work into §12 residuals.

## LOW observations (carried, unchanged by Rev 2)

1. `CURRENT_SCHEMA_VERSION = 3` still depends on PR discipline.
2. `compatible = true` still does not validate complete chain
   backfill integrity (Initiative 05 segmentless-turn skip).
3. `--state-db` override remains anti-scope in v1.
4. GUI/CLI DB-path divergence remains preserved, not resolved.
