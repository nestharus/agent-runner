# 06-schema-probe — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

Rev 2 closes Round 1's two MEDIUM audit findings (R1-F01, R1-F02) without
disturbing the supported-surface posture established in Rev 1. The proposal
remains a strictly additive, physically read-only CLI surface: §1.1 register,
§7 anti-scope, §8 side-effect contract, §11 supported surface, and §13
cross-feature checklist are unchanged in substance. Net value is preserved
and modestly improved — the JSON shape and `ReadOnlyOpenError` variant table
make harness implementations less ambiguous and §9.1 test obligations
auditable. No assumption is invalidated. Adjacent paths remain bit-identical
under D7. Phase 5 hookpoints may proceed.

## Concern 1 — Assumption walk on §1.1 (delta-only)

The §1.1 register text is byte-identical to Rev 1; the Rev 1 verdicts carry
forward. Spot-checking the assumptions whose evidence intersects the Rev 2
deltas:

| ID | Verdict | Rev 2 note |
| --- | --- | --- |
| A1 read-only open feasible | **HOLDS** | Enumerated `ReadOnlyOpenError` variants are all non-mutating classifications of an attempted read-only open. None of `Missing`, `NotADatabase`, `PermissionDenied`, `WalSidecarError`, `Operational` implies a write or schema-ensure path. The invalidator (compatibility requires mutation) still does not fire. |
| A2 `PRAGMA user_version` source | **HOLDS** | Untouched. |
| A3 compiled features binary-bound | **HOLDS** | Untouched. |
| A4 CLI default DB v1 target | **HOLDS** | Untouched. |
| A5 reviewable parallel to 06-locate | **HOLDS** | Untouched; §3 canonical map shape is locally defined and does not depend on locate's enum landing first. |
| A6 structural+version sufficient | **HOLDS** | The pinned nested map in §3 enumerates the same required tables/columns/indexes as §6.2 and Rev 1, just with a fixed serialization. No invariant scope expansion. |

**Termination signal #1 (`invalidated-assumption`) — DOES NOT FIRE.**

## Concern 2 — Net value vs. Rev 1 + Round 1 watch signals

Rev 1 closed nine of ten problem-map §6 gaps. Rev 2 does not change that
count, but tightens two:

- **WS1 (JSON shape stability for compatibility map) — CLEARED.** §3 now
  pins `tables` as a flat object, `required_columns` and `required_indexes`
  as nested table→key→boolean objects, with explicit prose ("No dotted keys
  such as `\"session_turns.parent_turn_id\"` are allowed in these maps") and
  a worked JSON example. §4 step 7 mirrors this in the resolution flow:
  required keys initialize even when their parent table is absent, so the
  shape is stable across missing/incompatible/compatible cases. §9.1 D6
  rows now assert "canonical §3 map shape" and "flat `tables`, nested
  `required_columns`, nested `required_indexes`." Harness can pre-compile
  the schema.
- **WS2 (`ReadOnlyOpenError` variant discipline) — CLEARED.** §6 now lists
  five named variants and a mapping table that ties each to its triggering
  condition, CLI exit behavior, stderr JSON code, and the §9.1 test row
  carrying the obligation. `Missing` → exit `0`; the four operational
  variants → exit `1` with `state-open-failed` (or `state-inspect-failed`
  for WAL inspection failures). The exit-code table in §5 and the variant
  table in §6 are mutually consistent.

Net value relative to Rev 1: **preserved and modestly improved.** The
harness contract is more deterministic; ambiguity that would have surfaced
in Phase 6 is decided in Phase 3. No surface is lost or weakened.

**Termination signal #2 (`non-positive-value`) — DOES NOT FIRE.**

## Concern 3 — Adjacent path preservation

§7 D7 still forbids retrofitting any existing read-intent command in v1.
§8 still forbids transcript reads, config edits, telemetry, and adapter
state mutation. The new variant enumeration is internal to a new API
(`StateDb::open_read_only`) consumed only by the new probe call site;
existing `StateDb::open` callers (`trace`, `repl`, `resume`, top-level
`--resume`, `migrate-db`, `migrate-config`, hidden `resume-list`, GUI/Tauri
state commands, direct CLI ingestion) are not touched. §13 cross-feature
checklist is unchanged. **PRESERVED.**

## Concern 4 — Migration / rollback / observability accuracy

Unchanged from Rev 1. Migration story (§11.1: existing DBs report
`user_version = 0`, harness treats exit `14` as refusal until a future
mutating-open PR stamps the version) is identical. Rollback (§11.1:
binary uninstall; probe writes no durable state) is identical. The Rev 2
JSON shape pin actually *improves* observability because the absent-table
case still emits canonical keys with `false`, so harness diffing against
expected schemas is well-defined.

## Concern 5 — Findings carried forward from Rev 1

| Rev 1 finding | Rev 2 status |
| --- | --- |
| #1 Stamping-PR coordination (permanent exit `14` until a future PR stamps) | **Carried.** Substantive coordination item for Phase 5 hookpoints; Rev 2 did not address it because it is out-of-scope for an additive read-only PR. §1 line 25-26 and §12 residual #1 still document the seam. |
| #2 `safe_for_import_replace` permanently `false` in v1 | **Carried.** §3.4 unchanged; README §10 still owes a sentence calling this out. Minor. |
| #3 D7 leaves locate's A6 caveat documentary | **Carried.** Coordination item between schema-probe Phase 6 and any future 06-locate Rev. Not a blocker. |
| #4 Storage-vocabulary duplication risk | **Carried.** §3.3 D5 still allows reuse-if-present, duplicate-otherwise. Phase 5 should pin the branch. |
| #5 WAL/permission read variability is platform-dependent | **Carried.** §9.1 "D3 WAL read behavior" row still residualizes platform variance honestly, and Rev 2's `WalSidecarError`/`Operational` split actually makes the residual more inspectable. |

No new findings emerge from the Rev 2 deltas. Both audit findings are
closed at the §3 and §6 source rather than papered over with prose.

## Concern 6 — 06-locate forward-compat unchanged

§6 still lands `StateDb::open_read_only(&Path)` with the semantics 06-locate's
A6 residual was waiting on. The added `ReadOnlyOpenError` enum is the public
error type 06-locate's eventual retrofit will need to pattern-match against,
which strengthens — not weakens — that forward-compat. D7 still forbids the
retrofit in this PR.

## Verdict

**LOW. No termination signal fires.** Rev 2 closes WS1 and WS2 cleanly,
preserves all six §1.1 assumptions, and improves net value by removing
ambiguity around the compatibility-map shape and the read-only open error
surface. The five Rev 1 findings remain valid coordination items for Phase
5/6/10, none are blockers. Cleared for Phase 5 hookpoints.
