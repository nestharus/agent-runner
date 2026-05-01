# 06-export - Phase 4 Audit Risk Report (Rev 1)

**Verdict: MEDIUM**

The proposal satisfies most Phase 4 audit obligations: it defines the CLI
surface, canonical JSONL record contract, error namespace, parser/API
boundary, anti-scope, supported-surface track, assumption register,
test-intent track with fixture sources, and residual-risk expectations.
One audit finding remains: the no-side-effect contract depends on a
read-only transcript-locator path that the proposal has not made
contractual.

## Prior-round status

No prior `06-export` audit report exists in this worktree, so there are no
prior findings to close, regress, weaken, or carry forward.

## Checklist audit

| Obligation | Status | Evidence |
| --- | --- | --- |
| Command and output contract present | Present | `session export <session-id> [--format canonical-jsonl]`, compact JSONL stdout, buffered validation, and stderr JSON are defined in `proposals/06-export.md:67-101`. |
| Canonical schema present | Present | Required record fields and source preimage fields are defined in `proposals/06-export.md:103-168`. |
| Resolution and parser contract present | Present with one finding below | UUID parse, read-only state open, locate reuse, storage dispatch, compaction, ordering, and no partial stdout are defined in `proposals/06-export.md:170-216`. |
| Exit-code contract present | Present | Exit codes `0`, `1`, `2`, `10`, `11`, `12`, and `15` are mapped in `proposals/06-export.md:218-233`, matching the harness request at `02-session-export.md:44-54`. |
| Reusable API contract present | Present | `CanonicalRecord`, `ContentChunk`, `RecordSource`, `ExportError`, and `read_canonical_transcript` are defined in `proposals/06-export.md:235-304`. |
| Migration / rollback track present | Present | No user-state migration, fail-closed existing-session behavior, additive rollback, and observability are covered in `proposals/06-export.md:384-414`. |
| Test-intent track present | Present | The table names change risks, acceptance behavior, test level, fixture source/application point, assumptions, observable signal, and residual risk in `proposals/06-export.md:344-360`. |
| Fixture source present | Present | New byte-level JSONL, parser-level Claude/Codex, and CLI route fixtures are called out in `proposals/06-export.md:362-366`. |
| Residual-risk artifact expectation present | Present | Parser drift residuals are directed to `risk/06-export-test-residuals.md` in `proposals/06-export.md:362-366`; implementation residuals are listed in `proposals/06-export.md:416-435`. |
| Cross-feature constraints present | Present | The compliance table covers shared errors, resolver reuse, read-only state open, no provider spawn, no quota refresh, no config edits, and canonical reader handoff in `proposals/06-export.md:437-450`. |

## Findings

### R1-F01 (MEDIUM) - The read-only locator dependency is conditional, not a proposal-level contract

The harness requires export to be read-only and specifically says it must
not mutate `state.db`, update ingest cursors, write temp files, or launch a
provider session (`02-session-export.md:54-64`). The approved problem map
identifies a current collision with that requirement: `locate_transcript`
creates the adapter `STATE_DIR` before running the locator
(`research/06-export-problem-map.md:59`; `src-tauri/src/sessions/mod.rs:183-185`).
06-locate explicitly accepted that mkdir as a locate-side caveat
(`/home/nes/projects/agent-runner/worktrees/06-locate/risk/06-locate-audit.md:13`).

The export proposal inherits `locate_session_metadata(...)` for transcript
path resolution (`proposals/06-export.md:185-187`) while also forbidding
temp files, parent-directory mutations, provider maintenance commands,
turn scripts, scans, and quota jobs (`proposals/06-export.md:306-331`).
The proposal then says that if the current locator helper still creates
`STATE_DIR`, Phase 5 must identify a read-only locator path or revise the
proposal (`proposals/06-export.md:333-339`).

That leaves a required acceptance property outside the reviewed contract.
Phase 4 is supposed to review the proposal artifact, not a future
hookpoint discovery outcome. The test-intent row for read-only behavior
would snapshot DB rows, transcript mtimes, adapter state, and temp dirs
(`proposals/06-export.md:359`), but the design under review does not say
which non-side-effecting path the implementation is obligated to use when
`locate_session_metadata` reaches today’s side-effecting helper. As written,
an implementation could follow the proposed resolution flow and still
violate the harness side-effect requirement before the parser starts.

Closure expectation: the proposal must make the transcript path resolution
contract reviewable without relying on a Phase 5 conditional, and the
test-intent track must verify that exact contract. Until then, the
read-only side-effect guarantee remains medium audit risk.

## Non-findings / accepted residuals

- Parser drift is adequately identified as the largest residual, with
  Claude/Codex fixture coverage and `risk/06-export-test-residuals.md`
  expected for unverified cases (`proposals/06-export.md:352-366`,
  `proposals/06-export.md:420-435`).
- Codex compaction being unsupported in v1 is explicit and fail-closed at
  the value-contract level: Codex emits full mappable transcript unless a
  marker is proven and the proposal is revised (`proposals/06-export.md:48`,
  `proposals/06-export.md:195-202`, `proposals/06-export.md:423-425`).
- Whole-transcript buffering is a documented implementation residual and
  aligns with the no-partial-stdout contract (`proposals/06-export.md:209-211`,
  `proposals/06-export.md:296-300`, `proposals/06-export.md:428-429`).

## Determination

This report is `MEDIUM`, so Phase 4 does not clear. The proposal needs a
Rev 2 risk round after the read-only locator contract is made reviewable.
