# WU-16-01 PR Test Audit

## Verdict
LOW

Audited `git diff main..HEAD` at current commit
`b4bac1cd7b30bdc030eb0b154344c8d5483c9a3d`
(`fix(release): ship adapter scripts as release assets`).

## Per-AC coverage table

AC | encoded? | test fn / residual | evidence
---|---|---|---
AC-1 | Yes | `src-tauri/tests/release_yml_contract.rs::release_yml_restores_windows_and_target_suffixed_bare_binaries`, lines 253-278 | The test locates `softprops/action-gh-release@v2` through `step_by_uses`, reads `with.files` through `string_at`, parses trimmed non-empty lines into a `BTreeSet`, and asserts exact equality against eight entries: `artifacts/*` plus `scripts/anthropic-usage`, `scripts/chatgpt-usage`, `scripts/claude-code-locate-transcript`, `scripts/claude-code-turns`, `scripts/codex-locate-transcript`, `scripts/codex-turns`, and `scripts/zai-usage`. The actual workflow lists the same entries in `.github/workflows/release.yml:181-189`.
AC-2 | Yes | Same test, lines 253-289; helper reuse at lines 343-361 | The structural extension replaces only the old scalar `with.files == "artifacts/*"` assertion with exact set equality. It reuses `step_by_uses` and `string_at`; the existing `BTreeSet` import remains at line 2. The WU-13-01 `collect_step_run_bare_binary_hits` assertion remains present at lines 280-289 and still requires only Linux, macOS, and Windows collect steps to contain the target-suffixed bare binary.
AC-3 | Doc-only, residual documented | `risk/16-release-scripts-test-residuals.md`, lines 37-60 | The contract routes AC-3 to documentation review rather than executable coverage. The residual file explicitly says AC-3 is not encoded in `release_yml_contract.rs`, and records the required reviewer checks: insertion next to `## Reference quota adapters`, all seven `gh release download --pattern` flags, the matched-version warning, and the stale-script `body` omission failure mode. The README snippet appears at `README.md:352-363`.
AC-4 | Yes, with live-CI residual | Same structural test plus residual coverage in `risk/16-release-scripts-test-residuals.md`, lines 83-103 | AC-4's structural signal is the AC-2 test passing on the post-fix workflow. The GREEN log at `tmp/scratch/wu-16-01/phase6/release-yml-contract-green-run.log` shows `1 passed; 0 failed`. Live CI/platform execution is not encoded by the structural test; the residual artifact records the live CI dependency under CI residuals.
AC-5 | Residual by design | `risk/16-release-scripts-test-residuals.md`, lines 83-103 | The residual artifact lists the Rust gate obligations (`cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --no-fail-fast`), frontend gate documentation, and live Linux/macOS CI dependency. Step 6c log records Rust gates as passed and frontend gates as environment-blocked by missing dependencies/tools, not by this YAML/docs change.
AC-6 | Yes structurally, live release residual documented | Same test, existing assertions at lines 14-250 and 280-289; residual lines 105-129 | Existing WU-13-01 assertions still guard matrix rows, bundle collection for Linux/macOS/Windows, artifact upload/download shape, and target-suffixed bare binaries. The residual file separately records that live `.deb`, `.dmg`, `.msi`, NSIS `.exe`, and bare-binary release-page materialization remains outside structural merge-time coverage.

## Findings

ID | severity | section | statement | closure
---|---|---|---|---
None | none | n/a | No blocking or ordinary fix-pass findings found in the WU-16-01 test surface. | Phase 8 Test Audit can clear with LOW.

## Observations

- Proposal §4 test intent requires a particular-integration structural YAML
  test for AC-1/AC-2, exact `BTreeSet` equality over `with.files`, and residual
  documentation for non-executable/live-release gaps. The branch implements
  that shape.
- The audited diff is tightly scoped to the release workflow, README docs,
  `scripts/README.md`, the residual artifact, and
  `src-tauri/tests/release_yml_contract.rs`. No runtime Rust module, frontend
  view, adapter script body, or installer packaging config is changed.
- The release workflow edit uses the contracted block scalar shape:
  `files: |` followed by one non-empty path/glob per line. That is the shape
  the test parses, and it keeps `artifacts/*` visible rather than staging
  scripts into the existing artifact directory.
- The expected set deliberately excludes `scripts/README.md`,
  `scripts/tests/`, and `scripts/migrate-model-names.sh`. Exact equality means
  a future broad `scripts/*` upload or a missing adapter path fails the test.
- Contract §3 maps AC-1 and AC-2 to
  `release_yml_restores_windows_and_target_suffixed_bare_binaries`; AC-3 is
  intentionally doc-only; AC-5 and AC-6 keep live CI/release materialization as
  residual evidence. The Step 6b output index records AC-1/2/4/5/6 against the
  same test and records AC-3 as residual-only.
- Contract §4's Step 6b output-index obligations are present: AC mapping,
  explicit AC-3 doc-only statement, RED-run log reference, and "No Step 6c
  product code written." The Phase 6 process-tree audit independently records
  the same companion artifacts as present.
- RED firstness evidence is present:
  `release-yml-contract-red-run.log` shows the new assertion failing on the
  pre-fix workflow with `left: {"artifacts/*"}` and `right:` equal to the full
  eight-entry expected set.
- The RED run is semantically meaningful for AC-1: it fails before the release
  workflow contains the script paths, not because of parser setup, missing test
  infrastructure, or unrelated workflow damage.
- GREEN evidence is present:
  `release-yml-contract-green-run.log` shows
  `release_yml_restores_windows_and_target_suffixed_bare_binaries ... ok` and
  `test result: ok. 1 passed; 0 failed`.
- The GREEN run is the same target named in the contract:
  `cargo test --test release_yml_contract --no-fail-fast`, captured after the
  release workflow and docs edits.
- No changed test is skipped, ignored, or deleted. The only changed test file is
  `src-tauri/tests/release_yml_contract.rs`, and the diff strengthens the
  publish-step assertion from one scalar value to exact membership over all
  required release assets.
- No test weakening was found. Existing matrix, bundle, artifact upload,
  artifact download, and target-suffixed bare-binary assertions remain in the
  same test. The `collect_step_run_bare_binary_hits` helper and assertion were
  not removed.
- The residual artifact lists the structural test's non-executable limits:
  AC-3 README review, AC-2 live release asset materialization, AC-5 live
  Linux/macOS CI, and AC-6 live release bundle materialization. AC-4's live-CI
  aspect is covered by the CI residual entry while its workflow-shape signal is
  covered by the AC-2 structural test.
- AC-3's README implementation matches the documented residual checks: the
  source-build install snippet remains in place, the binary-install snippet
  follows it, all seven `--pattern` entries are present, and the text states
  that scripts and binary versions must match for body ingestion.
- AC-5 has no new executable test beyond the structural contract test, which is
  appropriate for this WU because the remaining signal is live CI/gate
  execution. The residual artifact records that dependency rather than
  pretending the YAML parser test proves platform CI.
- AC-6 remains partly structural and partly residual by the WU-13 precedent.
  The unchanged assertions still protect release job shape, but actual
  release-page asset materialization remains a live release concern.
- The Step 6c log says Rust format, clippy, and cargo test passed. It also says
  frontend gates were blocked by missing tools/dependencies (`biome`,
  `vitest`, `solid-js`, etc.); this is documented as environmental rather than
  caused by the YAML/docs-only product edits.
- Phase 6 process-tree audit reports PASS: Step 6b and Step 6c were separate
  invocations, Step 6b produced the test/index/RED log/residual before Step 6c
  product edits, and Step 6c consumption evidence was recorded by the
  same-session resume log.
- Same-agent authorship was not observed in the required Phase 6 evidence:
  Step 6b and Step 6c have separate invocation UUIDs, and the process-tree
  audit maps both required nodes as PASS.
- No supported-surface value collapse surfaced during this test audit. The
  release-asset contract is executable at merge time, and the non-executable
  live-release/CI gaps are explicitly residualized.

## Status
LOW
