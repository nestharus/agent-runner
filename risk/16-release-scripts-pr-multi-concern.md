# WU-16-01 PR Multi-Concern Review

Reviewer: Phase 8 multi-concern check (`claude-opus`)
Branch: `impl/wu-16-01` vs `main`
Commits in scope:
- `b4bac1c` — `fix(release): ship adapter scripts as release assets`

## Verdict

**SINGLE_CONCERN.**

The diff implements one install-process gap closure (binary-install
users not receiving matched adapter scripts) through the standard
implementation pipeline. Every product-file change is a load-bearing
edge of the same fix; the planning artifacts ship per the WU-13/14/15
precedent. There is no decomposition that would cut the diff into
two independently shippable PRs without breaking CI between them or
stranding doc/test signal.

## Concern enumeration

The diff has five product-file edges and one planning-artifact bundle:

1. **Publish-step expansion** — `.github/workflows/release.yml:181-189`
   replaces the single `files: artifacts/*` scalar with a multi-line
   block scalar that retains `artifacts/*` and adds the seven AC-1
   script paths. Single concern: contract §2.1.
2. **Structural test extension** — `src-tauri/tests/release_yml_contract.rs:251-280`
   replaces the scalar `assert_eq!(string_at(...), "artifacts/*")`
   with a `BTreeSet`-parsed exact-equality assertion against the
   eight expected entries. Single concern: contract §2.2.
3. **README binary-install snippet** — `README.md:352-364` inserts
   the matched-versions paragraph + `gh release download` + `chmod
   +x` block between the existing source-build snippet (`:340-350`,
   preserved byte-identical) and `## Session Ingestion` (`:366`).
   Single concern: contract §2.3 / ticket AC-3.
4. **scripts/README cross-reference** — `scripts/README.md:7-8` adds
   one line pointing to the README §Reference quota adapters
   release-asset path. Single concern: contract §2.4 (optional).
5. **Planning artifacts** — `proposals/16-release-scripts.md`,
   `research/16-release-scripts-{problem-map,hookpoints}.md`,
   `risk/16-release-scripts-{audit,scope,shortcut,supported-surface,
   test-residuals,process-tree-audit-phase4,process-tree-audit-phase6}.md`,
   and `product-strategy/contracts/wu-16-01-release-scripts.md`.
   All carry the `16-release-scripts` slug; none reference unrelated
   WUs. Single concern: implementation pipeline lifecycle output.

These five edges all serve the single concern named in the ticket
§Source: v0.1.26 shipped a body-aware binary without matched scripts,
so binary-install users silently get NULL `session_turns.body`. The
fix is "ship the scripts as release assets and document how to fetch
matching ones."

## Decomposition analysis

Not applicable (verdict is `SINGLE_CONCERN`). For completeness, the
conceivable split lines are all rejected:

- **Workflow YAML vs. structural test.** The structural test is the
  RED→GREEN signal for AC-1+AC-2. Landing the test before the YAML
  edit puts a known-RED test on `main` (every CI run for every
  unrelated PR fails). Landing the YAML before the test removes the
  pre-fix RED capture and lets future omissions go undetected.
  Contract §2.2 requires both in the same merge.
- **Workflow YAML vs. README snippet.** The README's matched-versions
  warning is meaningless without the release-asset path actually
  existing — it would document a feature that does not exist.
  Conversely, shipping release assets without telling
  binary-install users how to fetch them leaves the install-process
  gap (the literal ticket symptom) only partially closed.
- **README vs. scripts/README cross-reference.** The cross-reference
  is one line and exists only to redirect from `scripts/README.md`
  to the new README section. Splitting it into a follow-up PR is
  pure churn (no independent value, no reduction in review surface).
- **Planning artifacts vs. fix.** Per WU-13/14/15 precedent (PR #38,
  #39, #40), proposals/research/risk/contract files ship with the
  product commit they justify. Splitting them off would either
  strand the contract on `main` ahead of the test that encodes it,
  or leave the audit trail incomplete after the fix lands. Phase
  6c's RED-run capture and the contract's §5 input obligations
  bind these files to the same PR.

## Findings

### F1 — All product changes map to contract surfaces

Each touched product file maps to an enumerated `wu-16-01-release-scripts.md`
§2 surface:

- `release.yml:181-189` ↔ contract §2.1 (publish step block scalar
  with eight entries; `artifacts/*` retained, seven script paths
  added).
- `release_yml_contract.rs:251-280` ↔ contract §2.2 (extend
  `release_yml_restores_windows_and_target_suffixed_bare_binaries`,
  reuse `step_by_uses` / `string_at` / `BTreeSet`, parse-and-compare
  rule).
- `README.md:352-364` ↔ contract §2.3 (insertion between :350 and
  :352, matched-versions warning, `v0.1.X` placeholder, seven
  `--pattern` flags, `chmod +x` of all seven).
- `scripts/README.md:7-8` ↔ contract §2.4 (the verbatim one-line
  cross-reference).

### F2 — Anti-scope respected

Spot-checked against ticket §Anti-scope and contract §1
out-of-scope:

- WU-13-01 platform-suffix contract untouched: the
  `collect_step_run_bare_binary_hits` assertion at
  `release_yml_contract.rs:280` is unchanged; no edits to bare-binary
  build/upload steps in `release.yml`.
- `.deb` / `.dmg` / `.msi` Tauri bundle config unchanged.
- The seven adapter scripts themselves are not edited (no
  `scripts/claude-code-turns`, `scripts/codex-turns`, etc. in the
  diff).
- `scripts/migrate-model-names.sh` and `scripts/tests/` are not in
  the publish list.
- No frontend (`src/`) or Rust runtime (`src-tauri/src/`) changes.
- No script-versioning logic, no stale-script detection, no
  PATH-modification helpers.

### F3 — Source-build snippet preserved byte-identical

`README.md:340-350` (the `install -m 755 \ scripts/claude-code-turns
... ~/.local/bin/` block) is unchanged in the diff. The new
binary-install paragraph is inserted *after* line 350 and *before*
line 352 (`## Session Ingestion`), exactly as contract §2.3 / Phase
5 hookpoints §2 entry 3 require. AC-3's "source-build snippet
remains valid" obligation is met by non-modification.

### F4 — Single commit, contract-compliant subject

The PR is a single commit `b4bac1c fix(release): ship adapter scripts
as release assets`. Subject describes the user-visible delivery
(scripts shipped as release assets), matches the ticket §Source
narrative, and follows the prior `fix(release): ...` pattern from
PRs #38 / #36. No tagalong refactors hidden in the commit body.

### F5 — Planning artifacts are scoped to WU-16-01

All eight planning files carry the `16-release-scripts` slug; none
reference unrelated WUs. The contract is the WU-16-01 contract; the
proposal/research/risk filenames all begin with `16-`. Two
process-tree audits (phase4, phase6) are present, matching the
WU-15-01 pattern.

## Observations

- The diff is small in product surface (10 lines of YAML, 22 lines
  of test parse-and-compare, 13 lines of README, 2 lines of
  scripts/README) and large in planning artifacts (~3,100 lines)
  — that ratio matches the WU-14-01 / WU-15-01 precedent for
  short-fix WUs that nonetheless went through the full
  implementation pipeline. Planning-artifact size is not a
  decomposition signal.
- The structural test now uses `BTreeSet` exact-equality, so
  adding an eighth adapter script in a future WU will deliberately
  force an edit to this test (RISK-01 in contract §6). This is
  desired drift-detection, not a maintenance trap.
- No decision-log entry is added in this PR. The contract does not
  require one and ticket AC-8 (the WU-15-01 DECISIONS pattern) is
  absent from the WU-16-01 ticket. Correct omission, not a gap.

## Status

Verdict: **SINGLE_CONCERN.** Ready to advance.
