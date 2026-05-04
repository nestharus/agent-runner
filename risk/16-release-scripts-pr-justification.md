# WU-16-01 PR Justification Review

Reviewer: claude-opus (Phase 8 justification)
Branch HEAD: `b4bac1c fix(release): ship adapter scripts as release assets`
Base: `main` (`bc6df8e fix(state): persist session turn bodies in state.db (#40)`)
Diff command: `git diff main..HEAD`

## Verdict

LOW_CONCERN

Every code/config/doc hunk traces directly to an AC and to the Phase 6a
contract. No drive-by cleanup, no speculative abstractions, no
behavior changes outside the install-process gap. All anti-scope items
verified untouched.

## Per-file justification table

| file | hunks | trace | verdict |
|---|---|---|---|
| `.github/workflows/release.yml` | 1 hunk (+8/-1) at L178-189: `files: artifacts/*` → block scalar adding the seven AC-1 script paths | AC-1 (each script uploaded as release asset); Contract §2.1 (Option A, YAML block scalar, `artifacts/*` retained, no other entries); proposal §6 / hookpoints Q-G | PURPOSEFUL |
| `src-tauri/tests/release_yml_contract.rs` | 1 hunk (+24/-7) at L251-280: single-scalar `assert_eq!` → trimmed-line `BTreeSet` exact-equality over 8 entries | AC-2 + AC-6 (extend WU-13 structural test, exact set membership prevents silent broadening or omission); Contract §2.2 (split/trim/filter/`BTreeSet`, reuse `step_by_uses`, `string_at`, existing `BTreeSet` import); proposal §4 AC-1 | PURPOSEFUL |
| `README.md` | 1 hunk (+13) inserted between L350 and L352: prose paragraph + fenced `bash` block | AC-3 (binary-install snippet adjacent to source-build snippet, matched-versions warning); Contract §2.3 (insertion point, 7 `--pattern` flags, `chmod +x`, `v0.1.X` placeholder, source-build snippet byte-identical) | PURPOSEFUL |
| `scripts/README.md` | 1 hunk (+2) at L5-7: one-line cross-reference | AC-3 optional; Contract §2.4 verbatim (`"For release-asset installation of the bundled reference adapters, see README §Reference quota adapters."`) | PURPOSEFUL |
| `proposals/16-release-scripts.md` | new file (670 lines) | Phase 3 proposal artifact required by pipeline | PURPOSEFUL (process artifact) |
| `product-strategy/contracts/wu-16-01-release-scripts.md` | new file (241 lines) | Phase 6a contract artifact (binding interface between 6b and 6c) | PURPOSEFUL (process artifact) |
| `research/16-release-scripts-problem-map.md` | new file (485 lines) | Phase 1 problem map artifact | PURPOSEFUL (process artifact) |
| `research/16-release-scripts-hookpoints.md` | new file (599 lines) | Phase 5 hookpoints artifact | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-audit.md` | new file (193 lines) | Phase 4 audit artifact | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-scope.md` | new file (109 lines) | Phase 4 scope artifact | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-shortcut.md` | new file (253 lines) | Phase 4 shortcut artifact | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-supported-surface.md` | new file (256 lines) | Phase 4 supported-surface artifact | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-process-tree-audit-phase4.md` | new file (90 lines) | Phase 4 process audit | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-process-tree-audit-phase6.md` | new file (67 lines) | Phase 6 process audit | PURPOSEFUL (process artifact) |
| `risk/16-release-scripts-test-residuals.md` | new file (129 lines) | Required residual-risk artifact for AC-3 (doc-only), AC-5 (live CI), AC-6 (live release); promised in proposal §4 and contract §2.3, §3 | PURPOSEFUL (process artifact) |

## Anti-scope check

Verified against ticket §"Anti-scope" + proposal §1 + contract §1
out-of-scope list:

- Bare-binary platform-suffix contract (WU-13-01): UNCHANGED.
  - No edits to `.github/workflows/release.yml:139-158` (collect
    steps); only L178-189 (publish step) touched.
  - `collect_step_run_bare_binary_hits` assertion at the test's
    `:269-278` region remains in place — visible as untouched context
    immediately after the modified hunk.
- `.deb` / `.dmg` / `.msi` Tauri bundle contents: UNCHANGED.
  - No edits under `src-tauri/tauri.conf.json` or any platform
    collect step.
- The seven adapter scripts themselves: UNCHANGED.
  - `git diff main..HEAD -- 'scripts/*' ':!scripts/README.md'`
    returns empty.
- Runtime version-skew detection: NOT ADDED.
  - No edits under `src-tauri/src/` or `src/`.
- `scripts.tar.gz` bundle: NOT CREATED.
  - Publish step lists individual files, matching ticket
    recommendation.
- Adding scripts to system `PATH` automatically: NOT DONE.
  - README snippet remains an explicit user `gh release download` +
    `chmod +x` action.
- Frontend (`src/`) and Rust runtime (`src-tauri/src/`): UNCHANGED.
  - `git diff` confirms no files outside the four declared
    in-scope code/doc files plus pipeline artifacts.
- `scripts/migrate-model-names.sh`, `scripts/tests/`,
  `scripts/README.md` (as a release asset): NOT uploaded.
  - Publish step's `files:` list contains exactly the seven AC-1
    scripts plus `artifacts/*`; no `scripts/README.md` entry, no
    `scripts/migrate-model-names.sh` entry, no `scripts/tests/`
    entry.

## Findings

None at LOW_CONCERN level. The diff is minimal and every hunk maps
1:1 to a contract section.

## Observations

- The publish-step hunk and the structural-test hunk are co-located
  with their WU-13-01 predecessors; the structural test is extended
  in place (per contract Q-H decision) rather than as a sibling test.
  This is the proposal's stated preference and keeps publish-step
  ownership in one assertion block.
- The structural test now uses `BTreeSet` exact equality (not
  superset / regex / glob), which would catch both silent broadening
  (e.g., a future careless `scripts/*`) and silent omission of any
  AC-1 script. This satisfies RISK-01 from contract §6.
- README snippet preserves the `v0.1.X` placeholder verbatim, as
  specified by contract §2.3 (RISK-03 acknowledged: no
  auto-substitution promised).
- The optional `scripts/README.md` cross-reference is the
  byte-exact line specified in contract §2.4; no duplicate install
  block was introduced.
- DECISIONS.md is intentionally not edited; the pipeline doc
  states consolidation happens at WU close.
- Pipeline artifacts (proposals, research, risk, contract) account
  for ~3,100 of the 3,139 added lines; only ~50 net lines of
  product/test/doc change.
- Residual-risk artifact `risk/16-release-scripts-test-residuals.md`
  is present, as required by proposal §4 (AC-1, AC-2, AC-4, AC-5,
  AC-6 residual paths) and contract §3 (AC-3 residual path).

## Status

LOW_CONCERN
