# 06-schema-probe — Phase 4 scope risk gate (claude-opus, Round 2)

**Verdict: LOW.**

Rev 2's only diff against Rev 1 is the two authorized audit closures
(R1-F01 §3 compatibility-map shape; R1-F02 §6 `ReadOnlyOpenError`
variants), plus the §1 change-log block and the §4 / §9.1 wording that
references the now-pinned shape. Both closures are clarification — they
narrow ambiguity that already existed in Rev 1 without adding any new
field, command, helper, decision, side effect, or surface promise. No
new D-decision was introduced, no anti-scope item was relaxed, and no
prior LOW nit was silently widened. Round 1's MEDIUM observation about
`PRAGMA user_version` stamping (carried in audit-history as scope's
verdict-line caveat) is not on the authorized closure list for this
round and remains outside Round 2 scope.

## Round 2 direction analysis (Rev 2 changes only)

| Change | Closure target | Direction | Notes |
| --- | --- | --- | --- |
| §1 lines 35-43 add a "Rev 2 changes" change-log block. | meta | clarification | Names R1-F01 and R1-F02 explicitly; no new commitment. |
| §3 row text for `state_db.tables` / `required_columns` / `required_indexes` (lines 127-129) and the new shape paragraph (lines 134-138) pin flat-for-tables vs nested-for-columns/indexes; no dotted keys. | R1-F01 | clarification (reduction of ambiguity) | The Rev 1 schema row already named these as "object boolean map"; Rev 2 fixes the serialization shape. No new field; no field renamed; no field removed. The illustrative JSON block (lines 140-163) is non-normative and matches the pinned text. |
| §4 step 7 (lines 259-263) describes how the inspector builds the maps — flat for tables, nested for columns/indexes, with every required key initialized to `false` even when the parent table is absent. | R1-F01 | clarification | Aligns the resolution flow with the §3 shape. Does not add or remove an inspection step; PRAGMA/`sqlite_master` calls are unchanged from Rev 1. |
| §9.1 rows D3 / D6-missing / D6-older-newer-missing now assert "canonical §3 map shape" and "booleans preserved at canonical keys". | R1-F01 | clarification | Tests the existing observable; no new fixture class; no new tooling beyond what Rev 1 already required. |
| §6 lines 303-309 enumerate `ReadOnlyOpenError::{Missing, NotADatabase, PermissionDenied, WalSidecarError, Operational}`. | R1-F02 | clarification (named contract for an existing return type) | Rev 1 already declared `open_read_only` returns `Result<Self, ReadOnlyOpenError>`. Rev 2 fixes the variant set; it does not add a sixth public return type, change `default_path` / `user_version` / `inspect_session_schema`, or refactor `StateDb::open`. |
| §6 lines 315-323 map each variant to a triggering condition, CLI exit behavior, and a §9.1 test obligation. | R1-F02 | clarification | The exit codes and stderr error codes (`0`, `1` `state-open-failed` / `state-inspect-failed`, no `14` for these variants) are identical to Rev 1's §5 table; the mapping just makes the linkage explicit. `Missing` cleanly produces exit `0` with the §3-shape success object, matching Rev 1 §4 step 4. |

### Drift check (no Rev-2 change found outside the two closures)

- §2 subcommand surface, §3.1-§3.4 D-decisions, §5 exit-code table, §7
  anti-scope, §8 side-effect contract, §10 README updates, §11
  supported-surface, §12 residuals, §13 cross-feature checklist — all
  match Rev 1 textually.
- No new helper on `StateDb`. The Rev 1 surface (`default_path`,
  `open_read_only`, `user_version`, `inspect_session_schema`) is
  unchanged.
- No new feature flag, no new storage-vocabulary entry, no new exit
  code, no change to `safe_for_import_replace` predicate conditions.
- The `WalSidecarError` variant is permitted by Rev 1's §3.1 D3 / §5
  WAL classification; it does not move WAL failures from operational
  exit `1` into schema-incompatible exit `14`.

## Round 1 LOW nits — closure status

- **L1 (provider-secret anti-scope only implicit).** Not on the
  authorized closure list for Round 2. §7/§8 still forbid config edits
  and transcript reads; explicit "no config / credential reads" line is
  still absent. Carry forward; not a blocker.
- **L2 (`default_path()` mini-API split-out).** Unchanged in Rev 2.
  Still within the harness ask. No action.
- **L3 (D5 storage-vocabulary duplication).** Unchanged; §3.3 and §12
  still acknowledge the on-merge reuse path.
- **L4 (`inspect_session_schema` shape).** Unchanged; §6.2 still leaves
  naming to Phase 5 while keeping the helper non-mutating.
- **L5 (README framing as documentation).** Unchanged; §10 still
  documentation-only.

## Findings ≥ MEDIUM

None for Round 2. R1-F01 and R1-F02 are closed by §3 / §6 wording that
matches the Round 1 watch signals (WS1: §3 shape stability; WS2: API
error-variant discipline). The Round 1 scope caveat about
`user_version` stamping ambiguity is unchanged in Rev 2 but is not an
authorized Round 2 closure target and is outside this round's
direction-analysis remit.

## LOW nits (Round 2)

- **R2-L1.** §3 illustrative JSON (lines 140-163) is partial — it shows
  only the `state_db` subtree. The §3 row table remains the normative
  source. Acceptable; flagged so Phase 5 reads the table, not the
  example, when generating the report builder.
- **R2-L2.** §6 variant mapping table cites two §9.1 test obligation
  labels for a single variant in the WAL row ("`state-open-failed` or
  `state-inspect-failed`"). This matches §5 exit table and §9.1 D3 row,
  so it is internally consistent; no action.
