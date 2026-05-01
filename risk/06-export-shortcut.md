# 06-export — Phase 4 Shortcut Risk Assessment (Rev 2, Round 2)

## Verdict: LOW

Rev 2 makes one targeted change (`proposals/06-export.md:35-39`,
`:339-350`): §8 now pins the `STATE_DIR` mkdir behavior of
`locate_transcript` as accepted-residual by anchoring to the
harness's transcript-locator anti-scope clause (`01-session-locate.md:46`
— "Running configured transcript locators is allowed only if already
part of the current trace/session contract"), and notes the same
behavior is already exhibited by `trace --json` and `agents session
locate`. The clause also affirms that no file inside the directory
is written by `export`. This change closes the audit-owned finding
R1-F01 (audit's domain, verified below) and resolves the prior
shortcut-side watchpoint Sh-W3 by promoting it from "Phase 5 evidence
requirement / silent-violation risk if Phase 5 is mute" to "explicit
carried-residual with cited harness-contract authorization." No new
shortcut surface is introduced. Rev 1's other watchpoints (Sh-W1
Codex compaction marker, Sh-W2 timestamp-regression real-fixture
coverage, Sh-W4 large-transcript memory ceiling) carry forward
unchanged. No `MEDIUM` or `HIGH` shortcut findings.

## R1-F01 closure check (audit-owned; shortcut-side verification only)

R1-F01 is an audit-domain finding (see
`risk/06-export-audit-history.md:11-14`). The shortcut reviewer's
obligation is to confirm closure does not introduce shortcut
regressions. Verification:

- Audit history records R1-F01 as the open Round 1 issue and Rev 2
  as the closure dispatch (`risk/06-export-audit-history.md:11-14`).
- Proposal §1 Rev 2 changes block names the §8 STATE_DIR clause as
  the closure mechanism (`proposals/06-export.md:35-39`).
- Proposal §8 contains the new explicit clause naming
  `src-tauri/src/sessions/mod.rs:184-185`, citing the matching
  behavior in `trace --json` and `agents session locate`, citing
  the harness anti-scope authorization, and asserting "No file
  inside the directory is written by `export`"
  (`proposals/06-export.md:339-350`).
- The cited harness anti-scope language is verbatim present at
  `01-session-locate.md:46`. The export spec (`02-session-export.md:54`)
  does not contradict it — its forbidden list is `state.db` mutation,
  ingest cursor updates, temp file writes, and provider launches; an
  empty parent-directory creation is not on that list.
- The closure stance matches 06-locate Rev 3's accepted carried-
  residual disposition (R1-F03 closure, referenced in Sh-W3 below),
  preserving cross-feature consistency.

Audit-side closure verdict (LOW vs. carried-residual; pinned-clause
adequacy) is the audit reviewer's call. From the shortcut layer:
**closure is consistent and introduces no shortcut regression.**

## Fresh assessment of Rev 2 §8 STATE_DIR clause

The new clause (`proposals/06-export.md:339-350`) is the only Rev 2
proposal change. Shortcut analysis:

| Question | Finding |
| --- | --- |
| Does pinning rather than deferring this residual constitute a shortcut against the §8 read-only side-effect contract? | No. The clause does not weaken the contract; it documents that the directory creation is *outside* the contract's forbidden set, citing the harness anti-scope clause that explicitly authorizes "configured transcript locator" side effects when those side effects are already part of the trace/session contract — which they are (`trace --json`, `agents session locate`). |
| Does the clause silently launder a side effect through "we are reusing locate"? | No. The clause names the exact source line (`src-tauri/src/sessions/mod.rs:184-185`), names the precedent surfaces, and explicitly carves out that no *file* inside the directory is written. The reader can verify the claim. |
| Does pinning remove a Phase 5 evidence requirement that should have been kept? | No. The Phase 5 evidence requirement was about resolving an *unresolved* contract conflict between export's "stricter than locate" stance and locate's accepted-mkdir stance. By harmonizing export's contract to locate's at the proposal level — with cited harness-contract authority — the conflict no longer exists, so Phase 5 has no live question to answer here. The "lift the helper into a read-only mode" alternative remains a possible future refinement; deferring it indefinitely is not a shortcut because the harness contract permits the current behavior. |
| Does the clause silently widen export's anti-scope? | No. §7 anti-scope (`:312-326`) is unchanged. The carve-out is narrow: parent-directory creation only, no file writes, only via the same `locate_transcript` path that locate and trace already exercise. Anything beyond that (writing inside the directory, creating it on a code path other than the locator) would violate §8 as written. |
| Round-trip with import-replace: does the clause weaken what import-replace can rely on? | No. Import-replace can still trust that export does not mutate transcript bytes, DB rows, cursors, or temp files. The directory creation is a property of `locate_transcript` itself, not of export's transcript-reading work. |

**Verdict: LOW.** The §8 clause is purpose-fit. It chooses
contract-pinning with explicit harness-anchored authorization over
silent acceptance or indefinite deferral.

## Purpose-thread shortcut analysis (Rev 2)

Rev 2 does not alter any of the five purpose threads identified in
Rev 1. The Rev 1 table (`risk/06-export-shortcut.md` Rev 1 §
"Purpose-thread shortcut analysis") carries forward verbatim. Spot
re-check:

| Purpose thread | Rev 2 status |
| --- | --- |
| (a) Stable canonical surface that replaces harness direct parsing | Unchanged. D2 typed chunks, mandatory `unsupported_record`, single `canonical-jsonl` v1 format intact (`:118-127`, `:88-94`, `:115-116`). |
| (b) Auditable source preimage on every record | Unchanged. D1 mandatory source object and SHA-256 byte-slice rules intact (`:146-156`, `:158-162`, `:164-168`). |
| (c) Reusable canonical reader for import-replace round-trip | Unchanged. `Vec<CanonicalRecord>` return shape, public type exports, no-partial-stdout invariant intact (`:236-300`, `:99-101`, `:209-211`). |
| (d) Reuse of 06-locate ownership/path | **Strengthened.** Rev 2 §8 explicitly anchors the locator-side-effect carve-out to the harness contract that 06-locate's own §8 cites, removing the "export is stricter than locate" tension and making the reuse boundary cleaner. |
| (e) Read-only side-effect contract | **Pinned, not weakened.** Rev 2 makes the contract explicit about what is forbidden (DB writes, cursor writes, transcript writes, temp files, provider launches) and what is permitted (parent-directory creation as part of the existing locator contract). The stricter-than-locate ambiguity is resolved without silently relaxing. The dependency on 06-schema-probe's read-only `StateDb` open variant remains intact (`:177-180`). |

No purpose-thread regression detected.

## Specific shortcut surfaces re-evaluated against Rev 2

Sh-1 through Sh-8 from Rev 1 carry forward unchanged — Rev 2 did
not touch ordering policy (Sh-1), compaction asymmetry (Sh-2),
in-memory `Vec` buffer (Sh-3), `Other`-storage rejection (Sh-4),
public reader API surface (Sh-5), `sha2` dep policy (Sh-6),
`session_turns` non-fallback (Sh-7), or provider-native bookkeeping
skip (Sh-8). No regressions introduced.

The only change touches §8, addressed in the dedicated section
above.

## Watchpoint disposition (Rev 2)

| ID | Rev 1 status | Rev 2 disposition |
| --- | --- | --- |
| Sh-W1 Codex compaction marker hunt | Open Phase 5 watchpoint | **Unchanged.** Rev 2 did not touch §4 step 8 or A7. The Phase 5 evidence requirement to record the marker-hunt outcome (found / not found) carries forward verbatim. Silence by Phase 5 would still convert this into a shortcut. |
| Sh-W2 Real-transcript timestamp regression coverage | Open Phase 5/6 fixture watchpoint | **Unchanged.** Rev 2 did not touch §9 fixture intent or D5 ordering. Phase 6 fixture sampling of real Claude/Codex transcripts (including sidechain/retry paths) for regression behavior remains required. |
| Sh-W3 `locate_transcript` STATE_DIR mkdir residue | Open Phase 5 evidence watchpoint with silent-violation risk if Phase 5 mute | **Resolved.** Rev 2 §8 (`:339-350`) pins the residual at the proposal contract level by anchoring to the harness anti-scope clause at `01-session-locate.md:46` and citing matching `trace --json` / `agents session locate` precedent. The Phase-5-must-resolve-or-revise requirement is replaced by an in-contract carve-out with explicit scope (parent dir only; no files written by export). The "lift the helper into a strict read-only mode" alternative remains available but is no longer load-bearing for this proposal's correctness. |
| Sh-W4 Memory ceiling for very large transcripts | Open Phase 6 fixture-coverage gate | **Unchanged.** Rev 2 did not touch §4 step 10 or §12 memory residual. Phase 6 large-transcript fixture remains the gate; OOM under default platform limits is still a Phase 2.5 re-entry signal. |

## Findings (severity >= MEDIUM)

None.

## LOW-severity observations / nits

None beyond the three remaining sub-LOW watchpoints above (Sh-W1,
Sh-W2, Sh-W4). Sh-W3 is closed at the shortcut layer per the
disposition above.

## What this report does not cover

Per Phase 4 role separation, this report only evaluates whether
proposed shortcuts defeat the underlying purpose. R1-F01's audit-
side closure verdict (whether the §8 pinning text is sufficient as
an audit artifact) is the audit reviewer's call, not the shortcut
reviewer's; this report only confirms the closure introduces no
shortcut regression. Scope adherence
(`risk/06-export-scope.md`) and net-value on the supported
surface (`risk/06-export-supported-surface.md`) remain out of
scope for this report.
