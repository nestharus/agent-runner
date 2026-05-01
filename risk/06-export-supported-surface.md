# 06-export — Phase 4 Supported-Surface Risk Report (Rev 2)

**Termination signal:** `none`
**LOW / MEDIUM / HIGH:** **LOW**

`proposals/06-export.md` Rev 2 carries forward the Rev 1 net-positive
trade (one additive read-only subcommand, seven retired §6 entries,
no adjacent supported-path regression) and closes the only sub-finding
that the Rev 1 report flagged at the supported-surface gate — the
`STATE_DIR` directory-creation carve-out for `locate_transcript` —
by lifting 06-locate's already-Phase-8-approved §8 language verbatim.
Rev 2's surface change is small and confined to §8; no other supported
path, observability claim, harness acceptance bullet, or initiative-06
sequencing constraint shifts. Termination signals do not fire. Two
cosmetic findings (F02, F03) carry through unchanged; F01 is closed.

## Round 1 closure check (audit only)

| ID | Round 1 finding | Rev 2 closure | Verdict |
| --- | --- | --- | --- |
| R1-F01 (audit, MEDIUM) | §8 deferred the read-only locator / `STATE_DIR` carve-out to Phase 5 hookpoint research; the proposal contract did not pin whether export's stricter side-effect promise included or excluded `locate_transcript`'s `STATE_DIR` mkdir. | Rev 2 §8 (`proposals/06-export.md:343-350`) now states: "`agents session export` may create the locator adapter `state_dir` directory (`src-tauri/src/sessions/mod.rs:184-185`) when `locate_transcript` is invoked. This directory creation is the same behavior `trace --json` and `agents session locate` already exhibit and is part of the existing transcript-locator contract that the harness anti-scope explicitly permits (\"Running configured transcript locators is allowed only if already part of the current trace/session contract\"). No file inside the directory is written by `export`." This is structurally identical to 06-locate Rev 3 §8 (`worktrees/06-locate/proposals/06-locate.md:243`), which already passed Phase 8 LOW. The Rev 1 §8 conditional ("Phase 5 must either identify a read-only locator path or revise this proposal") is removed. Rev 2 changelog at §1 lines 35-39 names the change explicitly. | **closed** |

R1-F01 is the only audit finding from Round 1; closure is complete.

## Fresh assessment of Rev 2 changes

Rev 2 only edits two locations:

- §1 changelog lines 35-39: declares the Rev 2 change.
- §8 lines 343-350: adds the `STATE_DIR` mkdir carve-out, deletes the
  Phase-5-deferral sentence.

No assumption register entry, exit code, schema field, parser
dispatch decision, compaction policy, ordering guarantee, public
reader API, anti-scope bullet, README update, or supported-surface
clause is changed. The §1.1 register, §3 schema, §4 resolution flow,
§5 exit codes, §6 reader API, §7 anti-scope, §9 test-intent track,
§10 README updates, §11.1 supported-surface track, §12 residuals,
and §13 cross-feature constraints remain bit-for-bit equivalent to
Rev 1.

### Concern 1 — Assumption invalidation check (no change)

A1-A8 hold against the same evidence reviewed in Rev 1:

- A1, A2 still rest on the merged 06-locate Phase 8 LOW verdict and
  06-schema-probe Phase 8 artifacts visible in `git log`.
- A3 still rests on harness `02-session-export.md:20` and problem map
  §1.23, §2.2.
- A4-A8 are unchanged; no schema or parser dispatch decision moved.

The §8 edit does not strengthen, weaken, or invalidate any assumption.
Rev 2 is consistent with A2's invalidator ("Export starts from today's
mutating `StateDb::open_default()` without an accepted exception") —
schema-probe's read-only open is still required, and the new §8 clause
clarifies that locator-side `STATE_DIR` creation is a separately-scoped
carve-out that does not relax the read-only state-open requirement.

**Termination signal #1 (`invalidated-assumption`) does not fire.**

### Concern 2 — Net value on the current supported surface (no change)

Seven §6 problem-map entries retired (Rev 1 table), seven small
fail-closed additive failure modes added (Rev 1 table). Rev 2 does
not add or retire any §6 entry. Net value verdict is unchanged:
**clearly positive.**

The §8 mkdir clause does not enlarge blast radius — the directory
creation behavior already exists in `trace --json` and `session locate`
on the merged supported surface; export simply documents that it
inherits the same carve-out rather than promising a stricter contract
it cannot mechanize before 06-schema-probe's read-only locator work.

**Termination signal #2 (`non-positive-value`) does not fire.**

### Concern 3 — Adjacent supported-path continuity (no change)

All ten enumerated paths (`session locate`, `trace --json`, `resume`,
`repl --resume`, top-level `--resume`, hidden `resume-list`,
`migrate-db`, `migrate-config`, direct CLI ingestion, future
`06-import-replace`) remain PRESERVED / UNCOUPLED / FORWARD-COMPAT
exactly as Rev 1 recorded. The §8 carve-out is parallel to locate's
existing carve-out, so the locate path comparison row strengthens
rather than weakens — export's side-effect contract is now provably
no-stricter-and-no-looser than locate's at the locator-adapter
boundary.

No path is BROKEN or DEGRADED.

### Concern 4 — Migration / rollback / observability concreteness (no change)

§11.1's three load-bearing claims (no user state migration; rollback
by uninstall/revert; observability = success JSONL + stderr JSON)
remain VERIFIED. The "no partial stdout on error" mechanization at
§4 step 10 and the §9 end-to-end test row are unchanged. The Rev 2
§8 edit affects the side-effect contract (Concern 7 below), not the
migration/rollback/observability claims.

### Concern 5 — Harness acceptance criteria coverage (no change)

All seven harness AC bullets in `02-session-export.md` "Acceptance
criteria" remain covered by §3, §4, §5, §6, §9. Coverage verdict:
**complete.**

The harness anti-scope sentence Rev 2 quotes in §8 ("Running
configured transcript locators is allowed only if already part of
the current trace/session contract") is from `02-session-export.md`
itself; export Rev 2 is now aligned with the harness's own carve-out
language, which is a small but real strengthening of harness-contract
fidelity.

### Concern 6 — Initiative-06 sequencing forward-compat (no change)

`06-import-replace` consumer surface (`CanonicalRecord`, `RecordSource`,
`ExportError`, `read_canonical_transcript`) is untouched in Rev 2.
`06-pause-handshake` independence is untouched. `06-schema-probe`
upstream dependency is untouched (and is in fact reinforced by the
Rev 2 §8 clause noting the harness-permitted scope of the locator
mkdir behavior).

Initiative-wide error namespace use (`10`, `11`, `12`, `15`) is
unchanged from Rev 1.

### Concern 7 — Side-effect contract vs `locate_transcript` `STATE_DIR` creation (closed)

This was the only sub-finding the Rev 1 report flagged at the
supported-surface gate (recorded as Rev 1 F01). Rev 2 closes it.

The Rev 1 report named three acceptable Phase-5 resolutions: (a)
introduce a read-only locator variant; (b) revise §8 to inherit
locate Rev 3's carve-out; or (c) document the deviation in
`risk/06-export-test-residuals.md` if it survived to Phase 6b.

Rev 2 takes path (b). §8 lines 343-350 lift locate Rev 3's §8 clause
verbatim (`worktrees/06-locate/proposals/06-locate.md:243`), naming
the same source line (`src-tauri/src/sessions/mod.rs:184-185`),
asserting parallel behavior with `trace --json` and `session locate`,
and citing the same harness anti-scope carve-out. The "No file inside
the directory is written by `export`" sentence pins the strict
boundary: directory creation is permitted; file writes inside it
are not. The §9 "Read-only behavior" test row (DB rows / file mtimes
/ directory listings before/after) still snapshots directory
listings, so the test track will catch any unconditioned deviation
from the carve-out.

This is a clean closure: the contract is now mechanized at the
proposal level, not deferred to Phase 5. Rev 1 F01 is therefore
closed.

## Findings

- **F01 — closed in Rev 2.** The Rev 1 advisory finding on
  `locate_transcript`'s `STATE_DIR` directory creation is resolved by
  Rev 2 §8 lines 343-350. No carry-forward action required.

- **F02 (cosmetic; carries through from Rev 1; non-blocking)** —
  Problem-map §6 #7 ("storage-type support only indirectly observable")
  is partially retired by §11.1 + §10 README documenting the v1
  storage set. A programmatic feature-flag surface for "which storage
  types export supports" lives in `06-schema-probe`'s output (per
  `06-session-override-contract.md:44-50`), not in export. This is
  correct sequencing, not a finding against the proposal. Recorded
  for completeness so Phase 5/6 reviewers do not expect export to
  expose a parser-feature list.

- **F03 (cosmetic; carries through from Rev 1; non-blocking)** —
  A6's "real transcripts with benign clock skew would be rejected"
  residual is a legitimate trade — it preserves the strict
  fail-closed contract the harness asked for. Worth surfacing in §10
  README so harness consumers know that timestamp regression is a
  definitive failure rather than a normalization opportunity. README
  §10 already lists this in spirit ("compaction behavior ... live
  transcript from latest supported boundary") but does not
  explicitly say "regressing timestamps fail." Optional README
  copy-edit; not a contract problem.

No new findings introduced by Rev 2.

## Verdict rationale

**Termination signal #1** does not fire — Rev 2 changes no
assumption-bearing surface; A1-A8 hold against the same evidence
that grounded the Rev 1 LOW verdict.

**Termination signal #2** does not fire — Rev 2 retires no §6
entries (still seven), adds no new failure modes, and tightens the
side-effect contract by replacing a Phase-5 conditional with a
mechanized carve-out that mirrors locate's already-approved language.
Net value remains clearly positive.

**Standard verdict: LOW.** Adjacent supported-path continuity is
preserved across all ten enumerated paths (concern 3); migration
burden remains zero on user state and rollback is uninstall-or-avoid
(concern 4); harness acceptance bullets are completely covered by the
proposal's test-intent track and Rev 2 strengthens harness-contract
fidelity by quoting the harness anti-scope (concern 5); initiative-06
sequencing forward-compat is preserved for `06-import-replace`,
`06-pause-handshake`, and `06-schema-probe` (concern 6); the Rev 1
side-effect gap (`STATE_DIR` creation by `locate_transcript`) is now
resolved at the proposal level (concern 7).

**Recommendation:** Phase 5 (hookpoint research) may proceed.
F02 and F03 remain cosmetic Phase 5/6 surface notes; they do not
gate Phase 5.

**Final verdict: LOW. Termination signal: none.**
