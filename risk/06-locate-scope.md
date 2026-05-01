# 06-locate — Phase 4 Scope Risk Assessment (Rev 3)

**Assessor:** `claude-opus` (scope)
**Verdict:** **LOW** — Rev 3 executes the expansion that Rev 2's A4
invalidator literally named, closes one recorded LOW (R2-F01), and
introduces no design surface beyond the six authorized actions.

Rev 3 is a Codex success-path fold-in triggered by Phase 5 sampling
of 25 real Codex rollout files (`research/06-locate-hookpoints.md`
§I.WS1). Net direction is a controlled expansion: Codex sessions with
a derivable `session_meta.payload.cwd` now succeed at exit `0` instead
of fail-closing at exit `12`. The expansion lives entirely inside the
A4-owned workspace_root derivation surface, the §9.1 D7 row, and one
§12 residual removal. No anti-scope was touched, no §13 row changes,
no new ownership/cross-feature surface was introduced, and the
mutable-truth contract is unchanged (more sessions now qualify under
Rev 2 D3 conditions; the conditions themselves did not move).

---

## 1. R1/R2 nit closure regression

| Nit | Status | Evidence |
| --- | --- | --- |
| R1-F01..R1-F09 (Rev 2 closures) | intact | §9.1 D5 row present (line 259); §8 STATE_DIR mkdir clause present (lines 243); §4 step 3 `unwrap_or_default` citation intact (line 137); §11.1 / §12 `migrate-db` narrowing intact (lines 300, 315); §12 future `mutable` lock residual intact (line 316); §1/§6 module path committed (lines 16, 169); §10 mutable-as-eligibility-hint bullet intact (line 278). |
| R2-F01 (path-hash prose ambiguity) | closed by Rev 3 action E | §4 step 8 Claude paragraph (line 143) now reads "enumerate **all** candidate decompositions ... If exactly one decoded path exists, succeed ... If zero or two-or-more decoded paths exist, exit `12`." Aligns prose with §9.1 D7 ambiguity row's "deterministic only when ... single existing decoded path" (line 264). |
| R2-F02 (resume-parity malformed-config) | intact, not in Rev 3 scope | Inherited from resume; Rev 3 prompt did not authorize a touch. |

## 2. Rev 3 scope-direction analysis

| Rev 3 action | Direction | Reason |
| --- | --- | --- |
| A. §1.1 A4 rephrase | clarification + bounded expansion | A4 evidence now cites Phase 5 sampling (line 60); invalidator re-rephrased to forward-looking "real-world Claude path hashes ... OR upstream Codex schema drift removes/relocates `payload.cwd` ... OR harness requires roots for storage types with no path/config provenance." Adds Codex coverage to the assumption text but stays inside A4's existing slot. No new assumption introduced. |
| B. §4 step 8 Codex derivation | expansion (authorized by A4 invalidator) | Replaces the Rev 2 fail-closed Codex branch with a `payload.cwd` parse path (line 144). Rev 2 §1.1 A4 invalidator literally named "Phase 5 proves a stable Codex workspace-root field ... folding it into v1 rather than a follow-up" as the trigger; Phase 5 provided that evidence. The pipeline rule prescribed exactly this fold-in; expansion is contractual, not silent. |
| C. §9.1 D7 row update | expansion (test obligation matching action B) | Line 263 now covers "Codex `session_meta.payload.cwd` produces canonical UTF-8 `workspace_root`" plus failure fixtures for missing `session_meta`, absent `payload.cwd`, and invalid paths. One-row expansion mirroring action B; no new test category. |
| D. §12 Codex-deferral residual removal | reduction | Rev 2 listed Codex-deferral as a §12 residual; Rev 3 removes it (compare current §12, lines 308–316, against Rev 2). Net reduction in the residual list. The other six residuals (read-only DB, GUI divergence, multi-row, workspace-root rejection, `other` storage, partial chains, future `mutable` lock) are unchanged. |
| E. §4 step 8 Claude paragraph tightening | clarification (LOW closure) | R2-F01 closure. Rev 3 prompt explicitly authorized this. Stays within the existing rule: §9.1 D7-ambiguity row already pinned "single existing decoded path" semantics; Rev 3 prose just reflects it. No new tiebreaker invented (the previous prose-only "longest-prefix-existing wins" framing implied a tiebreaker the test row never granted; removing that implication is consistent with Rev 2's authoritative test pin per WS4). |
| F. Rev 3 changes block | additive (doc-only) | §1 lines 40–49 record exactly the six Rev 3 actions and cite Phase 5 evidence. Same disciplined pattern as the Rev 2 changes block (lines 28–38). |

**Net direction:** controlled expansion with one reduction (action D)
and one clarification (action E). The expansion is exactly the one
Rev 2 explicitly authorized via the A4 invalidator clause; no other
surface was touched.

### Watch-flag judgments

- **§11.1 supported-surface track edit needed?** No. §11.1 (lines 285–304) reads "primary consumer ... replacing its v1 direct `state.db`/JSONL locator" — provider-agnostic. The harness's v1 direct-read replacement applies to Claude AND Codex equally. Rev 2 was already true for both; Rev 3 making Codex success-capable does not change the surface description. Correctly left untouched.
- **§13 cross-feature checklist row changes?** None. All ten rows still hold under Codex success: error-code namespace unchanged; Codex success still uses `resolve_resume` (no second ownership path); lock observation still N/A for read-only locate; read-only DB open still deferred; no auto-resume / spawn / quota / config-edits / `migrate-config` coupling — Codex `payload.cwd` derivation is a JSONL line-walk (same pattern as `scripts/codex-locate-transcript`), not provider invocation; reusable `SessionMetadata` API still owns the new derivation. No row change required.
- **§7 anti-scope integrity?** Verified intact. Lines 219–230 unchanged from Rev 2. The Codex success path does not contradict any anti-scope clause: not transcript export/import/replace, not auto-resume, not provider spawn, not config edit. JSONL line-walk for one record is read-only.
- **`mutable: true` eligibility expansion?** Verified contract-stable. §3 D3 conditions (lines 124–130) are unchanged. Codex sessions can now satisfy condition 5 (workspace_root canonical+exists) where they previously could not — that is "more sessions qualify under existing Rev 2 D3 behavior," not a contract change. The five truth conditions, the "no quota consultation" rule, and the "transcript or workspace failure → exit 12 not partial JSON" rule all hold.
- **New design surface in action B?** Bounded. The Codex parser is named to live "alongside `SessionMetadata` (`src-tauri/src/session_metadata/`)" — Phase 5 already named this module as the new home (`research/06-locate-hookpoints.md` lines 40–55). Putting the helper there is consistent placement, not a new module. The line-walk pattern explicitly cites `scripts/codex-locate-transcript` precedent.

### Cross-feature consistency

- §13 row composition unchanged; all citations still resolve.
- Initiative cross-feature constraints (`initiatives/06-session-override-contract.md:106-122`) still honored: shared error-code namespace, single ownership via `resolve_resume`, deferred lock observation, deferred read-only DB open, no-auto-resume / no-spawn / no-quota / no-config-edits / no-`migrate-config` coupling, reusable `SessionMetadata` factoring. None of these is touched by Codex success-path enablement.
- Harness ask alignment (`01-session-locate.md:35,52`) strengthened, not changed: `storage_type=codex_session` now has a non-fail-closed success population, which is what the harness asked for from the start; Rev 2 narrowed it conditionally and Rev 3 restores it on Phase 5 evidence.

### Drift audit (S5)

Cross-checked the proposal against the six named actions A–F. No
edits found outside: §1.1 A4 row (action A), §4 step 8 Codex branch
(B) and Claude paragraph (E), §9.1 D7 row (C), §12 residuals (D —
removal only), §1 Rev 3 changes block (F). Sections §2, §3, §5, §6,
§7, §8, §10, §11, §11.1, §13 are all unchanged from Rev 2. No drift.

## 3. Findings (severity >= MEDIUM)

None.

## 4. Nits (severity LOW)

None. R2-F01 is closed by action E; no new scoping concerns are
introduced by the six Rev 3 actions.
