# WU-16-01 Shortcut Risk

Phase: 4 shortcut-risk gate
Inputs:
- `proposals/16-release-scripts.md`
- `research/16-release-scripts-problem-map.md`
- `tmp/scratch/wu-16-01/ticket.md`
- `risk/13-release-restore-shortcut.md` (precedent)

Scope: judge whether the proposal's chosen path delivers WU-16-01's
stated value (matched binary + adapter scripts on the release page,
discoverable via README) rather than only its appearance.

## Verdict

LOW

## Findings

### SHORTCUT-01

- severity: NON-BLOCKING
- location: proposal §6 "Publish-step extension" and §4 AC-1
  (`proposals/16-release-scripts.md:516-528`,
  `proposals/16-release-scripts.md:336-366`).
- statement: Option A (explicit seven-entry list) is preferred over
  a `scripts/*` glob. A future eighth adapter added under `scripts/`
  could ship with the binary but be silently omitted from the
  release-asset list because neither `release.yml` nor the
  structural test would auto-pick it up.
- evidence:
  - The publish-step assertion is set-equality over `artifacts/*` plus
    seven exact paths via `BTreeSet<String>` string comparison
    (`proposals/16-release-scripts.md:357-362`).
  - That assertion catches accidental removal or accidental over-broad
    glob (e.g. `scripts/*` that drags in `scripts/migrate-model-names.sh`
    or `scripts/README.md`); it does NOT catch "added a new adapter to
    `scripts/` and forgot to extend the release list" because the test
    has no awareness of future adapters.
  - Anti-scope and AC-1 explicitly bound the asset set to seven names
    (`tmp/scratch/wu-16-01/ticket.md:42-53`), so adding an eighth
    adapter is itself a future WU that will need its own AC-1 update,
    its own structural-test edit, and its own README snippet edit.
  - The WU-13-01 precedent uses the same explicit-naming pattern for
    bare binaries and is treated as the canonical shape
    (`proposals/13-release-restore.md:254-274`,
    `risk/13-release-restore-shortcut.md:244-275`).
- closure expectation: Phase 6 must implement the assertion as exact
  set equality (NOT `contains_any` / regex / glob inference). A future
  adapter add is then a deliberate three-way edit (release.yml, test,
  README), which is the intended drift surface — the structural test
  forces the maintainer to touch the release list whenever they touch
  the test, and vice versa. Drift to a `scripts/*` glob in either
  release.yml or the test would re-open this as a MEDIUM shortcut
  because it would silently include `migrate-model-names.sh` /
  `tests/` content.

### SHORTCUT-02

- severity: NON-BLOCKING
- location: proposal §4 AC-1 / AC-2 / AC-4 / AC-6 residual-risk
  paragraphs and §7 question 5
  (`proposals/16-release-scripts.md:363-366`,
  `proposals/16-release-scripts.md:395-397`,
  `proposals/16-release-scripts.md:432-435`,
  `proposals/16-release-scripts.md:473-479`,
  `proposals/16-release-scripts.md:642-645`).
- statement: The structural test guards workflow shape, NOT live
  release-asset materialization on the GitHub release page. This is
  the same boundary WU-13-01 hit and is properly disclosed here.
- evidence:
  - AC-1 residual risk explicitly says "structural YAML cannot prove
    GitHub's release page contains uploaded assets after a live release"
    and routes to `risk/16-release-scripts-test-residuals.md` if no
    trial release is run (`proposals/16-release-scripts.md:363-366`).
  - AC-2 residual risk requires the same residuals doc when no trial
    release is performed (`proposals/16-release-scripts.md:395-397`).
  - WU-13-01 set the precedent that live-release evidence is separate
    from structural merge-time coverage
    (`risk/13-release-restore-shortcut.md:118-145`).
- closure expectation: Phase 6b either runs a real `workflow_dispatch`
  trial release with an asset listing demonstrating the seven script
  basenames, OR writes `risk/16-release-scripts-test-residuals.md`
  naming the unverified live-release residual. Substituting only the
  structural test for AC-2 evidence without the residuals doc would
  re-open this as a MEDIUM shortcut.

### SHORTCUT-03

- severity: NON-BLOCKING
- location: proposal §6 "README snippet" and §4 AC-3
  (`proposals/16-release-scripts.md:569-579`,
  `proposals/16-release-scripts.md:399-418`); problem-map flag at
  `research/16-release-scripts-problem-map.md:329-336`.
- statement: The proposal preserves the existing source-build snippet
  unchanged. That snippet installs only five of the seven AC-1
  adapters (omits both transcript locators), so source-build users
  who follow it continue to under-install — the same gap-shape
  WU-16-01 is closing for binary-install users.
- evidence:
  - Existing source-build snippet at `README.md:340-350` installs
    `anthropic-usage`, `chatgpt-usage`, `zai-usage`,
    `claude-code-turns`, `codex-turns` — five of seven; locator
    scripts are absent (problem map records this at
    `research/16-release-scripts-problem-map.md:333-336`).
  - The new binary-install snippet in §2 lists all seven via
    `--pattern` plus all seven in `chmod +x`
    (`proposals/16-release-scripts.md:170-179`).
  - The proposal explicitly chooses not to expand the source-build
    snippet: "The primary AC-3 requirement is that it remains valid,
    not that it is replaced or expanded for parity with the
    release-asset snippet" (`proposals/16-release-scripts.md:573-575`).
  - Ticket AC-3's exact wording is "§Reference quota adapters install
    snippet remains valid for source-builds"
    (`tmp/scratch/wu-16-01/ticket.md:60-68`) — interpretively this
    permits the proposal's reading, but does not require it.
- statement (continued): This narrowing is consistent with the v1
  install-QA scope (binary-install users were the broken cohort) and
  does NOT compromise the stated fix. Source-build users have
  repo-clone access and could always edit the snippet themselves; the
  bug-class WU-16-01 closes (silent stale-script body omission for
  binary installers) is fully addressed by the new binary-install
  snippet, which lists all seven.
- closure expectation: Phase 6 must ensure the binary-install snippet
  inserted after `README.md:350` does name all seven adapters
  (verbatim from §2's snippet), so the supported-cohort fix is
  complete. Either expanding the source-build snippet to all seven
  in the same edit, OR documenting in the implementation notes that
  the source-build under-install is preexisting and out of WU-16-01's
  AC-3 scope, is acceptable. Silently dropping any of the seven
  scripts from the new binary-install snippet would re-open this as
  HIGH because it would re-introduce the install-QA gap.

### SHORTCUT-04

- severity: NON-BLOCKING
- location: proposal §2 binary-install snippet and §6 README snippet
  (`proposals/16-release-scripts.md:170-179`,
  `proposals/16-release-scripts.md:579`).
- statement: The README snippet uses a literal `v0.1.X` placeholder
  rather than a release-tag substitution that auto-resolves. Users
  who copy-paste without editing get a `gh release download` failure,
  not a silently-stale install. This is the failure-loud direction
  and matches the ticket's suggested form
  (`tmp/scratch/wu-16-01/ticket.md:143-154`).
- evidence:
  - Snippet at `proposals/16-release-scripts.md:171` is literally
    `gh release download v0.1.X --repo nestharus/agent-runner ...`.
  - The note required by §2 explicitly tells users to install scripts
    from the same release tag as the binary and that mismatched stale
    scripts may silently omit `body`
    (`proposals/16-release-scripts.md:185-189`).
- closure expectation: Phase 6 must keep the version-placeholder
  visible (not a hidden default) and must include the matched-version
  warning prose adjacent to the snippet so users understand why the
  version field matters. Replacing `v0.1.X` with a hardcoded current
  tag that immediately rots, or omitting the matched-version warning
  prose, would convert this to a MEDIUM shortcut against AC-3.

### SHORTCUT-05

- severity: NON-BLOCKING
- location: proposal §6 "Optional `scripts/README.md` cross-reference"
  and §7 question 3
  (`proposals/16-release-scripts.md:581-587`,
  `proposals/16-release-scripts.md:626-631`).
- statement: The cross-reference in `scripts/README.md` is marked
  optional and recommended. If omitted, users who navigate directly
  to `scripts/README.md` (the adapter-contract doc) will not see a
  pointer to the release-asset install path; they would have to
  bounce out to the top-level README to find it.
- evidence:
  - `scripts/README.md` documents adapter contracts and explicitly
    states scripts are standalone executables not linked into the
    binary (`scripts/README.md:1-5`); it does not currently include
    install instructions of any kind.
  - Top-level `README.md` §"Reference quota adapters" remains the
    canonical install entry point under both source-build and
    binary-install cohorts (`README.md:332-350` plus the new
    snippet).
  - Ticket marks `scripts/README.md` optional in Code Boundary
    (`tmp/scratch/wu-16-01/ticket.md:91-93`).
- closure expectation: Recommended close: include the one-line
  cross-reference per the proposal's preference. Skipping it does
  not compromise the install-QA fix because the canonical install
  doc is the top-level README. Adding more than a one-line pointer
  (e.g. duplicating the binary-install snippet into
  `scripts/README.md`) would re-open this as MEDIUM because it
  creates a second install procedure that can drift out of sync.

### SHORTCUT-06

- severity: NON-BLOCKING
- location: proposal §4 AC-1 assertion shape
  (`proposals/16-release-scripts.md:357-362`).
- statement: The structural-test extension is granular enough that
  adding or removing any adapter from the release list requires a
  deliberate test edit; it cannot pass with a partial list.
- evidence:
  - Assertion is parse `with.files` as `Value::String`, split lines
    via `files.lines().map(str::trim).filter(|line| !line.is_empty())`,
    collect into `BTreeSet<String>`, and use string equality against
    the canonical eight-entry set (`artifacts/*` plus the seven
    script paths).
  - The proposal explicitly rejects regex and glob matching as
    "stricter than regex and stricter than glob matching; it
    prevents accidental broad upload of `scripts/*` or omitted
    scripts while preserving order independence"
    (`proposals/16-release-scripts.md:360-362`).
- closure expectation: Phase 6 must implement the assertion as exact
  set equality. Any relaxation to "contains all of" without also
  asserting "and nothing else" would silently allow `scripts/*` glob
  drift; any relaxation to "contains at least one script path" would
  let a partial list pass. Either weakening would re-open this as a
  MEDIUM shortcut against AC-1.

## Observations

- The proposal's explicit-list choice (Option A) and the structural
  test's set-equality assertion form a coherent invariant: the
  release file list is the single source of truth, and any change
  to it requires a paired test edit. This matches the WU-13-01
  bare-binary contract pattern and inherits its drift bounds
  (`risk/13-release-restore-shortcut.md:244-275`).
- The seven-script set is anchored in three places — ticket AC-1
  (`tmp/scratch/wu-16-01/ticket.md:42-53`), proposal §4 AC-1
  expected-observable signal (`proposals/16-release-scripts.md:351-356`),
  and proposal §2 binary-install snippet patterns
  (`proposals/16-release-scripts.md:170-179`). Phase 6 must keep
  these three lists identical and in the same order-independent
  shape; divergence between any two would constitute a new shortcut
  finding.
- The residual-risk discipline is consistent: AC-1, AC-2, AC-4,
  AC-5, and AC-6 all route to `risk/16-release-scripts-test-residuals.md`
  if their structural / live-release evidence is incomplete
  (`proposals/16-release-scripts.md:481-485`). This matches the
  WU-13-01 evidence-record contract and avoids the "structural test
  pretending to be live-release evidence" failure mode flagged in
  the WU-13 precedent.
- AUDIT-01 closure removed YAML target blocks and set-literal
  assertion blocks from §6, leaving §4 AC-1 as the single
  authoritative location for the assertion technique
  (`proposals/16-release-scripts.md:649-658`). This avoids the
  "two specs of the same thing that drift" anti-pattern.
- The narrowings the proposal does take — source-build snippet not
  expanded to seven (SHORTCUT-03), `scripts/README.md` cross-ref
  optional (SHORTCUT-05), trial release deferred to residuals doc
  (SHORTCUT-02) — are all explicitly disclosed and reasoned, not
  hidden. None compromise the binary-install fix.

## Status

LOW
