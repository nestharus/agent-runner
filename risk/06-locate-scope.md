# 06-locate — Phase 4 Scope Risk Assessment (Rev 2)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW** — Rev 2 closes every R1 nit this role raised and
nets to a small reduction relative to Rev 1; no fresh design surface,
no silent expansion of anti-scope, no new ownership paths.

R1 generated nine findings across the four roles; Rev 2 surgically
addresses each. The dominant scope-direction signal is R1-F02 closure
via Codex `workspace_root` deferral, which removes a speculative
contract commitment and narrows v1 behavior. The R1-F05 path-hash
tiebreaker adds one named rule (longest-prefix-existing wins;
ambiguous → exit `12`), bounded by a single new §9.1 row. R1-F07/F09
are documentation-only residual/README additions. No Rev 2 change
extends past the R1 finding it closes.

---

## 1. R1 nit closure

| Nit | Status | Evidence |
| --- | --- | --- |
| #3.A — `mutable: false` future-lock not in §12 (= R1-F07) | closed | §12 line 306: "Once 06-pause-handshake lands, `mutable` will gain a sixth condition: no active session-scoped lock held by another writer …" |
| #3.B — `other` storage success-emitting branch under-documented | closed | §12 line 304 explicit residual; §3 line 110 spells the emission condition; §10 commits "Document success JSON fields exactly as §3" |
| #3.C — module path "proposed" (= R1-F08) | closed | §1 line 16 / §6 line 158 now read `src-tauri/src/session_metadata/` with no "proposed" qualifier |
| #3.D — initiative file harness-numbering | n/a | Out of scope to fix in proposal per Rev 2 prompt; initiative file unchanged |

## 2. Rev 2 scope-direction analysis

| Rev 2 change | Direction | Reason |
| --- | --- | --- |
| §9.1 D5 test row added (R1-F01) | additive (test-only) | Adds the row R1-F01 demanded; "no `--state-db`" intent is unchanged D5 reduction. No new semantic surface. |
| §4 step 8 / §1.1 A4 / §9.1 D7 / §12 Codex `workspace_root` deferred to Phase 5; v1 fail-close (R1-F02) | reduction | R1-F02 explicitly authorized "fail-closed for all Codex sessions" as one of two closures. Dropping `payload.cwd` removes a speculative contract; all Codex sessions now bucket to exit `12`. Narrows v1's effective harness surface, but the harness only requires the field be derivable when emitted. |
| §8 explicit `STATE_DIR` mkdir clause (R1-F03) | clarification | Documents the same `locate_transcript` directory-creation behavior `trace --json` exhibits today; supported-surface report A3 already classified it. No new I/O. |
| §4 step 3 commits to `unwrap_or_default`; citation fixed to `src-tauri/src/main.rs:1079-1084` (R1-F04) | clarification | Replaces a prior "operational error" misclaim with the actual resume-adjacent semantics the audit report cited. No behavior change. |
| §4 step 8 Claude tiebreaker rule (R1-F05) | additive (semantics defined) | Names a previously-undefined rule: longest-prefix-existing decomposition wins; ambiguous decompositions → exit `12`. R1-F05 demanded a tiebreaker; this is the minimal closure. New §9.1 D7-ambiguity row is the matching test obligation. |
| §11.1 / §12 `migrate-db` overpromise removed (R1-F06) | reduction | §11.1 line 289 and §12 line 305 narrow the prior "users can run `agents migrate-db`" claim to the actual `backfill_session_chains` skip-when-non-empty behavior. Partial-chain repair is now an explicit unowned residual, not a 06-locate promise. |
| §12 future `mutable: false` lock condition residual (R1-F07) | additive (doc-only) | Records the forward-extension contract; no semantic commitment in v1. Mirrors the cross-feature constraint already named in initiative line 114-117. |
| §1 / §6 module path committed (R1-F08) | clarification | Replaces "proposed" with a committed path. No new code surface. |
| §10 README mutable-framing bullet (R1-F09) | additive (doc-only) | Aligns README phrasing with §3 D3's "read-time eligibility" semantic. README plan stays paragraph-scale. |

**Net direction:** net-reduction. R1-F02 (Codex deferral) and R1-F06
(`migrate-db` claim narrowed) are real-scope reductions; the remaining
seven changes are clarifications, doc-only residuals, or test-only
additions matching specific R1 demands. No change extends past the
finding it closes.

### Watch-flag judgments

- **Codex fail-closed branch widening anti-scope or new test obligations beyond R1-F02?** No. §7 anti-scope is unchanged; the deferral is recorded as a §12 residual where it belongs (a deferred derivation is not anti-scope). The only new test obligation is one §9.1 D7 fixture-clause for "Codex provider with located JSONL but no supported root derivation," which is the minimal verification of the fail-close commitment. A4's invalidator was tightened to mention Phase 5 evidence as a re-fold trigger; that is a watch-condition rephrasing, not a new contract.
- **Path-hash tiebreaker introducing semantics beyond R1-F05?** No. The rule is one sentence in §4 step 8 plus one §9.1 row. The "longest-prefix-existing" framing is essentially a deterministic-when-single rule; multi-existing decompositions fall through to exit `12`. No new resolver path, no new ownership path. The rule lives entirely inside the Claude `workspace_root` derivation that A4 / D7 already owned.
- **§12 residuals expanding beyond R1-F06/F07?** No. Three new residuals correspond 1:1 to R1 findings: Codex defer (R1-F02), `migrate-db` partial-chain narrow (R1-F06), pause-handshake future lock (R1-F07). Pre-existing residuals (physical read-only, GUI DB divergence, multi-row ambiguity, workspace-root rejection, `other`-success) are unchanged.

### Cross-feature consistency

- §13 checklist is unchanged in row composition; entries for namespace
  (`10/11/12`), `resolve_resume` reuse, deferred read-only open,
  no-auto-resume / no-spawn / no-quota / no-config-edit / no-`migrate-config`-coupling,
  and `SessionMetadata` reuse all still cite the same initiative
  lines (`initiatives/06-session-override-contract.md:106-122`,
  `:41-43`).
- Harness ask alignment (`01-session-locate.md:35`,`:52`): `storage_type`
  still distinguishes `claude_code` / `codex_session` / `other`; the
  Codex fail-close narrows the success-path population but does not
  change the enum or the "fail rather than partial" stance the
  harness explicitly asked for.
- Initiative-cross-feature constraints
  (`initiatives/06-session-override-contract.md:112-122`) are still
  honored: no second ownership path; lock observation deferred to
  06-pause-handshake; read-only open deferred to 06-schema-probe.

## 3. Findings (severity >= MEDIUM)

None.

## 4. Nits (severity LOW)

None. The four Rev 1 nits are closed (#3.A/B/C) or out-of-scope to
this proposal (#3.D), and Rev 2 introduces no new scoping concerns.
