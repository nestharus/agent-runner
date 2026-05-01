# 06-locate — Phase 4 Audit Risk Report (Rev 3)

**Verdict: LOW**

R1/R2 closure remains intact and Rev 3 cleanly folds the Phase 5 Codex hookpoint evidence into the v1 contract. R2-F01 is closed: the Claude path-hash branch now requires exhaustive candidate enumeration and succeeds only when exactly one existing decoded root remains. The new Codex `session_meta.payload.cwd` branch is concrete, fail-closed, cited to Phase 5 evidence, and tied to `src-tauri/src/session_metadata/` plus component fixtures. I found no regressed prior findings, no new design surface outside actions A-F, and no fresh R3 finding.

## R1 / R2 closure regression check

| ID | Round | Severity | Status | Evidence |
| --- | --- | --- | --- | --- |
| R1-F01 | 1 | HIGH | closed, not regressed | D5 still has a §9.1 row: no `--state-db`, `open_default()` only, no GUI state DB integration (`proposals/06-locate.md:259`). |
| R1-F02 | 1 | HIGH | closed, superseded by Codex fold-in | Rev 3 no longer defers Codex; A4 and §4 step 8 commit to `session_meta.payload.cwd` with fail-closed validation, backed by Phase 5 sampling (`proposals/06-locate.md:60`, `proposals/06-locate.md:144`). |
| R1-F03 | 1 | MEDIUM | closed, not regressed | §8 still classifies the only allowed write-like side effect: locator adapter `state_dir` mkdir, with no file writes inside it (`proposals/06-locate.md:243`). |
| R1-F04 | 1 | MEDIUM | closed, not regressed | §4 step 3 still commits to `ProvidersConfig::load(...).unwrap_or_default()` and `SessionsConfig::load(...).unwrap_or_default()` parity with resume (`proposals/06-locate.md:137`). |
| R1-F05 | 1 | advisory | closed, tightened | Claude path-hash inversion now enumerates all decompositions and accepts exactly one existing decoded path; zero or multiple existing paths exit `12` (`proposals/06-locate.md:143`). |
| R1-F06 | 1 | advisory | closed, not regressed | §11.1/§12 still avoid a `migrate-db` repair promise and record partial-chain DB residual behavior (`proposals/06-locate.md:300`, `proposals/06-locate.md:315`). |
| R1-F07 | 1 | cosmetic | closed, not regressed | §12 still records the future pause-handshake lock condition for `mutable` (`proposals/06-locate.md:316`). |
| R1-F08 | 1 | cosmetic | closed, not regressed | The module path remains committed as `src-tauri/src/session_metadata/` in scope/API sections (`proposals/06-locate.md:16`, `proposals/06-locate.md:166-170`). |
| R1-F09 | 1 | cosmetic | closed, not regressed | README work still frames `mutable: true` as a read-time eligibility hint, not mutation permission or write safety (`proposals/06-locate.md:278`). |
| R2-F01 | 2 | LOW | closed | The old "pick first" ambiguity is gone; §4 step 8 is now implementable without consulting §9.1 (`proposals/06-locate.md:143`). |
| R2-F02 | 2 | cosmetic | unchanged accepted residual | Malformed provider/session config still follows resume parity via `unwrap_or_default()` and fails closed downstream; Rev 3 does not change this inherited limitation (`proposals/06-locate.md:137`). |

## Rev 3 watchpoints

### W1 A4 invalidator form

OK. A4's invalidators are concrete enough for future gates: Claude failure is observable as inability to derive exactly one existing local root; Codex drift names a specific field and gives examples ("nests it differently" or "makes it optional"); the storage-with-no-provenance clause is harness-requirement falsifiable (`proposals/06-locate.md:60`). The Codex clause is no longer the already-fired Phase 5 trigger.

Checked clauses:

- Claude: falsified by fixture or real path hashes where zero or multiple existing roots result.
- Codex: falsified by an upstream schema where `payload.cwd` is absent, optional, or moved.
- Other storage: falsified by a harness requirement for a provider with no path/config provenance.

### W2 Codex derivation spec

OK. §4 step 8 specifies a line-by-line scan until `session_meta`, extraction of `session_meta.payload.cwd`, absolute/canonical/existing/UTF-8 validation, and `12 unsupported-storage` for missing, malformed, absent, relative, non-existing, or non-UTF-8 root data (`proposals/06-locate.md:144`). It cites parser home `src-tauri/src/session_metadata/`, the existing line-walk precedent `scripts/codex-locate-transcript`, and Phase 5 §I.WS1 evidence inline.

The "one per file by Codex convention" claim is consistent with the cited line-walk script shape, which scans JSONL lines for `type == "session_meta"` and checks its payload (`scripts/codex-locate-transcript:45-60`). Phase 5 hookpoints record a sanitized 25-file rollout sample; each sampled file had exactly one `session_meta` record and `payload.cwd`. I found no contradictory evidence in the cited script or sanitized sample record. Line-iteration timeout/file-size limits are not separately specified, but the parser is streaming a single already-located local JSONL and follows the existing locator precedent, so I am not treating that as a Rev 3 finding.

Failure-mode coverage:

- Missing `session_meta`: specified as exit `12` and fixture-pinned in D7.
- Absent `payload.cwd`: specified as exit `12` and fixture-pinned in D7.
- Invalid root value: relative, non-existing, or non-UTF-8 values are specified and fixture-pinned.
- Malformed metadata: specified in §4 step 8; adequate for proposal risk gate even though Phase 6 can add a direct malformed-line fixture.

### W3 Claude paragraph tightening

OK. R2-F01 is closed. The branch now requires enumerating all candidate decompositions, checking existence for every candidate, succeeding only on exactly one existing decoded path, and exiting `12` on zero or two-or-more existing paths (`proposals/06-locate.md:143`). That is unambiguous without relying on §9.1.

This avoids the R2 short-circuit reading because implementation must know the full existing-candidate cardinality before returning success.

### W4 D7 row update

OK. The D7 row covers Claude success, Codex success, shared absent/relative/non-existing/non-UTF-8 failures, missing `session_meta`, and absent `payload.cwd`. The fixture column lists Claude temp JSONL/provider storage dirs, a Codex success fixture with `session_meta.payload.cwd`, and Codex failure fixtures for missing `session_meta`, absent `payload.cwd`, and invalid paths (`proposals/06-locate.md:263`). The neighboring D7 ambiguity row still covers Claude zero/one/multiple decomposition fixtures (`proposals/06-locate.md:264`).

No fixture drift found: the row now describes both storage families instead of carrying the Rev 2 Codex unsupported fixture.

### W5 §12 residual removal

OK. The Rev 2 Codex-deferral residual is gone from §12. Remaining residuals are the expected physical read-only open, GUI DB divergence, strict multi-row ambiguity, workspace-root rejection, `other` storage, partial-chain DBs, and future mutable lock condition (`proposals/06-locate.md:310-316`). Searches found no §12 residual that still says "Codex deferred" or "Phase 5 hookpoint"; the only remaining "deferred" in Rev 3 is the read-only-open sequencing row outside the Codex branch (`proposals/06-locate.md:325`).

Residual inventory:

- Physical read-only open remains assigned to 06-schema-probe.
- GUI DB divergence remains out of scope.
- Multi-row ambiguity remains resolver-parity only.
- Workspace-root rejection remains a conservative failure.
- `other` storage remains success-capable only with proven transcript and root.
- Partial-chain and future-lock residuals remain present.

### W6 Rev 3 changes block truthfulness

OK. Spot-check 1: the changes block says Codex derivation was folded into A4/§4/§9.1/§12 and the deferral residual dropped (`proposals/06-locate.md:42-47`); A4, §4 step 8, and D7 now all specify `session_meta.payload.cwd`, while §12 has no Codex-deferral residual (`proposals/06-locate.md:60`, `proposals/06-locate.md:144`, `proposals/06-locate.md:263`, `proposals/06-locate.md:310-316`).

OK. Spot-check 2: the changes block says the Claude paragraph tightened R2-F01 by enumerating all decompositions and succeeding iff exactly one exists (`proposals/06-locate.md:48-49`); §4 step 8 now says exactly that (`proposals/06-locate.md:143`).

### W7 R1/R2 regression

OK. All R1 findings remain closed at their intended closure points, R2-F01 is closed by Rev 3, and R2-F02 remains an accepted inherited limitation rather than a locate-specific regression. No same-label or same-family path-hash oscillation returns in Rev 3.

### W8 Citation spot-check

- OK — `scripts/codex-locate-transcript` is cited for the reusable JSONL line-walk pattern; the script opens rollout files as UTF-8, iterates lines, parses JSON, and selects `session_meta` payloads (`proposals/06-locate.md:144`; `scripts/codex-locate-transcript:45-60`).
- OK — `research/06-locate-hookpoints.md` §I.WS1 is cited for Phase 5 evidence; the research records 5,739 local rollout files and a 25-file sample with `session_meta.payload.cwd` present and `payload.workspace_root` absent (`proposals/06-locate.md:60`, `proposals/06-locate.md:144`; `research/06-locate-hookpoints.md:164-176`, `research/06-locate-hookpoints.md:381-390`).
- OK — `src-tauri/src/session_metadata/` remains the parser/API home and is cited in scope, API, and the Codex parser paragraph (`proposals/06-locate.md:16`, `proposals/06-locate.md:166-215`, `proposals/06-locate.md:144`).
- OK — problem-map A4 was updated with Phase 5 evidence and Codex drift invalidator, matching Rev 3 A4 at proposal level (`research/06-locate-problem-map.md:137`; `proposals/06-locate.md:60`).

## Findings

None.
