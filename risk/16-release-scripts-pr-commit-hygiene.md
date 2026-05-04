# WU-16-01 PR Commit Hygiene

## Verdict
PASS

## Checklist
- [x] One commit per concern
- [x] Conventional-commits header
- [x] Body explains why
- [x] Anti-scope acknowledged
- [x] No skipped hooks
- [x] Author/committer convention

## Findings

None.

## Observations

Branch shape:

- `git log main..HEAD` reports exactly one commit:
  `b4bac1c fix(release): ship adapter scripts as release assets`.
- The single commit is a coherent release-install concern. The product
  edits are limited to the release publish surface, release-contract test,
  and install documentation:
  `.github/workflows/release.yml`, `src-tauri/tests/release_yml_contract.rs`,
  `README.md`, and `scripts/README.md`.
- The added research, proposal, contract, and risk/process-tree artifacts
  support the same WU-16-01 release-script install gap rather than adding
  a separate product concern.
- No fixup, WIP, or unrelated cleanup commit appears in the visible branch
  history.

Header:

- Current header:
  `fix(release): ship adapter scripts as release assets`.
- The header follows the project's conventional-commits pattern:
  `<type>(<scope>): <imperative summary>`.
- Scope `release` matches the touched surface and aligns directly with
  prior WU release precedent `754ebb8 fix(release): restore Windows port +
  per-platform bare-binary names (#38)`.
- Tone is consistent with nearby project commits that use specific behavior
  summaries instead of generic wording:
  `bc6df8e fix(state): persist session turn bodies in state.db (#40)`,
  `e9649a1 fix(resume): put migrated transcripts where Claude looks (#39)`,
  and `754ebb8 fix(release): restore Windows port + per-platform bare-binary
  names (#38)`.
- The header is shorter and narrower than the WU-13 precedent, which is
  appropriate because this branch has one release asset concern rather than
  Windows restoration plus asset naming.

Body quality:

- The body explains why the change exists: WU-15-01 install QA found that
  v0.1.26 shipped a body-aware binary while release/bundle installers did
  not deliver the matching adapter scripts, causing stale local scripts to
  keep `session_turns.body` NULL.
- The body names the behavioral risk in user terms: binary-install users
  can receive the fixed runner while silently retaining stale scripts.
- The body describes the implementation at review-relevant granularity:
  seven adapter scripts are uploaded as individual release assets alongside
  existing Tauri bundles and bare binaries.
- The body lists the seven assets explicitly:
  `claude-code-turns`, `codex-turns`, `anthropic-usage`,
  `chatgpt-usage`, `zai-usage`, `claude-code-locate-transcript`,
  and `codex-locate-transcript`.
- The body ties the structural test change to its purpose: exact
  membership over the publish step's `files:` input makes future adapter
  additions require deliberate test edits.
- The README documentation change is explained as the binary-install path
  plus a matched-versions warning, which maps to ticket AC-3.

Anti-scope:

- The commit message has a dedicated anti-scope paragraph:
  "Anti-scope (anti-regression): the bare-binary platform-suffix contract
  from WU-13-01 stays unchanged; scripts are NOT bundled into
  .deb/.dmg/.msi; no runtime version-skew detection."
- This acknowledges the WU-13-01 release contract preservation requirement.
- This also matches the proposal and ticket anti-scope: no package bundling,
  no runtime stale-script detection, no backwards-compatibility shim, and
  no script body changes.

Pipeline artifacts:

- The commit body lists pipeline planning artifacts in a separate paragraph:
  "Pipeline planning artifacts (problem map, proposal, contract, risk
  reports, process-tree audits) travel with the code per WU-13/14/15
  precedent."
- That paragraph is separate from the behavior and anti-scope paragraphs,
  so the reason for product code remains readable independently.
- This matches the prior-WU tone where planning/risk artifacts are carried
  with implementation commits, especially `bc6df8e`, `e9649a1`, and
  `754ebb8`.

Hook and identity evidence:

- `git log -1 --format=fuller b4bac1c` shows:
  Author `nestharus <contact@nestharus.com>`.
- The same fuller log shows:
  Committer `nestharus <contact@nestharus.com>`.
- AuthorDate and CommitDate are both `Mon May 4 03:37:12 2026 -0700`.
- The commit body contains a single co-author trailer:
  `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`.
- There are no `Signed-off-by`, hook-skip, CI-skip, or GPG-skip trailers
  visible in the commit object.
- Commit-object metadata cannot independently prove that no force-push ever
  occurred, but the required inspectable evidence contains no force-push,
  skipped-hook, or skipped-signing marker.
- The author identity matches the project convention directly through
  `nestharus`; the co-author trailer also matches the allowed Claude Opus
  convention.

Commit-level assessment:

- Classification: release behavior + release-contract test + install docs
  + required WU artifacts for one install-process gap.
- Concerns count: 1.
- Message score: strong. It names the failure mode, why users are exposed,
  what is intentionally preserved, and where the planning artifacts live.
- Anti-patterns found: none. No "wip", "fixup", generic "address feedback",
  multi-concern bundling, or drop-then-restore history is visible because
  the branch has only one commit.

## Status
PASS
